use crate::model::{Conversation, Coverage, Entry, Message, Reply, Report, Session, Usage};
use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub fn read(paths: &[PathBuf]) -> Result<Report> {
    // ponytail: selected transcripts are held in memory; stream compact
    // summaries if archive size makes this one-shot reader too expensive.
    let (mut report, wrappers) = sessions(paths, parse_file, true);
    roots(&mut report.sessions.iter_mut().collect::<Vec<_>>(), true);
    deduplicate(&mut report);
    link(&mut report, wrappers);
    Ok(report)
}

/// The sessions under `paths` with their headers alone: what each transcript
/// says about itself, read by the same rules as [`read`], from only as many
/// of its records as settle them (see [`Scan::settled`]). No usage is read,
/// so the sessions carry no usage and no limitations.
pub fn headers(paths: &[PathBuf]) -> Result<Report> {
    let (mut report, wrappers) = sessions(paths, header_file, false);
    // Headers carry no limitations: a looping lineage stands alone silently.
    roots(&mut report.sessions.iter_mut().collect::<Vec<_>>(), false);
    report.schema = "vessa-session-headers.v1".into();
    link(&mut report, wrappers);
    Ok(report)
}

/// Every transcript under `paths`, read with `parse`, one session per native
/// identity, parents first; with the linked session records given. Both
/// readers start here, so they list the same sessions in the same order.
/// An opencode store's sessions come with them, their usage read when
/// `usage`.
fn sessions(
    paths: &[PathBuf],
    parse: fn(&Path) -> Result<Parsed>,
    usage: bool,
) -> (Report, Vec<Value>) {
    let mut report = Report {
        schema: "vessa-usage.v2".into(),
        ..Default::default()
    };
    let (files, stores) = transcripts(paths, &mut report.warnings);
    let mut wrappers = Vec::new();
    // Each store's model responses, read once, for all its sessions.
    let mut responses: HashMap<PathBuf, Result<crate::opencode::BySession, String>> =
        HashMap::new();
    for (store, stored) in opencode_sessions(&stores, &mut warn(&mut report.warnings)) {
        let mut s = stored.session;
        if usage {
            let read = responses.entry(store.clone()).or_insert_with(|| {
                crate::opencode::responses(&store).map_err(|error| format!("{error:#}"))
            });
            match read {
                Ok(by_session) => {
                    for (line, mut v) in by_session.remove(&s.id).unwrap_or_default() {
                        // A fork's copy of a response is that response: under
                        // its id, it is counted once, with the session forked
                        // from, which comes first as a fork's parent.
                        if let Some(origin) = stored.facts.copied.get(&line) {
                            v["id"] = origin.as_str().into();
                        }
                        if let Some(why) = v["malformed"].as_str() {
                            s.limitations.push(format!("message {line}: {why}"));
                        } else if let Err(error) = opencode(&v, line, &mut s) {
                            s.limitations.push(format!("message {line}: {error:#}"));
                        }
                        // The files its tool parts name, against its folder.
                        for path in v["data"]["paths"].as_array().into_iter().flatten() {
                            if let Some(path) = path.as_str() {
                                touch(&mut s.touched, path, &s.cwd);
                            }
                        }
                    }
                }
                Err(why) => s
                    .limitations
                    .push(format!("messages unreadable, so no usage read: {why}")),
            }
        }
        report.sessions.push(s);
    }
    for file in files {
        match parse(&file) {
            Ok(Parsed::Session(session)) => report.sessions.push(*session),
            Ok(Parsed::Wrappers(records, warnings)) => {
                wrappers.extend(records);
                report.warnings.extend(warnings);
            }
            Err(error) => report
                .warnings
                .push(format!("{}: {error:#}", file.display())),
        }
    }
    // Native IDs survive resume. Merge transcript copies, then deduplicate real
    // request/message identities; never use equal text to deduplicate usage.
    let mut merged: BTreeMap<(String, String), Session> = BTreeMap::new();
    for session in report.sessions {
        let key = (session.agent.clone(), session.id.clone());
        if let Some(old) = merged.get_mut(&key) {
            old.usage.extend(session.usage);
            old.work.extend(session.work);
            old.limitations.extend(session.limitations);
            old.launched.extend(session.launched);
            old.synthetic.extend(session.synthetic);
            old.touched.extend(session.touched);
        } else {
            merged.insert(key, session);
        }
    }
    report.sessions = merged.into_values().collect();
    // Process parents before descendants, so copied history stays with its
    // original session when both sources are available.
    report.sessions.sort_by_key(|s| s.parent.is_some());
    (report, wrappers)
}

/// Points each Codex subagent at the session at its root, through the
/// threads read. Newer Codex names the root itself (`session_id`); older
/// Codex names only the thread a subagent came from, which may be another
/// subagent. An opencode subagent names the session that started it, which
/// may be another subagent, and is followed the same way. A lineage that
/// loops has no root: each thread in it stands alone, and with `limit` says
/// so in its limitations.
fn roots(sessions: &mut [&mut Session], limit: bool) {
    let walked = |s: &Session| matches!(s.agent.as_str(), "codex" | "opencode");
    let links: HashMap<(String, String), String> = sessions
        .iter()
        .filter(|s| walked(s))
        .filter_map(|s| Some(((s.agent.clone(), s.id.clone()), s.root.clone()?)))
        .collect();
    for session in sessions.iter_mut().filter(|s| walked(s)) {
        let Some(mut root) = session.root.take() else {
            continue;
        };
        // The threads walked so far; reaching one again is a loop.
        let mut path = vec![session.id.clone()];
        let looped = loop {
            let Some(next) = links.get(&(session.agent.clone(), root.clone())) else {
                break false;
            };
            let again = path.contains(&root);
            path.push(std::mem::replace(&mut root, next.clone()));
            if again {
                break true;
            }
        };
        if !looped {
            session.root = Some(root);
        } else if limit {
            session.limitations.push(format!(
                "{} subagent lineage loops ({}); its root session is unknown, so it counts as a session of its own",
                if session.agent == "codex" { "Codex" } else { "opencode" },
                path.join(" -> ")
            ));
        }
    }
}

/// Every file under `paths` that may be a transcript or an ohjaa export, in
/// path order, once each, and the opencode stores given; what could not be
/// listed is a warning.
fn transcripts(paths: &[PathBuf], warnings: &mut Vec<String>) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let sources = transcript_sources(paths);
    warnings.extend(
        sources
            .gaps
            .into_iter()
            .map(|(path, why)| format!("{}: {why}", path.display())),
    );
    (sources.files, sources.stores)
}

struct Sources {
    files: Vec<PathBuf>,
    /// opencode stores: SQLite databases, each holding many sessions.
    stores: Vec<PathBuf>,
    gaps: Vec<(PathBuf, String)>,
}

fn transcript_sources(paths: &[PathBuf]) -> Sources {
    let mut files = Vec::new();
    let mut stores = Vec::new();
    let mut gaps = Vec::new();
    for path in paths {
        if crate::opencode::is_store(path) {
            match path.canonicalize() {
                Ok(store) => stores.push(store),
                Err(error) => gaps.push((path.clone(), error.to_string())),
            }
            continue;
        }
        if let Err(error) = discover(path, &mut files, &mut stores) {
            gaps.push((path.clone(), error.to_string()));
        }
        // Claude Code keeps a session's subagent transcripts beside it, in
        // `<session>/subagents/`. They are that session's usage, so a
        // transcript selected on its own brings them along.
        let subagents = path.with_extension("").join("subagents");
        if path.extension().is_some_and(|s| s == "jsonl")
            && std::fs::symlink_metadata(&subagents).is_ok()
        {
            if let Err(error) = discover(&subagents, &mut files, &mut stores) {
                gaps.push((subagents, error.to_string()));
            }
        }
    }
    files.sort();
    files.dedup();
    stores.sort();
    stores.dedup();
    Sources {
        files,
        stores,
        gaps,
    }
}

/// When a session was active: its first and last dated record.
pub type Span = (
    chrono::DateTime<chrono::FixedOffset>,
    chrono::DateTime<chrono::FixedOffset>,
);

/// Widens `span` to hold `at`.
pub fn widen(span: &mut Option<Span>, at: chrono::DateTime<chrono::FixedOffset>) {
    *span = Some(match *span {
        Some((first, last)) => (first.min(at), last.max(at)),
        None => (at, at),
    });
}

/// The session sources written to after a time.
pub struct Written {
    /// Transcripts and opencode stores, by their canonical paths.
    pub paths: Vec<PathBuf>,
    /// The other transcripts: what a session active since may count with.
    pub others: Vec<PathBuf>,
    /// What could not be listed or looked at.
    pub warnings: Vec<String>,
}

/// The transcripts and opencode stores under `paths` last written to after
/// `since`, found as every reader finds them; only their metadata is read.
/// A file of linked session records holds no session, and is left out.
pub fn written_since(paths: &[PathBuf], since: chrono::DateTime<chrono::FixedOffset>) -> Written {
    let mut warnings = Vec::new();
    let (files, stores) = transcripts(paths, &mut warnings);
    let (mut written, mut others) = (Vec::new(), Vec::new());
    let modified = |path: &Path| std::fs::metadata(path).and_then(|m| m.modified());
    for path in files.into_iter().chain(stores.iter().cloned()) {
        if path.extension().is_some_and(|s| s == "json") {
            continue;
        }
        let mut at = modified(&path);
        // SQLite writes a store's changes to its log, `<store>-wal`, first:
        // the store's own file can be older than what it holds.
        if stores.contains(&path) {
            let mut log = path.clone().into_os_string();
            log.push("-wal");
            if let (Ok(store), Ok(log)) = (&at, modified(Path::new(&log))) {
                at = Ok(log.max(*store));
            }
        }
        match at {
            Ok(at) if chrono::DateTime::<chrono::Utc>::from(at) > since => written.push(path),
            Ok(_) if !stores.contains(&path) => others.push(path),
            Ok(_) => {}
            Err(error) => warnings.push(format!("{}: {error}", path.display())),
        }
    }
    Written {
        paths: written,
        others,
        warnings,
    }
}

/// The sessions active after a time, as the commit hook reads them.
#[derive(Default)]
pub struct Active {
    /// Each session read, with its header and the files and folders its
    /// structured tool calls name, as `share` places it; no usage is read.
    /// Parents first, then by agent and id.
    pub sessions: Vec<Session>,
    /// When each was active after that time, by agent and id: its records
    /// dated after it, and for an opencode session its model responses.
    pub spans: HashMap<(String, String), Span>,
    /// Where each was read: its transcripts, or its store.
    pub read_from: HashMap<(String, String), Vec<PathBuf>>,
    pub warnings: Vec<String>,
}

/// The sessions of `written` (from [`written_since`]), each with when it was
/// active after `since`: a transcript read one record at a time, keeping
/// none, several at once; of a store, only its sessions with a message
/// after `since`, and those they count with. A subagent counts with its
/// session, whose folder and files place it: a session whose own transcript
/// was last written before `since` is read too, for that, when a subagent of
/// it was active after. Copies of one session are one, as every reader
/// merges them.
pub fn active(written: &Written, since: chrono::DateTime<chrono::FixedOffset>) -> Active {
    let mut found = Active::default();
    let (stores, files): (Vec<PathBuf>, Vec<PathBuf>) = written
        .paths
        .iter()
        .cloned()
        .partition(|path| crate::opencode::is_store(path));
    let mut read = Vec::new();
    let take = |files: &[PathBuf], read: &mut Vec<_>, warnings: &mut Vec<String>| {
        for (path, result) in files
            .iter()
            .zip(each(files, |path| active_file(path, since)))
        {
            match result {
                Ok(session) => read.push((path.clone(), session)),
                Err(error) => warnings.push(format!("{}: {error:#}", path.display())),
            }
        }
    };
    take(&files, &mut read, &mut found.warnings);
    for store in &stores {
        match active_store(store, since, &mut found.warnings) {
            Ok(sessions) => read.extend(sessions.into_iter().map(|s| (store.clone(), s))),
            Err(error) => found
                .warnings
                .push(format!("{}: {error:#}", store.display())),
        }
    }
    // The sessions subagents count with that were not read: each in the
    // transcript whose name ends with its id, as the agents name them,
    // read and checked. A Codex subagent may name a thread that is a
    // subagent too, so this goes on until none is wanted.
    let mut looked = std::collections::HashSet::new();
    loop {
        let mut sessions: Vec<&mut Session> = read.iter_mut().map(|(_, (s, _))| s).collect();
        roots(&mut sessions, false);
        let have: std::collections::HashSet<(String, String)> = read
            .iter()
            .map(|(_, (s, _))| (s.agent.clone(), s.id.clone()))
            .collect();
        let wanted: BTreeMap<(String, String), Vec<PathBuf>> = read
            .iter()
            .filter_map(|(_, (s, _))| Some((s.agent.clone(), s.subagent_of()?.to_owned())))
            .filter(|key| !have.contains(key) && looked.insert(key.clone()))
            .map(|(agent, id)| {
                let named = written
                    .others
                    .iter()
                    .filter(|p| {
                        p.file_stem()
                            .and_then(|s| s.to_str())
                            .is_some_and(|stem| stem.ends_with(id.as_str()))
                    })
                    .cloned()
                    .collect();
                ((agent, id), named)
            })
            .collect();
        if wanted.is_empty() {
            break;
        }
        let mut more = Vec::new();
        for named in wanted.values() {
            take(named, &mut more, &mut found.warnings);
        }
        read.extend(
            more.into_iter()
                .filter(|(_, (s, _))| wanted.contains_key(&(s.agent.clone(), s.id.clone()))),
        );
    }
    let mut merged: BTreeMap<(String, String), Session> = BTreeMap::new();
    for (path, (session, span)) in read {
        let key = (session.agent.clone(), session.id.clone());
        if let Some((first, last)) = span {
            let into = found.spans.entry(key.clone()).or_insert((first, last));
            *into = (into.0.min(first), into.1.max(last));
        }
        found.read_from.entry(key.clone()).or_default().push(path);
        match merged.get_mut(&key) {
            Some(old) => old.touched.extend(session.touched),
            None => {
                merged.insert(key, session);
            }
        }
    }
    let mut sessions: Vec<Session> = merged.into_values().collect();
    roots(&mut sessions.iter_mut().collect::<Vec<_>>(), false);
    sessions.sort_by_key(|s| s.parent.is_some());
    found.sessions = sessions;
    found
}

