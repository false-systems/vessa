//! `vessa collect` and `vessa digest`: what happened, since the person
//! last looked.
//!
//! The human view ohjaa once kept moved here with its 0.9.0 cut, as its
//! step 5 names vessa the home. `collect` gathers what a digest wants into
//! one snapshot, `$XDG_STATE_HOME/vessa/collected.json`
//! (`vessa-collected.v1`), under `collect.lock`, one collect at a time: the
//! sessions of the last seven days, each with its subagents, its usage and
//! the last time a record names, and the sources' warnings. It reads
//! nothing else — no git, no network, no other tool's state — so what it
//! gathers is what every vessa command reads.
//!
//! `digest` shows what happened since the last digest (or `--since`, which
//! moves no cursor), from that snapshot: one line per session, a day at a
//! time, who is live, and the gaps said as gaps. A snapshot older than five
//! minutes is collected first: the digest starts a collect and waits for it
//! at most ten seconds, then shows the snapshot it has while the collect
//! goes on. `--json` prints `vessa-digest.v1` and changes nothing.
//!
//! The digest's cursor (`vessa/digest.json`, `vessa-digest-cursor.v1`)
//! moves only when a person at a terminal reads a full digest: an agent's
//! shell may run the command, and `--since` answers a question rather than
//! catching up, so neither moves it. Exit codes: 0 everything could be
//! read, 4 something could not (it says what), or nothing was collected.

use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::line;
use crate::model;

const COLLECTED_SCHEMA: &str = "vessa-collected.v1";
const CURSOR_SCHEMA: &str = "vessa-digest-cursor.v1";
/// A digest starts a collect past this age of the snapshot.
const COLLECTS_AFTER_SECS: i64 = 5 * 60;
/// How long a digest waits for a collect it started.
const WAITS: Duration = Duration::from_secs(10);
/// A collect lock older than this is a collect that died: taken over.
const STALE_LOCK: Duration = Duration::from_secs(10 * 60);
/// The sessions a snapshot keeps: a digest looks back days, not years.
pub const KEPT: chrono::TimeDelta = chrono::TimeDelta::seconds(7 * 86_400);
/// A session whose last record is this recent is live.
const LIVE_WITHIN: chrono::TimeDelta = chrono::TimeDelta::seconds(30 * 60);
/// A first digest, with no cursor, shows this much.
const FIRST_WINDOW: chrono::TimeDelta = chrono::TimeDelta::seconds(86_400);

/// One session in the snapshot: a family, with its subagents counted and
/// its usage summed (an entry lacking a category makes it unknown, never
/// zero, as every total here).
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub(crate) struct SnapSession {
    pub agent: String,
    pub id: String,
    pub started_at: Option<String>,
    /// The latest time any usage record of the family names.
    pub last_at: Option<String>,
    pub subagents: usize,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cwd: String,
    pub limitations: usize,
}

/// The snapshot a collect writes and a digest reads.
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct Collected {
    pub schema: String,
    pub at: String,
    pub sessions: Vec<SnapSession>,
    /// Sessions read that no record places in time: not in the snapshot,
    /// not hidden either.
    pub unplaced: usize,
    pub warnings: Vec<String>,
    /// The repositories the sessions worked in, each judged by the sykli
    /// of this machine. Absent in an older snapshot: none known then.
    #[serde(default)]
    pub repos: Vec<RepoState>,
    /// Decisions waiting for the person: proposals in ohjaa's state.
    #[serde(default)]
    pub waiting: usize,
}

/// One repository the work touched, as the digest reports it.
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub(crate) struct RepoState {
    /// The git root's folder name.
    pub name: String,
    /// The git root, absolute.
    pub root: String,
    /// The default branch's proof there, as sykli says it: `proven`,
    /// `failed`, `none` — or `unknown`, with `warning` saying why.
    pub proof: String,
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
    pub warning: Option<String>,
}

/// The digest cursor: when a person last read a full digest.
#[derive(Serialize, Deserialize)]
struct Cursor {
    schema: String,
    at: String,
    shown_at: String,
}

