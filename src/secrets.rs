//! On-demand metadata only. Values exist only while a native record is scanned.
//!
//! A finding is an alarm: one of the credentials this machine holds
//! ([`crate::held`]) appeared in a transcript. Every other secret-shaped
//! string is counted by rule (`candidates`), the plain placeholders among
//! them apart (`placeholders`), and never listed.
use crate::held::{self, Held};
use crate::{model::Session, scrub};
use chrono::{DateTime, FixedOffset};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

#[derive(Serialize)]
pub struct Report {
    schema: &'static str,
    since: Option<String>,
    pub complete: bool,
    pub transcripts: usize,
    /// Readable finding records which produced a missing-timestamp warning.
    pub undated_records: usize,
    /// Each held credential that appeared: the alarms.
    pub findings: Vec<Finding>,
    pub unreadable: Vec<Gap>,
    /// Where this machine's credentials were looked for, by name.
    pub compared_with: Vec<held::Source>,
    /// Secret-shaped strings that are none of them, by rule: counted only.
    pub candidates: BTreeMap<&'static str, usize>,
    /// Of `candidates`, those that are plainly placeholders.
    pub placeholders: usize,
}

#[derive(Serialize)]
pub struct Finding {
    agent: String,
    session: String,
    cwd: Option<String>,
    source: String,
    line: usize,
    at: Option<String>,
    /// `at` is the time of the record before, as this one has none.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    at_inferred: bool,
    #[serde(rename = "where")]
    place: &'static str,
    rule: &'static str,
    count: usize,
    /// Which of this machine's credentials it is, by its source's name.
    held: String,
    /// A measured subset of count, in native signature/encrypted-content fields.
    opaque_count: Option<usize>,
    /// Positive opaque subsets were attributed at verified native positions.
    opaque_native: Option<bool>,
}

#[derive(Serialize)]
pub struct Gap {
    path: String,
    line: Option<usize>,
    why: String,
}

impl Report {
    pub fn new(since: Option<DateTime<FixedOffset>>) -> Self {
        Self {
            schema: "vessa-secrets.v1",
            since: since.map(|t| t.to_rfc3339()),
            complete: true,
            transcripts: 0,
            undated_records: 0,
            findings: Vec::new(),
            unreadable: Vec::new(),
            compared_with: Vec::new(),
            candidates: BTreeMap::new(),
            placeholders: 0,
        }
    }

    /// Where `held` looked: a source that exists but could not be read
    /// leaves the comparison partial, never an all-clear.
    pub fn compared(&mut self, held: &mut Held) {
        for source in std::mem::take(&mut held.sources) {
            if source.status == "unreadable" {
                let why = source.why.clone().unwrap_or_default();
                self.gap(
                    &source.source,
                    None,
                    format!("credentials could not be read to compare with: {why}"),
                );
            }
            self.compared_with.push(source);
        }
    }
    pub fn gap(&mut self, path: &str, line: Option<usize>, why: String) {
        self.complete = false;
        self.unreadable.push(Gap {
            path: path.into(),
            line,
            why,
        });
    }
}

/// The document contains metadata only, but native IDs and path components
/// can themselves look secret. Check complete strings and each path component
/// with the existing detector; never expose a matched value through a label.
pub fn scrub_metadata(value: &mut Value) {
    match value {
        Value::String(text) => {
            let mut scrub = scrub::Redactions::default();
            let whole = scrub.text(text);
            *text = whole
                .split_inclusive(['/', '\\'])
                .map(|part| scrub.text(part))
                .collect();
        }
        Value::Array(items) => items.iter_mut().for_each(scrub_metadata),
        Value::Object(fields) => fields.values_mut().for_each(scrub_metadata),
        _ => {}
    }
}

pub fn scan_file(
    path: &Path,
    session: &Session,
    since: Option<DateTime<FixedOffset>>,
    held: &Held,
    report: &mut Report,
) {
    let source = path.display().to_string();
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) => {
            report.gap(&source, None, error.to_string());
            return;
        }
    };
    report.transcripts += 1;
    let mut scan = Scan::new(source, session, since, held);
    for (index, line) in BufReader::new(file).split(b'\n').enumerate() {
        let number = index + 1;
        let bytes = match line {
            Ok(bytes) => bytes,
            Err(error) => {
                report.gap(&scan.source, Some(number), error.to_string());
                break;
            }
        };
        if bytes.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(record) => scan.record(number, &record, report),
            Err(error) => scan.damaged(
                number,
                &String::from_utf8_lossy(&bytes),
                format!("malformed JSON/UTF-8: {error}"),
                report,
            ),
        }
    }
}