/// `work` done on each of `items`, on as many threads as this machine runs
/// at once, the results in the items' order.
fn each<T: Send>(items: &[PathBuf], work: impl Fn(&Path) -> T + Sync) -> Vec<T> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let threads = std::thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .min(items.len());
    let mut done: Vec<(usize, T)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(item) = items.get(i) else {
                            return done;
                        };
                        done.push((i, work(item)));
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| {
                worker
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    });
    done.sort_by_key(|(i, _)| *i);
    done.into_iter().map(|(_, result)| result).collect()
}

/// One transcript's header, the paths its tool calls name, and when it was
/// active after `since`, read one record at a time. A record that is not
/// JSON is skipped, as `share` skips it.
fn active_file(
    path: &Path,
    since: chrono::DateTime<chrono::FixedOffset>,
) -> Result<(Session, Option<Span>)> {
    let file = BufReader::new(std::fs::File::open(path)?);
    let mut scan = Scan::default();
    // The records before one says which agent wrote the file, which decides
    // how its tool calls are read.
    let mut early = Vec::new();
    let mut agent = None;
    let (mut cwd, mut touched, mut span) = (String::new(), Default::default(), None);
    for line in file.split(b'\n') {
        let line = line?;
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let Ok(v) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        scan.observe(&v);
        if let Some(at) = timestamp(&v)
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok())
            .filter(|at| *at > since)
        {
            widen(&mut span, at);
        }
        match agent {
            Some(agent) => self::touched(&v, agent, &mut cwd, &mut touched),
            None => {
                early.push(v);
                if let Some((name, _)) = scan.opening() {
                    agent = Some(name);
                    for v in early.drain(..) {
                        self::touched(&v, name, &mut cwd, &mut touched);
                    }
                }
            }
        }
    }
    let mut session = scan.session(path, Vec::new())?;
    session.touched = touched;
    Ok((session, span))
}

/// The sessions of an opencode store with a message created after `since`,
/// each with the paths its tool parts name and when it was active. What the
/// store says it holds and vessa does not read is a warning.
fn active_store(
    store: &Path,
    since: chrono::DateTime<chrono::FixedOffset>,
    warnings: &mut Vec<String>,
) -> Result<Vec<(Session, Option<Span>)>> {
    let (stored, said) = crate::opencode::sessions(store)?;
    warnings.extend(
        said.into_iter()
            .map(|why| format!("{}: {why}", store.display())),
    );
    let after =
        |s: &crate::opencode::Stored| s.facts.last.is_some_and(|ms| ms > since.timestamp_millis());
    // The sessions active since, and each one they count with, up its
    // chain: a subagent's session places it.
    let by_id: HashMap<String, &crate::opencode::Stored> =
        stored.iter().map(|s| (s.session.id.clone(), s)).collect();
    let mut kept = std::collections::BTreeSet::new();
    for s in stored.iter().filter(|s| after(s)) {
        let mut at = Some(s);
        while let Some(s) = at.filter(|s| kept.insert(s.session.id.clone())) {
            at = s
                .session
                .root
                .as_ref()
                .and_then(|up| by_id.get(up).copied());
        }
    }
    drop(by_id);
    let stored: Vec<_> = stored
        .into_iter()
        .filter(|s| kept.contains(&s.session.id))
        .collect();
    if stored.is_empty() {
        return Ok(Vec::new());
    }
    let mut responses = crate::opencode::responses(store)?;
    let mut sessions = Vec::new();
    for s in stored {
        let mut session = s.session;
        // Its newest message, whoever wrote it, then its responses.
        let mut span = None;
        if let Some(last) = s
            .facts
            .last
            .and_then(chrono::DateTime::from_timestamp_millis)
            .filter(|last| *last > since)
        {
            widen(&mut span, last.fixed_offset());
        }
        for (_, v) in responses.remove(&session.id).unwrap_or_default() {
            if let Some(at) = timestamp(&v)
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok())
                .filter(|at| *at > since)
            {
                widen(&mut span, at);
            }
            for path in v["data"]["paths"].as_array().into_iter().flatten() {
                if let Some(path) = path.as_str() {
                    touch(&mut session.touched, path, &session.cwd);
                }
            }
        }
        sessions.push((session, span));
    }
    Ok(sessions)
}

/// Counts each request once across the sessions, and says what the usage
/// evidence leaves unknown.
fn deduplicate(report: &mut Report) {
    // ponytail: lineage lookup scans the selected sessions; index IDs and
    // source paths if large archives make that scan measurable.
    let pi_lineages: HashMap<_, _> = report
        .sessions
        .iter()
        .filter(|s| s.agent == "pi")
        .map(|s| {
            let mut current = s;
            let mut visited = std::collections::HashSet::new();
            while visited.insert(current.id.as_str()) {
                let Some(parent) = &current.parent else {
                    break;
                };
                let Some(found) = report.sessions.iter().find(|other| {
                    other.agent == "pi" && (&other.id == parent || &other.source == parent)
                }) else {
                    break;
                };
                current = found;
            }
            (s.id.clone(), current.id.clone())
        })
        .collect();
    // One record per usage identity: (owning session, slot, owner ID).
    let mut seen: HashMap<(String, String), (usize, usize, String)> = HashMap::new();
    let mut kept: Vec<Vec<Usage>> = report.sessions.iter().map(|_| Vec::new()).collect();
    // Requests a session holds that an earlier session already counted.
    let mut elsewhere: Vec<std::collections::HashSet<String>> =
        report.sessions.iter().map(|_| Default::default()).collect();
    // Of those, the ones counted outside its own session and subagents, by
    // the session that counted them: its own total leaves them out.
    let mut outside: Vec<BTreeMap<String, usize>> =
        report.sessions.iter().map(|_| Default::default()).collect();
    // The session each one counts to, itself or the one it is a subagent of.
    let family: Vec<(String, String)> = report
        .sessions
        .iter()
        .map(|s| (s.agent.clone(), s.subagent_of().unwrap_or(&s.id).to_owned()))
        .collect();
    // A session whose transcript holds another's launch records holds that
    // session's history: the session it copies counts first, so the
    // requests both hold are the original's whichever id sorts first.
    // Otherwise the order stands: sessions first, then subagents.
    let copies = copies(&report.sessions);
    let mut memo = HashMap::new();
    let depth: Vec<Option<usize>> = family
        .iter()
        .map(|(agent, top)| match agent.as_str() {
            "claude" => copy_depth(top, &copies.of, &mut memo, &mut Vec::new()),
            _ => Some(0),
        })
        .collect();
    let mut order: Vec<usize> = (0..report.sessions.len()).collect();
    order.sort_by_key(|&i| (depth[i].unwrap_or(0), report.sessions[i].parent.is_some()));
    for &index in &order {
        let session = &mut report.sessions[index];
        if depth[index].is_none() && session.subagent_of().is_none() {
            session.limitations.push(
                "its transcript and another session's hold copies of each other's launch records, so where the requests they share began is unknown; each is counted with the first session by id".into(),
            );
        }
        for item in session.usage.drain(..) {
            let started = session
                .started_at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
            let at = item
                .at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
            if session.parent.is_some() && started.zip(at).is_some_and(|(start, at)| at < start) {
                // Copied context is not new consumption, even when the original
                // session file is unavailable or a grandchild sorts first.
                continue;
            }
            let event_id = if session.agent == "pi" {
                format!("{}:{}", pi_lineages[&session.id], item.id)
            } else {
                item.id.clone()
            };
            let key = (session.agent.clone(), event_id);
            if let Some((owner, slot, owner_id)) = seen.get(&key) {
                if *owner != index
                    && elsewhere[index].insert(item.id.clone())
                    && family[*owner] != family[index]
                {
                    *outside[index].entry(owner_id.clone()).or_default() += 1;
                }
                let old = &mut kept[*owner][*slot];
                if extends(&old.entry, &item.entry) {
                    // A later snapshot of the same request. Within one file it
                    // becomes the evidence line; a copy elsewhere only supplies
                    // the count, so the record stays with its owner's source.
                    if old.source == item.source {
                        *old = item;
                    } else {
                        old.entry = item.entry;
                    }
                } else if !extends(&item.entry, &old.entry) && old.entry != item.entry {
                    session.limitations.push(format!(
                        "conflicting usage {} (first seen in {owner_id}): {}; first record retained",
                        item.id,
                        difference(&old.entry, &item.entry)
                    ));
                }
            } else {
                seen.insert(key, (index, kept[index].len(), session.id.clone()));
                kept[index].push(item);
            }
        }
    }
    // A launched subagent writes its own transcript. One that was not read
    // leaves the session's usage incomplete, never silently smaller.
    let read: std::collections::HashSet<_> = report
        .sessions
        .iter()
        .filter(|s| s.agent == "claude")
        .map(|s| s.id.clone())
        .collect();
    for (index, session) in report.sessions.iter_mut().enumerate() {
        if session.agent != "claude" {
            continue;
        }
        let top = session.subagent_of().unwrap_or(&session.id).to_owned();
        let mut named = std::collections::HashSet::new();
        for (n, launch) in std::mem::take(&mut session.launched).iter().enumerate() {
            let id = format!("{top}/{}", launch.agent);
            // A copy of another session's launch: its subagent is that one's.
            if read.contains(&id)
                || copies.cleared.contains(&(index, n))
                || !named.insert(id.clone())
            {
                continue;
            }
            let mut said = format!(
                "line {}: subagent {id} was launched, but its transcript was not read; its usage is unknown, not zero",
                launch.line
            );
            if let Some(twins) = copies.twins.get(&(index, n)) {
                said.push_str(&format!(
                    "; {} {} the same launch record, and no transcript read shows which session ran it",
                    twins.iter().cloned().collect::<Vec<_>>().join(", "),
                    if twins.len() == 1 { "holds" } else { "hold" },
                ));
            }
            session.limitations.push(said);
        }
    }
    for (((session, usage), elsewhere), outside) in report
        .sessions
        .iter_mut()
        .zip(kept)
        .zip(elsewhere)
        .zip(outside)
    {
        session.usage = usage;
        session.counted_elsewhere = elsewhere.len();
        // A subagent whose every request its session (or another of its
        // subagents) already holds has had its usage counted there; it is
        // not unknown. Nor is a session whose only answers were synthetic:
        // what it recorded holds no usage.
        let away: usize = outside.values().sum();
        let within = elsewhere.len() - away;
        let subagent = session.subagent_of().is_some();
        let others = outside.keys().cloned().collect::<Vec<_>>().join(", ");
        let them = |n: usize| if n == 1 { "it" } else { "them" };
        if session.usage.is_empty() && !(subagent && within > 0) && session.synthetic.is_empty() {
            session.limitations.push(if outside.is_empty() {
                "No usage records found; consumption is unknown, not zero".into()
            } else {
                // Its requests were counted once, with another session, so
                // the total of the session it belongs to leaves them out.
                format!(
                    "No usage of its own: its {} {} counted with another session ({others}), so this session's total leaves {} out",
                    crate::render::many(elsewhere.len(), "request"),
                    if elsewhere.len() == 1 { "was" } else { "were" },
                    them(elsewhere.len()),
                )
            });
        } else if subagent && away > 0 {
            // Some of a subagent's requests, counted with an unrelated
            // session, are missing from its own session's total too.
            session.limitations.push(format!(
                "{away} of its {} {} counted with another session ({others}), so this session's total leaves {} out",
                crate::render::many(session.usage.len() + elsewhere.len(), "request"),
                if away == 1 { "was" } else { "were" },
                them(away),
            ));
        }
        session.limitations.sort();
        session.limitations.dedup();
    }
}