/// `vessa collect`: gather the snapshot. One at a time; one started by
/// `digest` says nothing.
pub fn collect(paths: &[PathBuf], warnings: &[String], background: bool) -> Result<i32> {
    let Some(dir) = state_dir() else {
        anyhow::bail!("neither XDG_STATE_HOME nor HOME names a state directory");
    };
    std::fs::create_dir_all(&dir)?;
    let _lock = match Lock::take(&dir.join("collect.lock"), STALE_LOCK, background)? {
        Some(lock) => lock,
        // Held by another collect; `Lock::take` said so unless this one is
        // the background one, which says nothing.
        None => return Ok(0),
    };
    let started = std::time::Instant::now();
    let report = match crate::import::read(paths) {
        Ok(report) => report,
        Err(error) => {
            // The old snapshot stays: a collect that cannot read keeps what
            // it held, as of then.
            eprintln!("vessa collect: the sessions cannot be read: {error:#}");
            return Ok(4);
        }
    };
    let mut warnings = warnings.to_vec();
    warnings.extend(report.warnings);
    let now = line::now();
    let mut sessions = Vec::new();
    let mut unplaced = 0;
    for family in model::families(&report.sessions) {
        let (mut input, mut output, mut cache_read) = (Some(0u64), Some(0u64), Some(0u64));
        let mut timed = 0usize;
        for usage in family.usage() {
            timed += 1;
            input = model::add(input, usage.entry.input_tokens)?;
            output = model::add(output, usage.entry.output_tokens)?;
            cache_read = model::add(cache_read, usage.entry.cache_read_tokens)?;
        }
        let unknown = |sum: Option<u64>| if timed == 0 { None } else { sum };
        let last_at = last_at(family.usage());
        let placed = last_at
            .clone()
            .or_else(|| family.main.started_at.clone())
            .and_then(|at| parse(&at));
        // Not placed in time, or older than the snapshot keeps: the first
        // is a gap the collect reports; the second is not news.
        match placed {
            None => {
                unplaced += 1;
                continue;
            }
            Some(at) if now - at > KEPT => continue,
            Some(_) => {}
        }
        sessions.push(SnapSession {
            agent: family.main.agent.clone(),
            id: family.main.id.clone(),
            started_at: family.main.started_at.clone(),
            last_at,
            subagents: family.subagents.len(),
            input_tokens: unknown(input),
            output_tokens: unknown(output),
            cache_read_tokens: unknown(cache_read),
            cwd: family.main.cwd.clone(),
            limitations: family.main.limitations.len().saturating_add(
                family
                    .subagents
                    .iter()
                    .map(|session| session.limitations.len())
                    .sum(),
            ),
        });
    }
    sessions.sort_by(|a, b| b.last_at.cmp(&a.last_at));
    // The work: the repositories the sessions ran in, each judged by this
    // machine's sykli (the repos part of the human view; a session's folder
    // is where it ran, resolved to its git root). Collected apart, so a
    // session that cannot be placed in time still names its repository.
    let mut roots: Vec<PathBuf> = Vec::new();
    for session in &report.sessions {
        let cwd = PathBuf::from(&session.cwd);
        if cwd.as_os_str().is_empty() || roots.iter().any(|known| known == &cwd) {
            continue;
        }
        if let Ok(root) = git(
            &cwd,
            &["rev-parse", "--path-format=absolute", "--show-toplevel"],
        ) {
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
    }
    roots.sort();
    let repos: Vec<RepoState> = roots.iter().map(|root| repo_state(root)).collect();
    // Waiting for the person: proposals in ohjaa's state, counted, nothing
    // else read of them.
    let waiting = state_dir()
        .and_then(|vessa| vessa.parent().map(|state| state.join("ohjaa/proposals")))
        .map(|proposals| count_files(&proposals))
        .unwrap_or(0);
    let collected = Collected {
        schema: COLLECTED_SCHEMA.to_owned(),
        at: now.to_rfc3339(),
        sessions,
        unplaced,
        warnings,
        repos,
        waiting,
    };
    write(
        &dir.join("collected.json"),
        &serde_json::to_vec(&collected)?,
    )?;
    if !background {
        println!(
            "vessa: collected {} in {:.1}s; {}",
            crate::render::many(collected.sessions.len(), "session"),
            started.elapsed().as_secs_f64(),
            if collected.unplaced == 0 {
                format!("the last {} days", KEPT.num_days())
            } else {
                format!("{} more could not be placed in time", collected.unplaced)
            }
        );
        for warning in &collected.warnings {
            eprintln!("vessa: {warning}");
        }
    }
    Ok(
        if collected.warnings.is_empty() && collected.unplaced == 0 {
            0
        } else {
            4
        },
    )
}

/// `vessa digest`: what happened since the person last looked.
pub fn digest(paths: &[PathBuf], since: Option<&str>, no_collect: bool, json: bool) -> Result<i32> {
    let Some(dir) = state_dir() else {
        anyhow::bail!("neither XDG_STATE_HOME nor HOME names a state directory");
    };
    let now = line::now();
    let mut pending = None;
    let mut collected = read(&dir);
    if !no_collect
        && collected.as_ref().is_none_or(|c| {
            now - parse(&c.at).unwrap_or(now) > chrono::TimeDelta::seconds(COLLECTS_AFTER_SECS)
        })
    {
        let before = collected.as_ref().map(|c| c.at.clone());
        start_collect(paths);
        let deadline = std::time::Instant::now() + WAITS;
        loop {
            collected = read(&dir);
            if collected.as_ref().map(|c| c.at.clone()) != before {
                break;
            }
            if std::time::Instant::now() >= deadline {
                pending = Some("a collect goes on in the background; the snapshot is as it was");
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let Some(collected) = collected else {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "schema": "vessa-digest.v1",
                    "collected": false
                }))?
            );
        } else {
            println!("vessa digest: nothing was collected yet; run vessa collect");
        }
        return Ok(4);
    };
    // The window: a question asked with --since, or catch-up from the
    // cursor, or the first day.
    let from = match since.map(crate::since).transpose()? {
        Some(from) => from.with_timezone(&chrono::Utc),
        None => cursor(&dir)
            .and_then(|cursor| parse(&cursor.at))
            .unwrap_or(now - FIRST_WINDOW),
    };
    let as_of = parse(&collected.at).unwrap_or(now);
    let in_window: Vec<&SnapSession> = collected
        .sessions
        .iter()
        .filter(|session| {
            time_of(session).is_some_and(|at| at >= from && at <= as_of + LIVE_WITHIN)
        })
        .collect();
    let live: Vec<&SnapSession> = collected
        .sessions
        .iter()
        .filter(|session| {
            session
                .last_at
                .as_deref()
                .and_then(parse)
                .is_some_and(|at| as_of - at <= LIVE_WITHIN && at >= from)
        })
        .collect();
    let mut text = String::new();
    let mut day = String::new();
    for session in in_window.iter().rev() {
        let at = time_of(session).expect("the window filtered on a time");
        let this_day = at.date_naive().to_string();
        if this_day != day {
            day = this_day;
            text.push_str(&format!("\n{day}\n"));
        }
        let when = at.format("%H:%M");
        let where_ = Path::new(&session.cwd)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| session.cwd.clone());
        let tokens = match (&session.input_tokens, &session.output_tokens) {
            (Some(input), Some(output)) => format!(
                "{} in · {} out",
                crate::line::magnitude(*input),
                crate::line::magnitude(*output)
            ),
            _ => "usage unknown".to_owned(),
        };
        let subagents = match session.subagents {
            0 => String::new(),
            n => format!("  ({})", crate::render::many(n, "subagent")),
        };
        text.push_str(&format!(
            "  {when}  {}  {where_}  {tokens}{subagents}\n",
            session.agent
        ));
    }
    let mut by_agent: Vec<(&str, usize, Option<u64>, Option<u64>)> = Vec::new();
    for session in &in_window {
        let entry = by_agent
            .iter_mut()
            .find(|(agent, ..)| *agent == session.agent);
        match entry {
            Some((_, sessions, input, output)) => {
                *sessions += 1;
                *input = model::add(*input, session.input_tokens)?;
                *output = model::add(*output, session.output_tokens)?;
            }
            None => by_agent.push((
                &session.agent,
                1,
                session.input_tokens,
                session.output_tokens,
            )),
        }
    }
    if !by_agent.is_empty() {
        let parts: Vec<String> = by_agent
            .iter()
            .map(|(agent, sessions, input, output)| {
                format!(
                    "{agent}: {} · {} in · {} out",
                    crate::render::many(*sessions, "session"),
                    input
                        .map(crate::line::magnitude)
                        .unwrap_or_else(|| "unknown".into()),
                    output
                        .map(crate::line::magnitude)
                        .unwrap_or_else(|| "unknown".into()),
                )
            })
            .collect();
        text.push_str(&format!("\nby agent: {}\n", parts.join("; ")));
    }
    if !live.is_empty() {
        let agents: Vec<&str> = live.iter().map(|s| s.agent.as_str()).collect();
        text.push_str(&format!(
            "\nlive now: {} ({})\n",
            crate::render::many(live.len(), "session"),
            agents.join(", ")
        ));
    }
    // The work: the repositories the sessions ran in, each judged by sykli.
    // Said whatever the snapshot holds, however old — a collect refreshes it.
    if !collected.repos.is_empty() {
        text.push_str("\nthe work\n");
        for repo in &collected.repos {
            let mut line = format!("  {}  main {}", repo.name, repo.proof);
            if let Some(ahead) = repo.ahead.filter(|ahead| *ahead > 0) {
                line.push_str(&format!(", {ahead} ahead"));
            }
            if let Some(behind) = repo.behind.filter(|behind| *behind > 0) {
                line.push_str(&format!(", {behind} behind"));
            }
            if let Some(why) = &repo.warning {
                line.push_str(&format!(" ({why})"));
            }
            text.push_str(&line);
            text.push('\n');
        }
    }
    if collected.waiting > 0 {
        text.push_str(&format!(
            "\nwaiting for you: {} in ohjaa (`ohjaa review`)\n",
            crate::render::many(collected.waiting, "proposal")
        ));
    }
    let repos_unread = collected
        .repos
        .iter()
        .filter(|repo| repo.warning.is_some())
        .count();
    let gaps = collected.warnings.len() + collected.unplaced + repos_unread;
    if gaps > 0 {
        let mut said = Vec::new();
        if collected.unplaced > 0 {
            said.push(format!(
                "{} not placed in time",
                crate::render::many(collected.unplaced, "session")
            ));
        }
        if !collected.warnings.is_empty() {
            said.push(crate::render::many(
                collected.warnings.len(),
                "source warning",
            ));
        }
        text.push_str(&format!(
            "\nnot all read: {} (vessa sessions says more)\n",
            said.join(", ")
        ));
    }
    if let Some(pending) = pending {
        text.push_str(&format!("\n{pending}\n"));
    }
    let record = serde_json::json!({
        "schema": "vessa-digest.v1",
        "since": from.to_rfc3339(),
        "collected_at": collected.at,
        "sessions": in_window.iter().map(|session| serde_json::json!({
            "agent": session.agent, "id": session.id,
            "started_at": session.started_at, "last_at": session.last_at,
            "subagents": session.subagents,
            "input_tokens": session.input_tokens,
            "output_tokens": session.output_tokens,
            "cache_read_tokens": session.cache_read_tokens,
            "cwd": session.cwd, "limitations": session.limitations,
        })).collect::<Vec<_>>(),
        "by_agent": by_agent.iter().map(|(agent, sessions, input, output)| serde_json::json!({
            "agent": agent, "sessions": sessions,
            "input_tokens": input, "output_tokens": output,
        })).collect::<Vec<_>>(),
        "live": live.iter().map(|session| session.agent.clone()).collect::<Vec<_>>(),
        "warnings": collected.warnings,
        "unplaced": collected.unplaced,
        "repos": collected.repos,
        "waiting": collected.waiting,
    });
    if json {
        println!("{}", serde_json::to_string_pretty(&record)?);
        return Ok(if gaps > 0 { 4 } else { 0 });
    }
    println!("what happened since {} UTC", from.format("%Y-%m-%d %H:%M"));
    print!("{text}");
    // The cursor moves only when a person at a terminal catches up, not for
    // a question asked with --since, and not in an agent's shell.
    if since.is_none()
        && std::io::stdout().is_terminal()
        && line::person_watching(|name: &str| std::env::var_os(name))
    {
        write(
            &dir.join("digest.json"),
            &serde_json::to_vec(&Cursor {
                schema: CURSOR_SCHEMA.to_owned(),
                at: now.to_rfc3339(),
                shown_at: now.to_rfc3339(),
            })?,
        )?;
    }
    Ok(if gaps > 0 { 4 } else { 0 })
}