/// One opencode session's messages, as [`crate::opencode::records`] reads
/// them from `store`: each scanned as a transcript's record is, its `seq`
/// for a line. A message whose data is no JSON is scanned as text.
pub fn scan_records(
    store: &Path,
    session: &Session,
    records: Vec<(usize, Value)>,
    since: Option<DateTime<FixedOffset>>,
    held: &Held,
    report: &mut Report,
) {
    report.transcripts += 1;
    let mut scan = Scan::new(store.display().to_string(), session, since, held);
    for (seq, record) in records {
        match (record["malformed"].as_str(), record["data"].as_str()) {
            (Some(why), Some(text)) => {
                scan.damaged(seq, text, format!("malformed JSON: {why}"), report)
            }
            _ => scan.record(seq, &record, report),
        }
    }
}

/// One transcript's scan, record by record, in the order it was written.
struct Scan<'a> {
    source: String,
    session: &'a Session,
    since: Option<DateTime<FixedOffset>>,
    held: &'a Held,
    /// The time of the last record that had one: a transcript is appended
    /// to, so a record without a time was written at or after it.
    previous: Option<(String, DateTime<FixedOffset>)>,
}

impl<'a> Scan<'a> {
    fn new(
        source: String,
        session: &'a Session,
        since: Option<DateTime<FixedOffset>>,
        held: &'a Held,
    ) -> Self {
        Scan {
            source,
            session,
            since,
            held,
            previous: None,
        }
    }

    /// A record that could not be read: a gap. A detectable occurrence in it
    /// is still kept; its role and time are unknown, never silently assigned
    /// to a window.
    fn damaged(&mut self, number: usize, text: &str, why: String, report: &mut Report) {
        report.gap(&self.source, Some(number), why);
        let mut marks = BTreeMap::new();
        count(text, "other", None, self.held, &mut marks, report);
        append(
            report,
            self.session,
            &self.source,
            number,
            (None, false),
            marks,
        );
    }

    fn record(&mut self, number: usize, record: &Value, report: &mut Report) {
        let session = self.session;
        let stamp = record.get("timestamp").or_else(|| {
            (session.agent == "claude" && record["type"] == "file-history-snapshot")
                .then(|| record["snapshot"].get("timestamp"))
                .flatten()
        });
        let at = stamp
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                stamp
                    .and_then(Value::as_i64)
                    .and_then(DateTime::from_timestamp_millis)
                    .map(|t| t.to_rfc3339())
            });
        let parsed = at
            .as_deref()
            .and_then(|t| DateTime::parse_from_rfc3339(t).ok());
        let before = self.previous.take();
        self.previous = at.clone().zip(parsed).or(before.clone());
        let since = self.since;
        if since.zip(parsed).is_some_and(|(from, at)| at <= from) {
            return;
        }
        // No time of its own: the record before's, when that places it.
        // After `since` it places this one there too; before, it cannot.
        let at = match (parsed.and(at), before) {
            (Some(at), _) => (Some(at), false),
            (None, Some((at, t))) if since.is_none_or(|from| t > from) => (Some(at), true),
            (None, _) => (None, false),
        };
        let mut marks = BTreeMap::new();
        for (place, value, opaque_field) in pieces(&session.agent, record) {
            count_piece(value, place, opaque_field, self.held, &mut marks, report);
        }
        if !marks.is_empty() && since.is_some() && at.0.is_none() {
            report.gap(
                &self.source,
                Some(number),
                "finding has no valid time; it is included but cannot be placed after --since"
                    .into(),
            );
            report.undated_records += 1;
        }
        append(report, session, &self.source, number, at, marks);
    }
}

struct Counts {
    total: usize,
    opaque: Option<usize>,
}
impl Default for Counts {
    fn default() -> Self {
        Self {
            total: 0,
            opaque: Some(0),
        }
    }
}
/// The held credentials found in one record, by where, rule and source.
type Marks = BTreeMap<(&'static str, &'static str, String), Counts>;