/// Which Claude transcripts hold another session's launch records. A copy
/// is proven by the record alone: the same record (its `uuid`, or the tool
/// call it answers) in another session's transcripts, where the subagent it
/// names was read as that session's. A subagent of the same id elsewhere
/// proves nothing, and nor does a record whose subagent was read under none
/// or several of the sessions that hold it: those launches stay missing.
#[derive(Default)]
struct Copies {
    /// The sessions each session's transcripts copy launch records from.
    of: HashMap<String, std::collections::BTreeSet<String>>,
    /// Launches that are copies, by session index and launch position.
    cleared: std::collections::HashSet<(usize, usize)>,
    /// Launches whose record other sessions hold too, when none of them
    /// shows it ran there.
    twins: HashMap<(usize, usize), std::collections::BTreeSet<String>>,
}

fn copies(sessions: &[Session]) -> Copies {
    let top = |s: &Session| s.subagent_of().unwrap_or(&s.id).to_owned();
    // The sessions each subagent read ran in, by agent id.
    let mut ran: HashMap<&str, std::collections::BTreeSet<String>> = HashMap::new();
    for s in sessions.iter().filter(|s| s.agent == "claude") {
        if let Some(parent) = s.subagent_of() {
            if let Some(agent) = s.id.strip_prefix(parent).and_then(|r| r.strip_prefix('/')) {
                ran.entry(agent).or_default().insert(parent.to_owned());
            }
        }
    }
    let launches: Vec<(usize, usize, String, &crate::model::Launch)> = sessions
        .iter()
        .enumerate()
        .filter(|(_, s)| s.agent == "claude")
        .flat_map(|(i, s)| {
            let family = top(s);
            s.launched
                .iter()
                .enumerate()
                .map(move |(n, l)| (i, n, family.clone(), l))
        })
        .collect();
    let mut copies = Copies::default();
    for (i, n, family, launch) in &launches {
        let ran_in = |f: &String| {
            ran.get(launch.agent.as_str())
                .is_some_and(|r| r.contains(f))
        };
        if ran_in(family) {
            continue;
        }
        let twins: std::collections::BTreeSet<String> = launches
            .iter()
            .filter(|(_, _, other, l)| other != family && l.same_record(launch))
            .map(|(_, _, other, _)| other.clone())
            .collect();
        let origins: Vec<&String> = twins.iter().filter(|f| ran_in(f)).collect();
        if let [origin] = origins[..] {
            copies.cleared.insert((*i, *n));
            copies
                .of
                .entry(family.clone())
                .or_default()
                .insert(origin.clone());
        } else if !twins.is_empty() {
            copies.twins.insert((*i, *n), twins);
        }
    }
    copies
}

/// How many copies deep a session's history goes: 0 for one that copies no
/// other, else one more than the deepest it copies. None in a loop.
fn copy_depth(
    family: &str,
    of: &HashMap<String, std::collections::BTreeSet<String>>,
    memo: &mut HashMap<String, Option<usize>>,
    path: &mut Vec<String>,
) -> Option<usize> {
    if let Some(depth) = memo.get(family) {
        return *depth;
    }
    if path.iter().any(|f| f == family) {
        return None;
    }
    path.push(family.to_owned());
    let mut depth = Some(0);
    for origin in of.get(family).into_iter().flatten() {
        depth = depth
            .zip(copy_depth(origin, of, memo, path))
            .map(|(d, o)| d.max(o + 1));
    }
    path.pop();
    memo.insert(family.to_owned(), depth);
    depth
}

/// Attaches each linked session record to the sessions it explicitly names,
/// and says which name none, or that no session was found at all.
fn link(report: &mut Report, wrappers: Vec<Value>) {
    for session in &mut report.sessions {
        for wrapper in &wrappers {
            if wrapper["usage"].as_array().is_some_and(|receipts| {
                receipts
                    .iter()
                    .any(|r| r["agent"] == session.agent && r["session_id"] == session.id)
            }) {
                let mut record = wrapper.clone();
                // A linked record may carry spending fields. Keep its
                // token evidence, but do not export money data.
                if let Some(receipts) = record["usage"].as_array_mut() {
                    for receipt in receipts {
                        if let Some(entries) = receipt["entries"].as_array_mut() {
                            for entry in entries {
                                if let Some(entry) = entry.as_object_mut() {
                                    for key in [
                                        "estimated_usd_micros",
                                        "estimate_basis",
                                        "billed",
                                        "credits",
                                    ] {
                                        entry.remove(key);
                                    }
                                }
                            }
                        }
                    }
                }
                session.ohjaa.push(record);
            }
        }
    }
    for wrapper in wrappers {
        if !report
            .sessions
            .iter()
            .any(|s| s.ohjaa.iter().any(|w| w["id"] == wrapper["id"]))
        {
            report.warnings.push(format!(
                "linked session record {} names no session that was read",
                wrapper["id"].as_str().unwrap_or("unknown")
            ));
        }
    }
    if report.sessions.is_empty() {
        report
            .warnings
            .push("No supported native sessions found; usage is unknown".into());
    }
}

/// True when `new` is a strictly later snapshot of the same request as `old`:
/// only the output count has grown, and everything else, including the input
/// and cache counts, agrees. Claude Code writes one line per content block,
/// and in subagent transcripts the output count grows across those lines.
fn extends(old: &Entry, new: &Entry) -> bool {
    let (Some(before), Some(after)) = (old.output_tokens, new.output_tokens) else {
        return false;
    };
    after > before
        && Entry {
            output_tokens: old.output_tokens,
            ..new.clone()
        } == *old
}

/// Names what two snapshots of one request disagree on, so a partial result
/// can be judged: a model fallback reads differently from changed counts.
fn difference(old: &Entry, new: &Entry) -> String {
    let show = |n: Option<u64>| n.map_or("unknown".into(), |n| n.to_string());
    let mut parts = Vec::new();
    for (name, a, b) in [
        ("provider", &old.provider, &new.provider),
        ("model", &old.model, &new.model),
    ] {
        if a != b {
            parts.push(format!("{name} {a} then {b}"));
        }
    }
    for (name, a, b) in [
        ("input", old.input_tokens, new.input_tokens),
        ("output", old.output_tokens, new.output_tokens),
        ("cache read", old.cache_read_tokens, new.cache_read_tokens),
        (
            "cache write",
            old.cache_write_tokens,
            new.cache_write_tokens,
        ),
    ] {
        if a != b {
            parts.push(format!("{name} {} then {}", show(a), show(b)));
        }
    }
    if parts.is_empty() {
        parts.push("recorded amounts differ".into());
    }
    parts.join(", ")
}

/// The transcripts under `path` into `out`, and the opencode stores in the
/// folders under it (each named `opencode.db`, as opencode names its own)
/// into `stores`. A store given as a file is taken before this.
fn discover(path: &Path, out: &mut Vec<PathBuf>, stores: &mut Vec<PathBuf>) -> Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        bail!("symlink not followed; provide its real path");
    }
    if meta.is_dir() {
        for item in std::fs::read_dir(path)? {
            let item = item?;
            let kind = item.file_type()?;
            if kind.is_file() && item.file_name() == "opencode.db" {
                stores.push(item.path().canonicalize()?);
                continue;
            }
            // Native session directories also hold JSON metadata, not usage
            // exports. JSON exports must be selected explicitly with --input.
            if kind.is_dir() || item.path().extension().is_some_and(|s| s == "jsonl") {
                discover(&item.path(), out, stores)?;
            }
        }
    } else if meta.is_file()
        && matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("jsonl" | "json")
        )
    {
        out.push(path.canonicalize()?);
    }
    Ok(())
}

enum Parsed {
    Session(Box<Session>),
    Wrappers(Vec<Value>, Vec<String>),
}

/// An `ohjaa-sessions.v1` file: its session records, and the sessions it
/// says it could not read.
fn parse_export(path: &Path) -> Result<Parsed> {
    let value: Value = serde_json::from_reader(std::fs::File::open(path)?)?;
    ensure!(
        value["schema"] == "ohjaa-sessions.v1",
        "unsupported JSON export (expected ohjaa-sessions.v1)"
    );
    let records = value["sessions"]
        .as_array()
        .context("missing sessions")?
        .clone();
    let warnings = value["unreadable"]
        .as_array()
        .context("missing unreadable coverage")?
        .iter()
        .map(|id| format!("the linked records file says it could not read session {id}"))
        .collect();
    Ok(Parsed::Wrappers(records, warnings))
}

/// A transcript's header alone: the [`Scan`] that [`parse_file`] makes of
/// every record, made of the records up to the one that settles it. The rest
/// of the file is not read, and no record is kept.
fn header_file(path: &Path) -> Result<Parsed> {
    if path.extension().is_some_and(|s| s == "json") {
        return parse_export(path);
    }
    Ok(Parsed::Session(Box::new(header(path)?)))
}

/// A native transcript's header alone; see [`header_file`].
fn header(path: &Path) -> Result<Session> {
    let file = BufReader::new(std::fs::File::open(path)?);
    let mut scan = Scan::default();
    for line in file.split(b'\n') {
        let line = line?;
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        // A malformed record is skipped, as `parse_file` skips it; saying so
        // is a usage limitation, which this reader does not report.
        if let Ok(value) = serde_json::from_slice::<Value>(&line) {
            scan.observe(&value);
            if scan.settled() {
                break;
            }
        }
    }
    scan.session(path, Vec::new())
}

/// What a transcript says about itself, gathered record by record in file
/// order: which agent wrote it, its native identity, lineage, start, folder
/// and how it was started. [`parse_file`] shows it every record, and
/// [`header_file`] only the records until [`Scan::settled`].
#[derive(Default)]
struct Scan {
    /// The first Codex `session_meta` record.
    codex: Option<Opening>,
    /// The first pi `session` record, and whether any names its session.
    pi: Option<Opening>,
    pi_named: bool,
    /// The first record that carries a Claude `sessionId`.
    claude: Option<Opening>,
    /// The first record's value of each, where the opening record lacks it.
    started_at: Option<String>,
    cwd: Option<String>,
    entrypoint: Option<String>,
    agent_id: Option<String>,
    /// The first branch a record names: Claude Code's `gitBranch`, Codex's
    /// `session_meta.payload.git.branch`. An empty one names none.
    branch: Option<String>,
    /// Whether any record is a Claude sidechain, and whether any says.
    sidechain: bool,
    sidechain_said: bool,
    /// Whether any record says it is no sidechain. Records that say nothing
    /// (Claude Code's `api-request` records, in subagent transcripts too)
    /// do not count.
    not_sidechain: bool,
}

/// The header facts of one record that opens a session.
struct Opening {
    id: String,
    parent: Option<String>,
    started_at: Option<String>,
    cwd: String,
    originator: Option<String>,
    subagent_source: bool,
    /// The session a Codex thread belongs to: `session_id`, which a child
    /// thread shares with the session at its root; else, where Codex writes
    /// none, the thread it was spawned or forked from.
    root: Option<String>,
}

impl Opening {
    /// `meta` is the record itself, or Codex's `payload`; `id_key` names the
    /// field of `meta` that holds the session id.
    fn new(record: &Value, meta: &Value, id_key: &str) -> Self {
        Opening {
            id: string(meta, id_key),
            parent: meta["parentSession"]
                .as_str()
                .or_else(|| meta["forked_from_id"].as_str())
                .map(str::to_owned),
            started_at: timestamp(meta).or_else(|| timestamp(record)),
            cwd: string(meta, "cwd"),
            originator: meta["originator"].as_str().map(str::to_owned),
            subagent_source: !meta["source"]["subagent"].is_null(),
            root: meta["session_id"]
                .as_str()
                .or_else(|| meta["parent_thread_id"].as_str())
                .or_else(|| meta["forked_from_id"].as_str())
                .map(str::to_owned),
        }
    }
}

impl Scan {
    fn observe(&mut self, v: &Value) {
        if self.codex.is_none() && v["type"] == "session_meta" {
            self.codex = Some(Opening::new(v, &v["payload"], "id"));
        }
        if self.pi.is_none() && v["type"] == "session" {
            self.pi = Some(Opening::new(v, v, "id"));
        }
        self.pi_named |= v["type"] == "session" && v["id"].is_string();
        if self.claude.is_none() && v["sessionId"].is_string() {
            self.claude = Some(Opening::new(v, v, "sessionId"));
        }
        if self.started_at.is_none() {
            self.started_at = timestamp(v);
        }
        for (slot, key) in [
            (&mut self.cwd, "cwd"),
            (&mut self.entrypoint, "entrypoint"),
            (&mut self.agent_id, "agentId"),
        ] {
            if slot.is_none() {
                *slot = v[key].as_str().map(str::to_owned);
            }
        }
        if self.branch.is_none() {
            let named = if v["type"] == "session_meta" {
                &v["payload"]["git"]["branch"]
            } else {
                &v["gitBranch"]
            };
            self.branch = named.as_str().filter(|b| !b.is_empty()).map(str::to_owned);
        }
        self.sidechain |= v["isSidechain"] == true;
        self.sidechain_said |= v["isSidechain"].is_boolean();
        self.not_sidechain |= v["isSidechain"] == false;
    }