/// The latest time any usage record of the family names, as RFC 3339.
fn last_at<'a>(usage: impl Iterator<Item = &'a model::Usage>) -> Option<String> {
    usage
        .filter_map(|usage| usage.at.as_deref())
        .filter_map(parse)
        .max()
        .map(|at| at.to_rfc3339())
}

/// A session's place in time: its last record, else its start.
fn time_of(session: &SnapSession) -> Option<chrono::DateTime<chrono::Utc>> {
    session
        .last_at
        .as_deref()
        .or(session.started_at.as_deref())
        .and_then(parse)
}

fn parse(text: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.with_timezone(&chrono::Utc))
}

/// One git answer, run in `cwd`: its stdout, trimmed. `git` is how a
/// repository is found and named; vessa itself reads nothing of it beyond
/// the root.
fn git(cwd: &Path, args: &[&str]) -> Result<PathBuf, String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|error| format!("git cannot run: {error}"))?;
    if !output.status.success() {
        return Err(format!("git {:?} failed in {}", args, cwd.display()));
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim(),
    ))
}

/// What this machine's sykli says of the repository at `root`: its default
/// branch's proof, ahead and behind. Sykli absent, or an answer vessa cannot
/// read, is a warning in the report — never a guess.
fn repo_state(root: &Path) -> RepoState {
    let name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string());
    let said = std::process::Command::new("sykli")
        .args(["status", "--json"])
        .current_dir(root)
        .output();
    let mut state = RepoState {
        name,
        root: root.display().to_string(),
        proof: "unknown".to_owned(),
        ahead: None,
        behind: None,
        warning: None,
    };
    let output = match said {
        Ok(output) => output,
        Err(error) => {
            state.warning = Some(format!("sykli cannot run: {error}"));
            return state;
        }
    };
    if !output.status.success() {
        state.warning = Some(format!(
            "sykli status exited {} in {}",
            output.status.code().unwrap_or(-1),
            root.display()
        ));
        return state;
    }
    let read: serde_json::Value = match serde_json::from_slice(&output.stdout) {
        Ok(read) => read,
        Err(error) => {
            state.warning = Some(format!("sykli's answer is not JSON: {error}"));
            return state;
        }
    };
    // `sykli-branch-status.v1`: the base names the default branch, and the
    // entry of that branch carries its proof.
    let base = read["base"]["branch"].as_str().unwrap_or("main").to_owned();
    let entry = read["branches"]
        .as_array()
        .and_then(|branches| {
            branches
                .iter()
                .find(|entry| entry["branch"].as_str() == Some(base.as_str()))
                .cloned()
        })
        .unwrap_or(serde_json::Value::Null);
    state.proof = entry["proof"].as_str().unwrap_or("unknown").to_owned();
    state.ahead = entry["ahead"].as_u64();
    state.behind = entry["behind"].as_u64();
    state
}