fn count_piece(
    value: &Value,
    place: &'static str,
    field: Option<&'static str>,
    held: &Held,
    marks: &mut Marks,
    report: &mut Report,
) {
    if let Some((field, text)) =
        field.and_then(|field| value[field].as_str().map(|text| (field, text)))
    {
        // Keep scanning both portions. Context is not an exemption: even a
        // provider-shaped key in a metadata field remains a finding.
        count(text, place, Some(true), held, marks, report);
        let mut rest = value.clone();
        rest.as_object_mut()
            .expect("a field belongs to an object")
            .remove(field);
        let mut text = String::new();
        flatten(&rest, &mut text);
        count(&text, place, Some(false), held, marks, report);
    } else {
        let mut text = String::new();
        flatten(value, &mut text);
        count(&text, place, Some(false), held, marks, report);
    }
}

/// `text`, as a terminal shows it: each credential this machine holds that
/// stands in it is marked for a finding, under the rule the detector gives
/// it there (`no_known_shape` where it gives none, as for a value only a
/// label makes look secret); every other secret-shaped string is counted
/// by its rule, and as a placeholder when it plainly is one.
fn count(
    text: &str,
    place: &'static str,
    opaque: Option<bool>,
    held: &Held,
    marks: &mut Marks,
    report: &mut Report,
) {
    let shown = scrub::shown(text);
    let found = scrub::found(&shown);
    for (rule, value) in &found {
        if held.which(value).is_none() {
            *report.candidates.entry(rule).or_default() += 1;
            report.placeholders += usize::from(placeholder(value));
        }
    }
    for (value, source) in held.each() {
        let n = held::occurrences(&shown, value);
        if n == 0 {
            continue;
        }
        let rule = found
            .iter()
            .find(|(_, v)| v == value)
            .map_or("no_known_shape", |(rule, _)| rule);
        let counts = marks.entry((place, rule, source.to_owned())).or_default();
        counts.total += n;
        match (counts.opaque.as_mut(), opaque) {
            (Some(count), Some(true)) => *count += n,
            (_, None) => counts.opaque = None,
            _ => {}
        }
    }
}

/// A secret-shaped string that is plainly no secret: made of a few
/// characters repeated (`sk-a1a1a1…`), holding a straight run of the
/// alphabet or of digits (`ABCDEF…`), or ending as AWS's documentation
/// examples do (`…EXAMPLE`, `…EXAMPLEKEY`). A real key is random, and is
/// none of these.
fn placeholder(value: &str) -> bool {
    let alnum: Vec<u8> = value.bytes().filter(u8::is_ascii_alphanumeric).collect();
    let distinct = alnum
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let run = alnum
        .windows(6)
        .any(|w| w.windows(2).all(|pair| pair[1] == pair[0] + 1));
    distinct < 10 || run || value.ends_with("EXAMPLE") || value.ends_with("EXAMPLEKEY")
}

fn append(
    report: &mut Report,
    session: &Session,
    source: &str,
    line: usize,
    (at, at_inferred): (Option<String>, bool),
    marks: Marks,
) {
    for ((place, rule, held), counts) in marks {
        report.findings.push(Finding {
            agent: session.agent.clone(),
            session: session.id.clone(),
            cwd: (!session.cwd.is_empty()).then(|| session.cwd.clone()),
            source: source.into(),
            line,
            at: at.clone(),
            at_inferred: at.is_some() && at_inferred,
            place,
            rule,
            count: counts.total,
            held,
            opaque_count: counts.opaque,
            opaque_native: counts.opaque.map(|_| true),
        });
    }
}

/// Keep raw strings and their key labels, not JSON-escaped representations:
/// multiline keys, hidden terminal characters and contextual rules still work.
fn flatten(value: &Value, out: &mut String) {
    match value {
        Value::String(text) => out.push_str(text),
        Value::Array(items) => {
            for item in items {
                flatten(item, out);
                out.push('\n');
            }
        }
        Value::Object(fields) => {
            for (key, value) in fields {
                out.push_str(key);
                out.push_str(": ");
                flatten(value, out);
                out.push('\n');
            }
        }
        _ => {}
    }
}