    /// The agent the records so far say wrote the file, with the record that
    /// opens its session: Codex when any is its `session_meta`, else pi when
    /// any is a pi session header with an id, else Claude when any carries a
    /// `sessionId`.
    fn opening(&self) -> Option<(&'static str, &Opening)> {
        match (&self.codex, &self.pi, &self.claude) {
            (Some(codex), _, _) => Some(("codex", codex)),
            (None, Some(pi), _) if self.pi_named => Some(("pi", pi)),
            (None, _, Some(claude)) => Some(("claude", claude)),
            _ => None,
        }
    }

    /// Whether the records not yet read can no longer change the header.
    ///
    /// Each fact is "the first record that has it", final once found, so a
    /// transcript that never states one is read to its end. Two rules look
    /// at every record, and here the formats decide:
    ///
    /// * The agent: a later Codex or pi header would outrank a pi or Claude
    ///   one. Each agent opens its own transcripts with its header and no
    ///   transcript holds another agent's records, so the first decides.
    /// * A Claude sidechain: Claude Code marks every record of a transcript
    ///   alike, so the first that says decides.
    fn settled(&self) -> bool {
        let Some((agent, opening)) = self.opening() else {
            return false;
        };
        let dated = opening.started_at.is_some() || self.started_at.is_some();
        let placed = !opening.cwd.is_empty() || self.cwd.is_some();
        let claude = || {
            self.entrypoint.is_some()
                && self.sidechain_said
                && (!self.sidechain || self.agent_id.is_some())
        };
        dated && placed && (agent != "claude" || claude())
    }

    /// The session the records seen say this is, with no usage read yet.
    fn session(self, path: &Path, limitations: Vec<String>) -> Result<Session> {
        let Some((agent, _)) = self.opening() else {
            bail!("unsupported or missing native session header");
        };
        let opening = match agent {
            "codex" => self.codex,
            "pi" => self.pi,
            _ => self.claude,
        }
        .unwrap();
        let mut id = opening.id;
        ensure!(!id.is_empty(), "missing session identity");
        let mut parent = opening.parent;
        if agent == "claude" && self.sidechain {
            parent = Some(id.clone());
            // Claude Code names the file `agent-<agentId>.jsonl`, so a file
            // whose records omit the agentId gives the id its launch names.
            let child = self.agent_id.unwrap_or_else(|| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.strip_prefix("agent-").unwrap_or(s))
                    .unwrap_or("unknown")
                    .to_owned()
            });
            id = format!("{id}/{child}");
        }
        if agent == "codex" && opening.subagent_source && parent.is_none() {
            parent = Some("parent identity unavailable (subagent source)".into());
        }
        // A Codex subagent, or any thread another spawned, counts to the
        // session at its root, as a Claude subagent counts to its session.
        let root = (agent == "codex" && opening.subagent_source)
            .then_some(opening.root)
            .flatten()
            .filter(|root| *root != id);
        // Current Claude Code transcripts open with records (`last-prompt`,
        // `mode`) that carry the session id but no time and no folder: those
        // come from the first record that has them.
        Ok(Session {
            agent: agent.into(),
            id,
            parent,
            started_at: opening.started_at.or(self.started_at),
            cwd: Some(opening.cwd)
                .filter(|cwd| !cwd.is_empty())
                .or(self.cwd)
                .unwrap_or_default(),
            entrypoint: (agent == "claude").then_some(self.entrypoint).flatten(),
            originator: (agent == "codex").then_some(opening.originator).flatten(),
            branch: matches!(agent, "claude" | "codex")
                .then_some(self.branch)
                .flatten(),
            touched: Default::default(),
            source: path.display().to_string(),
            usage: Vec::new(),
            limitations,
            ohjaa: Vec::new(),
            work: Vec::new(),
            launched: Vec::new(),
            counted_elsewhere: 0,
            synthetic: Default::default(),
            root,
            subagent: (agent == "claude" && self.sidechain)
                || (agent == "codex" && opening.subagent_source),
            partly_sidechain: agent == "claude" && self.sidechain && self.not_sidechain,
        })
    }
}

fn parse_file(path: &Path) -> Result<Parsed> {
    if path.extension().is_some_and(|s| s == "json") {
        return parse_export(path);
    }
    let file = BufReader::new(std::fs::File::open(path)?);
    let mut records = Vec::new();
    let mut limitations = Vec::new();
    for (index, line) in file.split(b'\n').enumerate() {
        let line = line?;
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice::<Value>(&line) {
            Ok(value) => records.push((index + 1, value)),
            Err(error) => limitations.push(format!("line {}: malformed JSON: {error}", index + 1)),
        }
    }
    let mut scan = Scan::default();
    for (_, value) in &records {
        scan.observe(value);
    }
    let mut session = scan.session(path, limitations)?;
    let agent = session.agent.clone();
    let mut model = "unknown".to_owned();
    // Codex is OpenAI's agent; its session_meta names the provider when it
    // is another one. Pi is multi-provider and gets no default.
    let mut provider = if agent == "codex" {
        "openai"
    } else {
        "unknown"
    }
    .to_owned();
    let mut previous: Option<[u64; 4]> = None;
    let mut pi_model_changed = false;
    // The folder the session recorded at this point.
    let mut cwd = session.cwd.clone();
    for (line, value) in records {
        crate::work::observe(&value, line, &mut session);
        touched(&value, &agent, &mut cwd, &mut session.touched);
        let result = match agent.as_str() {
            "claude" => claude(&value, line, &mut session),
            "codex" => codex(
                &value,
                line,
                &mut session,
                &mut model,
                &mut provider,
                &mut previous,
            ),
            _ => pi(
                &value,
                line,
                &mut session,
                &mut model,
                &mut provider,
                &mut pi_model_changed,
            ),
        };
        if let Err(error) = result {
            session.limitations.push(format!("line {line}: {error:#}"));
        }
    }
    Ok(Parsed::Session(Box::new(session)))
}

/// The files and folders one record's structured tool calls name, into
/// `out`; `cwd` follows the folder the session records. Only fields that
/// hold a path are read, never free text: a shell command, a prompt or a
/// reply is not searched for paths.
///
/// * Claude Code: `Read`, `Write`, `Edit` and `MultiEdit`'s `file_path`,
///   `NotebookEdit`'s `notebook_path`, `Glob` and `Grep`'s `path`; each
///   record's `cwd`.
/// * Codex: a shell call's `workdir` (`exec_command`, `shell`), and the
///   files an `apply_patch` names on its `*** Add File:`, `*** Update
///   File:`, `*** Delete File:` and `*** Move to:` lines; the `cwd` of
///   `session_meta` and of each `turn_context`.
/// * pi: a tool call's `path` argument; the session header's `cwd`.
fn touched(
    v: &Value,
    agent: &str,
    cwd: &mut String,
    out: &mut std::collections::BTreeSet<PathBuf>,
) {
    if agent == "codex" {
        let p = &v["payload"];
        if matches!(v["type"].as_str(), Some("session_meta" | "turn_context")) {
            if let Some(dir) = p["cwd"].as_str().filter(|d| !d.is_empty()) {
                dir.clone_into(cwd);
                // Every folder a session records is a place it worked.
                out.insert(PathBuf::from(dir));
            }
        }
        if v["type"] != "response_item" {
            return;
        }
        let name = p["name"].as_str().unwrap_or_default();
        let arguments = || -> Value {
            p["arguments"]
                .as_str()
                .and_then(|a| serde_json::from_str(a).ok())
                .unwrap_or(Value::Null)
        };
        match (p["type"].as_str(), name) {
            (Some("function_call"), "exec_command" | "shell" | "shell_command") => {
                if let Some(dir) = arguments()["workdir"].as_str() {
                    touch(out, dir, cwd);
                }
            }
            (Some("function_call"), "apply_patch") => {
                if let Some(patch) = arguments()["input"].as_str() {
                    patched(patch).for_each(|path| touch(out, path, cwd));
                }
            }
            (Some("custom_tool_call"), "apply_patch") => {
                if let Some(patch) = p["input"].as_str() {
                    patched(patch).for_each(|path| touch(out, path, cwd));
                }
            }
            _ => {}
        }
        return;
    }
    if let Some(dir) = v["cwd"].as_str().filter(|d| !d.is_empty()) {
        dir.clone_into(cwd);
        // Every folder a session records is a place it worked.
        out.insert(PathBuf::from(dir));
    }
    let m = &v["message"];
    if m["role"] != "assistant" {
        return;
    }
    for c in m["content"].as_array().into_iter().flatten() {
        let (input, keys): (&Value, &[&str]) = match (c["type"].as_str(), c["name"].as_str()) {
            (Some("tool_use"), Some("Read" | "Write" | "Edit" | "MultiEdit")) => {
                (&c["input"], &["file_path"])
            }
            (Some("tool_use"), Some("NotebookEdit")) => {
                (&c["input"], &["notebook_path", "file_path"])
            }
            (Some("tool_use"), Some("Glob" | "Grep")) => (&c["input"], &["path"]),
            (Some("toolCall"), _) if agent == "pi" => (&c["arguments"], &["path"]),
            _ => continue,
        };
        for key in keys {
            if let Some(path) = input[*key].as_str() {
                touch(out, path, cwd);
            }
        }
    }
}

/// The files a Codex patch names on its header lines.
fn patched(patch: &str) -> impl Iterator<Item = &str> {
    patch.lines().filter_map(|line| {
        [
            "*** Add File: ",
            "*** Update File: ",
            "*** Delete File: ",
            "*** Move to: ",
        ]
        .iter()
        .find_map(|header| line.strip_prefix(header))
        .map(str::trim)
        .filter(|path| !path.is_empty())
    })
}

/// `path` into `out`, made absolute against `cwd`; a relative path with no
/// folder to resolve it against is left out.
fn touch(out: &mut std::collections::BTreeSet<PathBuf>, path: &str, cwd: &str) {
    if path.is_empty() {
        return;
    }
    let path = Path::new(path);
    if path.is_absolute() {
        out.insert(path.to_path_buf());
    } else if !cwd.is_empty() {
        out.insert(Path::new(cwd).join(path));
    }
}

fn string(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}
fn timestamp(v: &Value) -> Option<String> {
    if let Some(text) = v["timestamp"].as_str() {
        Some(text.to_owned())
    } else {
        v["timestamp"]
            .as_i64()
            .and_then(chrono::DateTime::from_timestamp_millis)
            .map(|t| t.to_rfc3339())
    }
}
fn count(v: &Value, key: &str) -> Result<Option<u64>> {
    if v[key].is_null() {
        return Ok(None);
    }
    Ok(Some(v[key].as_u64().with_context(|| {
        format!("invalid nonnegative integer {key}")
    })?))
}
fn base(provider: String, model: String) -> Entry {
    Entry {
        provider,
        model,
        input_tokens: None,
        output_tokens: None,
        cache_read_tokens: None,
        cache_write_tokens: None,
    }
}
fn add(s: &mut Session, v: &Value, line: usize, id: String, entry: Entry) {
    s.usage.push(Usage {
        id,
        entry,
        at: timestamp(v),
        source: s.source.clone(),
        line,
    });
}
fn fallback(s: &Session, line: usize) -> String {
    format!("{}:{}:{line}", s.id, s.source)
}

fn claude(v: &Value, line: usize, s: &mut Session) -> Result<()> {
    if v["sessionId"].is_string() {
        ensure!(
            v["sessionId"] == s.id || s.parent.as_ref().is_some_and(|p| v["sessionId"] == *p),
            "mixed native session IDs"
        );
    }
    // Launching a subagent returns a tool result naming its agent id.
    if let Some(agent) = v["toolUseResult"]["agentId"].as_str() {
        let tool_use = v["message"]["content"]
            .as_array()
            .and_then(|content| content.iter().find_map(|c| c["tool_use_id"].as_str()));
        s.launched.push(crate::model::Launch {
            agent: agent.to_owned(),
            line,
            record: v["uuid"].as_str().map(str::to_owned),
            tool_use: tool_use.map(str::to_owned),
        });
    }
    let m = &v["message"];
    if v["type"] == "assistant" && m["usage"].is_object() {
        let usage = &m["usage"];
        let mut entry = base(
            v["provider"]
                .as_str()
                .unwrap_or_else(|| claude_provider(v))
                .into(),
            m["model"].as_str().unwrap_or("unknown").into(),
        );
        entry.input_tokens = count(usage, "input_tokens")?;
        entry.output_tokens = count(usage, "output_tokens")?;
        entry.cache_read_tokens = count(usage, "cache_read_input_tokens")?;
        entry.cache_write_tokens = count(usage, "cache_creation_input_tokens")?;
        let id = v["requestId"]
            .as_str()
            .or_else(|| m["id"].as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| fallback(s, line));
        // Claude Code writes a `<synthetic>` message itself, with zero usage,
        // where a request failed or was refused; it may carry that request's
        // id. It is no request, so it is counted apart, never as usage that
        // conflicts with the request. One that reports usage is read as usage.
        let counts = [
            entry.input_tokens,
            entry.output_tokens,
            entry.cache_read_tokens,
            entry.cache_write_tokens,
        ];
        if entry.model == "<synthetic>" && counts.iter().all(|n| n.unwrap_or(0) == 0) {
            let own = m["id"].as_str().map_or_else(|| id.clone(), str::to_owned);
            s.synthetic.insert(own);
            return Ok(());
        }
        // Claude writes a line per content block of one request. Equal usage
        // deduplicates, a growing output count keeps the largest snapshot, and
        // conflicting snapshots are reported instead of summed (see `read`).
        add(s, v, line, id, entry);
    }
    Ok(())
}

