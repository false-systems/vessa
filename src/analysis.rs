use crate::model::{Report, Session};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Serialize)]
pub struct Finding {
    pub kind: String,
    pub method: String,
    pub evidence: Vec<Evidence>,
}
#[derive(Debug, Serialize)]
pub struct Evidence {
    pub agent: String,
    pub session: String,
    pub message: String,
    pub source: String,
    pub line: usize,
}

pub fn patterns(sessions: &[Session]) -> Vec<Finding> {
    let mut found = Vec::new();
    let mut repeats: BTreeMap<String, Vec<Evidence>> = BTreeMap::new();
    for session in sessions {
        // Subagent instructions are not the person's chat.
        if session.parent.is_some() {
            continue;
        }
        for message in session.messages.iter().filter(|m| m.role == "user") {
            let normalized = message
                .text
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            let evidence = || Evidence {
                agent: session.agent.clone(),
                session: session.id.clone(),
                message: message.id.clone(),
                source: message.source.clone(),
                line: message.line,
            };
            // ponytail: bounded English phrase rules; use labelled examples to
            // evaluate a semantic classifier before broadening these labels.
            if normalized.len() <= 2000
                && [
                    "i already told you",
                    "i told you",
                    "that's not what",
                    "that is not what",
                    "you forgot",
                    "stop asking",
                    "do not use",
                    "don't use",
                ]
                .iter()
                .any(|p| normalized.contains(p))
            {
                found.push(Finding {
                    kind: "possible correction".into(),
                    method: "phrase-rules.v1; review context, not a proven failure".into(),
                    evidence: vec![evidence()],
                });
            }
            if (20..=2000).contains(&normalized.len()) {
                repeats.entry(normalized).or_default().push(evidence());
            }
        }
    }
    for evidence in repeats.into_values().filter(|v| v.len() > 1) {
        found.push(Finding {
            kind: "repeated user text".into(),
            method: "normalized-text.v1; repetition does not establish wasted work".into(),
            evidence,
        });
    }
    found
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rate {
    pub provider: String,
    pub model: String,
    pub effective_from: String,
    pub effective_until: String,
    pub source: String,
    /// USD millionths per one million tokens, in non-overlapping categories.
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

pub fn apply_rates(report: &mut Report, rates: &[Rate]) -> Result<()> {
    let parsed = rates
        .iter()
        .map(|r| {
            let start = chrono::DateTime::parse_from_rfc3339(&r.effective_from)?;
            let end = chrono::DateTime::parse_from_rfc3339(&r.effective_until)?;
            ensure!(
                start < end && !r.source.trim().is_empty(),
                "rate needs an ordered validity interval and source"
            );
            Ok((r, start, end))
        })
        .collect::<Result<Vec<_>>>()?;
    for session in &mut report.sessions {
        for usage in &mut session.usage {
            let Some(at) = usage
                .at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            else {
                continue;
            };
            let matching: Vec<_> = parsed
                .iter()
                .filter(|(r, start, end)| {
                    r.provider == usage.entry.provider
                        && r.model == usage.entry.model
                        && at >= *start
                        && at < *end
                })
                .collect();
            ensure!(
                matching.len() <= 1,
                "overlapping rate cards for {}/{}",
                usage.entry.provider,
                usage.entry.model
            );
            let Some((r, _, _)) = matching.first() else {
                continue;
            };
            let e = &mut usage.entry;
            let tokens = [
                e.input_tokens,
                e.output_tokens,
                e.cache_read_tokens,
                e.cache_write_tokens,
            ];
            if tokens.iter().any(Option::is_none) {
                continue;
            }
            let mut total = 0u128;
            for (count, rate) in
                tokens
                    .into_iter()
                    .zip([r.input, r.output, r.cache_read, r.cache_write])
            {
                total = total
                    .checked_add(u128::from(count.unwrap()) * u128::from(rate))
                    .context("cost overflow")?;
            }
            let micros = u64::try_from(
                total.checked_add(500_000).context("rounding overflow")? / 1_000_000,
            )?;
            e.estimated_usd_micros = Some(micros);
            e.estimate_basis = Some(format!(
                "API-equivalent estimate; {}; {} to {}",
                r.source, r.effective_from, r.effective_until
            ));
        }
    }
    Ok(())
}
