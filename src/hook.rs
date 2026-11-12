//! Pointers from commits to the agent sessions that made them.
//!
//! `vessa hook install` writes a `prepare-commit-msg` hook that calls
//! `vessa hook prepare-commit-msg`. That appends one `Agent-Session` trailer
//! for each agent session of this clone (placed by the rule `share` uses)
//! that was active after the commit's parent: the agent, the agent's own
//! session id, how many messages people typed in it since, and when it was
//! active. Only pointers go into a commit, never words, paths or names: the
//! words stay on this machine, where `vessa messages --commit` lists them.
//!
//! The hook never stops or holds up a commit: on any error, or past
//! [`BUDGET`], it adds nothing, says why in one line on stderr, and exits 0.

use crate::import::{self, Span};
use crate::model;
use crate::render::many;
use crate::team::{self, Here};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, FixedOffset};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The trailer's key.
pub const KEY: &str = "Agent-Session";

/// How far back a repository's first commit looks for sessions, having no
/// parent to start from: a day.
const RECENT: i64 = 24 * 3600;

/// How long the hook may read sessions before it gives up and adds nothing.
const BUDGET: Duration = Duration::from_secs(3);

/// The line by which a later install recognises the hook it wrote.
const MARKER: &str =
    "# Installed by `vessa hook install`: Agent-Session pointers; it never stops a commit.";

/// The hook `install` writes: the work is vessa's; whatever happens, the
/// commit goes on.
fn script() -> String {
    format!("#!/bin/sh\n{MARKER}\nvessa hook prepare-commit-msg \"$@\" || true\n")
}

