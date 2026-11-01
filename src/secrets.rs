//! On-demand metadata only. Values exist only while a native record is scanned.
use crate::{model::Session, scrub::Redactions};
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
    pub findings: Vec<Finding>,
    pub unreadable: Vec<Gap>,
}

#[derive(Serialize)]
pub struct Finding {
    agent: String,
    session: String,
    cwd: Option<String>,
    source: String,
    line: usize,
    at: Option<String>,
    #[serde(rename = "where")]
    place: &'static str,
    rule: &'static str,
    count: usize,
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
            let mut scrub = Redactions::default();
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
    for (index, line) in BufReader::new(file).split(b'\n').enumerate() {
        let number = index + 1;
        let bytes = match line {
            Ok(bytes) => bytes,
            Err(error) => {
                report.gap(&source, Some(number), error.to_string());
                break;
            }
        };
        if bytes.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let record: Value = match serde_json::from_slice(&bytes) {
            Ok(record) => record,
            Err(error) => {
                report.gap(
                    &source,
                    Some(number),
                    format!("malformed JSON/UTF-8: {error}"),
                );
                // Retain a detectable occurrence even in a damaged record; its
                // role and time are unknown, never silently assigned to a window.
                let text = String::from_utf8_lossy(&bytes);
                let mut marks = BTreeMap::new();
                count(&text, "other", None, &mut marks);
                append(report, session, &source, number, None, marks);
                continue;
            }
        };
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
        if since.zip(parsed).is_some_and(|(from, at)| at <= from) {
            continue;
        }
        let at = parsed.and(at);
        let mut marks = BTreeMap::new();
        for (place, value, opaque_field) in pieces(&session.agent, &record) {
            count_piece(value, place, opaque_field, &mut marks);
        }
        if !marks.is_empty() && since.is_some() && at.is_none() {
            report.gap(
                &source,
                Some(number),
                "finding has no valid time; it is included but cannot be placed after --since"
                    .into(),
            );
            report.undated_records += 1;
        }
        append(report, session, &source, number, at, marks);
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
type Marks = BTreeMap<(&'static str, &'static str), Counts>;

fn count_piece(value: &Value, place: &'static str, field: Option<&'static str>, marks: &mut Marks) {
    if let Some((field, text)) =
        field.and_then(|field| value[field].as_str().map(|text| (field, text)))
    {
        // Keep scanning both portions. Context is not an exemption: even a
        // provider-shaped key in a metadata field remains a finding.
        count(text, place, Some(true), marks);
        let mut rest = value.clone();
        rest.as_object_mut()
            .expect("a field belongs to an object")
            .remove(field);
        let mut text = String::new();
        flatten(&rest, &mut text);
        count(&text, place, Some(false), marks);
    } else {
        let mut text = String::new();
        flatten(value, &mut text);
        count(&text, place, Some(false), marks);
    }
}

fn count(text: &str, place: &'static str, opaque: Option<bool>, marks: &mut Marks) {
    let mut found = Redactions::default();
    let _ = found.text(text);
    for (&rule, &n) in found.by_rule() {
        let counts = marks.entry((place, rule)).or_default();
        counts.total += n;
        match (counts.opaque.as_mut(), opaque) {
            (Some(count), Some(true)) => *count += n,
            (_, None) => counts.opaque = None,
            _ => {}
        }
    }
}

fn append(
    report: &mut Report,
    session: &Session,
    source: &str,
    line: usize,
    at: Option<String>,
    marks: Marks,
) {
    for ((place, rule), counts) in marks {
        report.findings.push(Finding {
            agent: session.agent.clone(),
            session: session.id.clone(),
            cwd: (!session.cwd.is_empty()).then(|| session.cwd.clone()),
            source: source.into(),
            line,
            at: at.clone(),
            place,
            rule,
            count: counts.total,
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

fn pieces<'a>(agent: &str, record: &'a Value) -> Vec<Piece<'a>> {
    let mut out = Vec::new();
    if agent == "codex" && record["type"] == "response_item" {
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
