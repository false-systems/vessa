use crate::model::{self, Entry, Report, Usage};
use anyhow::Result;
use chrono::{DateTime, Datelike, Utc};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum Period {
    Day,
    Week,
    Month,
    Year,
}
#[derive(Serialize)]
pub struct Bucket {
    pub period: String,
    pub agent: String,
    /// Top-level sessions with usage in the period. A subagent's usage
    /// (Claude or Codex) counts its session, not a session of its own.
    pub sessions: usize,
    /// Subagents with usage in the period, counted apart.
    pub subagents: usize,
    pub entries: Vec<Entry>,
    pub records: usize,
}
#[derive(Default)]
struct Group<'a> {
    sessions: BTreeSet<&'a str>,
    subagents: BTreeSet<&'a str>,
    usage: Vec<&'a Usage>,
}
pub fn history(report: &Report, period: Period) -> Result<Vec<Bucket>> {
    let mut groups: BTreeMap<(String, String), Group<'_>> = BTreeMap::new();
    for s in &report.sessions {
        for u in &s.usage {
            let key = match u
                .at
                .as_deref()
                .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
            {
                None => "undated".into(),
                Some(t) => {
                    let t = t.with_timezone(&Utc);
                    match period {
                        Period::Day => t.format("%Y-%m-%d").to_string(),
                        Period::Week => {
                            format!("{}-W{:02}", t.iso_week().year(), t.iso_week().week())
                        }
                        Period::Month => t.format("%Y-%m").to_string(),
                        Period::Year => t.format("%Y").to_string(),
                    }
                }
            };
            let group = groups.entry((key, s.agent.clone())).or_default();
            let parent = s.subagent_of();
            group.sessions.insert(parent.unwrap_or(&s.id));
            if parent.is_some() {
                group.subagents.insert(&s.id);
            }
            group.usage.push(u);
        }
    }
    groups
        .into_iter()
        .map(|((period, agent), group)| {
            let usage = group.usage;
            Ok(Bucket {
                records: usage.len(),
                period,
                agent,
                sessions: group.sessions.len(),
                subagents: group.subagents.len(),
                entries: model::aggregate_usage(usage.iter().copied())?,
            })
        })
        .collect()
}

/// Claude supplies this session's identity and token counters directly.
///
/// A status line that fails goes blank, so this always renders: a field it
/// cannot read (absent, of another type, a negative count) is
/// `unknown`, and the others still show. Input that is not JSON at all is
/// given here as null, and every field is unknown.
pub fn claude_statusline(input: &serde_json::Value) -> String {
    // Source strings are untrusted terminal content; never emit control codes.
    // They are scrubbed of secrets before they are cut, so a cut leaves no
    // piece of one.
    let clean = |s: &str| {
        crate::scrub::Redactions::default()
            .text(s)
            .chars()
            .filter(|c| crate::render::shown(*c))
            .take(100)
            .collect::<String>()
    };
    let id = input["session_id"]
        .as_str()
        .map_or_else(|| "unknown session".into(), clean);
    let model = input["model"]["display_name"]
        .as_str()
        .or_else(|| input["model"]["id"].as_str())
        .map_or_else(|| "unknown model".into(), clean);
    let usage = &input["context_window"]["current_usage"];
    let tokens = |key: &str| {
        usage[key]
            .as_u64()
            .map_or_else(|| "unknown".into(), |n| n.to_string())
    };
    format!(
        "{model} · {id}\nLast API call: input {} · output {} · cache read {} · cache write {}",
        tokens("input_tokens"),
        tokens("output_tokens"),
        tokens("cache_read_input_tokens"),
        tokens("cache_creation_input_tokens"),
    )
}