/// Claude Code transcripts never name a provider, and the same transcript
/// shape comes from Anthropic's API, Amazon Bedrock, Google Vertex AI or an
/// Anthropic-compatible gateway. The response ids tell them apart: Anthropic
/// returns `msg_01…` with `req_…` request ids, Bedrock `msg_bdrk_…` and Vertex
/// `msg_vrtx_…`. Anything else stays unknown; a model name proves nothing.
fn claude_provider(v: &Value) -> &'static str {
    let message = v["message"]["id"].as_str().unwrap_or_default();
    if message.starts_with("msg_bdrk_") {
        "bedrock"
    } else if message.starts_with("msg_vrtx_") {
        "vertex"
    } else if message.starts_with("msg_01")
        || v["requestId"]
            .as_str()
            .is_some_and(|id| id.starts_with("req_"))
    {
        "anthropic"
    } else {
        "unknown"
    }
}

fn codex(
    v: &Value,
    line: usize,
    s: &mut Session,
    model: &mut String,
    provider: &mut String,
    previous: &mut Option<[u64; 4]>,
) -> Result<()> {
    let p = &v["payload"];
    if matches!(v["type"].as_str(), Some("session_meta" | "turn_context")) {
        if let Some(m) = p["model"].as_str() {
            *model = m.into();
        }
        if let Some(p) = p["model_provider"].as_str() {
            *provider = p.into();
        }
    }
    if v["type"] == "event_msg" && p["type"] == "token_count" && p["info"].is_object() {
        let total = &p["info"]["total_token_usage"];
        let values = [
            count(total, "input_tokens")?,
            count(total, "output_tokens")?,
            count(total, "cached_input_tokens")?,
        ];
        let [Some(input), Some(output), Some(cached)] = values else {
            bail!("missing cumulative token categories; cannot safely derive request usage");
        };
        // Codex can also count cache writes. Vessa assumes that, like cached
        // reads, they are part of the inclusive input. Nothing confirms it,
        // so a non-zero one is reported. A counter without the field records
        // none: OpenAI prompt caching has no write category.
        let written = cache_writes(total)?;
        if written > 0 {
            let note = "cache_write_input_tokens is non-zero: Vessa assumes cache writes are part of input_tokens, which no transcript has confirmed; uncached input may be undercounted";
            if !s.limitations.iter().any(|l| l == note) {
                s.limitations.push(note.into());
            }
        }
        ensure!(
            cached
                .checked_add(written)
                .is_some_and(|cache| cache <= input),
            "cached tokens exceed inclusive input tokens"
        );
        let current = [input, output, cached, written];
        let mut prior = previous.unwrap_or([0; 4]);
        if previous.is_none() && s.parent.is_some() {
            let breakdown = &p["info"]["last_token_usage"];
            let last = [
                count(breakdown, "input_tokens")?,
                count(breakdown, "output_tokens")?,
                count(breakdown, "cached_input_tokens")?,
            ];
            let lw = cache_writes(breakdown)?;
            *previous = Some(current);
            let [Some(li), Some(lo), Some(lc)] = last else {
                bail!("first child counter has no last-request breakdown; inherited usage cannot be attributed");
            };
            for (i, last) in [li, lo, lc, lw].into_iter().enumerate() {
                prior[i] = current[i]
                    .checked_sub(last)
                    .context("last-request tokens exceed cumulative total")?;
            }
        }
        *previous = Some(current);
        let delta: Vec<_> = current
            .iter()
            .zip(prior)
            .map(|(a, b)| {
                a.checked_sub(b)
                    .context("cumulative counters reset; baseline re-established")
            })
            .collect::<Result<_>>()?;
        if delta.iter().all(|n| *n == 0) {
            return Ok(());
        }
        let uncached = delta[0]
            .checked_sub(delta[2])
            .and_then(|rest| rest.checked_sub(delta[3]))
            .context("cache delta exceeds input delta")?;
        let mut entry = base(provider.clone(), model.clone());
        entry.input_tokens = Some(uncached);
        entry.output_tokens = Some(delta[1]);
        entry.cache_read_tokens = Some(delta[2]);
        entry.cache_write_tokens = Some(delta[3]);
        add(
            s,
            v,
            line,
            format!("{}:total:{input}:{output}:{cached}", s.id),
            entry,
        );
    }
    Ok(())
}

/// The sessions of each opencode store, with the store that holds each;
/// what a store could not say is handed to `gap`, with the store.
fn opencode_sessions(
    stores: &[PathBuf],
    gap: &mut dyn FnMut(&Path, String),
) -> Vec<(PathBuf, crate::opencode::Stored)> {
    let mut all = Vec::new();
    for store in stores {
        match crate::opencode::sessions(store) {
            Ok((sessions, said)) => {
                said.into_iter().for_each(|why| gap(store, why));
                all.extend(sessions.into_iter().map(|s| (store.clone(), s)));
            }
            Err(error) => gap(store, format!("{error:#}")),
        }
    }
    all
}

/// A warning about `path`, as the readers word one.
fn warn(warnings: &mut Vec<String>) -> impl FnMut(&Path, String) + '_ {
    |path, why| warnings.push(format!("{}: {why}", path.display()))
}

/// One opencode model response (an assistant message, or the summary a
/// compaction asks for): one request, when it records tokens. opencode
/// counts reasoning apart from output; it is output, as Codex's
/// `output_tokens` counts it. Input is uncached, as opencode keeps it. A
/// response that records no tokens at all, such as an error before any, is
/// a known record with no usage, as Claude Code's `<synthetic>` replies are.
fn opencode(v: &Value, line: usize, s: &mut Session) -> Result<()> {
    let d = &v["data"];
    let t = &d["tokens"];
    if !matches!(v["type"].as_str(), Some("assistant" | "compaction")) || !t.is_object() {
        return Ok(());
    }
    let named = |key: &str| {
        d["model"][key]
            .as_str()
            .filter(|n| !n.is_empty())
            .unwrap_or("unknown")
            .to_owned()
    };
    let mut entry = base(named("providerID"), named("id"));
    entry.input_tokens = count(t, "input")?;
    entry.output_tokens = match (count(t, "output")?, count(t, "reasoning")?) {
        (Some(output), reasoning) => Some(
            output
                .checked_add(reasoning.unwrap_or(0))
                .context("output and reasoning tokens overflow")?,
        ),
        (None, _) => None,
    };
    entry.cache_read_tokens = count(&t["cache"], "read")?;
    entry.cache_write_tokens = count(&t["cache"], "write")?;
    let counts = [
        entry.input_tokens,
        entry.output_tokens,
        entry.cache_read_tokens,
        entry.cache_write_tokens,
    ];
    let id = v["id"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| fallback(s, line));
    if counts.iter().all(|n| n.unwrap_or(0) == 0) {
        s.synthetic.insert(id);
        return Ok(());
    }
    add(s, v, line, id, entry);
    Ok(())
}

/// Codex cache writes, zero when the counter predates the field.
fn cache_writes(usage: &Value) -> Result<u64> {
    Ok(count(usage, "cache_write_input_tokens")?.unwrap_or(0))
}

fn pi(
    v: &Value,
    line: usize,
    s: &mut Session,
    model: &mut String,
    provider: &mut String,
    changed: &mut bool,
) -> Result<()> {
    if v["type"] == "session" {
        ensure!(
            v["version"].as_u64().is_some_and(|n| (1..=3).contains(&n)),
            "unsupported pi session version"
        );
    }
    if v["type"] == "model_change" {
        *model = string(v, "modelId");
        *provider = string(v, "provider");
        *changed = true;
    }
    let m = if v["type"] == "message" {
        &v["message"]
    } else {
        v
    };
    let id = v["id"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| fallback(s, line));
    if m["usage"].is_object() {
        let u = &m["usage"];
        let mut entry = base(
            m["provider"]
                .as_str()
                .unwrap_or(if *changed { provider } else { "unknown" })
                .into(),
            m["model"]
                .as_str()
                .unwrap_or(if *changed { model } else { "unknown" })
                .into(),
        );
        entry.input_tokens = count(u, "input")?;
        entry.output_tokens = count(u, "output")?;
        entry.cache_read_tokens = count(u, "cacheRead")?;
        entry.cache_write_tokens = count(u, "cacheWrite")?;
        add(s, v, line, id, entry);
    }
    Ok(())
}

// ---- messages ----------------------------------------------------------------

/// How much of the agent's preceding reply travels with a message: its last
/// characters, as ohjaa intake has taken them.
const REPLY_TAIL: usize = 600;

/// What `vessa messages` selects, besides its inputs.
#[derive(Default)]
pub struct Selection {
    pub agent: Option<String>,
    pub session: Option<String>,
    /// Only the sessions a commit names, by agent and id, each with its
    /// subagents; empty for every session. One whose own transcript is not
    /// read is not found, and said in [`Listing::named`], not an error.
    pub named: Vec<(String, String)>,
    /// Only messages after `since`, and up to `until`.
    pub since: Option<chrono::DateTime<chrono::FixedOffset>>,
    pub until: Option<chrono::DateTime<chrono::FixedOffset>>,
}

/// What `vessa messages` read, beside the messages it handed on.
#[derive(Default)]
pub struct Listing {
    pub sessions: Vec<Conversation>,
    pub coverage: Coverage,
    pub warnings: Vec<String>,
    /// Each session [`Selection::named`] names, in its order: whether its
    /// own transcript was read, and how many messages it and its subagents
    /// gave.
    pub named: Vec<Named>,
}

/// A session a commit names, as `messages` found it.
pub struct Named {
    pub agent: String,
    pub id: String,
    pub found: bool,
    pub messages: usize,
}

