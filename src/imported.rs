//! `vessa import`: usage receipts from elsewhere, held in vessa's own
//! state and counted with what the transcripts here say.
//!
//! A receipt is an `agent-usage.v1` document exactly as `vessa receipt`
//! writes it. The import is strict, as an import of an assertion must be:
//! only `agent-usage.v1`, only the fields that schema has, and no field of
//! money — prices, credits, billing are out for good, so a receipt that
//! carries one is refused whole, never stripped and counted. An import
//! never changes the file it reads; the receipt is kept in vessa's own
//! state, one file per session
//! (`$XDG_STATE_HOME/vessa/imported/<agent>-<session>.json`), so a session
//! is never counted twice however often its receipt is imported: the same
//! bytes again are said to be kept already, different bytes replace what
//! was there, and are said to. Remove one by deleting its file.
//!
//! What is kept is counted by `vessa usage` and `vessa stats`, alongside
//! the native sessions: each entry becomes a usage record of the receipt's
//! session, placed at the receipt's observation time (and `undated` when
//! the receipt names none). A receipt whose session is also read from its
//! transcript here is skipped by those commands and said so, so nothing is
//! counted twice.
//!
//! `vessa export` is not a command: `vessa receipt` writes the very
//! document this command reads, for one session, selected with `--session`.

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{bail, ensure, Context, Result};

use crate::model;

const RECEIPT: &str = "agent-usage.v1";
/// Every field a receipt may carry; an import refuses a receipt that names
/// anything else, rather than guessing at what it does not know.
const RECEIPT_FIELDS: [&str; 8] = [
    "schema",
    "complete",
    "agent",
    "session_id",
    "source",
    "observed_at_ms",
    "limitations",
    "entries",
];
/// Every field an entry may carry.
const ENTRY_FIELDS: [&str; 6] = [
    "provider",
    "model",
    "input_tokens",
    "output_tokens",
    "cache_read_tokens",
    "cache_write_tokens",
];

/// `vessa import --file …`: keep each receipt, said as kept or refused.
/// Exit codes: 0 every file kept (newly or already), 2 a file refused; a
/// refused file keeps nothing.
pub fn store(files: &[PathBuf]) -> Result<i32> {
    ensure!(
        !files.is_empty(),
        "import needs at least one --file, an agent-usage.v1 receipt as `vessa receipt` writes it"
    );
    let dir = state_dir().context("neither XDG_STATE_HOME nor HOME names a state directory")?;
    std::fs::create_dir_all(dir.join("imported"))
        .with_context(|| format!("{} cannot be made", dir.join("imported").display()))?;
    let mut code = 0;
    for file in files {
        match keep(file, &dir) {
            Ok(Kept::Newly {
                name,
                agent,
                id,
                entries,
            }) => println!(
                "vessa: kept {name}: {agent} session {}, {}",
                id,
                crate::render::many(entries, "entry")
            ),
            Ok(Kept::Already { name }) => println!("vessa: kept {name}: already imported"),
            Ok(Kept::Replaced { name }) => {
                println!("vessa: kept {name}: replaced an earlier receipt of that session")
            }
            Err(error) => {
                eprintln!("vessa import: {}: {error:#}", file.display());
                code = 2;
            }
        }
    }
    Ok(code)
}

/// What keeping one file came to.
enum Kept {
    Newly {
        name: String,
        agent: String,
        id: String,
        entries: usize,
    },
    Already {
        name: String,
    },
    Replaced {
        name: String,
    },
}

/// Read, check and keep one receipt file, whole or not at all.
fn keep(file: &std::path::Path, dir: &std::path::Path) -> Result<Kept> {
    let bytes =
        std::fs::read(file).with_context(|| format!("cannot read it: {}", file.display()))?;
    let receipt: serde_json::Value =
        serde_json::from_slice(&bytes).context("it is not valid JSON")?;
    check(&receipt)?;
    let agent = receipt["agent"].as_str().unwrap_or("?");
    let id = receipt["session_id"].as_str().unwrap_or("?");
    let name = format!("{}.json", file_name(agent, id));
    let kept = dir.join("imported").join(&name);
    if std::fs::read(&kept).is_ok_and(|there| there == bytes) {
        return Ok(Kept::Already { name });
    }
    let replaced = kept.exists();
    std::fs::write(&kept, &bytes)
        .with_context(|| format!("{} cannot be written", kept.display()))?;
    Ok(if replaced {
        Kept::Replaced { name }
    } else {
        Kept::Newly {
            agent: agent.to_owned(),
            id: id.to_owned(),
            entries: receipt["entries"].as_array().map(Vec::len).unwrap_or(0),
            name,
        }
    })
}

