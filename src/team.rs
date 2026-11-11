//! Team usage: what each person's agents used in one repository, shared
//! through that repository's own git refs. Whoever can push to it is its
//! team; there is no server.
//!
//! `share` adds a commit to `refs/vessa/usage/<key>` whose one file holds
//! the person's session records that are new or changed since their latest
//! record, and pushes that ref. Nothing written is ever rewritten: the
//! ref's history is the record. `team` fetches every person's ref into
//! `refs/vessa/remotes/<remote>/usage/` and reads each history, keeping the
//! latest record of each session. Pushing and fetching those refs is the
//! only network use, and only these two commands make it. A record holds
//! counts, times, the names of agents, providers, models and branches,
//! native session ids and the person's git name and email: no words, no
//! path, no machine name.

use crate::model::{self, Entry};
use crate::render::{for_terminal as clean, many, usage_line};
use crate::scrub::Redactions;
use anyhow::{anyhow, bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// The record's schema.
const RECORD: &str = "vessa-team-usage.v1";
/// The schema of `team --json`.
const TEAM: &str = "vessa-team.v1";
/// The record's one file in each commit.
const FILE: &str = "usage.json";
/// Where each person's share is kept, by key.
const SHARED: &str = "refs/vessa/usage/";

/// What every fetch is given, so it fetches the refspec it names and
/// nothing else, whatever this clone's settings say: no tags, no
/// submodules, and no FETCH_HEAD written.
const ONLY_THE_REFSPEC: [&str; 4] = [
    "--quiet",
    "--no-tags",
    "--no-recurse-submodules",
    "--no-write-fetch-head",
];

/// Where `remote`'s shares are fetched to: apart from [`SHARED`], so a
/// forced fetch never replaces this clone's own share.
fn fetched(remote: &str) -> String {
    format!("refs/vessa/remotes/{remote}/usage/")
}

/// One share: the one file of a commit on the person's ref.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    schema: String,
    person: Person,
    shared_at: String,
    /// Each session's entries include its subagents'. Always true.
    entries_include_subagents: bool,
    /// Warnings of this share alone: sources vessa could not read, sessions
    /// of this repository among them, maybe; and one more when sessions were
    /// left out for their id.
    source_warnings: usize,
    /// The sessions new since, or changed from, their latest record.
    sessions: Vec<Shared>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Person {
    name: String,
    email: String,
}

/// One session with its subagents, in numbers, as it stood when shared.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Shared {
    agent: String,
    id: String,
    started_at: Option<String>,
    /// The time of its latest usage record, its subagents' included.
    ended_at: Option<String>,
    branch: Option<String>,
    /// The subagents its entries include.
    subagents: Vec<Subagent>,
    /// How many repositories it touched, when more than one: it counts in
    /// full in each.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repositories_touched: Option<usize>,
    partial: bool,
    /// vessa's limitations of this session, by kind (see [`kind`]).
    limitations: BTreeMap<String, usize>,
    entries: Vec<Entry>,
}

/// A subagent a session's entries include: its native id, and its own
/// numbers, as `vessa status` shows them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Subagent {
    id: String,
    entries: Vec<Entry>,
}

/// What `vessa share` was asked.
pub struct Share<'a> {
    pub remote: &'a str,
    pub push: bool,
    pub dry_run: bool,
    pub json: bool,
}

/// Shares the sessions of this repository, read from `paths`: those new or
/// changed since their latest record, as one commit, pushed.
pub fn share(paths: &[PathBuf], not_installed: Vec<String>, asked: Share) -> Result<i32> {
    remote_name(asked.remote)?;
    let roots: Vec<PathBuf> = worktrees()?.iter().filter_map(|w| canonical(w)).collect();
    ensure!(
        !roots.is_empty(),
        "git lists no worktree of this clone, so no session can be placed in it"
    );
    let person = person()?;
    let key = key(&person.email)?;
    // Every session is read, as `vessa usage` reads them, so each is counted
    // as it counts it; those of this repository are then chosen.
    let mut report = crate::import::read(paths)?;
    report.warnings.extend(not_installed);
    let mut found = crate::scrub::report(&mut report);
    for warning in &report.warnings {
        eprintln!("vessa: {}", clean(warning));
    }
    let mut source_warnings = report.warnings.len();
    let mut places = Places::default();
    let mut current = Vec::new();
    let (mut by_folder, mut by_files, mut withheld) = (0, 0, 0);
    for family in model::families(&report.sessions) {
        let folder = inside(&family.main.cwd, &roots);
        if !folder && !places.touches(&family, &roots) {
            continue;
        }
        // A session is known by its id, and its subagents by theirs. One
        // that reads as a secret or a path cannot be written as it is, and
        // any stand-in made from it would be its fingerprint: the session
        // is left out, and said.
        let mut known_by = std::iter::once(family.main).chain(family.subagents.iter().copied());
        if !known_by.all(writable_id) {
            withheld += 1;
            continue;
        }
        let repositories = places.repositories(&family);
        current.push(shared(&family, repositories, &mut found)?);
        if folder {
            by_folder += 1;
        } else {
            by_files += 1;
        }
    }
    eprintln!(
        "vessa: {} of this repository: {by_folder} by the folder they recorded, {by_files} by the files they touched",
        many(current.len(), "session"),
    );
    if withheld > 0 {
        eprintln!(
            "vessa: {} of this repository {} an id that reads as a secret or a path, so {} not shared",
            many(withheld, "session"),
            if withheld == 1 { "has" } else { "have" },
            if withheld == 1 { "it is" } else { "they are" },
        );
        source_warnings += 1;
    }

    // The shares this one follows: this clone's, and the remote's as last
    // fetched, unless one already holds the other.
    let mine = format!("{SHARED}{key}");
    let tracked = format!("{}{key}", fetched(asked.remote));
    let known = refs(&[&mine, &tracked])?;
    let local = known.get(&mine).cloned();
    let theirs = known.get(&tracked).cloned();
    let parents = parents(local.as_deref(), theirs.as_deref())?;
    // Each session's latest record in their history; a session whose record
    // is the same is not written again.
    let mut latest = BTreeMap::new();
    // The warnings of the most recent share, which `team` shows.
    let mut last_warnings = None;
    for (commit, read) in history(&parents)? {
        let record = read.map_err(|why| {
            anyhow!("the share at {commit} is not a {RECORD} record ({why}); nothing was written")
        })?;
        last_warnings.get_or_insert(record.source_warnings);
        for s in record.sessions {
            latest.entry((s.agent.clone(), s.id.clone())).or_insert(s);
        }
    }
    let new: Vec<Shared> = current
        .iter()
        .filter(|s| latest.get(&(s.agent.clone(), s.id.clone())) != Some(*s))
        .cloned()
        .collect();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let record = Record {
        schema: RECORD.into(),
        person,
        shared_at: show(chrono::DateTime::from_timestamp(i64::try_from(now)?, 0).context("clock")?),
        entries_include_subagents: true,
        source_warnings,
        sessions: new,
    };
    let mut bytes = serde_json::to_vec_pretty(&record)?;
    bytes.push(b'\n');

    let partial: Vec<&Shared> = current.iter().filter(|s| s.partial).collect();
    let finish = |code: i32| -> i32 {
        for s in &partial {
            eprintln!(
                "vessa: {} {}: partial ({}); `vessa status --session {}` says why",
                clean(&s.agent),
                clean(&s.id),
                kinds(&s.limitations),
                clean(&s.id),
            );
        }
        if let Some(said) = found.said() {
            eprintln!("vessa: {said}");
        }
        if code == 4 {
            eprintln!(
                "vessa: partial usage evidence (exit 4): {} with limitations, {source_warnings} source {}",
                many(partial.len(), "session"),
                if source_warnings == 1 { "warning" } else { "warnings" },
            );
        }
        code
    };
    let code = if partial.is_empty() && source_warnings == 0 {
        0
    } else {
        4
    };
    // Two copies of the ref that went apart are joined by a commit on both,
    // whether or not it adds a session; and warnings other than the most
    // recent share's are new too, since `team` shows those.
    let nothing = record.sessions.is_empty()
        && parents.len() < 2
        && last_warnings.is_none_or(|last| last == source_warnings);
    if asked.dry_run {
        if nothing {
            eprintln!("vessa: dry run: nothing new to share");
        } else {
            std::io::stdout().lock().write_all(&bytes)?;
            eprintln!("vessa: dry run: nothing was written or pushed");
        }
        return Ok(finish(code));
    }
    if nothing {
        // With --json, stdout holds a record written, and only that.
        let say = |text: String| {
            if asked.json {
                eprintln!("vessa: {text}");
            } else {
                println!("{text}");
            }
        };
        if current.is_empty() {
            say(
                "No session of this repository was found on this machine, so nothing was shared."
                    .into(),
            );
        } else {
            say(format!(
                "Nothing new to share: {} of this repository {} as last shared.",
                many(current.len(), "session"),
                if current.len() == 1 { "is" } else { "are" },
            ));
        }
        // A share this clone made and has not pushed yet is pushed now.
        if let (true, Some(local)) = (asked.push, &local) {
            if theirs.as_ref() != Some(local) && parents == [local.clone()] {
                push(asked.remote, &mine, &tracked, local)?;
                say(format!(
                    "{mine} is at {}; pushed to {}.",
                    short(local),
                    clean(asked.remote)
                ));
            }
        }
        return Ok(finish(code));
    }
    let commit = commit(&record, &bytes, &parents, now)?;
    run(&[
        "update-ref",
        "-m",
        "vessa share",
        &mine,
        &commit,
        local.as_deref().unwrap_or(""),
    ])?;
    let pushed = if asked.push {
        push(asked.remote, &mine, &tracked, &commit)?;
        format!("pushed to {}", clean(asked.remote))
    } else {
        "kept in this clone, not pushed (--no-push)".into()
    };
    if asked.json {
        std::io::stdout().lock().write_all(&bytes)?;
    } else {
        let written = record.sessions.len();
        println!(
            "Shared {} of this repository as {} <{}>, new or changed; {} as last shared.",
            many(written, "session"),
            clean(&record.person.name),
            clean(&record.person.email),
            current.len().saturating_sub(written),
        );
        println!("{mine} is at {}; {pushed}.", short(&commit));
    }
    Ok(finish(code))
}