/// The human messages of the transcripts under `paths`, each handed to
/// `emit` as soon as its transcript is read, so memory holds one
/// transcript's turns at a time. Transcripts are read in path order, a Codex
/// fork after the session it was forked from.
///
/// With `scrub`, each reply is scrubbed of secrets before it is cut to its
/// tail; the rest of a message is the caller's to scrub, as it prints it.
pub fn messages(
    paths: &[PathBuf],
    select: &Selection,
    scrub: bool,
    emit: &mut dyn FnMut(&Message) -> Result<()>,
) -> Result<Listing> {
    let mut warnings = Vec::new();
    let mut coverage = Coverage {
        since: select.since.map(|t| t.to_rfc3339()),
        until: select.until.map(|t| t.to_rfc3339()),
        later: select.until.map(|_| 0),
        ..Default::default()
    };
    // Each transcript's header, read as `sessions --headers` reads it, and
    // each opencode session, with the store that holds it.
    let mut headers = Vec::new();
    let (files, stores) = transcripts(paths, &mut warnings);
    // What each opencode store says of its sessions besides their headers.
    let mut stored: HashMap<(PathBuf, String), crate::opencode::Facts> = HashMap::new();
    for (store, s) in opencode_sessions(&stores, &mut warn(&mut warnings)) {
        stored.insert((store.clone(), s.session.id.clone()), s.facts);
        headers.push((store, s.session));
    }
    for file in files {
        if file.extension().is_some_and(|s| s == "json") {
            warnings.push(format!(
                "{}: a file of linked session records holds no messages; not read",
                file.display()
            ));
            continue;
        }
        match header(&file) {
            Ok(session) => headers.push((file, session)),
            Err(error) => warnings.push(format!("{}: {error:#}", file.display())),
        }
    }
    // As the other readers do, so `--session` selects the same subagents.
    roots(
        &mut headers.iter_mut().map(|(_, s)| s).collect::<Vec<_>>(),
        false,
    );
    // Each named session, and whether its own transcript was read: only
    // then are it and its subagents listed.
    let named: Vec<(&str, &str, bool)> = select
        .named
        .iter()
        .map(|(agent, id)| {
            let found = headers
                .iter()
                .any(|(_, s)| s.agent == *agent && s.id == *id);
            (agent.as_str(), id.as_str(), found)
        })
        .collect();
    // The named session a session is listed for: itself, or its session.
    let named_as = |s: &Session| {
        named.iter().position(|(agent, id, found)| {
            *found && *agent == s.agent && (*id == s.id || s.subagent_of() == Some(*id))
        })
    };
    let chosen = |s: &Session| {
        select.agent.as_ref().is_none_or(|a| *a == s.agent)
            && select
                .session
                .as_ref()
                .is_none_or(|id| *id == s.id || s.subagent_of() == Some(id.as_str()))
            && (named.is_empty() || named_as(s).is_some())
    };
    if let Some(id) = &select.session {
        // A subagent's own id selects it; a session's selects it with its
        // subagents, and only when its main transcript was read.
        let (subagents, sessions): (Vec<_>, Vec<_>) = headers
            .iter()
            .filter(|(_, s)| chosen(s))
            .partition(|(_, s)| s.subagent && s.id != *id);
        ensure!(
            !sessions.is_empty(),
            "native session {id} not found{}",
            if subagents.is_empty() {
                String::new()
            } else {
                format!(
                    "; its main transcript was not read, only {} of its subagents",
                    subagents.len()
                )
            }
        );
    }
    let mut work = Vec::new();
    // Codex sessions a selected fork was forked from: their transcripts tell
    // the fork's copied history from its own turns.
    let mut wanted = std::collections::HashSet::new();
    for (path, s) in &headers {
        if !chosen(s) {
            continue;
        }
        // A person can write to a Claude or opencode subagent, so its
        // transcript is read for that. Nothing in a Codex subagent's says a
        // person wrote.
        if s.subagent && !matches!(s.agent.as_str(), "claude" | "opencode") {
            coverage.subagent_transcripts += 1;
            continue;
        }
        // A transcript last written before `since` holds no message from
        // after it. An opencode store holds every session, so a session of
        // one is placed by its newest message instead: none created after
        // `since`, none listed.
        if let Some(since) = select.since {
            let unmodified = if s.agent == "opencode" {
                stored
                    .get(&(path.clone(), s.id.clone()))
                    .is_some_and(|facts| facts.last.is_none_or(|ms| ms <= since.timestamp_millis()))
            } else {
                std::fs::metadata(path)
                    .and_then(|m| m.modified())
                    .is_ok_and(|m| chrono::DateTime::<chrono::Utc>::from(m) < since)
            };
            if unmodified {
                coverage.unmodified_transcripts += 1;
                continue;
            }
        }
        if s.agent == "codex" {
            if let Some(parent) = &s.parent {
                wanted.insert(parent.clone());
            }
        }
        work.push((path, s));
    }
    // A fork of a fork comes after its parent fork.
    let codex: HashMap<&str, &Session> = headers
        .iter()
        .filter(|(_, s)| s.agent == "codex")
        .map(|(_, s)| (s.id.as_str(), s))
        .collect();
    let depth = |s: &Session| {
        let mut seen = std::collections::HashSet::new();
        let (mut depth, mut current) = (0, s);
        while let Some(parent) = current.parent.as_deref().filter(|_| s.agent == "codex") {
            match codex.get(parent) {
                Some(next) if seen.insert(parent) => (depth, current) = (depth + 1, *next),
                _ => break,
            }
        }
        depth
    };
    work.sort_by_key(|(_, s)| depth(s));
    let mut parents: HashMap<String, Vec<Vec<u64>>> = HashMap::new();
    for (path, s) in &headers {
        if s.agent == "codex" && wanted.contains(&s.id) && !work.iter().any(|(p, _)| p == &path) {
            // Only its digests are wanted, which are of the words as written.
            match turns(path, s, false) {
                Ok(read) => parents
                    .entry(s.id.clone())
                    .or_default()
                    .push(read.transcript()),
                Err(error) => warnings.push(format!("{}: {error:#}", path.display())),
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    let mut sessions: BTreeMap<(String, String), Conversation> = BTreeMap::new();
    let mut counts = vec![0; named.len()];
    for (path, s) in work {
        let mut read = match turns(path, s, scrub) {
            Ok(read) => read,
            Err(error) if s.agent == "opencode" => {
                warnings.push(format!(
                    "{}: opencode session {}: {error:#}",
                    path.display(),
                    s.id
                ));
                continue;
            }
            Err(error) => {
                warnings.push(format!("{}: {error:#}", path.display()));
                continue;
            }
        };
        if let Some(note) = stored
            .get(&(path.clone(), s.id.clone()))
            .and_then(crate::opencode::Facts::unlisted)
        {
            read.limitations.push(note);
        }
        coverage.transcripts += 1;
        if wanted.contains(&s.id) {
            parents
                .entry(s.id.clone())
                .or_default()
                .push(read.transcript());
        }
        let listed = list(
            s,
            read,
            &parents,
            (select.since, select.until),
            &mut seen,
            &mut coverage,
            emit,
        )?;
        if let Some(i) = named_as(s) {
            counts[i] += listed.messages;
        }
        match sessions.entry((s.agent.clone(), s.id.clone())) {
            std::collections::btree_map::Entry::Occupied(mut old) => {
                let old = old.get_mut();
                old.messages += listed.messages;
                old.headless &= listed.headless;
                old.limited |= listed.limited;
            }
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(listed);
            }
        }
    }
    Ok(Listing {
        sessions: sessions.into_values().collect(),
        coverage,
        warnings,
        named: named
            .iter()
            .zip(counts)
            .map(|((agent, id, found), messages)| Named {
                agent: (*agent).to_owned(),
                id: (*id).to_owned(),
                found: *found,
                messages,
            })
            .collect(),
    })
}

/// One turn of a transcript: a user turn with its text, or an agent turn
/// reduced to the tail a message carries as its reply.
struct Turn {
    line: usize,
    user: bool,
    /// An agent turn that failed: a Claude API error, a Codex error event, a
    /// pi error stop.
    failed: bool,
    /// Of the turn's whole text, to recognise a Codex fork's copy of its
    /// parent's transcript; none for a turn without text.
    digest: Option<u64>,
    /// A user turn's whole text; an agent turn's last characters.
    text: String,
    truncated: bool,
    at: Option<String>,
    id: Option<String>,
    cwd: Option<String>,
    /// Why a user turn is no person's message, where its record says so.
    rule: Option<&'static str>,
    /// Sent by a program: Claude's `sdk-*` entrypoints and `sdk` prompts.
    headless: bool,
    /// After a Codex fork's copy of its parent's header.
    copied: bool,
    /// What a person's words are, as `Message::kind` says.
    kind: &'static str,
    /// The question an `answer` answers.
    question: Option<String>,
    /// Which of one record's answers this is.
    part: usize,
    /// What was redacted in an agent turn's last characters, scrubbed
    /// before they were cut.
    redacted: crate::scrub::Redactions,
}

impl Turn {
    fn user(text: String, rule: Option<&'static str>) -> Self {
        Turn {
            line: 0,
            user: true,
            failed: false,
            digest: (!text.trim().is_empty()).then(|| digest("user", &text)),
            text,
            truncated: false,
            at: None,
            id: None,
            cwd: None,
            rule,
            headless: false,
            copied: false,
            kind: "message",
            question: None,
            part: 0,
            redacted: Default::default(),
        }
    }

    /// A person's words found inside a record: an answer or a rejection.
    fn words(kind: &'static str, text: &str, part: usize) -> Self {
        Turn {
            kind,
            part,
            ..Turn::user(text.to_owned(), None)
        }
    }

    /// An agent turn, or none for one without prose, such as a tool call:
    /// whole, until [`turns`] cuts it as its record is read.
    fn agent(text: String, failed: bool) -> Option<Self> {
        if text.trim().is_empty() {
            return None;
        }
        Some(Turn {
            user: false,
            failed,
            digest: Some(digest("assistant", &text)),
            text,
            ..Turn::user(String::new(), None)
        })
    }

    /// An agent turn reduced to the last characters a message carries as its
    /// reply: scrubbed whole first, unless `scrub` is off (`--raw`), so a
    /// cut never splits a secret into pieces no rule knows (see
    /// [`crate::scrub::tail`]).
    fn cut(&mut self, scrub: bool) {
        if scrub {
            let tail = crate::scrub::tail(&self.text, REPLY_TAIL);
            (self.text, self.truncated, self.redacted) = (tail.text, tail.truncated, tail.redacted);
        } else {
            let count = self.text.chars().count();
            self.truncated = count > REPLY_TAIL;
            self.text = self
                .text
                .chars()
                .skip(count.saturating_sub(REPLY_TAIL))
                .collect();
        }
    }
}

/// A transcript's turns, as [`turns`] reads them.
#[derive(Default)]
struct Turns {
    turns: Vec<Turn>,
    limitations: Vec<String>,
    /// A `codex exec` run with no turn submitted interactively.
    exec: bool,
}

impl Turns {
    /// The digests of its turns with text, in file order.
    fn transcript(&self) -> Vec<u64> {
        self.turns.iter().filter_map(|t| t.digest).collect()
    }
}

/// Reads a transcript's turns, one record at a time, keeping no record; an
/// agent turn's text is cut to its tail as its record is read, scrubbed
/// first when `scrub`.
fn turns(path: &Path, s: &Session, scrub: bool) -> Result<Turns> {
    if s.agent == "opencode" {
        return Ok(opencode_turns(
            crate::opencode::records(path, &s.id)?,
            scrub,
            s.subagent,
        ));
    }
    let file = BufReader::new(std::fs::File::open(path)?);
    let mut read = Turns::default();
    let (mut copying, mut exec, mut interactive) = (false, false, false);
    let mut cwd: Option<String> = None;
    // The questions Codex's question tool asked, by item id.
    let mut questions: HashMap<String, Value> = HashMap::new();
    let mut found = Vec::new();
    for (index, line) in file.split(b'\n').enumerate() {
        let line = line?;
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let number = index + 1;
        let v: Value = match serde_json::from_slice(&line) {
            Ok(v) => v,
            Err(error) => {
                read.limitations
                    .push(format!("line {number}: malformed JSON: {error}"));
                continue;
            }
        };
        match s.agent.as_str() {
            "claude" => claude_turns(&v, s, &mut found),
            "codex" => {
                let p = &v["payload"];
                match (v["type"].as_str(), p["type"].as_str()) {
                    (Some("session_meta"), _) => {
                        if p["id"] == s.id.as_str() {
                            exec |= p["originator"] == "codex_exec" || p["source"] == "exec";
                        } else if s.parent.is_some() {
                            // A fork copies its parent's transcript, headers
                            // included, behind its own header.
                            copying = true;
                        } else {
                            bail!("mixed native session headers");
                        }
                    }
                    (Some("turn_context"), _) => {
                        if let Some(dir) = p["cwd"].as_str() {
                            cwd = Some(dir.to_owned());
                        }
                    }
                    (Some("event_msg"), Some("user_message")) => interactive = true,
                    (Some("event_msg"), Some("item_completed")) => {
                        let item = &p["item"];
                        if let (Some(id), true) =
                            (item["id"].as_str(), item["questions"].is_array())
                        {
                            questions.insert(id.to_owned(), item["questions"].clone());
                        }
                    }
                    (Some("event_msg"), Some("error")) => found.extend(
                        Turn::agent(string(p, "message"), true)
                            .or_else(|| Turn::agent("error reply without text".into(), true)),
                    ),
                    (Some("response_item"), Some("message")) => match p["role"].as_str() {
                        Some("user") => {
                            let text = content(&p["content"]);
                            let own = |turn: Turn| Turn {
                                id: p["id"].as_str().map(str::to_owned),
                                cwd: cwd.clone(),
                                ..turn
                            };
                            match codex_answers(&text, &questions) {
                                Some(words) if words.is_empty() => {
                                    found.push(own(Turn::user(text, Some("question_reply"))))
                                }
                                Some(words) => found.extend(words.into_iter().map(own)),
                                None => found.push(own(Turn::user(text, None))),
                            }
                        }
                        Some("assistant") => {
                            found.extend(Turn::agent(content(&p["content"]), false))
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
            _ => found.extend(pi_turn(&v)),
        }
        for mut turn in found.drain(..) {
            if !turn.user {
                turn.cut(scrub);
            }
            turn.line = number;
            turn.at = timestamp(&v);
            turn.copied = copying;
            read.turns.push(turn);
        }
    }
    // Codex records how a run was started once per file; `codex exec` is
    // headless unless a turn in it was submitted interactively.
    read.exec = exec && !interactive;
    Ok(read)
}

/// Claude Code's fixed words before the reason a person gives for
/// rejecting a tool call.
const REJECTED: &str = "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). To tell you how to proceed, the user said:\n";

fn claude_turns(v: &Value, s: &Session, out: &mut Vec<Turn>) {
    let m = &v["message"];
    if v["type"] == "assistant" {
        let failed = v["isApiErrorMessage"] == true;
        let text = content(&m["content"]);
        out.extend(if failed && text.trim().is_empty() {
            Turn::agent("error reply without text".into(), true)
        } else {
            Turn::agent(text, failed)
        });
        return;
    }
    let own = |turn: Turn| Turn {
        id: v["uuid"].as_str().map(str::to_owned),
        cwd: v["cwd"].as_str().map(str::to_owned),
        headless: v["entrypoint"]
            .as_str()
            .is_some_and(|e| e.starts_with("sdk-"))
            || v["promptSource"] == "sdk",
        ..turn
    };
    // A message typed while the agent works: Claude Code queues it and
    // hands it to the agent mid-turn, recorded as an attachment.
    if v["type"] == "attachment" {
        let a = &v["attachment"];
        if a["type"] == "queued_command" {
            let rule = (a["origin"]["kind"] != "human").then_some("harness_origin");
            out.push(Turn {
                kind: "queued",
                ..own(Turn::user(content(&a["prompt"]), rule))
            });
        }
        return;
    }
    if m["role"] != "user" {
        return;
    }
    // A subagent's records name its session's id.
    let session = s.subagent_of().unwrap_or(&s.id);
    let results: Vec<&Value> = m["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "tool_result")
        .collect();
    let rule = if v["type"] != "user" {
        // `api-request-blob` records copy a request's content; not a turn.
        Some("request_record")
    } else if v["sessionId"].as_str().is_some_and(|id| id != session) {
        Some("other_session")
    } else if v["origin"]["kind"] == "human" {
        // The record says a person wrote it. In a subagent's transcript that
        // is a message a person sent the subagent, which Claude Code also
        // marks as meta.
        None
    } else if !results.is_empty() {
        let words = claude_words(v, &results);
        if !words.is_empty() {
            out.extend(words.into_iter().map(own));
            return;
        }
        Some("tool_result")
    } else if v["isSidechain"] == true {
        // In a subagent's transcript, the prompts its parent agent wrote.
        Some("subagent")
    } else if v["isMeta"] == true {
        Some("meta")
    } else if v["isCompactSummary"] == true {
        Some("compaction")
    } else if v["origin"]["kind"].is_string() || v["promptSource"] == "system" {
        Some("harness_origin")
    } else {
        None
    };
    out.push(own(Turn::user(content(&m["content"]), rule)));
}

/// What a person wrote inside a Claude tool result: an answer to Claude
/// Code's question tool that is not one of the options it offered, and the
/// reason given for rejecting a tool call.
fn claude_words(v: &Value, results: &[&Value]) -> Vec<Turn> {
    let mut words = Vec::new();
    let tool = &v["toolUseResult"];
    for (question, answer) in tool["answers"].as_object().into_iter().flatten() {
        let Some(answer) = answer.as_str() else {
            continue;
        };
        let labels: Vec<&str> = tool["questions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|q| q["question"] == question.as_str())
            .flat_map(|q| q["options"].as_array().into_iter().flatten())
            .filter_map(|o| o["label"].as_str())
            .collect();
        if !picked(answer, &labels) {
            words.push(Turn {
                question: Some(question.clone()),
                ..Turn::words("answer", answer, words.len())
            });
        }
    }
    for result in results {
        if let Some(reason) = content(&result["content"]).strip_prefix(REJECTED) {
            words.push(Turn::words("rejection", reason, words.len()));
        }
    }
    words
}

/// An answer that only picks offered options: one label, or labels joined
/// by ", " as Claude Code joins the picks of a question that allows several.
fn picked(answer: &str, labels: &[&str]) -> bool {
    !labels.is_empty()
        && (labels.contains(&answer) || answer.split(", ").all(|part| labels.contains(&part)))
}

/// What a person wrote in Codex's reply to its question tool: each answer
/// that is not one of the options its question offered. The options come
/// from the `item_completed` that asked; an answer to a question not seen is
/// kept, since it may be a person's own words. None when the text is not a
/// question reply.
fn codex_answers(text: &str, questions: &HashMap<String, Value>) -> Option<Vec<Turn>> {
    let inner = text
        .trim()
        .strip_prefix("<send_user_message_question_reply>")?
        .strip_suffix("</send_user_message_question_reply>")?;
    let Ok(replies) = serde_json::from_str::<Vec<Value>>(inner.trim()) else {
        return Some(Vec::new());
    };
    let mut words = Vec::new();
    for reply in &replies {
        let Some(answer) = reply["answer"].as_str() else {
            continue;
        };
        // `questionItemId` is the tool, the item that asked, and the index
        // of the question in it: an array, which Codex writes as a JSON
        // string.
        let item = match &reply["questionItemId"] {
            Value::String(text) => serde_json::from_str(text).unwrap_or_default(),
            other => other.clone(),
        };
        let asked = item[1]
            .as_str()
            .and_then(|id| questions.get(id))
            .and_then(|qs| qs.get(usize::try_from(item[2].as_u64()?).ok()?));
        let offered = asked.is_some_and(|q| {
            q["options"]
                .as_array()
                .is_some_and(|options| options.iter().any(|o| o == answer))
        });
        if !offered {
            words.push(Turn {
                question: reply["question"].as_str().map(str::to_owned),
                ..Turn::words("answer", answer, words.len())
            });
        }
    }
    Some(words)
}

/// An opencode session's turns. Only its `user` messages can be a person's:
/// opencode keeps what its harness adds (instruction files it loaded,
/// background shell results, restarts) as `synthetic` and `system`
/// messages, which are no turns. An assistant message's prose is its text
/// blocks; reasoning and tool calls are not.
///
/// In a `subagent` session a `user` message is the prompt an agent wrote
/// (`subagent`) when it opens as opencode opens every such prompt, or when
/// it names neither `files` nor `agents`: opencode's prompt box sends both
/// with whatever a person types, and its subagent tool neither. What a
/// person types into a subagent session is theirs.
fn opencode_turns(records: Vec<(usize, Value)>, scrub: bool, subagent: bool) -> Turns {
    let mut read = Turns::default();
    for (line, v) in records {
        if let Some(why) = v["malformed"].as_str() {
            read.limitations
                .push(format!("message {line}: malformed JSON: {why}"));
            continue;
        }
        let d = &v["data"];
        let turn = match v["type"].as_str() {
            Some("user") => {
                let text = string(d, "text");
                let prompted = subagent
                    && (text.starts_with(crate::opencode::SUBAGENT_PROMPT)
                        || (d.get("files").is_none() && d.get("agents").is_none()));
                Some(Turn {
                    id: v["id"].as_str().map(str::to_owned),
                    ..Turn::user(text, prompted.then_some("subagent"))
                })
            }
            Some("assistant") => {
                let failed = d["finish"] == "error" || d["error"].is_object();
                let mut text = content(&d["content"]);
                if failed && text.trim().is_empty() {
                    text = d["error"]["message"]
                        .as_str()
                        .unwrap_or("error reply without text")
                        .to_owned();
                }
                Turn::agent(text, failed)
            }
            _ => None,
        };
        if let Some(mut turn) = turn {
            if !turn.user {
                turn.cut(scrub);
            }
            turn.line = line;
            turn.at = timestamp(&v);
            read.turns.push(turn);
        }
    }
    read
}

fn pi_turn(v: &Value) -> Option<Turn> {
    if v["type"] != "message" {
        return None;
    }
    let m = &v["message"];
    match m["role"].as_str() {
        Some("user") => Some(Turn {
            id: v["id"].as_str().map(str::to_owned),
            ..Turn::user(content(&m["content"]), None)
        }),
        Some("assistant") => {
            let failed = m["stopReason"] == "error";
            let mut text = content(&m["content"]);
            if failed && text.trim().is_empty() {
                text = m["errorMessage"]
                    .as_str()
                    .unwrap_or("error reply without text")
                    .to_owned();
            }
            Turn::agent(text, failed)
        }
        _ => None,
    }
}

/// Decides which of a transcript's user turns are a person's messages,
/// hands those inside the window (after its first time, up to its second)
/// to `emit`, and counts the rest by rule.
fn list(
    s: &Session,
    read: Turns,
    parents: &HashMap<String, Vec<Vec<u64>>>,
    (since, until): (
        Option<chrono::DateTime<chrono::FixedOffset>>,
        Option<chrono::DateTime<chrono::FixedOffset>>,
    ),
    seen: &mut std::collections::HashSet<(String, String, String)>,
    coverage: &mut Coverage,
    emit: &mut dyn FnMut(&Message) -> Result<()>,
) -> Result<Conversation> {
    let mut limitations = read.limitations;
    // A Codex fork's copy is the longest run of its turns that matches its
    // parent's transcript turn for turn; what follows is the fork's own.
    // Without the parent there is nothing to match, so nothing is listed.
    let copy: Vec<u64> = read
        .turns
        .iter()
        .filter(|t| t.copied)
        .filter_map(|t| t.digest)
        .collect();
    let inherited = if copy.is_empty() {
        0
    } else {
        let parent = s.parent.clone().unwrap_or_default();
        let matched = parents
            .get(&parent)
            .into_iter()
            .flatten()
            .map(|transcript| {
                copy.iter()
                    .zip(transcript)
                    .take_while(|(a, b)| a == b)
                    .count()
            })
            .max()
            .unwrap_or(0);
        if matched == 0 {
            limitations.push(format!(
                "Codex fork of {parent}: parent transcript not read or not matching, so its copied history cannot be told from the fork's own turns; {} turns after the copied header excluded",
                copy.len()
            ));
            copy.len()
        } else {
            matched
        }
    };
    // A fork's history before it was forked is its parent's. A subagent
    // copies nothing.
    let fork = s.parent.is_some() && !s.subagent;
    let start = s
        .started_at
        .as_deref()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok());
    // Where each text a person typed last appears as an ordinary message, so
    // a queued copy of a message that later arrives as one counts once.
    let typed: HashMap<String, usize> = read
        .turns
        .iter()
        .enumerate()
        .filter(|(_, t)| t.user && t.kind == "message" && t.rule.is_none())
        .map(|(i, t)| (without_reminders(&t.text).into_owned(), i))
        .collect();
    let mut copied = 0;
    let mut reply: Option<Reply> = None;
    // The person's last message, and whether the agent answered it since.
    let mut last: Option<String> = None;
    let mut answered = false;
    let (mut humans, mut prompts, mut listed) = (0, 0, 0);
    for (index, turn) in read.turns.into_iter().enumerate() {
        let in_copy = turn.copied && turn.digest.is_some() && {
            copied += 1;
            copied <= inherited
        };
        if !turn.user {
            answered |= !turn.failed;
            reply = Some(Reply {
                text: turn.text,
                truncated: turn.truncated,
                failed: turn.failed,
                line: turn.line,
                redacted: turn.redacted,
            });
            continue;
        }
        coverage.user_turns += 1;
        let mut text = without_reminders(&turn.text).into_owned();
        let mut kind = turn.kind;
        // Words a person typed into a conversation, as opposed to words
        // found inside a tool's record (an answer, a rejection).
        let said = matches!(kind, "message" | "queued");
        let at = turn
            .at
            .as_deref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok());
        let rule = turn.rule.or_else(|| {
            if text.is_empty() {
                Some("empty")
            } else if said && text.starts_with(CONTINUATION) {
                Some("compaction")
            } else if said && envelope(&s.agent, &text) {
                Some("envelope")
            } else if said && text.starts_with(INIT) {
                Some("harness_prompt")
            } else if said && launched(&text) {
                match launch_task(&text) {
                    Some(task) => {
                        text = task;
                        kind = "task";
                        None
                    }
                    None => Some("harness_prompt"),
                }
            } else {
                None
            }
        });
        // A message is one identity however many copies hold it: its native
        // id, else its time and text.
        let key = match (&turn.id, &turn.at) {
            (Some(id), _) => Some(format!("id {id} {kind} {}", turn.part)),
            (None, Some(at)) => Some(format!("at {at} {kind} {:x}", digest("user", &text))),
            (None, None) => None,
        };
        let rule = rule.or_else(|| {
            if in_copy {
                Some("fork_copy")
            } else if fork && !turn.copied && !start.zip(at).is_some_and(|(start, at)| at >= start)
            {
                if start.is_none() || at.is_none() {
                    limitations.push(format!(
                        "line {}: fork timestamp boundary unknown; message excluded",
                        turn.line
                    ));
                }
                Some("inherited")
            } else if turn.headless || read.exec {
                prompts += 1;
                Some("headless")
            } else if key.is_some_and(|k| !seen.insert((s.agent.clone(), s.id.clone(), k)))
                || (kind == "queued" && typed.get(&text).is_some_and(|&i| i > index))
            {
                Some("duplicate")
            } else if said && !answered && last.as_deref() == Some(text.as_str()) {
                Some("retry")
            } else {
                None
            }
        });
        if let Some(rule) = rule {
            if rule == "other_session" {
                limitations.push(format!(
                    "line {}: a record of another session; not listed",
                    turn.line
                ));
            }
            *coverage.excluded.entry(rule).or_default() += 1;
            continue;
        }
        humans += 1;
        if said {
            last = Some(text.clone());
            answered = false;
        }
        // `since` is exclusive, as ohjaa intake windows: a reader can pass
        // the time of the last message it handled. `until` is inclusive.
        match at {
            None if since.is_some() || until.is_some() => coverage.undated += 1,
            Some(at) if since.is_some_and(|since| at <= since) => coverage.earlier += 1,
            Some(at) if until.is_some_and(|until| at > until) => {
                *coverage.later.get_or_insert(0) += 1;
            }
            _ => {
                coverage.human += 1;
                listed += 1;
                emit(&Message {
                    agent: s.agent.clone(),
                    session: s.id.clone(),
                    parent: s.parent.clone(),
                    kind,
                    id: turn.id,
                    at: turn.at,
                    cwd: turn.cwd.unwrap_or_else(|| s.cwd.clone()),
                    text,
                    question: turn.question,
                    reply: reply.clone(),
                    source: s.source.clone(),
                    line: turn.line,
                })?;
            }
        }
    }
    let limited = !limitations.is_empty();
    coverage.limitations.extend(
        limitations
            .into_iter()
            .map(|l| format!("{}/{}: {l}", s.agent, s.id)),
    );
    Ok(Conversation {
        agent: s.agent.clone(),
        id: s.id.clone(),
        parent: s.parent.clone(),
        subagent: s.subagent,
        started_at: s.started_at.clone(),
        cwd: s.cwd.clone(),
        source: s.source.clone(),
        headless: prompts > 0 && humans == 0,
        messages: listed,
        limited,
    })
}

/// A message's text: its text blocks, joined.
fn content(v: &Value) -> String {
    v.as_str().map(str::to_owned).unwrap_or_else(|| {
        v.as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| {
                        matches!(
                            b["type"].as_str(),
                            Some("text" | "input_text" | "output_text")
                        )
                    })
                    .filter_map(|b| b["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    })
}

/// A stable digest of one turn, for matching a fork's copy to its parent.
fn digest(role: &str, text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (role, text).hash(&mut hasher);
    hasher.finish()
}

/// The text without the `<system-reminder>` blocks a harness writes into a
/// person's turn, trimmed; as ohjaa intake reads it.
fn without_reminders(text: &str) -> std::borrow::Cow<'_, str> {
    const OPEN: &str = "<system-reminder>";
    const CLOSE: &str = "</system-reminder>";
    if !text.contains(OPEN) {
        return text.trim().into();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(OPEN) {
        out.push_str(&rest[..start]);
        rest = match rest[start..].find(CLOSE) {
            Some(end) => &rest[start + end + CLOSE.len()..],
            None => "",
        };
    }
    out.push_str(rest);
    out.trim().to_owned().into()
}

/// Elements a harness writes into a user turn: startup context,
/// notifications, slash-command records, interruptions, and Codex's review,
/// skill, recovery and plugin envelopes.
const ENVELOPE_TAGS: [&str; 17] = [
    "environment_context",
    "user_instructions",
    "INSTRUCTIONS",
    "task-notification",
    "turn_aborted",
    "command-name",
    "command-message",
    "command-args",
    "local-command-stdout",
    "local-command-stderr",
    "local-command-caveat",
    "system-reminder",
    "user_shell_command",
    "user_action",
    "skill",
    "codex_internal_context",
    "recommended_plugins",
];
/// Whole messages a harness writes verbatim.
const ENVELOPE_TEXTS: [&str; 3] = [
    "[Request interrupted by user]",
    "[Request interrupted by user for tool use]",
    "Caveat: The messages below were generated by the user while running local commands. DO NOT respond to these messages or otherwise consider them in your response unless the user explicitly asks you to.",
];
/// The fixed first sentence of Claude Code's continuation summary.
const CONTINUATION: &str =
    "This session is being continued from a previous conversation that ran out of context.";
/// The fixed first sentence of the prompt Codex sends for `/init`.
const INIT: &str =
    "Generate a file named AGENTS.md that serves as a contributor guide for this repository.";
/// The fixed openings of toimija's launch prompts, inside a repository,
/// outside any, and in a workspace (a folder that holds repositories),
/// which an agent gets as its first message. A person's task, when there is
/// one, follows the harness text.
const LAUNCH: [&str; 3] = [
    "Your toimija session packet is below. It is your workset contract:",
    "You are in a toimija session outside any Git repository (",
    "You are in a toimija session in a workspace, a folder that holds repositories (",
];
/// The heading of the alerts toimija puts in front of a launch prompt.
const ALERTS: &str = "## Before anything else (from the launcher)\n";
/// How each part of ohjaa's briefing opens, as ohjaa writes it: the unproven
/// branch flag, validation, still-running, orphaned and warned sessions,
/// Knowledge (whose preamble also opens scoped knowledge), the last session
/// here, how the session works, sibling worktrees, and the system perimeter.
/// toimija appends the briefing after the task, so in a prompt whose
/// briefing has no sentinel line the first of these after a blank line ends
/// the task.
const BRIEFING: [&str; 10] = [
    "FLAG: ",
    "Validation here (sykli's word, asked at launch;",
    "STILL RUNNING: session ",
    "ORPHANED: session ",
    "WARNING from ",
    "Knowledge (ohjaa, knowledge-node.v1):",
    "Last session here (",
    "How this session works (ohjaa):",
    "Sessions in the other worktrees of this clone at launch",
    "System perimeter (derived by ohjaa at launch;",
];
/// How the line ohjaa's briefing opens with begins and ends. ohjaa writes
/// `<!-- ohjaa-briefing v1 -->` to say
/// where its words begin. A line of this form, after trimming, ends the task
/// in any version: the version tells what follows, not where it starts.
const SENTINEL_OPENS: &str = "<!-- ohjaa-briefing ";
const SENTINEL_CLOSES: &str = "-->";

/// Whether `text` is a toimija launch prompt: its fixed opening, after the
/// launcher's alerts when there are any.
fn launched(text: &str) -> bool {
    let body = match text.strip_prefix(ALERTS) {
        // The alerts end where the launch prompt's opening begins a line.
        Some(alerts) => LAUNCH
            .iter()
            .filter_map(|p| alerts.find(&format!("\n{p}")).map(|at| &alerts[at + 1..]))
            .next()
            .unwrap_or(alerts),
        None => text,
    };
    LAUNCH.iter().any(|p| body.starts_with(p))
}

/// The task a person typed into a launch prompt. toimija writes the packet,
/// then `\nTask: <task>\n`, then `\n<briefing>\n`. A task line can also
/// hold `<task>\n\n<briefing>`, or the briefing alone. The task is what
/// lies between the last `Task:` line and ohjaa's briefing: its sentinel
/// line, or where there is none the first part of it. None when nothing
/// typed is left.
fn launch_task(text: &str) -> Option<String> {
    let task = match sentinel_line(text) {
        Some(end) => text[..end].rsplit_once("\n\nTask: ")?.1,
        None => {
            let end = BRIEFING
                .iter()
                .filter_map(|opening| text.find(&format!("\n\n{opening}")))
                .min()
                .unwrap_or(text.len());
            let (_, task) = text[..end].rsplit_once("\n\nTask: ")?;
            // A briefing with no sentinel can be the task line itself, when
            // no task was typed: `Task: <briefing>`.
            if BRIEFING.iter().any(|opening| task.starts_with(opening)) {
                return None;
            }
            task
        }
    };
    let task = task.trim();
    (!task.is_empty()).then(|| task.to_owned())
}

/// Where the first line of `text` that is a sentinel of ohjaa's briefing, and
/// nothing else but whitespace, begins.
fn sentinel_line(text: &str) -> Option<usize> {
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with(SENTINEL_OPENS) && trimmed.ends_with(SENTINEL_CLOSES) {
            return Some(at);
        }
        at += line.len();
    }
    None
}

/// Text a harness wrote into a user turn, matched in its exact form: a whole
/// sequence of known elements, a verbatim notice, Codex's AGENTS.md heading
/// over such elements, or pi's bare `/command`. A person who starts a message
/// with a path, "Caveat:", a tag name or a heading is still a person.
fn envelope(agent: &str, text: &str) -> bool {
    if ENVELOPE_TEXTS.contains(&text) {
        return true;
    }
    // Claude Code records slash commands as tags; pi writes the command alone.
    if agent == "pi" {
        if let Some(name) = text.strip_prefix('/') {
            return !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':'));
        }
    }
    let body = match text.split_once('\n') {
        Some((heading, rest))
            if heading.trim_end() == "# AGENTS.md instructions"
                || heading.starts_with("# AGENTS.md instructions for ") =>
        {
            rest
        }
        _ => text,
    };
    elements_only(body, &ENVELOPE_TAGS)
}

/// True when `text` is one or more of the elements named and nothing else.
fn elements_only(text: &str, names: &[&str]) -> bool {
    let mut text = text.trim_start();
    if text.is_empty() {
        return false;
    }
    while !text.is_empty() {
        let Some(rest) = text.strip_prefix('<') else {
            return false;
        };
        let Some(name) = names.iter().find(|name| {
            rest.strip_prefix(**name)
                .is_some_and(|after| after.starts_with(['>', ' ', '\n']))
        }) else {
            return false;
        };
        let close = format!("</{name}>");
        let Some(end) = text.find(&close) else {
            return false;
        };
        text = text[end + close.len()..].trim_start();
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeSet;

    fn read(agent: &str, records: &[Value], cwd: &str) -> BTreeSet<PathBuf> {
        let mut cwd = cwd.to_owned();
        let mut out = BTreeSet::new();
        for v in records {
            touched(v, agent, &mut cwd, &mut out);
        }
        out
    }

    fn paths(paths: &[&str]) -> BTreeSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn a_patch_names_its_files_on_its_header_lines() {
        let patch = "*** Begin Patch\n*** Add File: a.rs\n+x\n*** Update File: src/b.rs\n*** Move to: src/c.rs\n@@\n-*** Delete File: not/a/header\n*** Delete File: d.rs\n*** End Patch\n";
        assert_eq!(
            patched(patch).collect::<Vec<_>>(),
            ["a.rs", "src/b.rs", "src/c.rs", "d.rs"]
        );
    }

    #[test]
    fn every_folder_a_session_records_counts_not_only_its_first() {
        // A session started in a parent folder that later works with the
        // repository as its folder, with no structured file in between.
        let claude = [
            json!({"type":"user","cwd":"/w","message":{"role":"user","content":"start"}}),
            json!({"type":"assistant","cwd":"/w/repo","message":{"role":"assistant","content":[]}}),
        ];
        assert_eq!(read("claude", &claude, ""), paths(&["/w", "/w/repo"]));
        let codex = [
            json!({"type":"session_meta","payload":{"cwd":"/w"}}),
            json!({"type":"turn_context","payload":{"cwd":"/w/repo"}}),
        ];
        assert_eq!(read("codex", &codex, ""), paths(&["/w", "/w/repo"]));
    }

    #[test]
    fn claude_tool_calls_name_files_in_their_structured_fields_only() {
        let call = |name: &str, input: Value| {
            json!({"type":"assistant","cwd":"/w/repo","message":{"role":"assistant",
                "content":[{"type":"tool_use","name":name,"input":input}]}})
        };
        let records = [
            call("Edit", json!({"file_path":"/w/repo/a.rs"})),
            call("Read", json!({"file_path":"b.rs"})),
            call("NotebookEdit", json!({"notebook_path":"n.ipynb"})),
            call("Grep", json!({"pattern":"x","path":"src"})),
            call("Glob", json!({"pattern":"*.rs"})),
            // A shell command is free text: never read for paths.
            call("Bash", json!({"command":"cat /w/other/secret.rs"})),
            json!({"type":"user","cwd":"/w/repo","message":{"role":"user","content":"edit /w/other/x.rs"}}),
        ];
        assert_eq!(
            read("claude", &records, ""),
            paths(&[
                "/w/repo",
                "/w/repo/a.rs",
                "/w/repo/b.rs",
                "/w/repo/n.ipynb",
                "/w/repo/src"
            ])
        );
    }

    #[test]
    fn codex_names_a_call_s_workdir_and_a_patch_s_files_against_the_turn_s_folder() {
        let item = |payload: Value| json!({"type":"response_item","payload":payload});
        let records = [
            json!({"type":"session_meta","payload":{"cwd":"/w"}}),
            json!({"type":"turn_context","payload":{"cwd":"/w/repo"}}),
            item(json!({"type":"function_call","name":"exec_command",
                "arguments":json!({"cmd":"ls /w/other","workdir":"sub"}).to_string()})),
            item(json!({"type":"custom_tool_call","name":"apply_patch",
                "input":"*** Begin Patch\n*** Update File: src/lib.rs\n*** End Patch\n"})),
            item(json!({"type":"function_call","name":"exec_command",
                "arguments":json!({"cmd":"cd /w/other && make"}).to_string()})),
            item(json!({"type":"custom_tool_call","name":"exec","input":"open('/w/other/x')"})),
        ];
        assert_eq!(
            read("codex", &records, ""),
            paths(&["/w", "/w/repo", "/w/repo/sub", "/w/repo/src/lib.rs"])
        );
    }

    #[test]
    fn pi_names_a_tool_call_s_path_against_the_session_s_folder() {
        let records = [
            json!({"type":"session","cwd":"/w"}),
            json!({"type":"message","message":{"role":"assistant","content":[
                {"type":"toolCall","name":"edit","arguments":{"path":"repo/a.rs"}},
                {"type":"toolCall","name":"bash","arguments":{"command":"cat /w/other/x"}}]}}),
        ];
        assert_eq!(read("pi", &records, ""), paths(&["/w", "/w/repo/a.rs"]));
        // A relative path with no folder to resolve it against is left out.
        let records = [
            json!({"type":"message","message":{"role":"assistant","content":[
            {"type":"toolCall","name":"read","arguments":{"path":"a.rs"}}]}}),
        ];
        assert!(read("pi", &records, "").is_empty());
    }
}
