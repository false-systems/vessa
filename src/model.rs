use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub provider: String,
    pub model: String,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub estimated_usd_micros: Option<u64>,
    pub estimate_basis: Option<String>,
    pub credits: Option<Amount>,
    pub billed: Option<Amount>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Amount {
    pub unit: String,
    pub micros: u64,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub id: String,
    pub at: Option<String>,
    pub source: String,
    pub line: usize,
    pub entry: Entry,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub agent: String,
    pub id: String,
    pub parent: Option<String>,
    pub started_at: Option<String>,
    pub cwd: String,
    pub source: String,
    pub usage: Vec<Usage>,
    pub limitations: Vec<String>,
    pub ohjaa: Vec<serde_json::Value>,
    #[serde(default)]
    pub work: Vec<crate::work::Event>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Report {
    pub schema: String,
    pub sessions: Vec<Session>,
    pub warnings: Vec<String>,
}

pub fn aggregate(session: &Session) -> Result<Vec<Entry>> {
    aggregate_usage(session.usage.iter())
}

pub fn aggregate_usage<'a>(usage: impl Iterator<Item = &'a Usage>) -> Result<Vec<Entry>> {
    let mut groups: BTreeMap<(String, String), Entry> = BTreeMap::new();
    for usage in usage {
        let entry = &usage.entry;
        let key = (entry.provider.clone(), entry.model.clone());
        if let Some(total) = groups.get_mut(&key) {
            total.input_tokens = add(total.input_tokens, entry.input_tokens)?;
            total.output_tokens = add(total.output_tokens, entry.output_tokens)?;
            total.cache_read_tokens = add(total.cache_read_tokens, entry.cache_read_tokens)?;
            total.cache_write_tokens = add(total.cache_write_tokens, entry.cache_write_tokens)?;
            total.estimated_usd_micros =
                add(total.estimated_usd_micros, entry.estimated_usd_micros)?;
            if total.estimate_basis != entry.estimate_basis {
                total.estimate_basis = Some("mixed; see individual usage records".into());
            }
            total.credits = add_amount(&total.credits, &entry.credits)?;
            total.billed = add_amount(&total.billed, &entry.billed)?;
        } else {
            groups.insert(key, entry.clone());
        }
    }
    Ok(groups.into_values().collect())
}

fn add(a: Option<u64>, b: Option<u64>) -> Result<Option<u64>> {
    match (a, b) {
        (Some(a), Some(b)) => Ok(Some(
            a.checked_add(b)
                .ok_or_else(|| anyhow::anyhow!("usage overflow"))?,
        )),
        _ => Ok(None),
    }
}

fn add_amount(a: &Option<Amount>, b: &Option<Amount>) -> Result<Option<Amount>> {
    match (a, b) {
        (Some(a), Some(b)) => {
            ensure!(
                a.unit == b.unit,
                "cannot sum different billing/credit units"
            );
            Ok(Some(Amount {
                unit: a.unit.clone(),
                micros: add(Some(a.micros), Some(b.micros))?.unwrap(),
                source: if a.source == b.source {
                    a.source.clone()
                } else {
                    "multiple source records".into()
                },
            }))
        }
        _ => Ok(None),
    }
}

pub fn receipt(session: &Session) -> Result<serde_json::Value> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis();
    Ok(serde_json::json!({
        "schema": "agent-usage.v1", "agent": session.agent, "session_id": session.id,
        "source": session.source, "observed_at_ms": now, "complete": false,
        "limitations": std::iter::once("Local transcript snapshot; billing and unlogged requests are not verified".to_owned()).chain(session.limitations.clone()).collect::<Vec<_>>(),
        "entries": aggregate(session)?
    }))
}