/// `vessa hook install`: the hook in the clone's common git folder, so every
/// worktree of the clone has it. Another hook already there is left alone.
pub fn install() -> Result<i32> {
    let out = team::run(&[
        "rev-parse",
        "--path-format=absolute",
        "--git-common-dir",
        "--git-path",
        "hooks",
    ])
    .context("not inside a git repository")?;
    let mut lines = out.lines();
    let (Some(common), Some(active)) = (lines.next(), lines.next()) else {
        bail!("git did not say where this clone keeps its hooks");
    };
    let folder = Path::new(common).join("hooks");
    if Path::new(active) != folder {
        eprintln!(
            "vessa hook install: refusing: core.hooksPath sends git to {active}, so a hook in {} would never run; unset it, or call `vessa hook prepare-commit-msg \"$@\"` from the prepare-commit-msg hook there",
            folder.display()
        );
        return Ok(1);
    }
    let path = folder.join("prepare-commit-msg");
    let wanted = script();
    match std::fs::read(&path) {
        Ok(existing) if existing == wanted.as_bytes() => {
            println!("unchanged: {}", path.display());
            return Ok(0);
        }
        Ok(existing) => {
            let why = if String::from_utf8_lossy(&existing).contains(MARKER) {
                "is a vessa hook someone has edited"
            } else {
                "exists, and vessa did not write it"
            };
            eprintln!(
                "vessa hook install: refusing: {} {why}; vessa leaves it as it is. Call `vessa hook prepare-commit-msg \"$@\"` from it to add the pointers",
                path.display()
            );
            return Ok(1);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => bail!("cannot read {}: {error}", path.display()),
    }
    std::fs::create_dir_all(&folder)
        .and_then(|()| std::fs::write(&path, &wanted))
        .and_then(|()| executable(&path))
        .with_context(|| format!("cannot write {}", path.display()))?;
    println!("installed: {}", path.display());
    Ok(0)
}

#[cfg(unix)]
fn executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

/// Git for Windows runs hooks through its own `sh`, executable bit or not.
#[cfg(not(unix))]
fn executable(_: &Path) -> std::io::Result<()> {
    Ok(())
}

/// `vessa hook prepare-commit-msg FILE [SOURCE [SHA]]`, as git calls it:
/// adds a trailer for each agent session of this clone active since the
/// commit's parent, reading the sessions under `paths`. Always 0.
pub fn prepare_commit_msg(file: &Path, source: Option<&str>, paths: Vec<PathBuf>) -> i32 {
    // A message the person has yet to write gets none: git aborts a commit
    // left empty, or left as its template, only while it is, and a pointer
    // would make it neither.
    if !matches!(source, Some("message" | "commit" | "merge" | "squash")) {
        eprintln!(
            "vessa: no Agent-Session pointers: the message is yet to be written, and a pointer in it would keep git from aborting a commit left unwritten; commit with -m or -F to have them"
        );
        return 0;
    }
    let started = Instant::now();
    let (sent, received) = std::sync::mpsc::channel();
    let (path, from) = (file.to_path_buf(), source.map(str::to_owned));
    // Read apart, so a slow read can be left behind: the commit goes on.
    std::thread::spawn(move || {
        let _ = sent.send(pointers(&path, from.as_deref(), &paths));
    });
    let found = match received.recv_timeout(BUDGET) {
        Ok(Ok(found)) => found,
        Ok(Err(error)) => {
            eprintln!(
                "vessa: no Agent-Session pointers added: {}",
                shown(&format!("{error:#}"))
            );
            return 0;
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            eprintln!(
                "vessa: no Agent-Session pointers added: reading this clone's agent sessions took over {} s",
                BUDGET.as_secs()
            );
            return 0;
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            eprintln!("vessa: no Agent-Session pointers added: vessa failed while reading");
            return 0;
        }
    };
    for note in &found.notes {
        eprintln!("vessa: {}", shown(note));
    }
    let Some(message) = found.message else {
        return 0;
    };
    // Written here, by vessa alone: no command git is configured with runs.
    match replace(file, &message) {
        Ok(()) => eprintln!(
            "vessa: this commit points to {} ({KEY} trailers, {} ms)",
            many(found.added, "agent session"),
            started.elapsed().as_millis()
        ),
        Err(error) => eprintln!(
            "vessa: no Agent-Session pointers added: cannot write {}: {error}",
            shown(&file.display().to_string())
        ),
    }
    0
}

/// `file` holds `text` in place of what it held, or, should anything fail,
/// still what it held: written beside it, then renamed over it.
fn replace(file: &Path, text: &str) -> std::io::Result<()> {
    let mut beside = file.as_os_str().to_owned();
    beside.push(format!(".vessa-{}", std::process::id()));
    let beside = PathBuf::from(beside);
    let written = std::fs::metadata(file).and_then(|old| {
        std::fs::write(&beside, text)?;
        std::fs::set_permissions(&beside, old.permissions())?;
        std::fs::rename(&beside, file)
    });
    if written.is_err() {
        let _ = std::fs::remove_file(&beside);
    }
    written
}

/// What the hook says on stderr of what it read, as every command says it:
/// scrubbed of secret-looking tokens, and safe for a terminal.
fn shown(text: &str) -> String {
    crate::render::for_terminal(&crate::scrub::Redactions::default().text(text))
}

/// What the hook found to add, and what it has to say.
#[derive(Default)]
struct Found {
    /// The message with its pointers, when it gains any.
    message: Option<String>,
    /// How many trailers it gains.
    added: usize,
    notes: Vec<String>,
}

/// `message` with an `Agent-Session` trailer for each of `values` it does
/// not already hold, where git keeps trailers: after the last paragraph,
/// which they join when it is one of trailers already (not the subject), and
/// before the comments that close the message and the diff `git commit -v`
/// shows below its cut line. None when it gains none.
fn with_trailers(message: &str, values: &[String]) -> Option<(String, usize)> {
    let lines: Vec<&str> = message.split_inclusive('\n').collect();
    let cut = lines
        .iter()
        .position(|line| scissors(line))
        .unwrap_or(lines.len());
    let blank = |line: &str| line.trim().is_empty();
    let mut end = cut;
    while end > 0 && (blank(lines[end - 1]) || lines[end - 1].starts_with('#')) {
        end -= 1;
    }
    let body = &lines[..end];
    let start = body
        .iter()
        .rposition(|line| blank(line))
        .map_or(0, |i| i + 1);
    let paragraph = &body[start..];
    // A trailer line is `Key: value`; a line that opens with a space or a
    // tab goes on the one before.
    let trailer = |line: &&str| {
        line.split_once(':').is_some_and(|(key, _)| {
            !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
    };
    let joined = start > 0
        && paragraph.first().is_some_and(trailer)
        && paragraph
            .iter()
            .all(|line| trailer(line) || line.starts_with([' ', '\t']));
    let mut new: Vec<String> = Vec::new();
    for value in values {
        let line = format!("{KEY}: {value}");
        let held = joined && paragraph.iter().any(|old| old.trim_end() == line);
        if !held && !new.contains(&line) {
            new.push(line);
        }
    }
    if new.is_empty() {
        return None;
    }
    let mut out: String = body.concat();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if !joined && !out.is_empty() {
        out.push('\n');
    }
    for line in &new {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&lines[end..].concat());
    Some((out, new.len()))
}

/// Whether `line` is the line `git commit -v` puts above the diff.
fn scissors(line: &str) -> bool {
    line.starts_with('#') && line.trim_start_matches('#').trim() == SCISSORS
}

/// The pointers a commit's message `file` lacks.
fn pointers(file: &Path, source: Option<&str>, paths: &[PathBuf]) -> Result<Found> {
    let message = std::fs::read(file)
        .with_context(|| format!("cannot read the message {}", file.display()))?;
    let message = String::from_utf8(message).context("the message is not UTF-8")?;
    // `rev-parse` names the git folder even before a first commit, which it
    // says by failing to verify HEAD.
    let out = team::output(
        &["rev-parse", "--absolute-git-dir", "-q", "--verify", "HEAD"],
        None,
        &[],
    )?;
    let said = String::from_utf8_lossy(&out.stdout);
    let mut lines = said.lines();
    let git_dir = PathBuf::from(lines.next().context("not inside a git repository")?);
    let head = lines.next().filter(|_| out.status.success());
    let mut found = Found::default();
    // `git commit -m ""`, or an empty file to -F: git aborts a commit whose
    // message holds no words, and a pointer would keep it from doing so.
    let words = message
        .lines()
        .take_while(|line| !scissors(line))
        .any(|line| !line.trim().is_empty() && !line.starts_with('#'));
    if !words {
        found.notes.push(
            "no Agent-Session pointers: the message holds no words, and a pointer would keep git from aborting the commit".into(),
        );
        return Ok(found);
    }
    // The trailers' values, in the order they are added.
    let mut values: Vec<String> = Vec::new();
    // A rebase, a cherry-pick or `git am` replays commits made before: each
    // keeps the pointers its message carries, and gets none of the
    // sessions active now.
    if ["rebase-merge", "rebase-apply", "CHERRY_PICK_HEAD"]
        .iter()
        .any(|state| git_dir.join(state).exists())
    {
        return Ok(found);
    }
    let mut parents: Vec<String> = head.into_iter().map(str::to_owned).collect();
    // A merge's own work is what came after every commit it joins. git
    // says `merge` only of the message it wrote: one concluded with `-m`
    // is a merge too, as its `MERGE_HEAD` says.
    match std::fs::read_to_string(git_dir.join("MERGE_HEAD")) {
        Ok(merged) => parents.extend(merged.split_whitespace().map(str::to_owned)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => bail!("cannot read MERGE_HEAD: {error}"),
    }
    let since = match newest(&parents)? {
        Some(at) => at,
        None => now()? - chrono::TimeDelta::seconds(RECENT),
    };
    let mut named = named_in(&message);
    if source == Some("squash") {
        // A squash holds the messages of the commits it squashes, indented,
        // under `commit <sha>` lines: their pointers become its own.
        let squashed: Vec<&str> = message
            .lines()
            .filter_map(|line| line.strip_prefix("commit "))
            .filter(|sha| sha.len() >= 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()))
            .collect();
        for value in trailers_of(&squashed)? {
            if let Some(session) = session_of(&value) {
                named.insert(session);
                if !values.contains(&value) {
                    values.push(value);
                }
            }
        }
    }
    let mut here = Here::clone()?;
    let sources = import::written_since(paths, since);
    found.notes.extend(sources.warnings.iter().cloned());
    let active = import::active(&sources, since);
    found.notes.extend(active.warnings.iter().cloned());
    // The sessions of this clone active since, each with its subagents:
    // placed by the rule `share` uses.
    let mut chosen = Vec::new();
    for family in model::families(&active.sessions) {
        // Those of it active since: a session read only to place its
        // subagents was not, and says nothing of this window.
        let members: Vec<&model::Session> = std::iter::once(family.main)
            .chain(family.subagents.iter().copied())
            .filter(|s| active.spans.contains_key(&(s.agent.clone(), s.id.clone())))
            .collect();
        let mut span = None;
        for s in &members {
            if let Some(&(first, last)) = active.spans.get(&(s.agent.clone(), s.id.clone())) {
                import::widen(&mut span, first);
                import::widen(&mut span, last);
            }
        }
        if span.is_none() || here.placed(&family).is_none() {
            continue;
        }
        chosen.push((family.main, members, span));
    }
    // Their messages, read from their own transcripts, and from the
    // transcript a Codex fork was forked from, which tells the history it
    // copied from its own turns.
    let mut read_from: Vec<PathBuf> = chosen
        .iter()
        .flat_map(|(_, members, _)| members)
        .flat_map(|s| {
            let fork = (s.agent == "codex" && !s.subagent)
                .then(|| s.parent.clone())
                .flatten();
            std::iter::once(s.id.clone())
                .chain(fork)
                .filter_map(|id| active.read_from.get(&(s.agent.clone(), id)))
        })
        .flatten()
        .cloned()
        .collect();
    read_from.sort();
    read_from.dedup();
    let mut said: HashMap<(String, String), (usize, Option<Span>)> = HashMap::new();
    let listing = if read_from.is_empty() {
        import::Listing::default()
    } else {
        import::messages(
            &read_from,
            &import::Selection {
                since: Some(since),
                ..Default::default()
            },
            false,
            &mut |m| {
                let seen = said
                    .entry((m.agent.clone(), m.session.clone()))
                    .or_default();
                seen.0 += 1;
                if let Some(at) =
                    m.at.as_deref()
                        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                {
                    import::widen(&mut seen.1, at);
                }
                Ok(())
            },
        )?
    };
    for warning in &listing.warnings {
        if !found.notes.contains(warning) {
            found.notes.push(warning.clone());
        }
    }
    let read: HashMap<(&str, &str), bool> = listing
        .sessions
        .iter()
        .map(|c| ((c.agent.as_str(), c.id.as_str()), c.limited))
        .collect();
    let mut withheld = 0;
    for (main, members, mut span) in chosen {
        let (mut messages, mut partial) = (0, false);
        for s in &members {
            let typed = said.get(&(s.agent.clone(), s.id.clone()));
            if let Some((first, last)) = typed.and_then(|t| t.1) {
                import::widen(&mut span, first);
                import::widen(&mut span, last);
            }
            messages += typed.map_or(0, |t| t.0);
            // A Codex subagent's turns are its parent's prompts, never read
            // for messages; any other transcript not read leaves its count
            // unknown.
            let unread = !(s.agent == "codex" && s.subagent);
            partial |= read
                .get(&(s.agent.as_str(), s.id.as_str()))
                .map_or(unread, |limited| *limited);
        }
        let Some(span) = span else {
            continue;
        };
        let agent = main.agent.clone();
        let id = main.subagent_of().unwrap_or(&main.id).to_owned();
        if named.contains(&(agent.clone(), id.clone())) {
            continue;
        }
        if !team::plain_id(&id) {
            withheld += 1;
            continue;
        }
        values.push(format!(
            "{agent} {id} ({}{}, {})",
            if partial { "at least " } else { "" },
            many(messages, "message"),
            when(span)
        ));
        named.insert((agent, id));
    }
    if withheld > 0 {
        found.notes.push(format!(
            "{} of this clone {} an id that reads as a secret or a path, so no pointer is added for {}",
            many(withheld, "agent session"),
            if withheld == 1 { "has" } else { "have" },
            if withheld == 1 { "it" } else { "them" },
        ));
    }
    if let Some((message, added)) = with_trailers(&message, &values) {
        found.message = Some(message);
        found.added = added;
    }
    Ok(found)
}

/// The sessions the `Agent-Session` lines of a message name, by agent and
/// id: every such line that is no comment, in its trailers or above them,
/// down to the line below which `git commit -v` shows the diff.
fn named_in(message: &str) -> BTreeSet<(String, String)> {
    message
        .lines()
        .take_while(|line| !scissors(line))
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| {
            let line = line.trim();
            let (key, value) = line.split_once(':')?;
            key.trim().eq_ignore_ascii_case(KEY).then_some(value)
        })
        .filter_map(session_of)
        .collect()
}

/// The line `git commit -v` puts above the diff, after its comment sign.
const SCISSORS: &str = "------------------------ >8 ------------------------";

/// The agent and session id an `Agent-Session` value names: its first two
/// words.
pub fn session_of(value: &str) -> Option<(String, String)> {
    let mut words = value.split_whitespace();
    Some((words.next()?.to_owned(), words.next()?.to_owned()))
}

/// When a session was active, to the minute, in UTC: `2026-11-12
/// 11:02–11:40 UTC`, with the second date when it differs.
fn when((first, last): Span) -> String {
    let (first, last) = (first.to_utc(), last.to_utc());
    let minute = "%Y-%m-%d %H:%M";
    let (from, to) = (
        first.format(minute).to_string(),
        last.format(minute).to_string(),
    );
    if from == to {
        format!("{from} UTC")
    } else if first.date_naive() == last.date_naive() {
        format!("{from}–{} UTC", last.format("%H:%M"))
    } else {
        format!("{from}–{to} UTC")
    }
}

fn now() -> Result<DateTime<FixedOffset>> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
    Ok(DateTime::from_timestamp(i64::try_from(now.as_secs())?, 0)
        .context("the clock is out of range")?
        .fixed_offset())
}

/// The commit time of the newest of `commits`; none for none.
fn newest(commits: &[String]) -> Result<Option<DateTime<FixedOffset>>> {
    if commits.is_empty() {
        return Ok(None);
    }
    let mut args = vec!["show", "-s", "--format=%ct", "--end-of-options"];
    args.extend(commits.iter().map(String::as_str));
    let said = team::run(&args)?;
    let mut newest = None;
    for line in said.lines() {
        let at: i64 = line
            .trim()
            .parse()
            .with_context(|| format!("git gave {line:?} for a commit time"))?;
        let at = DateTime::from_timestamp(at, 0).context("a commit time out of range")?;
        newest = newest.max(Some(at.fixed_offset()));
    }
    Ok(newest)
}

/// The values of the `Agent-Session` trailers of `commits`, as git reads
/// them.
fn trailers_of(commits: &[&str]) -> Result<Vec<String>> {
    if commits.is_empty() {
        return Ok(Vec::new());
    }
    let format = format!("--format=%(trailers:key={KEY},valueonly,unfold)");
    let mut args = vec!["show", "-s", &format, "--end-of-options"];
    args.extend(commits);
    Ok(team::run(&args)?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

/// A commit, as `vessa messages --commit` reads it.
pub struct Commit {
    /// Its full id.
    pub id: String,
    /// Its newest parent's time (a first commit's own, less a day), and its
    /// own: the window its sessions' messages are read in, as the hook read
    /// them when it was made.
    pub after: DateTime<FixedOffset>,
    pub until: DateTime<FixedOffset>,
    /// Its `Agent-Session` trailers, each with the session it names, if it
    /// names one.
    pub trailers: Vec<(String, Option<(String, String)>)>,
}

/// The commit `rev` names, with its pointers.
pub fn commit(rev: &str) -> Result<Commit> {
    let format = format!("--format=%H%n%ct%n%P%n%(trailers:key={KEY},valueonly,unfold)");
    let commit = format!("{rev}^{{commit}}");
    let said = team::run(&["show", "-s", &format, "--end-of-options", &commit])?;
    let mut lines = said.lines();
    let id = lines.next().context("git named no commit")?.to_owned();
    let until = lines
        .next()
        .and_then(|t| t.parse().ok())
        .and_then(|t| DateTime::from_timestamp(t, 0))
        .context("git gave no commit time")?
        .fixed_offset();
    let parents: Vec<String> = lines
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let after = match newest(&parents)? {
        Some(at) => at,
        None => until - chrono::TimeDelta::seconds(RECENT),
    };
    let trailers = lines
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|value| (value.to_owned(), session_of(value)))
        .collect();
    Ok(Commit {
        id,
        after,
        until,
        trailers,
    })
}
