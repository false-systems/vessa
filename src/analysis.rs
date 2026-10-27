use crate::model::Report;
use anyhow::{ensure, Context, Result};
use serde::Deserialize;

/// One dated rate card: a provider's per-token prices for one model over an
/// interval, as its source states them.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rate {
    pub provider: String,
    pub model: String,
    pub effective_from: String,
    pub effective_until: String,
    pub source: String,
    /// USD millionths per one million tokens, in non-overlapping categories.
    /// Null where the price is unknown or ambiguous: a record with tokens in
    /// that category stays unpriced. Each key must be given, so a category
    /// left out by mistake is refused, not taken for unknown.
    #[serde(deserialize_with = "Option::deserialize")]
    pub input: Option<u64>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub output: Option<u64>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub cache_read: Option<u64>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub cache_write: Option<u64>,
    /// The card prices only records whose prompt (input, cache-read and
    /// cache-write tokens together) is larger than this.
    #[serde(default)]
    pub prompt_over: Option<u64>,
    /// The card prices only records whose prompt is at most this.
    #[serde(default)]
    pub prompt_up_to: Option<u64>,
    /// What else its source says, or what the card leaves out: for the
    /// person who reads the card, not for Vessa.
    #[serde(default)]
    #[allow(dead_code)]
    pub notes: Option<String>,
}

const CATEGORIES: [&str; 4] = ["input", "output", "cache-read", "cache-write"];

impl Rate {
    fn prices(&self) -> [Option<u64>; 4] {
        [self.input, self.output, self.cache_read, self.cache_write]
    }
    fn covers(&self, prompt: Option<u64>) -> bool {
        let over = self
            .prompt_over
            .is_none_or(|n| prompt.is_some_and(|p| p > n));
        let up_to = self
            .prompt_up_to
            .is_none_or(|n| prompt.is_some_and(|p| p <= n));
        over && up_to
    }
    fn named(&self) -> String {
        format!(
            "rate card {}/{} ({} to {})",
            self.provider, self.model, self.effective_from, self.effective_until
        )
    }
}

