use crate::model::{Entry, Report, Session, Usage};
use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub fn read(paths: &[PathBuf]) -> Result<Report> {
    // ponytail: selected transcripts are held in memory; stream compact
    // summaries if archive size makes this one-shot reader too expensive.
    let (mut report, wrappers) = sessions(paths, parse_file);
    deduplicate(&mut report);
    link(&mut report, wrappers);
    Ok(report)
}

/// The sessions under `paths` with their headers alone: what each transcript
/// says about itself, read by the same rules as [`read`], from only as many
/// of its records as settle them (see [`Scan::settled`]). No usage is read,
/// so the sessions carry no usage and no limitations.
pub fn headers(paths: &[PathBuf]) -> Result<Report> {
    let (mut report, wrappers) = sessions(paths, header_file);
    report.schema = "vessa-session-headers.v1".into();
    link(&mut report, wrappers);
    Ok(report)
}

/// Every transcript under `paths`, read with `parse`, one session per native
/// identity, parents first; with the ohjaa wrapper records selected. Both
/// readers start here, so they list the same sessions in the same order.
fn sessions(paths: &[PathBuf], parse: fn(&Path) -> Result<Parsed>) -> (Report, Vec<Value>) {
    let mut report = Report {
        schema: "vessa-usage.v1".into(),
        ..Default::default()
    };
    let mut files = Vec::new();
    for path in paths {
        if let Err(error) = discover(path, &mut files) {
            report.warnings.push(format!("{}: {error}", path.display()));
        }
        // Claude Code keeps a session's subagent transcripts beside it, in
        // `<session>/subagents/`. They are that session's usage, so a
        // transcript selected on its own brings them along.
        let subagents = path.with_extension("").join("subagents");
        if path.extension().is_some_and(|s| s == "jsonl")
            && std::fs::symlink_metadata(&subagents).is_ok()
        {
            if let Err(error) = discover(&subagents, &mut files) {
                report
                    .warnings
                    .push(format!("{}: {error}", subagents.display()));
            }
        }
    }
    files.sort();
    files.dedup();
    let mut wrappers = Vec::new();
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
    // request/message identities; never use equal text to deduplicate spending.
    let mut merged: BTreeMap<(String, String), Session> = BTreeMap::new();
    for session in report.sessions {
        let key = (session.agent.clone(), session.id.clone());
        if let Some(old) = merged.get_mut(&key) {
            old.usage.extend(session.usage);
            old.work.extend(session.work);
            old.limitations.extend(session.limitations);
            old.launched.extend(session.launched);
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
    for (index, session) in report.sessions.iter_mut().enumerate() {
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
                if *owner != index {
                    elsewhere[index].insert(item.id.clone());
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
    for session in report.sessions.iter_mut().filter(|s| s.agent == "claude") {
        let top = session.subagent_of().unwrap_or(&session.id).to_owned();
        let mut named = std::collections::HashSet::new();
        for (agent, line) in std::mem::take(&mut session.launched) {
            let id = format!("{top}/{agent}");
            if !read.contains(&id) && named.insert(id.clone()) {
                session.limitations.push(format!(
                    "line {line}: subagent {id} was launched, but its transcript was not read; its usage is unknown, not zero"
                ));
            }
        }
    }
    for ((session, usage), elsewhere) in report.sessions.iter_mut().zip(kept).zip(elsewhere) {
        session.usage = usage;
        session.counted_elsewhere = elsewhere.len();
        // A subagent whose every request its session already holds has had
        // its usage counted there; it is not unknown.
        let counted = session.subagent_of().is_some() && session.counted_elsewhere > 0;
        if session.usage.is_empty() && !counted {
            session
                .limitations
                .push("No usage records found; consumption is unknown, not zero".into());
        }
        session.limitations.sort();
        session.limitations.dedup();
    }
}

/// Attaches each ohjaa wrapper record to the sessions it explicitly names,
/// and says which name none, or that no session was found at all.
fn link(report: &mut Report, wrappers: Vec<Value>) {
    for session in &mut report.sessions {
        for wrapper in &wrappers {
            if wrapper["usage"].as_array().is_some_and(|receipts| {
                receipts
                    .iter()
                    .any(|r| r["agent"] == session.agent && r["session_id"] == session.id)
            }) {
                session.ohjaa.push(wrapper.clone());
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
                "ohjaa session {} has no explicit matching native transcript",
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
        (
            "estimate",
            old.estimated_usd_micros,
            new.estimated_usd_micros,
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

fn discover(path: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        bail!("symlink not followed; provide its real path");
    }
    if meta.is_dir() {
        for item in std::fs::read_dir(path)? {
            let item = item?;
            // Native session directories also hold JSON metadata, not usage
            // exports. JSON exports must be selected explicitly with --input.
            if item.file_type()?.is_dir() || item.path().extension().is_some_and(|s| s == "jsonl") {
                discover(&item.path(), out)?;
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

/// An ohjaa `.json` export: its wrapper session records, and what it could
/// not read.
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
        .map(|id| format!("ohjaa could not read session {id}"))
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
    let file = BufReader::new(std::fs::File::open(path)?);
    let mut scan = Scan::default();
    for line in file.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        // A malformed record is skipped, as `parse_file` skips it; saying so
        // is a usage limitation, which this reader does not report.
        if let Ok(value) = serde_json::from_str::<Value>(&line) {
            scan.observe(&value);
            if scan.settled() {
                break;
            }
        }
    }
    Ok(Parsed::Session(Box::new(scan.session(path, Vec::new())?)))
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
    /// Whether any record is a Claude sidechain, and whether any says.
    sidechain: bool,
    sidechain_said: bool,
}

/// The header facts of one record that opens a session.
struct Opening {
    id: String,
    parent: Option<String>,
    started_at: Option<String>,
    cwd: String,
    originator: Option<String>,
    subagent_source: bool,
}

impl Opening {
    /// `meta` is the record itself, or Codex's `payload`; `id` its id field.
    fn new(record: &Value, meta: &Value, id: &str) -> Self {
        Opening {
            id: string(meta, id),
            parent: meta["parentSession"]
                .as_str()
                .or_else(|| meta["forked_from_id"].as_str())
                .map(str::to_owned),
            started_at: timestamp(meta).or_else(|| timestamp(record)),
            cwd: string(meta, "cwd"),
            originator: meta["originator"].as_str().map(str::to_owned),
            subagent_source: !meta["source"]["subagent"].is_null(),
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
        self.sidechain |= v["isSidechain"] == true;
        self.sidechain_said |= v["isSidechain"].is_boolean();
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
            source: path.display().to_string(),
            usage: Vec::new(),
            limitations,
            ohjaa: Vec::new(),
            work: Vec::new(),
            launched: Vec::new(),
            counted_elsewhere: 0,
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
    for (index, line) in file.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(&line) {
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
    for (line, value) in records {
        crate::work::observe(&value, line, &mut session);
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
        estimated_usd_micros: None,
        estimate_basis: None,
        credits: None,
        billed: None,
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
        s.launched.push((agent.to_owned(), line));
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
        // Newer Codex also counts cache writes. Vessa assumes that, like cached
        // reads, they are part of the inclusive input; every real value seen so
        // far is zero, so a non-zero one is reported until that is confirmed.
        // Older Codex omits them: OpenAI prompt caching has no write category.
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
        if let Some(cost) = u["cost"]["total"].as_f64() {
            ensure!(
                cost.is_finite() && cost >= 0.0 && cost * 1_000_000.0 < u64::MAX as f64,
                "invalid cost"
            );
            entry.estimated_usd_micros = Some((cost * 1_000_000.0).round() as u64);
            entry.estimate_basis = Some("pi recorded model rates; not provider billing".into());
        }
        add(s, v, line, id, entry);
    }
    Ok(())
}