fn short(commit: &str) -> &str {
    &commit[..12.min(commit.len())]
}

/// One session with its subagents, as the record keeps it; `repositories`
/// is how many it touched.
fn shared(family: &model::Family, repositories: usize, found: &mut Redactions) -> Result<Shared> {
    let main = family.main;
    let mut limitations = BTreeMap::new();
    for said in family.limitations() {
        *limitations.entry(kind(&said).to_owned()).or_insert(0) += 1;
    }
    let entries = named_entries(family.totals()?, found)?;
    // Sessions come in id order, so a session's subagents do too.
    let subagents = family
        .subagents
        .iter()
        .map(|s| {
            Ok(Subagent {
                id: s.id.clone(),
                entries: named_entries(model::aggregate(s)?, found)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Shared {
        agent: main.agent.clone(),
        // Checked by `share`: the id is written as it is.
        id: main.id.clone(),
        started_at: main.started_at.as_deref().and_then(time).map(show),
        ended_at: family
            .usage()
            .filter_map(|u| u.at.as_deref().and_then(time))
            .max()
            .map(show),
        branch: main.branch.as_deref().map(|b| branch(b, found)),
        subagents,
        repositories_touched: (repositories > 1).then_some(repositories),
        partial: !limitations.is_empty(),
        limitations,
        entries,
    })
}

/// `entries` with their names as the record keeps them; names withheld
/// alike are one entry.
fn named_entries(entries: Vec<Entry>, found: &mut Redactions) -> Result<Vec<Entry>> {
    let mut by_name: BTreeMap<(String, String), Entry> = BTreeMap::new();
    for e in entries {
        let provider = name(&e.provider, found);
        let model = model_name(&e.provider, &e.model, found);
        match by_name.entry((provider.clone(), model.clone())) {
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(Entry {
                    provider,
                    model,
                    ..e
                });
            }
            std::collections::btree_map::Entry::Occupied(mut slot) => {
                let total = slot.get_mut();
                total.input_tokens = model::add(total.input_tokens, e.input_tokens)?;
                total.output_tokens = model::add(total.output_tokens, e.output_tokens)?;
                total.cache_read_tokens = model::add(total.cache_read_tokens, e.cache_read_tokens)?;
                total.cache_write_tokens =
                    model::add(total.cache_write_tokens, e.cache_write_tokens)?;
            }
        }
    }
    Ok(by_name.into_values().collect())
}

/// A branch name a transcript gives, as the record keeps it: as it is when
/// it is a valid git branch name, scrubbed of secrets; else `[path]`.
fn branch(text: &str, found: &mut Redactions) -> String {
    if branch_name(text) {
        found.text(text)
    } else {
        "[path]".into()
    }
}

/// Whether `name` is a valid git branch name, by `git check-ref-format
/// --branch`'s rules, and is not a path from the root, from home or from a
/// dot.
fn branch_name(name: &str) -> bool {
    !name.is_empty()
        && name != "HEAD"
        && !name.starts_with(['-', '/', '~', '.'])
        && !Path::new(name).is_absolute()
        && ref_name(&format!("refs/heads/{name}"))
}

/// Git's rules for a full ref name (git-check-ref-format): no component
/// starts with `.` or ends with `.lock`; no `..`, `@{`, `//` or `\`; no
/// control character, space, `~`, `^`, `:`, `?`, `*` or `[`; it neither
/// starts nor ends with `/`, does not end with `.`, and is not `@`.
fn ref_name(name: &str) -> bool {
    let forbidden = |c: char| {
        c.is_ascii_control() || matches!(c, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\')
    };
    name != "@"
        && !name.contains("..")
        && !name.contains("@{")
        && !name.contains("//")
        && !name.starts_with('/')
        && !name.ends_with('/')
        && !name.ends_with('.')
        && !name.chars().any(forbidden)
        && name
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}

/// A provider name a transcript gives, as the record keeps it: scrubbed of
/// secrets, and `[path]` when it reads as one.
fn name(text: &str, found: &mut Redactions) -> String {
    let text = found.text(text);
    if path_like(&text) {
        "[path]".into()
    } else {
        text
    }
}

/// A model name, as [`name`] keeps it, except the one shape with a
/// separator that vessa itself writes: its provider, `/`, then the model,
/// as in `anthropic/claude-…` under the provider `anthropic`.
fn model_name(provider: &str, model: &str, found: &mut Redactions) -> String {
    let model = found.text(model);
    let composed = !path_like(provider)
        && model
            .strip_prefix(provider)
            .and_then(|rest| rest.strip_prefix('/'))
            .is_some_and(|rest| !rest.is_empty() && !path_like(rest));
    if composed || !path_like(&model) {
        model
    } else {
        "[path]".into()
    }
}

/// Whether a session's id can be written as it is: the scrub leaves it
/// alone, and it reads as no path, unless it is the one id with a separator
/// vessa itself writes, a Claude Code subagent's `<session>/<agentId>`.
fn writable_id(s: &model::Session) -> bool {
    if Redactions::default().text(&s.id) != s.id {
        return false;
    }
    if !path_like(&s.id) {
        return true;
    }
    s.agent == "claude"
        && s.subagent_of().is_some_and(|session| {
            !path_like(session)
                && s.id
                    .strip_prefix(session)
                    .and_then(|rest| rest.strip_prefix('/'))
                    .is_some_and(|agent| !agent.is_empty() && !path_like(agent))
        })
}

/// File extensions local model weights are kept in.
const MODEL_FILES: [&str; 14] = [
    "gguf",
    "ggml",
    "ggjt",
    "bin",
    "safetensors",
    "pt",
    "pth",
    "onnx",
    "ckpt",
    "h5",
    "tflite",
    "mlmodel",
    "llamafile",
    "pkl",
];

/// Whether `text` reads as a file path: it holds a separator (`/` or `\`),
/// starts from home or a dot, is a file URL, or ends in an extension model
/// weights are kept in. The rule for provider and model names and session
/// ids; a branch name has its own ([`branch_name`]).
fn path_like(text: &str) -> bool {
    text.contains(['/', '\\'])
        || text.starts_with(['~', '.'])
        || text.starts_with("file:")
        || text.rsplit_once('.').is_some_and(|(_, extension)| {
            MODEL_FILES.contains(&extension.to_ascii_lowercase().as_str())
        })
}

/// The kind of one of vessa's limitations. The record keeps the kind, a
/// name from this fixed list, and never the limitation's own words, which
/// quote record ids, other sessions' ids and names.
fn kind(said: &str) -> &'static str {
    // A subagent's limitation, as its session names it.
    let own = said
        .strip_prefix("subagent ")
        .and_then(|rest| rest.split_once(": "))
        .map_or(said, |(_, own)| own);
    if let Some((head, rest)) = own.split_once(": ") {
        if head.starts_with("line ") || head.starts_with("message ") {
            return if rest.starts_with("malformed JSON") || rest == "its data is not JSON" {
                "malformed_record"
            } else if rest.starts_with("subagent ")
                && rest.contains("was launched, but its transcript was not read")
            {
                "subagent_not_read"
            } else {
                "unreadable_record"
            };
        }
    }
    const OPENINGS: [(&str, &str); 8] = [
        ("conflicting usage ", "conflicting_usage"),
        ("No usage records found", "no_usage_recorded"),
        ("No usage of its own", "counted_with_another_session"),
        ("Codex subagent lineage loops", "lineage_loop"),
        ("opencode subagent lineage loops", "lineage_loop"),
        (
            "its transcript and another session's hold copies",
            "copied_launch_records",
        ),
        ("cache_write_input_tokens is non-zero", "codex_cache_writes"),
        ("messages unreadable", "messages_unreadable"),
    ];
    for (opening, kind) in OPENINGS {
        if own.starts_with(opening) {
            return kind;
        }
    }
    // "N of its M requests were counted with another session (…)".
    if own.starts_with(|c: char| c.is_ascii_digit())
        && own.contains(" counted with another session (")
    {
        return "counted_with_another_session";
    }
    "other"
}

/// Limitation kinds with their counts, for a terminal.
fn kinds(limitations: &BTreeMap<String, usize>) -> String {
    limitations
        .iter()
        .map(|(kind, n)| format!("{} {n}", clean(kind)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn time(text: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|t| t.with_timezone(&chrono::Utc))
}

/// A time as the record writes it: RFC 3339, in UTC.
fn show(t: chrono::DateTime<chrono::Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
}

// ---- where a session belongs -------------------------------------------------

/// The top folders of this clone's worktrees, the main one first, as git
/// lists them; an error outside a repository.
fn worktrees() -> Result<Vec<PathBuf>> {
    let args = ["worktree", "list", "--porcelain", "-z"];
    let out = output(&args, None, &[])?;
    if !out.status.success() {
        bail!(
            "not inside a git repository: run vessa share and vessa team in a clone of the project ({})",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(listed(&out.stdout))
}

/// The worktree folders in `git worktree list --porcelain -z` output, the
/// bare repository's own entry left out.
fn listed(bytes: &[u8]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut current: Option<PathBuf> = None;
    let mut bare = false;
    for field in bytes.split(|b| *b == 0) {
        if field.is_empty() {
            if let Some(path) = current.take().filter(|_| !bare) {
                found.push(path);
            }
            bare = false;
        } else if let Some(path) = field.strip_prefix(b"worktree ") {
            current = Some(path_from(path));
        } else if field == b"bare" {
            bare = true;
        }
    }
    if let Some(path) = current.filter(|_| !bare) {
        found.push(path);
    }
    found
}

#[cfg(unix)]
fn path_from(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
}

#[cfg(not(unix))]
fn path_from(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}

/// `path` as the file system resolves it, when it exists. One that no
/// longer does is normalized by its words (`.` dropped, `..` taking away
/// the name before it), then its longest part that exists is resolved and
/// the rest kept as given. None for a relative path.
fn canonical(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    if let Ok(real) = path.canonicalize() {
        return Some(real);
    }
    let normal = lexical(path);
    let mut missing = Vec::new();
    let mut at = normal.as_path();
    loop {
        if let Ok(real) = at.canonicalize() {
            return Some(missing.into_iter().rev().fold(real, |p, name| p.join(name)));
        }
        missing.push(at.file_name()?.to_owned());
        at = at.parent()?;
    }
}

/// `path` normalized by its words alone: `.` dropped, and `..` taking away
/// the name before it (never past the root).
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Whether the folder a session recorded is inside one of `roots`, the
/// canonical worktree folders: compared as paths, by their parts, after
/// canonicalizing it too. A session that recorded none belongs nowhere.
fn inside(cwd: &str, roots: &[PathBuf]) -> bool {
    !cwd.is_empty()
        && canonical(Path::new(cwd))
            .is_some_and(|cwd| roots.iter().any(|root| cwd.starts_with(root)))
}

/// Where the paths sessions touched are: each resolved once, and the
/// repository each folder is in. On this machine only, to decide: no path
/// is kept past the run, written, logged or printed.
#[derive(Default)]
struct Places {
    resolved: HashMap<PathBuf, Option<PathBuf>>,
    repository: HashMap<PathBuf, Option<PathBuf>>,
}

impl Places {
    fn resolve(&mut self, path: &Path) -> Option<PathBuf> {
        if let Some(known) = self.resolved.get(path) {
            return known.clone();
        }
        let resolved = canonical(path);
        self.resolved.insert(path.to_path_buf(), resolved.clone());
        resolved
    }

    /// The paths a session and its subagents touched.
    fn touched<'a>(family: &'a model::Family) -> impl Iterator<Item = &'a PathBuf> {
        std::iter::once(family.main)
            .chain(family.subagents.iter().copied())
            .flat_map(|s| &s.touched)
    }

    /// Whether a session or its subagents named a file or folder inside one
    /// of `roots` in a structured tool call.
    fn touches(&mut self, family: &model::Family, roots: &[PathBuf]) -> bool {
        Self::touched(family).any(|path| {
            self.resolve(path)
                .is_some_and(|path| roots.iter().any(|root| path.starts_with(root)))
        })
    }

    /// How many repositories a session touched: the one its folder is in,
    /// and those of the files and folders its tool calls name. The
    /// worktrees of one clone are one repository.
    fn repositories(&mut self, family: &model::Family) -> usize {
        let cwd = (!family.main.cwd.is_empty()).then(|| PathBuf::from(&family.main.cwd));
        let paths: Vec<PathBuf> = cwd
            .into_iter()
            .chain(Self::touched(family).cloned())
            .collect();
        let mut repositories = BTreeSet::new();
        for path in paths {
            if let Some(repository) = self.resolve(&path).and_then(|p| self.repository_of(&p)) {
                repositories.insert(repository);
            }
        }
        repositories.len()
    }

    /// The repository a resolved path is in, as its git common folder: the
    /// nearest folder above it with a `.git`, followed to the folder every
    /// worktree of that clone shares. None outside any.
    fn repository_of(&mut self, path: &Path) -> Option<PathBuf> {
        let mut visited = Vec::new();
        let mut found = None;
        for folder in path.ancestors() {
            if let Some(known) = self.repository.get(folder) {
                found = known.clone();
                break;
            }
            visited.push(folder.to_path_buf());
            if let Some(common) = common_dir(folder) {
                found = Some(common);
                break;
            }
        }
        for folder in visited {
            self.repository.insert(folder, found.clone());
        }
        found
    }
}

/// The git folder a worktree's `.git` leads to, shared by every worktree of
/// its clone: `.git` itself when a folder; else the folder its `gitdir:`
/// names, followed by that folder's `commondir` when there is one.
fn common_dir(folder: &Path) -> Option<PathBuf> {
    let git = folder.join(".git");
    let meta = std::fs::metadata(&git).ok()?;
    if meta.is_dir() {
        return git.canonicalize().ok();
    }
    let text = std::fs::read_to_string(&git).ok()?;
    let gitdir = folder.join(text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim());
    match std::fs::read_to_string(gitdir.join("commondir")) {
        Ok(common) => gitdir.join(common.trim()).canonicalize().ok(),
        Err(_) => gitdir.canonicalize().ok(),
    }
}

// ---- git -----------------------------------------------------------------------

/// git, run where vessa was started, saying what it says in one language.
fn output(args: &[&str], input: Option<&[u8]>, env: &[(&str, &str)]) -> Result<Output> {
    let mut command = Command::new("git");
    command
        .args(args)
        .env("LC_ALL", "C")
        .envs(env.iter().copied())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let Some(input) = input else {
        return command
            .stdin(Stdio::null())
            .output()
            .context("could not start git");
    };
    let mut child = command
        .stdin(Stdio::piped())
        .spawn()
        .context("could not start git")?;
    let mut stdin = child.stdin.take().context("git's stdin is piped")?;
    let bytes = input.to_vec();
    // Written beside the wait: git may answer before it has read it all.
    let writer = std::thread::spawn(move || stdin.write_all(&bytes));
    let out = child.wait_with_output().context("git did not finish")?;
    let written = writer
        .join()
        .map_err(|_| anyhow!("writing to git panicked"))?;
    if out.status.success() {
        written.with_context(|| format!("git {}", args.join(" ")))?;
    }
    Ok(out)
}

fn failed(args: &[&str], out: &Output) -> anyhow::Error {
    anyhow!(
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr).trim()
    )
}

/// What git printed, trimmed; an error when it failed.
fn git(args: &[&str], input: Option<&[u8]>, env: &[(&str, &str)]) -> Result<String> {
    let out = output(args, input, env)?;
    if !out.status.success() {
        return Err(failed(args, &out));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn run(args: &[&str]) -> Result<String> {
    git(args, None, &[])
}

/// The person git names here: `user.name` and `user.email`.
fn person() -> Result<Person> {
    let args = ["config", "-z", "--get-regexp", r"^user\.(name|email)$"];
    let out = output(&args, None, &[])?;
    // git says it found none with exit 1.
    if !out.status.success() && out.status.code() != Some(1) {
        return Err(failed(&args, &out));
    }
    let (mut name, mut email) = (None, None);
    for field in out.stdout.split(|b| *b == 0) {
        let field = String::from_utf8_lossy(field);
        match field.split_once('\n') {
            Some(("user.name", value)) => name = Some(value.trim().to_owned()),
            Some(("user.email", value)) => email = Some(value.trim().to_owned()),
            _ => {}
        }
    }
    match (name.filter(|n| !n.is_empty()), email.filter(|e| !e.is_empty())) {
        (Some(name), Some(email)) => Ok(Person { name, email }),
        _ => bail!(
            "vessa shares as the person git names here: set `git config user.name` and `git config user.email` first"
        ),
    }
}

/// The person's key: the first 16 hex digits of the git hash of the
/// lower-cased email, so a ref name does not spell the address.
fn key(email: &str) -> Result<String> {
    let lowered = email.trim().to_lowercase();
    let hash = git(&["hash-object", "--stdin"], Some(lowered.as_bytes()), &[])?;
    ensure!(
        hash.len() >= 16 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
        "git hash-object printed {hash:?}, not a hash"
    );
    Ok(hash[..16].to_owned())
}

/// A remote name vessa may hand git and put in a ref name: letters, digits,
/// `-`, `_` and `.`, and never an option.
fn remote_name(remote: &str) -> Result<()> {
    ensure!(
        !remote.is_empty()
            && !remote.starts_with(['-', '.'])
            && !remote.contains("..")
            && !remote.ends_with(".lock")
            && remote
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')),
        "--remote {remote:?} is not a remote name"
    );
    Ok(())
}

/// The commit each ref under `patterns` names, by ref.
fn refs(patterns: &[&str]) -> Result<BTreeMap<String, String>> {
    let mut args = vec!["for-each-ref", "--format=%(objectname) %(refname)"];
    args.extend(patterns);
    Ok(run(&args)?
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(commit, reference)| (reference.to_owned(), commit.to_owned()))
        .collect())
}

/// The record in each of `commits`, read with one git.
fn records(commits: &[String]) -> Result<Vec<Result<Record, String>>> {
    if commits.is_empty() {
        return Ok(Vec::new());
    }
    let asked: String = commits.iter().map(|c| format!("{c}:{FILE}\n")).collect();
    let args = ["cat-file", "--batch"];
    let out = output(&args, Some(asked.as_bytes()), &[])?;
    if !out.status.success() {
        return Err(failed(&args, &out));
    }
    let mut rest = &out.stdout[..];
    let mut read = Vec::new();
    for _ in commits {
        let end = rest
            .iter()
            .position(|b| *b == b'\n')
            .context("git cat-file ended early")?;
        let header = String::from_utf8_lossy(&rest[..end]).into_owned();
        rest = &rest[end + 1..];
        let fields: Vec<&str> = header.split(' ').collect();
        match fields[..] {
            [_, "blob", size] => {
                let size: usize = size.parse().context("git cat-file size")?;
                let body = rest.get(..size).context("git cat-file ended early")?;
                rest = rest.get(size + 1..).unwrap_or_default();
                read.push(
                    serde_json::from_slice::<Record>(body)
                        .map_err(|error| error.to_string())
                        .and_then(|r| {
                            if r.schema == RECORD {
                                Ok(r)
                            } else {
                                Err(format!("its schema is {:?}", r.schema))
                            }
                        }),
                );
            }
            [.., "missing"] => read.push(Err(format!("it holds no {FILE}"))),
            _ => {
                // Another kind of object: its body follows, and is skipped.
                if let Some(size) = fields.get(2).and_then(|s| s.parse::<usize>().ok()) {
                    rest = rest.get(size + 1..).unwrap_or_default();
                }
                read.push(Err(format!("its {FILE} is no file")));
            }
        }
    }
    Ok(read)
}

fn is_ancestor(ancestor: &str, descendant: &str) -> Result<bool> {
    let args = ["merge-base", "--is-ancestor", ancestor, descendant];
    let out = output(&args, None, &[])?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(failed(&args, &out)),
    }
}

/// Every commit reachable from `tips`, all parents of a join included, the
/// most recent first (git's date order: a commit before the ones it
/// follows, else by time), with its parents.
fn graph(tips: &[String]) -> Result<Vec<(String, Vec<String>)>> {
    if tips.is_empty() {
        return Ok(Vec::new());
    }
    let mut args = vec!["rev-list", "--date-order", "--parents"];
    args.extend(tips.iter().map(String::as_str));
    Ok(run(&args)?
        .lines()
        .filter_map(|line| {
            let mut ids = line.split(' ').map(str::to_owned);
            Some((ids.next()?, ids.collect()))
        })
        .collect())
}

/// The shares in the history of `tips`, the most recent first.
fn history(tips: &[String]) -> Result<Vec<(String, Result<Record, String>)>> {
    let commits: Vec<String> = graph(tips)?.into_iter().map(|(c, _)| c).collect();
    let read = records(&commits)?;
    Ok(commits.into_iter().zip(read).collect())
}

/// The commits a new share follows: this clone's share, and the remote's
/// as last fetched, each unless the other already holds it.
fn parents(local: Option<&str>, remote: Option<&str>) -> Result<Vec<String>> {
    Ok(match (local, remote) {
        (None, None) => Vec::new(),
        (Some(one), None) | (None, Some(one)) => vec![one.to_owned()],
        (Some(l), Some(r)) if l == r || is_ancestor(r, l)? => vec![l.to_owned()],
        (Some(l), Some(r)) if is_ancestor(l, r)? => vec![r.to_owned()],
        (Some(l), Some(r)) => vec![l.to_owned(), r.to_owned()],
    })
}

/// Writes the record as the one file of a commit on `parents`, made by
/// the person at `now`, exactly as its bytes are.
fn commit(record: &Record, bytes: &[u8], parents: &[String], now: u64) -> Result<String> {
    let blob = git(&["hash-object", "-w", "--stdin"], Some(bytes), &[])?;
    let tree = git(
        &["mktree"],
        Some(format!("100644 blob {blob}\t{FILE}\n").as_bytes()),
        &[],
    )?;
    let message = format!("vessa share: {}", many(record.sessions.len(), "session"));
    let mut args = vec!["commit-tree", "--no-gpg-sign", "-m", &message];
    for parent in parents {
        args.extend(["-p", parent]);
    }
    args.push(&tree);
    let when = format!("{now} +0000");
    let (name, email) = (&record.person.name, &record.person.email);
    git(
        &args,
        None,
        &[
            ("GIT_AUTHOR_NAME", name),
            ("GIT_AUTHOR_EMAIL", email),
            ("GIT_COMMITTER_NAME", name),
            ("GIT_COMMITTER_EMAIL", email),
            ("GIT_AUTHOR_DATE", &when),
            ("GIT_COMMITTER_DATE", &when),
        ],
    )
}

/// Pushes the person's ref. When the remote holds a share under the key
/// that this clone has not seen, it is fetched and nothing is pushed.
fn push(remote: &str, mine: &str, tracked: &str, commit: &str) -> Result<()> {
    let spec = format!("{mine}:{mine}");
    // That ref alone, whatever this clone's settings say of tags and
    // submodules.
    let args = [
        "push",
        "--porcelain",
        "--no-follow-tags",
        "--recurse-submodules=no",
        remote,
        &spec,
    ];
    let out = output(&args, None, &[])?;
    if out.status.success() {
        run(&["update-ref", tracked, commit])?;
        return Ok(());
    }
    let porcelain = String::from_utf8_lossy(&out.stdout);
    let moved = porcelain
        .lines()
        .any(|line| line.starts_with('!') && line.contains("[rejected]"));
    if !moved {
        bail!(
            "your share is kept in this clone as {mine}, but the push to {remote} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let fetch = [
        "fetch",
        ONLY_THE_REFSPEC[0],
        ONLY_THE_REFSPEC[1],
        ONLY_THE_REFSPEC[2],
        ONLY_THE_REFSPEC[3],
        remote,
        &format!("+{mine}:{tracked}"),
    ];
    let whose = match output(&fetch, None, &[]) {
        Ok(out) if out.status.success() => match refs(&[tracked])?.get(tracked) {
            Some(theirs) => match records(std::slice::from_ref(theirs))?.pop() {
                Some(Ok(r)) => format!(
                    ", shared {} by {} <{}>,",
                    r.shared_at, r.person.name, r.person.email
                ),
                _ => String::new(),
            },
            None => String::new(),
        },
        Ok(out) => format!(
            " (which could not be fetched: {})",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(error) => format!(" (which could not be fetched: {error:#})"),
    };
    bail!(
        "not pushed: {remote} holds a share under your key ({mine}){whose} that this clone has not seen. \
         Nothing on {remote} was overwritten, and your share is kept in this clone. \
         If that share was yours, from another clone or machine, run `vessa share` again: \
         it keeps the sessions of both and pushes. If it was not, someone else shares with your email: \
         give this clone your own with `git config user.email`."
    )
}

// ---- team ----------------------------------------------------------------------

/// How `team` groups the sessions, besides by person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum By {
    Agent,
    Model,
    Week,
    Branch,
}

impl By {
    fn name(by: Option<By>) -> &'static str {
        match by {
            None => "person",
            Some(By::Agent) => "agent",
            Some(By::Model) => "model",
            Some(By::Week) => "week",
            Some(By::Branch) => "branch",
        }
    }

    /// What a group stands for when its key is unknown.
    fn unknown(by: Option<By>) -> &'static str {
        match by {
            Some(By::Model) => "(no usage recorded)",
            Some(By::Week) => "(undated)",
            Some(By::Branch) => "(no branch recorded)",
            _ => "(unknown)",
        }
    }
}

/// What one person shared: the latest record of each session in their
/// ref's history.
struct Shares {
    key: String,
    /// The person, as their most recent share names them.
    person: Person,
    /// When their most recent share was made, and its own warnings.
    shared_at: String,
    source_warnings: usize,
    /// How many shares their history holds.
    shares: usize,
    sessions: Vec<Shared>,
    /// Records of a subagent alone that a session of theirs names: counted
    /// with that session, not apart.
    folded: usize,
    /// Of those, records shared after their session's, with other numbers
    /// than it holds for them: what they add is not counted.
    newer: usize,
    /// This clone's share of theirs is on the remote, as last fetched.
    pushed: bool,
}

impl Shares {
    /// Their most recent share had warnings, a session's latest record is
    /// partial, or a subagent's own record is newer than what its session
    /// holds of it.
    fn partial(&self) -> bool {
        self.source_warnings > 0 || self.newer > 0 || self.sessions.iter().any(|s| s.partial)
    }
}

type SessionKey = (String, String, String);

/// Sessions summed by agent, provider and model.
#[derive(Default)]
struct Group {
    key: Option<String>,
    people: BTreeSet<String>,
    sessions: BTreeSet<SessionKey>,
    partial: BTreeSet<SessionKey>,
    /// Sessions that touched several repositories.
    several: BTreeSet<SessionKey>,
    lines: BTreeMap<(String, String, String), (Entry, BTreeSet<SessionKey>)>,
}

impl Group {
    fn add(&mut self, who: &str, s: &Shared, entry: Option<&Entry>) -> Result<()> {
        let id = (who.to_owned(), s.agent.clone(), s.id.clone());
        self.people.insert(who.to_owned());
        self.sessions.insert(id.clone());
        if s.partial {
            self.partial.insert(id.clone());
        }
        if s.repositories_touched.is_some() {
            self.several.insert(id.clone());
        }
        let Some(e) = entry else {
            return Ok(());
        };
        let (total, sessions) = self
            .lines
            .entry((s.agent.clone(), e.provider.clone(), e.model.clone()))
            .or_insert_with(|| {
                (
                    Entry {
                        provider: e.provider.clone(),
                        model: e.model.clone(),
                        input_tokens: Some(0),
                        output_tokens: Some(0),
                        cache_read_tokens: Some(0),
                        cache_write_tokens: Some(0),
                    },
                    BTreeSet::new(),
                )
            });
        total.input_tokens = model::add(total.input_tokens, e.input_tokens)?;
        total.output_tokens = model::add(total.output_tokens, e.output_tokens)?;
        total.cache_read_tokens = model::add(total.cache_read_tokens, e.cache_read_tokens)?;
        total.cache_write_tokens = model::add(total.cache_write_tokens, e.cache_write_tokens)?;
        sessions.insert(id);
        Ok(())
    }

    fn json(&self) -> serde_json::Value {
        let entries: Vec<serde_json::Value> = self
            .lines
            .iter()
            .map(|((agent, _, _), (e, sessions))| {
                serde_json::json!({
                    "agent": agent, "provider": e.provider, "model": e.model,
                    "input_tokens": e.input_tokens, "output_tokens": e.output_tokens,
                    "cache_read_tokens": e.cache_read_tokens, "cache_write_tokens": e.cache_write_tokens,
                    "sessions": sessions.len(),
                })
            })
            .collect();
        serde_json::json!({
            "key": self.key, "people": self.people.len(), "sessions": self.sessions.len(),
            "partial_sessions": self.partial.len(),
            "sessions_in_several_repositories": self.several.len(), "entries": entries,
        })
    }

    fn print(&self) {
        for ((agent, _, _), (entry, sessions)) in &self.lines {
            println!(
                "  {} {} · {}",
                clean(agent),
                usage_line(entry),
                many(sessions.len(), "session")
            );
        }
    }
}

fn people(n: usize) -> String {
    if n == 1 {
        "1 person".into()
    } else {
        format!("{n} people")
    }
}

/// The ISO week of a time, as `stats --period week` names it.
fn week(text: &str) -> Option<String> {
    use chrono::Datelike;
    let t = time(text)?;
    Some(format!(
        "{}-W{:02}",
        t.iso_week().year(),
        t.iso_week().week()
    ))
}

/// What the team shared, read from this clone's refs after fetching the
/// remote's, unless `fetch` is false: each person's history, this clone's
/// copy and the remote's together, and in it the latest record of each
/// session.
pub fn team(remote: &str, fetch: bool, by: Option<By>, json: bool) -> Result<i32> {
    remote_name(remote)?;
    let probe = ["rev-parse", "--git-dir"];
    let out = output(&probe, None, &[])?;
    if !out.status.success() {
        bail!(
            "not inside a git repository: run vessa share and vessa team in a clone of the project ({})",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let tracked = fetched(remote);
    let mut warnings = Vec::new();
    let mut was_fetched = false;
    if fetch {
        let refspec = format!("+{SHARED}*:{tracked}*");
        let args = [
            "fetch",
            ONLY_THE_REFSPEC[0],
            ONLY_THE_REFSPEC[1],
            ONLY_THE_REFSPEC[2],
            ONLY_THE_REFSPEC[3],
            "--prune",
            remote,
            &refspec,
        ];
        match output(&args, None, &[]) {
            Ok(out) if out.status.success() => was_fetched = true,
            Ok(out) => warnings.push(format!(
                "could not fetch from {remote}, so shares made since this clone last fetched are missing: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )),
            Err(error) => warnings.push(format!(
                "could not fetch from {remote}, so shares made since this clone last fetched are missing: {error:#}"
            )),
        }
    }

    // Each person's ref: this clone's copy and the remote's.
    let mut keys: BTreeMap<String, (Option<String>, Option<String>)> = BTreeMap::new();
    for (reference, commit) in refs(&[SHARED, &tracked])? {
        if let Some(key) = reference.strip_prefix(SHARED) {
            keys.entry(key.to_owned()).or_default().0 = Some(commit);
        } else if let Some(key) = reference.strip_prefix(&tracked) {
            keys.entry(key.to_owned()).or_default().1 = Some(commit);
        }
    }
    // Every person's history, read with one git each for the commits and
    // their records.
    let tips: Vec<String> = keys
        .values()
        .flat_map(|(l, r)| l.iter().chain(r))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let order = graph(&tips)?;
    let parents: HashMap<&str, &[String]> = order
        .iter()
        .map(|(c, p)| (c.as_str(), p.as_slice()))
        .collect();
    let commits: Vec<String> = order.iter().map(|(c, _)| c.clone()).collect();
    let mut records: HashMap<&str, Result<Record, String>> = commits
        .iter()
        .map(String::as_str)
        .zip(records(&commits)?)
        .collect();
    let mut shares = Vec::new();
    for (key, (local, theirs)) in &keys {
        // The commits this person's copies reach.
        let mut reach: HashSet<&str> = HashSet::new();
        let mut todo: Vec<&str> = local.iter().chain(theirs).map(String::as_str).collect();
        while let Some(commit) = todo.pop() {
            if reach.insert(commit) {
                todo.extend(
                    parents
                        .get(commit)
                        .into_iter()
                        .flat_map(|p| p.iter().map(String::as_str)),
                );
            }
        }
        // Each session's latest record, with how recent its share is (0 the
        // most recent).
        let mut latest: BTreeMap<(String, String), (usize, Shared)> = BTreeMap::new();
        let mut newest: Option<(Person, String, usize)> = None;
        let mut count = 0;
        for commit in commits
            .iter()
            .map(String::as_str)
            .filter(|c| reach.contains(c))
        {
            match records.remove(commit) {
                Some(Ok(record)) => {
                    newest.get_or_insert((record.person, record.shared_at, record.source_warnings));
                    for s in record.sessions {
                        latest
                            .entry((s.agent.clone(), s.id.clone()))
                            .or_insert((count, s));
                    }
                    count += 1;
                }
                Some(Err(why)) => warnings.push(format!(
                    "the share {commit} under {key} cannot be read, so it is left out: {why}"
                )),
                None => {}
            }
        }
        let Some((person, shared_at, source_warnings)) = newest else {
            continue;
        };
        // A subagent is counted with the session that names it: a record of
        // it alone gives way, and is counted here. One shared after that
        // session's record, with other numbers than that record holds for
        // it, went on after: what it added is not counted, so the person is
        // partial.
        let mut named: BTreeMap<(String, String), (usize, Vec<Entry>)> = BTreeMap::new();
        for (at, s) in latest.values() {
            for sub in &s.subagents {
                named.insert(
                    (s.agent.clone(), sub.id.clone()),
                    (*at, sub.entries.clone()),
                );
            }
        }
        let newer = latest
            .iter()
            .filter(|(key, (at, s))| {
                named
                    .get(*key)
                    .is_some_and(|(session_at, held)| at < session_at && &s.entries != held)
            })
            .count();
        let before = latest.len();
        latest.retain(|key, _| !named.contains_key(key));
        let latest: BTreeMap<(String, String), Shared> =
            latest.into_iter().map(|(k, (_, s))| (k, s)).collect();
        let pushed = match (local, theirs) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(l), Some(r)) => l == r || is_ancestor(l, r)?,
        };
        shares.push(Shares {
            key: key.clone(),
            person,
            shared_at,
            source_warnings,
            shares: count,
            folded: before - latest.len(),
            newer,
            sessions: latest.into_values().collect(),
            pushed,
        });
    }
    shares.sort_by_key(|s| {
        (
            s.person.name.to_lowercase(),
            s.person.email.to_lowercase(),
            s.key.clone(),
        )
    });

    let mut total = Group::default();
    let mut groups: Vec<Group> = Vec::new();
    let mut index: HashMap<Option<String>, usize> = HashMap::new();
    for share in &shares {
        if by.is_none() {
            // A person with no session still has a block.
            index.entry(Some(share.key.clone())).or_insert_with(|| {
                groups.push(Group {
                    key: Some(share.key.clone()),
                    ..Group::default()
                });
                groups.len() - 1
            });
        }
        for s in &share.sessions {
            let mut add = |key: Option<String>, entry: Option<&Entry>| -> Result<()> {
                let at = *index.entry(key.clone()).or_insert_with(|| {
                    groups.push(Group {
                        key,
                        ..Group::default()
                    });
                    groups.len() - 1
                });
                groups[at].add(&share.key, s, entry)
            };
            let key = |entry: Option<&Entry>| match by {
                None => Some(share.key.clone()),
                Some(By::Agent) => Some(s.agent.clone()),
                Some(By::Model) => entry.map(|e| format!("{}/{}", e.provider, e.model)),
                Some(By::Week) => s.started_at.as_deref().and_then(week),
                Some(By::Branch) => s.branch.clone(),
            };
            if s.entries.is_empty() {
                add(key(None), None)?;
                total.add(&share.key, s, None)?;
            }
            for entry in &s.entries {
                add(key(Some(entry)), Some(entry))?;
                total.add(&share.key, s, Some(entry))?;
            }
        }
    }
    if by.is_some() {
        // In key order, the unknown last; by person, in the people's order.
        groups.sort_by(|a, b| (a.key.is_none(), &a.key).cmp(&(b.key.is_none(), &b.key)));
    }
    let partial_people = shares.iter().filter(|s| s.partial()).count();
    let code = if partial_people == 0 && warnings.is_empty() {
        0
    } else {
        4
    };

    if json {
        let people: Vec<serde_json::Value> = shares
            .iter()
            .map(|s| {
                serde_json::json!({
                    "key": s.key, "name": s.person.name, "email": s.person.email,
                    "shared_at": s.shared_at, "shares": s.shares, "pushed": s.pushed,
                    "partial": s.partial(), "source_warnings": s.source_warnings,
                    "subagents_counted_with_their_session": s.folded,
                    "subagents_newer_than_their_session": s.newer, "sessions": s.sessions,
                })
            })
            .collect();
        let mut sum = total.json();
        sum.as_object_mut()
            .context("a group is an object")?
            .remove("key");
        crate::json(&serde_json::json!({
            "schema": TEAM, "remote": remote, "fetched": was_fetched, "by": By::name(by),
            "people": people, "groups": groups.iter().map(Group::json).collect::<Vec<_>>(),
            "total": sum, "warnings": warnings,
        }))?;
    } else {
        print_team(remote, fetch, was_fetched, by, &shares, &groups, &total);
    }
    for warning in &warnings {
        eprintln!("vessa: {}", clean(warning));
    }
    if code == 4 {
        eprintln!(
            "vessa: partial team usage (exit 4): {} partial, {}",
            people(partial_people),
            many(warnings.len(), "warning"),
        );
    }
    Ok(code)
}

fn print_team(
    remote: &str,
    fetch: bool,
    was_fetched: bool,
    by: Option<By>,
    shares: &[Shares],
    groups: &[Group],
    total: &Group,
) {
    let source = if was_fetched {
        format!("fetched from {}", clean(remote))
    } else if fetch {
        format!("could not fetch from {}", clean(remote))
    } else {
        "not fetched (--no-fetch)".into()
    };
    if shares.is_empty() {
        println!("Nobody has shared usage for this repository yet ({source}). `vessa share` shares yours.");
        return;
    }
    println!(
        "Team usage of this repository · {} shared · {source}",
        match shares.len() {
            1 => "1 person has".into(),
            n => format!("{n} people have"),
        }
    );
    println!("Input excludes cached tokens. Unknown is not zero.");
    let about = |s: &Shares| {
        let mut said = format!(
            "{} <{}> · last shared {} · {}",
            clean(&s.person.name),
            clean(&s.person.email),
            clean(&s.shared_at),
            many(s.sessions.len(), "session"),
        );
        let partial = s.sessions.iter().filter(|x| x.partial).count();
        if partial > 0 {
            said += &format!(" · {partial} partial");
        }
        let several = s
            .sessions
            .iter()
            .filter(|x| x.repositories_touched.is_some())
            .count();
        if several > 0 {
            said += &format!(" · {several} also in other repositories");
        }
        if s.folded > 0 {
            said += &format!(
                " · {} counted with {}",
                many(s.folded, "subagent"),
                if s.folded == 1 {
                    "its session"
                } else {
                    "their sessions"
                }
            );
        }
        if s.newer > 0 {
            said += &format!(
                " · partial: {} went on after {} shared, not counted",
                many(s.newer, "subagent"),
                if s.newer == 1 {
                    "its session was"
                } else {
                    "their sessions were"
                }
            );
        }
        if s.source_warnings > 0 {
            said += &format!(
                " · partial: {} unreadable in the latest share",
                many(s.source_warnings, "source")
            );
        }
        if !s.pushed {
            said += " · not pushed";
        }
        said
    };
    if by.is_none() {
        for (share, group) in shares.iter().zip(groups) {
            println!();
            println!("{}", about(share));
            group.print();
        }
    } else {
        println!();
        println!("By {}", By::name(by));
        for group in groups {
            let mut said = format!(
                "{} · {} · {}",
                group
                    .key
                    .as_deref()
                    .map_or_else(|| By::unknown(by).to_owned(), clean),
                people(group.people.len()),
                many(group.sessions.len(), "session"),
            );
            if !group.partial.is_empty() {
                said += &format!(" · {} partial", group.partial.len());
            }
            println!("{said}");
            group.print();
        }
        println!();
        println!("Shared by");
        for share in shares {
            println!("  {}", about(share));
        }
    }
    println!();
    let mut said = format!(
        "Team total · {} · {}",
        people(total.people.len()),
        many(total.sessions.len(), "session")
    );
    if !total.partial.is_empty() {
        said += &format!(" · {} partial", total.partial.len());
    }
    println!("{said}");
    total.print();
    if !total.several.is_empty() {
        println!();
        println!(
            "{} touched several repositories and {} in full in each; totals across repositories would count {} more than once:",
            many(total.several.len(), "session"),
            if total.several.len() == 1 { "counts" } else { "count" },
            if total.several.len() == 1 { "it" } else { "them" },
        );
        for share in shares {
            for s in &share.sessions {
                if let Some(n) = s.repositories_touched {
                    println!(
                        "  {} · {} {} · {n} repositories",
                        clean(&share.person.name),
                        clean(&s.agent),
                        clean(&s.id),
                    );
                }
            }
        }
    }
    if !total.partial.is_empty() {
        println!();
        println!("Partial sessions: their numbers are incomplete");
        for share in shares {
            for s in share.sessions.iter().filter(|s| s.partial) {
                println!(
                    "  {} · {} {} · {}",
                    clean(&share.person.name),
                    clean(&s.agent),
                    clean(&s.id),
                    kinds(&s.limitations)
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_limitation_vessa_writes_has_its_kind() {
        let cases = [
            ("line 2: malformed JSON: expected value at line 1 column 1", "malformed_record"),
            ("message 7: its data is not JSON", "malformed_record"),
            (
                "line 3: subagent s1/ghost was launched, but its transcript was not read; its usage is unknown, not zero",
                "subagent_not_read",
            ),
            (
                "line 3: subagent s1/ghost was launched, but its transcript was not read; its usage is unknown, not zero; s2 holds the same launch record, and no transcript read shows which session ran it",
                "subagent_not_read",
            ),
            ("line 9: invalid nonnegative integer input_tokens", "unreadable_record"),
            ("message 4: invalid nonnegative integer input", "unreadable_record"),
            (
                "conflicting usage req_1 (first seen in s0): model a then b; first record retained",
                "conflicting_usage",
            ),
            ("No usage records found; consumption is unknown, not zero", "no_usage_recorded"),
            (
                "No usage of its own: its 2 requests were counted with another session (s9), so this session's total leaves them out",
                "counted_with_another_session",
            ),
            (
                "3 of its 5 requests were counted with another session (s9), so this session's total leaves them out",
                "counted_with_another_session",
            ),
            (
                "Codex subagent lineage loops (a -> b -> a); its root session is unknown, so it counts as a session of its own",
                "lineage_loop",
            ),
            (
                "opencode subagent lineage loops (a -> b -> a); its root session is unknown, so it counts as a session of its own",
                "lineage_loop",
            ),
            (
                "its transcript and another session's hold copies of each other's launch records, so where the requests they share began is unknown; each is counted with the first session by id",
                "copied_launch_records",
            ),
            (
                "cache_write_input_tokens is non-zero: Vessa assumes cache writes are part of input_tokens, which no transcript has confirmed; uncached input may be undercounted",
                "codex_cache_writes",
            ),
            (
                "messages unreadable, so no usage read: sqlite3 failed on /Users/ada/opencode.db",
                "messages_unreadable",
            ),
            ("subagent s1/agent-a: line 4: malformed JSON: EOF", "malformed_record"),
            ("subagent s1/agent-a: No usage records found; consumption is unknown, not zero", "no_usage_recorded"),
            ("something vessa will say one day", "other"),
        ];
        for (said, expected) in cases {
            assert_eq!(kind(said), expected, "{said}");
        }
    }

    #[test]
    fn a_name_that_reads_as_a_path_is_withheld() {
        for path in [
            "/Users/ada/models/local.gguf",
            "~/models/x",
            "./model",
            "../model",
            r"C:\models\x",
            "C:/models/x",
            r"\\server\share",
            "file:///tmp/x",
            r"models\x",
            "models/private.gguf",
            "private.GGUF",
            "weights.safetensors",
            "feat/team",
            "anthropic/claude-opus",
            ".hidden",
        ] {
            assert!(path_like(path), "{path}");
        }
        for name in [
            "demo-model",
            "glm-4.6",
            "claude-3.5",
            "gpt-5.1-codex",
            "llama3:8b",
            "HEAD",
            "main",
        ] {
            assert!(!path_like(name), "{name}");
        }
        let mut found = Redactions::default();
        assert_eq!(name("/home/ada/x.gguf", &mut found), "[path]");
        assert_eq!(name("feat/team", &mut found), "[path]");
        let key = ["gh", "p_", "Ab3dE5gH7jK9mN1pQ3sT5vW7yZ9bC1dF3hJ5"].concat();
        assert_eq!(name(&key, &mut found), "[redacted]");
        // A model keeps a separator only in vessa's own shape: its provider,
        // then the model.
        let model = |provider: &str, model: &str| model_name(provider, model, &mut found.clone());
        assert_eq!(
            model("anthropic", "anthropic/claude-opus"),
            "anthropic/claude-opus"
        );
        assert_eq!(model("openrouter", "anthropic/claude-opus"), "[path]");
        assert_eq!(model("anthropic", "anthropic/models/x"), "[path]");
        assert_eq!(model("anthropic", "anthropic/x.gguf"), "[path]");
        assert_eq!(model("anthropic", "anthropic/"), "[path]");
        assert_eq!(model("a/b", "a/b/c"), "[path]");
        assert_eq!(model("anthropic", "models/private.gguf"), "[path]");
    }

    #[test]
    fn a_branch_name_is_kept_when_git_would_take_it() {
        // Each as `git check-ref-format --branch` judges it.
        for valid in [
            "main",
            "feat/team",
            "user/ada/x",
            "release-1.2",
            "fix_it",
            "@",
        ] {
            assert!(branch_name(valid), "{valid}");
        }
        for invalid in [
            "",
            "HEAD",
            "/abs/path",
            "~/x",
            ".hidden",
            "feat/.x",
            "a..b",
            "a b",
            "x.lock",
            "a//b",
            "-x",
            "a:b",
            r"C:\x",
            r"a\b",
            "x/",
            "x.",
            "a@{b",
            "a*b",
            "a[b",
            "a?b",
            "a^b",
        ] {
            assert!(!branch_name(invalid), "{invalid}");
        }
        let mut found = Redactions::default();
        assert_eq!(branch("feat/team", &mut found), "feat/team");
        assert_eq!(branch("~/secret", &mut found), "[path]");
    }

    #[test]
    fn a_missing_path_is_normalized_by_its_words() {
        assert_eq!(lexical(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
        assert_eq!(lexical(Path::new("/../a")), PathBuf::from("/a"));
        assert_eq!(lexical(Path::new("/a/b/../../..")), PathBuf::from("/"));
    }

    #[test]
    fn an_id_with_a_separator_is_written_only_as_a_claude_subagent_s() {
        let session = |agent: &str, id: &str, parent: Option<&str>| model::Session {
            agent: agent.into(),
            id: id.into(),
            parent: parent.map(str::to_owned),
            started_at: None,
            cwd: String::new(),
            source: String::new(),
            usage: Vec::new(),
            limitations: Vec::new(),
            ohjaa: Vec::new(),
            work: Vec::new(),
            entrypoint: None,
            originator: None,
            branch: None,
            touched: Default::default(),
            launched: Vec::new(),
            counted_elsewhere: 0,
            synthetic: Default::default(),
            root: None,
            subagent: false,
            partly_sidechain: false,
        };
        assert!(writable_id(&session("claude", "s-1", None)));
        assert!(writable_id(&session("claude", "s-1/agent-a", Some("s-1"))));
        assert!(!writable_id(&session(
            "claude",
            "s-1/agent-a/b",
            Some("s-1")
        )));
        assert!(!writable_id(&session("claude", "s-1/x.gguf", Some("s-1"))));
        assert!(!writable_id(&session("claude", "drafts/plan", None)));
        assert!(!writable_id(&session(
            "claude",
            "drafts/plan",
            Some("other")
        )));
        assert!(!writable_id(&session("codex", "s-1/agent-a", Some("s-1"))));
        assert!(!writable_id(&session("pi", r"notes\plan", None)));
        let key = ["gh", "p_", "Ab3dE5gH7jK9mN1pQ3sT5vW7yZ9bC1dF3hJ5"].concat();
        assert!(!writable_id(&session("claude", &key, None)));
    }

    #[test]
    fn worktrees_are_read_from_porcelain_and_a_bare_entry_is_left_out() {
        let listing =
            b"worktree /srv/repo.git\0bare\0\0worktree /home/ada/wt\0HEAD 0123\0branch refs/heads/x\0\0worktree C:/Users/ada/w2\0HEAD 4567\0detached\0prunable gitdir file points to non-existent location\0\0";
        assert_eq!(
            listed(listing),
            vec![
                PathBuf::from("/home/ada/wt"),
                PathBuf::from("C:/Users/ada/w2")
            ]
        );
    }

    #[test]
    fn a_folder_is_inside_by_its_parts_after_canonicalizing() {
        let root = std::env::temp_dir().join(format!("vessa-inside-{}", std::process::id()));
        let clone = root.join("clone");
        std::fs::create_dir_all(clone.join("src")).unwrap();
        std::fs::create_dir_all(root.join("clone-other")).unwrap();
        let roots = vec![canonical(&clone).unwrap()];
        let at = |p: &Path| p.to_str().unwrap().to_owned();
        assert!(inside(&at(&clone), &roots));
        assert!(inside(&at(&clone.join("src")), &roots));
        // A folder since deleted is placed by the part that exists.
        assert!(inside(&at(&clone.join("gone").join("deeper")), &roots));
        // A name that starts with the clone's is another folder.
        assert!(!inside(&at(&root.join("clone-other")), &roots));
        // A missing folder that climbs out cannot be placed.
        assert!(!inside(
            &at(&clone.join("gone").join("..").join("..").join("clone-other")),
            &roots
        ));
        assert!(!inside("", &roots));
        assert!(!inside("src", &roots));
        // A Windows folder, on Windows a path like any other; elsewhere it
        // is relative, so it is nowhere.
        if cfg!(windows) {
            let windows = at(&clone.join("src")).replace('/', "\\");
            assert!(inside(&windows, &roots));
        } else {
            assert!(!inside(r"C:\Users\ada\clone\src", &roots));
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