/// What one card did with the records it could have priced.
#[derive(Default)]
struct Tally {
    /// Records of its provider and model, of them those in its interval,
    /// and of those the ones whose prompt it covers.
    model: usize,
    dated: usize,
    covered: usize,
    priced: usize,
    /// Covered records left unpriced: by a category whose price it leaves
    /// unknown, and for token counts the record leaves unknown.
    unknown_price: [usize; 4],
    unknown_counts: usize,
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
    let mut tally: Vec<Tally> = parsed.iter().map(|_| Tally::default()).collect();
    for session in &mut report.sessions {
        for usage in &mut session.usage {
            let at = usage
                .at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
            let e = &mut usage.entry;
            let tokens = [
                e.input_tokens,
                e.output_tokens,
                e.cache_read_tokens,
                e.cache_write_tokens,
            ];
            let prompt = [tokens[0], tokens[2], tokens[3]]
                .into_iter()
                .try_fold(0u64, |sum, n| sum.checked_add(n?));
            let mut dated = Vec::new();
            for (card, (r, start, end)) in parsed.iter().enumerate() {
                if r.provider != e.provider || r.model != e.model {
                    continue;
                }
                tally[card].model += 1;
                if at.is_some_and(|at| at >= *start && at < *end) {
                    tally[card].dated += 1;
                    dated.push(card);
                }
            }
            let matching: Vec<_> = dated
                .iter()
                .copied()
                .filter(|&card| parsed[card].0.covers(prompt))
                .collect();
            ensure!(
                matching.len() <= 1,
                "overlapping rate cards for {}/{}",
                e.provider,
                e.model
            );
            let Some(&card) = matching.first() else {
                if let (false, Some(prompt)) = (dated.is_empty(), prompt) {
                    // Cards for its date, but none for a prompt of its size.
                    e.estimate_basis = Some(format!(
                        "unpriced: no {}/{} rate card for its date covers a prompt of {prompt} tokens",
                        e.provider, e.model
                    ));
                }
                continue;
            };
            let (r, _, _) = &parsed[card];
            let t = &mut tally[card];
            t.covered += 1;
            // Unknown token categories prevent repricing.
            if tokens.iter().any(Option::is_none) {
                t.unknown_counts += 1;
                continue;
            }
            let mut total = 0u128;
            // Categories with tokens and no known price, with their counts.
            let mut unknown: Vec<(u64, &str)> = Vec::new();
            for (i, (count, price)) in tokens.into_iter().zip(r.prices()).enumerate() {
                let count = count.unwrap();
                match price {
                    // No tokens: what they would cost does not matter.
                    _ if count == 0 => {}
                    Some(price) => {
                        total = total
                            .checked_add(u128::from(count) * u128::from(price))
                            .context("cost overflow")?;
                    }
                    None => {
                        t.unknown_price[i] += 1;
                        unknown.push((count, CATEGORIES[i]));
                    }
                }
            }
            if !unknown.is_empty() {
                let join = |part: &dyn Fn(&(u64, &str)) -> String| {
                    unknown.iter().map(part).collect::<Vec<_>>().join(" and ")
                };
                e.estimate_basis = Some(format!(
                    "unpriced: {} leaves the {} price unknown, and this record has {} tokens",
                    r.named(),
                    join(&|(_, category)| category.to_string()),
                    join(&|(n, category)| format!("{n} {category}")),
                ));
                continue;
            }
            let micros = u64::try_from(
                total.checked_add(500_000).context("rounding overflow")? / 1_000_000,
            )?;
            e.estimated_usd_micros = Some(micros);
            t.priced += 1;
            e.estimate_basis = Some(format!(
                "API-equivalent estimate; {}; {} to {}{}",
                r.source,
                r.effective_from,
                r.effective_until,
                match (r.prompt_over, r.prompt_up_to) {
                    (None, None) => String::new(),
                    (Some(over), None) => format!("; prompts over {over} tokens"),
                    (None, Some(up_to)) => format!("; prompts up to {up_to} tokens"),
                    (Some(over), Some(up_to)) => {
                        format!("; prompts over {over} and up to {up_to} tokens")
                    }
                }
            ));
        }
    }
    // A card that matches nothing is almost always a provider, model or date
    // mismatch, and one that leaves records it covers unpriced has a price
    // it does not know. Say so, rather than leaving estimates silently
    // unknown; these notices concern pricing, not the usage evidence.
    for ((r, _, _), t) in parsed.iter().zip(tally) {
        // A card whose provider, model and dates match records, none of a
        // prompt size it covers (a long-context card where no prompt was
        // long), matches as it should and priced what there was: nothing.
        if t.covered == 0 && t.dated == 0 {
            let why = if t.model == 0 {
                format!("no selected record is {}/{}", r.provider, r.model)
            } else {
                format!(
                    "no selected {}/{} record ({}) is dated in its interval",
                    r.provider,
                    r.model,
                    crate::render::many(t.model, "record")
                )
            };
            report.warnings.push(format!(
                "{} priced no usage records: {why}; compare its provider, model and dates with `vessa usage`",
                r.named()
            ));
        } else if t.priced < t.covered {
            let mut why: Vec<String> = t
                .unknown_price
                .iter()
                .zip(CATEGORIES)
                .filter(|(n, _)| **n > 0)
                .map(|(n, category)| {
                    format!(
                        "{n} {} {category} tokens, whose price it leaves unknown",
                        if *n == 1 { "holds" } else { "hold" }
                    )
                })
                .collect();
            if t.unknown_counts > 0 {
                why.push(format!(
                    "{} {} unknown token counts",
                    t.unknown_counts,
                    if t.unknown_counts == 1 { "has" } else { "have" }
                ));
            }
            report.warnings.push(format!(
                "{} priced {} of the {} usage records it covers; of the others, {}",
                r.named(),
                t.priced,
                t.covered,
                why.join("; ")
            ));
        }
    }
    Ok(())
}