/// A file name safe to keep: the agent and the session id, every character
/// a file name cannot hold made `_`, cut to a length a path tolerates.
fn file_name(agent: &str, id: &str) -> String {
    let safe = |text: &str| {
        text.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                    c
                } else {
                    '_'
                }
            })
            .take(60)
            .collect::<String>()
    };
    format!("{}-{}", safe(agent), safe(id))
}

/// A receipt that passes: the schema vessa writes, only the fields that
/// schema has, no money anywhere, and an agent and a session to belong to.
fn check(receipt: &serde_json::Value) -> Result<()> {
    let unknown = |given: &serde_json::Map<String, serde_json::Value>, allowed: &[&str]| {
        given
            .keys()
            .filter(|field| !allowed.contains(&field.as_str()))
            .map(String::to_owned)
            .collect::<BTreeSet<_>>()
    };
    let fields = receipt
        .as_object()
        .context("it is not a JSON object, as an agent-usage.v1 receipt is")?;
    let fields_unknown = unknown(fields, &RECEIPT_FIELDS);
    ensure!(
        fields_unknown.is_empty(),
        "it names fields the agent-usage.v1 schema does not have: {}",
        fields_unknown
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    );
    ensure!(
        receipt["schema"] == RECEIPT,
        "its schema is not {RECEIPT}; an import reads only what `vessa receipt` writes"
    );
    ensure!(
        receipt["agent"].is_string() && !receipt["agent"].as_str().unwrap().is_empty(),
        "it names no agent"
    );
    ensure!(
        receipt["session_id"].is_string() && !receipt["session_id"].as_str().unwrap().is_empty(),
        "it names no session"
    );
    let entries = receipt["entries"]
        .as_array()
        .context("its entries are not a list")?;
    for (index, entry) in entries.iter().enumerate() {
        let entry = entry
            .as_object()
            .with_context(|| format!("entry {index} is not an object"))?;
        let entry_unknown = unknown(entry, &ENTRY_FIELDS);
        ensure!(
            entry_unknown.is_empty(),
            "entry {index} names fields the agent-usage.v1 schema does not have: {}",
            entry_unknown.iter().cloned().collect::<Vec<_>>().join(", ")
        );
        for field in ["provider", "model"] {
            ensure!(
                entry.get(field).is_none_or(|value| value.is_string()),
                "entry {index}: {field} is not a string"
            );
        }
        for field in ENTRY_FIELDS.iter().skip(2) {
            let value = entry
                .get(*field)
                .with_context(|| format!("entry {index}: {field} is missing"))?;
            ensure!(
                value.is_u64(),
                "entry {index}: {field} is not a count (a number, never negative)"
            );
        }
    }
    Ok(())
}