/// Every regular file under `dir`, recursively: the count of what waits.
fn count_files(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut count = 0;
    for entry in entries.flatten() {
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => count += count_files(&entry.path()),
            Ok(_) => count += 1,
            Err(_) => {}
        }
    }
    count
}

fn state_dir() -> Option<PathBuf> {
    let absolute = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
    match absolute {
        Some(state) => Some(state.join("vessa")),
        None => std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join(".local")
                .join("state")
                .join("vessa")
        }),
    }
}

/// The last snapshot, wherever this command runs: what the line reads for
/// what waits. Old snapshots parse too — their absent parts are defaults.
pub(crate) fn read_snapshot() -> Option<Collected> {
    state_dir().and_then(|dir| read(&dir))
}

fn read(dir: &Path) -> Option<Collected> {
    let text = std::fs::read_to_string(dir.join("collected.json")).ok()?;
    let collected: Collected = serde_json::from_str(&text).ok()?;
    (collected.schema == COLLECTED_SCHEMA).then_some(collected)
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes).with_context(|| format!("{} cannot be written", path.display()))
}

fn cursor(dir: &Path) -> Option<Cursor> {
    let text = std::fs::read_to_string(dir.join("digest.json")).ok()?;
    let cursor: Cursor = serde_json::from_str(&text).ok()?;
    (cursor.schema == CURSOR_SCHEMA).then_some(cursor)
}

