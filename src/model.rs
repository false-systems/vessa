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
    /// Requests this transcript holds that an earlier session counted.
    #[serde(skip)]
    pub counted_elsewhere: usize,
    /// Claude Code's own `<synthetic>` messages, by id: written with zero
    /// usage for an API error or a refusal, they are no model request.
    /// Known records with no usage, so neither usage nor a conflict; the
    /// usage export counts them as `synthetic_records`.
    #[serde(skip)]
    pub synthetic: std::collections::BTreeSet<String>,
    /// The session a Codex subagent counts to: the root its `session_meta`
    /// names. None for every other session.
    #[serde(skip)]
    pub root: Option<String>,
    /// Another agent started it: a Claude sidechain, or a Codex session
    /// whose `source` names a subagent. Its user turns are that agent's
    /// prompts, not a person's.
    #[serde(skip)]
    pub subagent: bool,
    /// A Claude transcript some of whose records say they are a sidechain
    /// and some do not: read as a subagent's, though it may be its
    /// session's own.
    #[serde(skip)]
    pub partly_sidechain: bool,
}

/// What a session's transcript says about itself, without its usage: one
/// entry of `vessa-session-headers.v1`. Each field is the session's field of
/// the same name in `vessa-usage.v1`.
#[derive(Debug, Serialize)]
pub struct Header<'a> {
    pub agent: &'a str,
    pub id: &'a str,
    pub parent: Option<&'a str>,
    pub started_at: Option<&'a str>,
    pub cwd: &'a str,
    pub source: &'a str,
    pub entrypoint: Option<&'a str>,
    pub originator: Option<&'a str>,
}

impl Session {
    pub fn header(&self) -> Header<'_> {
        Header {
            agent: &self.agent,
            id: &self.id,
            parent: self.parent.as_deref(),
            started_at: self.started_at.as_deref(),
            cwd: &self.cwd,
            source: &self.source,
            entrypoint: self.entrypoint.as_deref(),
            originator: self.originator.as_deref(),
        }
    }

    /// The session a subagent belongs to. Claude Code writes each subagent
    /// to its own transcript under the parent's session id; Vessa names it
    /// `parent/agentId`. A Codex subagent, or another thread a session
    /// spawned, keeps its own thread id and names its root session.
    pub fn subagent_of(&self) -> Option<&str> {
        if self.agent == "codex" {
            return self.root.as_deref();
        }
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

/// How many sessions these transcripts are, and how many of them are
/// subagents, counted with their session as `stats` counts them: a subagent
/// read without its session's transcript still counts that session.
pub fn count(sessions: &[Session]) -> (usize, usize) {
    let roots: std::collections::BTreeSet<_> = sessions
        .iter()
        .map(|s| (s.agent.as_str(), s.subagent_of().unwrap_or(&s.id)))
        .collect();
    let subagents = sessions
        .iter()
        .filter(|s| s.subagent_of().is_some())
        .count();
    (roots.len(), subagents)
}

/// "N sessions", with " · M subagents" when there are any.
pub fn counted(sessions: &[Session]) -> String {
    use crate::render::many;
    match count(sessions) {
        (n, 0) => many(n, "session"),
        (n, m) => format!("{} · {}", many(n, "session"), many(m, "subagent")),
    }
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

/// How many subagents a receipt names. A session can have dozens, and their
/// ids would make one limitation thousands of characters long.
const NAMED_SUBAGENTS: usize = 10;

/// The receipt for one session, its subagents included. `agent-usage.v1`
/// has no field for them, and ohjaa refuses one it does not know, so the
/// receipt says it in `limitations`: how many, and the first ten by id.
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
        // Sessions come in id order, so a session's subagents do too.
        let ids = |count: usize| {
            family
                .subagents
                .iter()
                .take(count)
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let named = if n <= NAMED_SUBAGENTS {
            format!(": {}", ids(n))
        } else {
            format!(
                "; the first {NAMED_SUBAGENTS} in id order: {}; {} more are not named here, and `vessa status --session {}` lists every one",
                ids(NAMED_SUBAGENTS),
                n - NAMED_SUBAGENTS,
                session.id
            )
        };
        limitations.push(format!(
            "Entries include {n} {} {} of this session, each read from its own transcript{named}",
            if session.agent == "codex" {
                "Codex"
            } else {
                "Claude"
            },
            if n == 1 { "subagent" } else { "subagents" },
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

/// One message a person typed into an agent session: a record of
/// `vessa-messages.v1`.
#[derive(Debug, Serialize)]
pub struct Message {
    pub agent: String,
    /// The native session id; a Claude subagent's is `<session>/<agentId>`.
    pub session: String,
    /// The session it descends from: a Claude subagent's session, a Codex
    /// fork's `forked_from_id`, a pi `parentSession`. Null for none.
    pub parent: Option<String>,
    /// What the words are: `message`, typed into the conversation; `queued`,
    /// typed while the agent worked and handed to it mid-turn; `answer`, a
    /// person's own answer to the agent's question tool; `rejection`, the
    /// reason given for rejecting a tool call; `task`, the task typed into a
    /// launch prompt.
    pub kind: &'static str,
    /// The native id of the record it was read from, where the agent writes
    /// one (Claude `uuid`, pi `id`, Codex `payload.id` in newer versions).
    pub id: Option<String>,
    pub at: Option<String>,
    pub cwd: String,
    pub text: String,
    /// The question an `answer` answers.
    pub question: Option<String>,
    /// The agent turn this message follows in its transcript, bounded.
    pub reply: Option<Reply>,
    pub source: String,
    pub line: usize,
}

/// The agent turn a message follows: its last characters only.
#[derive(Debug, Clone, Serialize)]
pub struct Reply {
    pub text: String,
    /// The reply was longer than `text`.
    pub truncated: bool,
    /// The turn failed (a Claude API error, a Codex error event, a pi error
    /// stop): the message answers a failure, not a reply.
    pub failed: bool,
    pub line: usize,
}

/// A session whose transcript `vessa messages` read.
#[derive(Debug, Serialize)]
pub struct Conversation {
    pub agent: String,
    pub id: String,
    pub parent: Option<String>,
    /// A Claude subagent, read only for what a person sent it.
    pub subagent: bool,
    pub started_at: Option<String>,
    pub cwd: String,
    pub source: String,
    /// Every prompt in it came from a program (`claude -p`, an Agent SDK,
    /// `codex exec`), so no person is in it.
    pub headless: bool,
    /// Its messages listed.
    pub messages: usize,
}

/// What `vessa messages` read, and why each user turn it did not list is
/// not a person's message.
#[derive(Debug, Default, Serialize)]
pub struct Coverage {
    /// Transcripts read for messages, Claude subagents' among them.
    pub transcripts: usize,
    /// Codex subagent transcripts, not read: their user turns are the parent
    /// agent's prompts.
    pub subagent_transcripts: usize,
    /// Transcripts last modified before `since`, not read.
    pub unmodified_transcripts: usize,
    /// User turns read: `human`, `excluded`, `earlier` and `undated` sum to it.
    pub user_turns: usize,
    /// Messages listed.
    pub human: usize,
    /// User turns that are no person's message, by rule.
    pub excluded: BTreeMap<&'static str, usize>,
    pub since: Option<String>,
    /// Messages from `since` or before.
    pub earlier: usize,
    /// Messages with no time, which `since` cannot place.
    pub undated: usize,
    /// Sources Vessa knows of and does not read.
    pub gaps: Vec<String>,
    /// What a transcript leaves unknown, by session.
    pub limitations: Vec<String>,
}