/// The sessions the kept receipts stand for, as `usage` and `stats` count
/// them. A file that no longer parses is a session with a limitation, not
/// a silent gap; nothing here can refuse.
pub fn sessions() -> Vec<model::Session> {
    let Some(dir) = state_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir.join("imported")) else {
        return Vec::new();
    };
    let mut sessions = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                sessions.push(unreadable(name, &format!("cannot be read: {error}")));
                continue;
            }
        };
        let receipt: serde_json::Value = match serde_json::from_str(&text) {
            Ok(receipt) => receipt,
            Err(error) => {
                sessions.push(unreadable(name, &format!("no longer parses: {error}")));
                continue;
            }
        };
        if check(&receipt).is_err() {
            sessions.push(unreadable(name, "no longer passes the import's check"));
            continue;
        }
        let at = receipt["observed_at_ms"]
            .as_i64()
            .and_then(chrono::DateTime::from_timestamp_millis)
            .map(|at| at.to_rfc3339());
        let stem = name.strip_suffix(".json").unwrap_or(name);
        let usage = receipt["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(index, entry)| model::Usage {
                id: format!("imported:{stem}:{index}"),
                at: at.clone(),
                source: stem.to_owned(),
                line: 0,
                entry: model::Entry {
                    provider: entry["provider"].as_str().unwrap_or("unknown").to_owned(),
                    model: entry["model"].as_str().unwrap_or("unknown").to_owned(),
                    input_tokens: entry["input_tokens"].as_u64(),
                    output_tokens: entry["output_tokens"].as_u64(),
                    cache_read_tokens: entry["cache_read_tokens"].as_u64(),
                    cache_write_tokens: entry["cache_write_tokens"].as_u64(),
                },
            })
            .collect();
        let mut limitations = Vec::new();
        limitations.extend(
            receipt["limitations"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|value| value.as_str().map(str::to_owned)),
        );
        sessions.push(model::Session {
            agent: receipt["agent"].as_str().unwrap_or("unknown").to_owned(),
            id: receipt["session_id"]
                .as_str()
                .unwrap_or("unknown")
                .to_owned(),
            parent: None,
            started_at: at.clone(),
            cwd: String::new(),
            source: format!("imported:{stem}"),
            usage,
            limitations,
            ohjaa: Vec::new(),
            work: Vec::new(),
            entrypoint: None,
            originator: None,
            branch: None,
            touched: Default::default(),
            launched: Vec::new(),
            counted_elsewhere: 0,
            synthetic: Default::default(),
            root: None,
            subagent: false,
            partly_sidechain: false,
        });
    }
    sessions
}

/// A kept file that cannot be counted: said, never a gap that looks like
/// zero.
fn unreadable(name: &str, why: &str) -> model::Session {
    model::Session {
        agent: "imported".to_owned(),
        id: name.to_owned(),
        parent: None,
        started_at: None,
        cwd: String::new(),
        source: format!("imported:{name}"),
        usage: Vec::new(),
        limitations: vec![format!("an imported receipt {why}")],
        ohjaa: Vec::new(),
        work: Vec::new(),
        entrypoint: None,
        originator: None,
        branch: None,
        touched: Default::default(),
        launched: Vec::new(),
        counted_elsewhere: 0,
        synthetic: Default::default(),
        root: None,
        subagent: false,
        partly_sidechain: false,
    }
}

/// `vessa backup --to DIR`: copy every kept receipt into a directory of the
/// person's choosing, with a manifest. The receipts are the part of vessa's
/// state that cannot be re-read from anywhere; the cursors can. Restoring
/// is importing again: `vessa import --file DIR/<name>.json` per file.
pub fn backup(to: &std::path::Path) -> Result<i32> {
    let Some(dir) = state_dir() else {
        bail!("neither XDG_STATE_HOME nor HOME names a state directory");
    };
    std::fs::create_dir_all(to).with_context(|| format!("{} cannot be made", to.display()))?;
    let imported = dir.join("imported");
    let mut kept = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&imported) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                let name = path.file_name().unwrap_or_default();
                let there = to.join(name);
                std::fs::copy(&path, &there)
                    .with_context(|| format!("{} cannot be written", there.display()))?;
                kept.push(name.to_string_lossy().into_owned());
            }
        }
    }
    let manifest = serde_json::json!({
        "schema": "vessa-backup.v1",
        "receipts": kept.len(),
        "at": chrono::DateTime::from_timestamp(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs() as i64,
            0,
        )
        .unwrap_or_default()
        .to_rfc3339(),
    });
    std::fs::write(
        to.join("manifest.json"),
        serde_json::to_string_pretty(&manifest)?,
    )
    .with_context(|| format!("{} cannot be written", to.join("manifest.json").display()))?;
    println!(
        "vessa: backed up {} to {}",
        crate::render::many(kept.len(), "receipt"),
        to.display()
    );
    Ok(0)
}

fn state_dir() -> Option<PathBuf> {
    let absolute = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
    match absolute {
        Some(state) => Some(state.join("vessa")),
        None => std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join(".local")
                .join("state")
                .join("vessa")
        }),
    }
}
