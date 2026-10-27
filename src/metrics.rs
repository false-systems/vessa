use crate::model::{self, Entry, Report, Usage};
use anyhow::{Context, Result};
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
    /// The sum of the priced records' estimates: 0 when none is priced, so
    /// read it with `priced_records`.
    pub known_estimated_usd_micros: u64,
    pub priced_records: usize,
    pub unpriced_records: usize,
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
            let known = usage.iter().try_fold(0u64, |total, u| {
                total
                    .checked_add(u.entry.estimated_usd_micros.unwrap_or(0))
                    .context("estimate overflow")
            })?;
            Ok(Bucket {
                period,
                agent,
                sessions: group.sessions.len(),
                subagents: group.subagents.len(),
                entries: model::aggregate_usage(usage.iter().copied())?,
                known_estimated_usd_micros: known,
                priced_records: usage
                    .iter()
                    .filter(|u| u.entry.estimated_usd_micros.is_some())
                    .count(),
                unpriced_records: usage
                    .iter()
                    .filter(|u| u.entry.estimated_usd_micros.is_none())
                    .count(),
            })
        })
        .collect()
}

/// Claude supplies this session's identity and estimate directly; no guessing
/// from recent files, and no token/rate conversion into subscription charges.
///
/// A status line that fails goes blank, so this always renders: a field it
/// cannot read (absent, of another type, a negative cost or count) is
/// `unknown`, and the others still show. Input that is not JSON at all is
/// given here as null, and every field is unknown.
pub fn claude_statusline(input: &serde_json::Value) -> String {
    // Source strings are untrusted terminal content; never emit control codes.
    let clean = |s: &str| {
        s.chars()
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
    // An amount Vessa could not hold in USD millionths, as it holds every
    // other estimate, is no session's cost. `abs` makes -0.0 zero, the only
    // negative number the filter lets through.
    let estimate = input["cost"]["total_cost_usd"]
        .as_f64()
        .filter(|cost| cost.is_finite() && *cost >= 0.0 && cost * 1_000_000.0 < u64::MAX as f64)
        .map_or_else(|| "unknown".into(), |cost| format!("${:.2}", cost.abs()));
    let usage = &input["context_window"]["current_usage"];
    let tokens = |key: &str| {
        usage[key]
            .as_u64()
            .map_or_else(|| "unknown".into(), |n| n.to_string())
    };
    format!(
        "session estimate {estimate} · billed unknown · {model} · {id}\nLast API call: input {} · output {} · cache read {} · cache write {}",
        tokens("input_tokens"),
        tokens("output_tokens"),
        tokens("cache_read_input_tokens"),
        tokens("cache_creation_input_tokens"),
    )
}