fn role(role: Option<&str>) -> &'static str {
    match role {
        Some("user") => "person",
        Some("assistant") => "agent",
        Some("tool" | "toolResult") => "tool_output",
        _ => "other",
    }
}

type Piece<'a> = (&'static str, &'a Value, Option<&'static str>);

fn blocks<'a>(
    content: &'a Value,
    place: &'static str,
    native_thinking: bool,
    out: &mut Vec<Piece<'a>>,
) {
    if let Some(items) = content.as_array() {
        for block in items {
            let before = out.len();
            match block["type"].as_str() {
                Some("tool_use" | "toolCall") => {
                    for field in ["input", "arguments"] {
                        if !block[field].is_null() {
                            out.push(("tool_input", &block[field], None));
                        }
                    }
                }
                Some("tool_result" | "toolResult") => {
                    for field in ["content", "output"] {
                        if !block[field].is_null() {
                            out.push(("tool_output", &block[field], None));
                        }
                    }
                }
                Some("thinking") if native_thinking => out.push((place, block, Some("signature"))),
                _ => out.push((place, block, None)),
            }
            if out.len() == before {
                out.push(("other", block, None));
            }
        }
    } else {
        out.push((place, content, None));
    }
}

/// An opencode message's pieces. A person writes `user` messages; an
/// assistant message's text and reasoning blocks are the agent's, and a tool
/// block's `state` holds the call's `input` and, in every other field, what
/// came back. Everything else, harness messages included, is `other`.
fn opencode_pieces<'a>(record: &'a Value, out: &mut Vec<Piece<'a>>) {
    let d = &record["data"];
    let fields = |value: &'a Value| value.as_object().into_iter().flatten();
    match record["type"].as_str() {
        Some("user") => out.push(("person", d, None)),
        Some("assistant") if d.is_object() => {
            for (key, value) in fields(d) {
                if key != "content" {
                    out.push(("other", value, None));
                    continue;
                }
                for block in value.as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text" | "reasoning") => out.push(("agent", block, None)),
                        Some("tool") => {
                            for (key, value) in fields(block) {
                                if key != "state" {
                                    out.push(("other", value, None));
                                } else if value.is_object() {
                                    for (key, value) in fields(value) {
                                        let place = if key == "input" {
                                            "tool_input"
                                        } else {
                                            "tool_output"
                                        };
                                        out.push((place, value, None));
                                    }
                                } else {
                                    out.push(("tool_output", value, None));
                                }
                            }
                        }
                        _ => out.push(("other", block, None)),
                    }
                }
            }
        }
        _ => out.push(("other", d, None)),
    }
}

fn pieces<'a>(agent: &str, record: &'a Value) -> Vec<Piece<'a>> {
    let mut out = Vec::new();
    if agent == "opencode" {
        opencode_pieces(record, &mut out);
    } else if agent == "codex" && record["type"] == "response_item" {
        let p = &record["payload"];
        match p["type"].as_str() {
            Some("message") => blocks(&p["content"], role(p["role"].as_str()), false, &mut out),
            Some("reasoning") => out.push(("other", p, Some("encrypted_content"))),
            Some("function_call" | "custom_tool_call") => {
                for field in ["arguments", "input"] {
                    if !p[field].is_null() {
                        out.push(("tool_input", &p[field], None));
                    }
                }
            }
            Some("function_call_output" | "custom_tool_call_output") => {
                out.push(("tool_output", &p["output"], None))
            }
            _ => out.push(("other", p, None)),
        }
    } else if record["message"].is_object() {
        let m = &record["message"];
        let place = if record["isMeta"] == true {
            "other"
        } else {
            role(m["role"].as_str())
        };
        let native_thinking =
            agent == "claude" && record["type"] == "assistant" && place == "agent";
        blocks(&m["content"], place, native_thinking, &mut out);
    } else if agent == "codex" && record["type"] == "event_msg" {
        let p = &record["payload"];
        let place = match p["type"].as_str() {
            Some("user_message") => "person",
            Some("agent_message" | "agent_reasoning") => "agent",
            _ => "other",
        };
        out.push((place, p, None));
    } else {
        out.push(("other", record, None));
    }
    out.retain(|(_, value, _)| !value.is_null());
    if out.is_empty() {
        out.push(("other", record, None));
    }
    out
}