/// Start a collect detached, as `digest` does: it finishes whether or not
/// this waits for it. Nothing is said where it fails: the digest says the
/// snapshot is as it was.
fn start_collect(paths: &[PathBuf]) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut command = std::process::Command::new(exe);
    command.arg("collect").arg("--background");
    for path in paths {
        command.arg("--input").arg(path);
    }
    let _ = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// One collect at a time: a lock file made whole, taken over when its
/// holder died. Held by another collect: said, and the caller goes on.
struct Lock(Option<PathBuf>);

impl Lock {
    /// The lock, or `None` when another collect holds it: said, unless the
    /// caller is the background one, which says nothing.
    fn take(path: &Path, stale_after: Duration, background: bool) -> Result<Option<Self>> {
        let make = || {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
        };
        match make() {
            Ok(_) => Ok(Some(Lock(Some(path.to_path_buf())))),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                // A lock whose holder died is taken over, once.
                let died = std::fs::metadata(path)
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|held| held.elapsed().ok())
                    .is_some_and(|held| held > stale_after);
                if died {
                    let _ = std::fs::remove_file(path);
                    if make().is_ok() {
                        return Ok(Some(Lock(Some(path.to_path_buf()))));
                    }
                }
                if !background {
                    eprintln!(
                        "vessa collect: a collect is running already; its snapshot is the next one ({})",
                        path.display()
                    );
                }
                Ok(None)
            }
            Err(error) => {
                Err(error).with_context(|| format!("{} cannot be locked", path.display()))
            }
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}
