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
                count(&text, "other", &mut marks);
                append(report, session, &source, number, None, marks);
                continue;
            }
        };
        let at = record["timestamp"].as_str().map(str::to_owned).or_else(|| {
            record["timestamp"]
                .as_i64()
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
        for (place, value) in pieces(&session.agent, &record) {
            let mut text = String::new();
            flatten(value, &mut text);
            count(&text, place, &mut marks);
        }
        if !marks.is_empty() && since.is_some() && at.is_none() {
            report.gap(
                &source,
                Some(number),
                "finding has no valid time; it is included but cannot be placed after --since"
                    .into(),
            );
        }
        append(report, session, &source, number, at, marks);
    }
}

type Marks = BTreeMap<(&'static str, &'static str), usize>;
fn count(text: &str, place: &'static str, marks: &mut Marks) {
    let mut found = Redactions::default();
    let _ = found.text(text);
    for (&rule, &n) in found.by_rule() {
        *marks.entry((place, rule)).or_default() += n;
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
    for ((place, rule), count) in marks {
        report.findings.push(Finding {
            agent: session.agent.clone(),
            session: session.id.clone(),
            cwd: (!session.cwd.is_empty()).then(|| session.cwd.clone()),
            source: source.into(),
            line,
            at: at.clone(),
            place,
            rule,
            count,
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

fn blocks<'a>(content: &'a Value, place: &'static str, out: &mut Vec<(&'static str, &'a Value)>) {
    if let Some(items) = content.as_array() {
        for block in items {
            let before = out.len();
            match block["type"].as_str() {
                Some("tool_use" | "toolCall") => {
                    for field in ["input", "arguments"] {
                        if !block[field].is_null() {
                            out.push(("tool_input", &block[field]));
                        }
                    }
                }
                Some("tool_result" | "toolResult") => {
                    for field in ["content", "output"] {
                        if !block[field].is_null() {
                            out.push(("tool_output", &block[field]));
                        }
                    }
                }
                _ => out.push((place, block)),
            }
            if out.len() == before {
                out.push(("other", block));
            }
        }
    } else {
        out.push((place, content));
    }
}

fn pieces<'a>(agent: &str, record: &'a Value) -> Vec<(&'static str, &'a Value)> {
    let mut out = Vec::new();
    if agent == "codex" && record["type"] == "response_item" {
        let p = &record["payload"];
        match p["type"].as_str() {
            Some("message") => blocks(&p["content"], role(p["role"].as_str()), &mut out),
            Some("function_call" | "custom_tool_call") => {
                for field in ["arguments", "input"] {
                    if !p[field].is_null() {
                        out.push(("tool_input", &p[field]));
                    }
                }
            }
            Some("function_call_output" | "custom_tool_call_output") => {
                out.push(("tool_output", &p["output"]))
            }
            _ => out.push(("other", p)),
        }
    } else if record["message"].is_object() {
        let m = &record["message"];
        let place = if record["isMeta"] == true {
            "other"
        } else {
            role(m["role"].as_str())
        };
        blocks(&m["content"], place, &mut out);
    } else if agent == "codex" && record["type"] == "event_msg" {
        let p = &record["payload"];
        let place = match p["type"].as_str() {
            Some("user_message") => "person",
            Some("agent_message" | "agent_reasoning") => "agent",
            _ => "other",
        };
        out.push((place, p));
    } else {
        out.push(("other", record));
    }
    out.retain(|(_, value)| !value.is_null());
    if out.is_empty() {
        out.push(("other", record));
    }
    out
}
