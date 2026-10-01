//! Structural work evidence only. Never retain prompts, tool arguments or output.
use crate::model::{self, Entry, Session};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt::Write;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub source: String,
    pub line: usize,
    pub at: Option<String>,
    pub kind: String,
    pub call_id: Option<String>,
    pub edit: bool,
    pub failed: Option<bool>,
}

pub fn observe(v: &Value, line: usize, s: &mut Session) {
    let mut emit = |kind: &str, id: Option<&str>, edit: bool, failed: Option<bool>| {
        s.work.push(Event {
            source: s.source.clone(),
            line,
            at: v["timestamp"].as_str().map(str::to_owned),
            kind: kind.into(),
            call_id: id.map(str::to_owned),
            edit,
            failed,
        });
    };
    let p = &v["payload"];
    let m = &v["message"];
    if s.agent == "codex" {
        if v["type"] != "response_item" {
            return;
        }
        if p["type"] == "message" && p["role"] == "user" {
            emit("command", None, false, None);
        }
        if matches!(
            p["type"].as_str(),
            Some("function_call" | "custom_tool_call")
        ) {
            emit(
                "tool",
                p["call_id"].as_str(),
                is_edit(p["name"].as_str()),
                None,
            );
        }
        if matches!(
            p["type"].as_str(),
            Some("function_call_output" | "custom_tool_call_output")
        ) {
            let output = &p["output"];
            let parsed = output
                .as_str()
                .and_then(|s| serde_json::from_str::<Value>(s).ok());
            let data = parsed.as_ref().unwrap_or(output);
            let failed = data["exit_code"]
                .as_i64()
                .or_else(|| data["metadata"]["exit_code"].as_i64())
                .map(|n| n != 0)
                .or_else(|| data["isError"].as_bool())
                .or_else(|| data["is_error"].as_bool())
                .or_else(|| {
                    output.as_str().and_then(|s| {
                        s.lines().find_map(|l| {
                            l.strip_prefix("Process exited with code ")
                                .and_then(|n| n.trim().parse::<i64>().ok())
                                .map(|n| n != 0)
                        })
                    })
                });
            emit("result", p["call_id"].as_str(), false, failed);
        }
        return;
    }
    if m["role"] == "user" && v["isMeta"] != true {
        let content = &m["content"];
        if content.is_string()
            || content
                .as_array()
                .is_some_and(|a| a.iter().any(|c| c["type"] == "text"))
        {
            emit("command", None, false, None);
        }
    }
    if m["role"] == "toolResult" {
        emit(
            "result",
            m["toolCallId"].as_str(),
            false,
            m["isError"].as_bool(),
        );
    }
    if let Some(content) = m["content"].as_array() {
        for c in content {
            if m["role"] == "assistant"
                && matches!(c["type"].as_str(), Some("tool_use" | "toolCall"))
            {
                emit("tool", c["id"].as_str(), is_edit(c["name"].as_str()), None);
            } else if m["role"] == "user" && c["type"] == "tool_result" {
                // Claude omits is_error on successful results.
                emit(
                    "result",
                    c["tool_use_id"].as_str(),
                    false,
                    if c.get("is_error").is_none() {
                        Some(false)
                    } else {
                        c["is_error"].as_bool()
                    },
                );
            }
        }
    }
}

fn is_edit(name: Option<&str>) -> bool {
    matches!(
        name.and_then(|n| n.rsplit('.').next()),
        Some("apply_patch" | "Edit" | "Write" | "MultiEdit" | "edit" | "write")
    )
}

#[derive(Serialize)]
pub struct Window {
    pub source: String,
    pub line: usize,
    pub entries: Vec<Entry>,
    pub records: usize,
    pub elapsed_seconds: Option<i64>,
    pub tools: usize,
    pub failed_tools: usize,
    pub changes: Vec<Change>,
    pub waste_upper_bound_usd_micros: Option<u64>,
}
#[derive(Serialize)]
pub struct Change {
    pub call_line: usize,
    pub result_line: usize,
    pub entries: Vec<Entry>,
    pub records: usize,
}
#[derive(Serialize)]
pub struct Insights {
    pub commands: Vec<Window>,
    pub unattributed: Vec<Entry>,
    pub initial_request: Vec<Entry>,
    pub basis: &'static str,
}

