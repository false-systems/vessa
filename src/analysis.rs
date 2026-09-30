use crate::model::Report;
use anyhow::{ensure, Context, Result};
use serde::Deserialize;

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
