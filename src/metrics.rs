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
    pub sessions: usize,
    pub entries: Vec<Entry>,
    pub known_estimated_usd_micros: u64,
    pub unpriced_records: usize,
}
#[derive(Default)]
struct Group<'a> {
    sessions: BTreeSet<&'a str>,
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
            group.sessions.insert(&s.id);
            group.usage.push(u);
        }
    }
    groups
        .into_iter()
        .map(|((period, agent), Group { sessions, usage })| {
            let known = usage.iter().try_fold(0u64, |total, u| {
                total
                    .checked_add(u.entry.estimated_usd_micros.unwrap_or(0))
                    .context("estimate overflow")
            })?;
            Ok(Bucket {
                period,
                agent,
                sessions: sessions.len(),
                entries: model::aggregate_usage(usage.iter().copied())?,
                known_estimated_usd_micros: known,
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
pub fn claude_statusline(input: &serde_json::Value) -> Result<String> {
    let id = input["session_id"]
        .as_str()
        .context("statusline input missing session_id")?;
    let model = input["model"]["display_name"]
        .as_str()
        .or_else(|| input["model"]["id"].as_str())
        .unwrap_or("unknown model");
    let estimate = match input["cost"]["total_cost_usd"].as_f64() {
        Some(cost) => {
            anyhow::ensure!(cost.is_finite() && cost >= 0.0, "invalid session cost");
            format!("${cost:.2}")
        }
        None => "unknown".into(),
    };
    // Source strings are untrusted terminal content; never emit control codes.
    let clean = |s: &str| {
        s.chars()
            .filter(|c| !c.is_control())
            .take(100)
            .collect::<String>()
    };
    let usage = &input["context_window"]["current_usage"];
    let tokens = |key: &str| -> Result<String> {
        let value = &usage[key];
        if value.is_null() {
            return Ok("unknown".into());
        }
        Ok(value
            .as_u64()
            .context("invalid status-line token count")?
            .to_string())
    };
    Ok(format!(
        "session estimate {estimate} · billed unknown · {} · {}\nLast API call: input {} · output {} · cache read {} · cache write {}",
        clean(model),
        clean(id),
        tokens("input_tokens")?,
        tokens("output_tokens")?,
        tokens("cache_read_input_tokens")?,
        tokens("cache_creation_input_tokens")?,
    ))
}