pub fn summarize(s: &Session) -> Result<Insights> {
    // ponytail: scan selected-session records per command; index line ranges
    // if long sessions make interactive refresh measurably slow.
    let mut commands = Vec::new();
    let mut assigned = std::collections::BTreeSet::new();
    for (index, command) in s
        .work
        .iter()
        .enumerate()
        .filter(|(_, e)| e.kind == "command")
    {
        let end = s.work[index + 1..]
            .iter()
            .find(|e| e.kind == "command" && e.source == command.source)
            .map_or(usize::MAX, |e| e.line);
        let usage: Vec<_> = s
            .usage
            .iter()
            .enumerate()
            .filter(|(_, u)| u.source == command.source && u.line >= command.line && u.line < end)
            .collect();
        for (i, _) in &usage {
            assigned.insert(*i);
        }
        let events: Vec<_> = s
            .work
            .iter()
            .filter(|e| e.source == command.source && e.line >= command.line && e.line < end)
            .collect();
        let mut seen_calls = std::collections::BTreeSet::new();
        let tools: Vec<_> = events
            .iter()
            .copied()
            .filter(|e| e.kind == "tool")
            .filter(|e| e.call_id.as_ref().is_none_or(|id| seen_calls.insert(id)))
            .collect();
        let mut failures = 0;
        let mut changes = Vec::new();
        let mut previous_change = command.line;
        let mut seen_results = std::collections::BTreeSet::new();
        for result in events.iter().filter(|e| e.kind == "result") {
            let Some(call) = tools.iter().find(|c| {
                c.line <= result.line && c.call_id.is_some() && c.call_id == result.call_id
            }) else {
                continue;
            };
            if result.failed == Some(true) {
                if !seen_results.insert(&result.call_id) {
                    continue;
                }
                failures += 1;
            }
            if call.edit && result.failed == Some(false) {
                if !seen_results.insert(&result.call_id) {
                    continue;
                }
                let records: Vec<_> = usage
                    .iter()
                    .map(|(_, u)| *u)
                    .filter(|u| u.line >= previous_change && u.line <= result.line)
                    .collect();
                changes.push(Change {
                    call_line: call.line,
                    result_line: result.line,
                    entries: model::aggregate_usage(records.iter().copied())?,
                    records: records.len(),
                });
                previous_change = result.line + 1;
            }
        }
        let elapsed = command
            .at
            .as_deref()
            .and_then(parse_time)
            .zip(
                usage
                    .iter()
                    .filter_map(|(_, u)| u.at.as_deref().and_then(parse_time))
                    .max(),
            )
            .and_then(|(start, end)| (end >= start).then_some((end - start).num_seconds()));
        let waste = if failures > 0 && !usage.is_empty() {
            usage.iter().try_fold(0u64, |sum, (_, u)| {
                sum.checked_add(u.entry.estimated_usd_micros?)
            })
        } else {
            None
        };
        commands.push(Window {
            source: command.source.clone(),
            line: command.line,
            entries: model::aggregate_usage(usage.iter().map(|(_, u)| *u))?,
            records: usage.len(),
            elapsed_seconds: elapsed,
            tools: tools.len(),
            failed_tools: failures,
            changes,
            waste_upper_bound_usd_micros: waste,
        });
    }
    Ok(Insights {
        commands,
        unattributed: model::aggregate_usage(s.usage.iter().enumerate().filter(|(i, _)| !assigned.contains(i)).map(|(_, u)| u))?,
        initial_request: model::aggregate_usage(s.usage.first().into_iter())?,
        basis: "Commands are recorded user-message windows; injected context may also appear as user messages. Changes are successful named edit-tool results, not commits or verified surviving diffs. Change usage covers the window since the previous edit result (or command start); it is included in command totals, not additive or causal attribution. Opaque shell/exec edits and unrecognized results remain unmeasured. Elapsed time ends at the latest usage record, not command completion. Initial request includes context and generation; context-only cost is unknown. Waste: low-confidence range from zero to spending in a command containing a failed tool; failure can be useful work. No failure evidence means unknown waste, not zero. Conversation classification is not performed.",
    })
}
fn parse_time(s: &str) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    chrono::DateTime::parse_from_rfc3339(s).ok()
}

pub fn text(insights: &Insights) -> String {
    let mut out = String::new();
    let entries = |out: &mut String, values: &[Entry]| {
        if values.is_empty() {
            out.push_str("  usage unknown\n");
        }
        for e in values {
            let _ = writeln!(
                out,
                "  estimated={} · in={} out={} cache-read={} cache-write={} · {}/{}",
                crate::render::dollars(e.estimated_usd_micros),
                crate::render::number(e.input_tokens),
                crate::render::number(e.output_tokens),
                crate::render::number(e.cache_read_tokens),
                crate::render::number(e.cache_write_tokens),
                e.provider,
                e.model
            );
        }
    };
    out.push_str("Initial recorded request (included in totals; context-only cost unknown)\n");
    entries(&mut out, &insights.initial_request);
    for (i, c) in insights.commands.iter().enumerate() {
        let _ = writeln!(
            out,
            "Command {} · line {} · usage records={} tools={} recorded changes={} · observed elapsed {}s",
            i + 1,
            c.line,
            c.records,
            c.tools,
            c.changes.len(),
            c.elapsed_seconds
                .map_or("unknown".into(), |s| s.to_string())
        );
        entries(&mut out, &c.entries);
        for (j, change) in c.changes.iter().enumerate() {
            let _ = writeln!(
                out,
                "  Change {} · tool/result lines {}/{} · observed window, included in command",
                j + 1,
                change.call_line,
                change.result_line
            );
            entries(&mut out, &change.entries);
        }
        if c.failed_tools > 0 {
            let _ = writeln!(out, "  Estimated waste: $0 to {} (failed tools={}; low confidence, whole-command upper bound)", crate::render::dollars(c.waste_upper_bound_usd_micros), c.failed_tools);
        } else {
            out.push_str("  Estimated waste: unknown (no failed-tool evidence)\n");
        }
    }
    if insights.commands.is_empty() {
        out.push_str("Commands, changes and estimated waste: unknown (no command boundaries)\n");
    }
    if !insights.unattributed.is_empty() {
        out.push_str("Usage without a recorded command\n");
        entries(&mut out, &insights.unattributed);
    }
    out.push_str("Observed windows, not causal costs. Opaque edits remain unmeasured; failures may be useful work.\n");
    // All imported labels remain untrusted terminal content.
    out.chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .collect()
}
