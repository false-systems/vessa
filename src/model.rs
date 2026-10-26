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
    /// How a Claude session was started, as its transcript says: `cli` for
    /// an interactive session, `sdk-cli` for `claude -p`. Null when unknown.
    #[serde(default)]
    pub entrypoint: Option<String>,
    /// Which Codex front end started a session (`session_meta.originator`):
    /// `codex-tui`, or `codex_exec` for `codex exec`. Null when unknown.
    #[serde(default)]
    pub originator: Option<String>,
    /// Subagents a Claude transcript says it launched, by native agent id,
    /// with the line that says so. Checked against the transcripts read.
    #[serde(skip)]
    pub launched: Vec<(String, usize)>,
}

impl Session {
    /// The session a Claude subagent belongs to. Claude Code writes each
    /// subagent to its own transcript under the parent's session id; Vessa
    /// names it `parent/agentId`.
    pub fn subagent_of(&self) -> Option<&str> {
        let parent = self.parent.as_deref()?;
        (self.agent == "claude" && self.id.strip_prefix(parent)?.starts_with('/')).then_some(parent)
    }
}

/// One session with the subagent sessions read with it. A subagent whose
/// session was not read stands alone.
pub struct Family<'a> {
    pub main: &'a Session,
    pub subagents: Vec<&'a Session>,
}

impl Family<'_> {
    pub fn usage(&self) -> impl Iterator<Item = &Usage> {
        std::iter::once(self.main)
            .chain(self.subagents.iter().copied())
            .flat_map(|s| &s.usage)
    }
    /// Deduplicated totals of the session and its subagents.
    pub fn totals(&self) -> Result<Vec<Entry>> {
        aggregate_usage(self.usage())
    }
    /// The session's limitations, then each subagent's, named.
    pub fn limitations(&self) -> Vec<String> {
        let mut all = self.main.limitations.clone();
        for s in &self.subagents {
            all.extend(
                s.limitations
                    .iter()
                    .map(|l| format!("subagent {}: {l}", s.id)),
            );
        }
        all
    }
}

/// Groups sessions with their subagents, in the order the sessions come.
pub fn families(sessions: &[Session]) -> Vec<Family<'_>> {
    let parents: std::collections::HashSet<_> = sessions
        .iter()
        .filter(|s| s.subagent_of().is_none())
        .map(|s| (s.agent.as_str(), s.id.as_str()))
        .collect();
    let mut families: Vec<Family> = sessions
        .iter()
        .filter(|s| {
            s.subagent_of()
                .is_none_or(|p| !parents.contains(&(s.agent.as_str(), p)))
        })
        .map(|main| Family {
            main,
            subagents: Vec::new(),
        })
        .collect();
    let index: BTreeMap<_, _> = families
        .iter()
        .enumerate()
        .map(|(i, f)| ((f.main.agent.as_str(), f.main.id.as_str()), i))
        .collect();
    for s in sessions {
        if let Some(&i) = s
            .subagent_of()
            .and_then(|p| index.get(&(s.agent.as_str(), p)))
        {
            families[i].subagents.push(s);
        }
    }
    families
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

/// The receipt for one session, its subagents included. `agent-usage.v1`
/// has no field for them, and ohjaa refuses one it does not know, so the
/// receipt says it in `limitations`.
pub fn receipt(family: &Family) -> Result<serde_json::Value> {
    let session = family.main;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis();
    let mut limitations = vec![
        "Local transcript snapshot; billing and unlogged requests are not verified".to_owned(),
    ];
    if !family.subagents.is_empty() {
        let n = family.subagents.len();
        limitations.push(format!(
            "Entries include {n} Claude {} of this session, each read from its own transcript: {}",
            if n == 1 { "subagent" } else { "subagents" },
            family
                .subagents
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    limitations.extend(family.limitations());
    Ok(serde_json::json!({
        "schema": "agent-usage.v1", "agent": session.agent, "session_id": session.id,
        "source": session.source, "observed_at_ms": now, "complete": false,
        "limitations": limitations,
        "entries": family.totals()?
    }))
}
