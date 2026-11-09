//! opencode's session store: one SQLite database, read with the `sqlite3`
//! command, read-only, so Vessa takes no SQLite dependency. Only the layout
//! Vessa knows is read (`session_v2` and `session_message`); a store in any
//! other is unreadable, never empty.
use crate::model::Session;
use anyhow::{anyhow, bail, ensure, Context, Result};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where opencode keeps its store: `$XDG_DATA_HOME/opencode/opencode.db`,
/// else `~/.local/share/opencode/opencode.db`.
pub fn default_store() -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(data.join("opencode/opencode.db"))
}

/// Whether `path` is a file given as a store: a SQLite database, by its
/// header, or a `.db` file, which is read as one and said to be unreadable
/// when it is not.
pub fn is_store(path: &Path) -> bool {
    if !std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file()) {
        return false;
    }
    let mut magic = [0u8; 16];
    path.extension().is_some_and(|e| e == "db")
        || std::fs::File::open(path)
            .and_then(|mut f| f.read_exact(&mut magic))
            .is_ok_and(|()| &magic == b"SQLite format 3\0")
}

/// The columns Vessa reads, by table: what it needs of opencode's layout.
const LAYOUT: [(&str, &[&str]); 2] = [
    (
        "session_v2",
        &["id", "parent_id", "directory", "time_created"],
    ),
    (
        "session_message",
        &["id", "session_id", "type", "seq", "time_created", "data"],
    ),
];

/// The sessions in the store, parents first, with no usage read, and what
/// the store holds that Vessa does not read. A store whose layout is not
/// the one Vessa knows is an error: it cannot say what the store holds.
pub fn sessions(store: &Path) -> Result<(Vec<Session>, Vec<String>)> {
    let mut magic = [0u8; 16];
    std::fs::File::open(store)?
        .read_exact(&mut magic)
        .ok()
        .filter(|()| &magic == b"SQLite format 3\0")
        .context("not a SQLite database, so not an opencode store")?;
    let mut tables: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for row in query(
        store,
        "select m.name as tbl, p.name as col from sqlite_master m, pragma_table_info(m.name) p where m.type = 'table'",
    )? {
        tables
            .entry(text(&row, "tbl")?)
            .or_default()
            .insert(text(&row, "col")?);
    }
    for (table, columns) in LAYOUT {
        let have = tables.get(table);
        let missing: Vec<&str> = columns
            .iter()
            .copied()
            .filter(|c| !have.is_some_and(|have| have.contains(*c)))
            .collect();
        ensure!(
            missing.is_empty(),
            "opencode's session layout here is not one Vessa reads: {}",
            if have.is_none() {
                format!("no {table} table")
            } else {
                format!("{table} has no {}", missing.join(", "))
            }
        );
    }
    let mut warnings = Vec::new();
    // Older opencode kept sessions in `session`; newer copies them into
    // `session_v2`. One not copied is in the store and not read.
    if tables.get("session").is_some_and(|c| c.contains("id")) {
        let rows = query(
            store,
            "select count(*) as n from session where id not in (select id from session_v2)",
        )?;
        let n = rows.first().and_then(|r| r["n"].as_u64()).unwrap_or(0);
        if n > 0 {
            warnings.push(format!(
                "{}: {n} session(s) in opencode's older layout (table session) not read",
                store.display()
            ));
        }
    }
    let mut sessions = Vec::new();
    for row in query(
        store,
        "select id, parent_id, directory, time_created from session_v2 order by time_created, id",
    )? {
        let id = text(&row, "id")?;
        ensure!(!id.is_empty(), "a session without an id");
        let parent = row["parent_id"].as_str().map(str::to_owned);
        sessions.push(Session {
            agent: "opencode".into(),
            subagent: parent.is_some(),
            id,
            parent,
            started_at: row["time_created"]
                .as_i64()
                .and_then(chrono::DateTime::from_timestamp_millis)
                .map(|t| t.to_rfc3339()),
            cwd: row["directory"].as_str().unwrap_or_default().to_owned(),
            source: store.display().to_string(),
            usage: Vec::new(),
            limitations: Vec::new(),
            ohjaa: Vec::new(),
            work: Vec::new(),
            entrypoint: None,
            originator: None,
            launched: Vec::new(),
            counted_elsewhere: 0,
            synthetic: Default::default(),
            root: None,
            partly_sidechain: false,
        });
    }
    // A subagent may start one of its own: each counts to the session at
    // the top of its chain, as far as the store holds it. A chain that
    // loops has no top, and each session in it stands alone.
    let parents: BTreeMap<String, Option<String>> = sessions
        .iter()
        .map(|s| (s.id.clone(), s.parent.clone()))
        .collect();
    for s in &mut sessions {
        let mut seen = BTreeSet::from([s.id.clone()]);
        let mut top = s.parent.clone();
        while let Some(next) = top.as_ref().and_then(|t| parents.get(t)).cloned().flatten() {
            if !seen.insert(next.clone()) {
                top = None;
                s.limitations.push(format!(
                    "opencode subagent chain loops through {next}; its root session is unknown, so it counts as a session of its own"
                ));
                break;
            }
            top = Some(next);
        }
        s.root = top;
    }
    sessions.sort_by_key(|s| s.parent.is_some());
    Ok((sessions, warnings))
}

/// One session's messages in its order, each as a record of its own: its
/// `type`, `id`, `seq`, its creation as `timestamp` (milliseconds), and its
/// `data`. A message's `seq` is where it stands, as a line does in a JSONL
/// transcript. Data that is not JSON stays the text it is, under `data`,
/// with why under `malformed`.
pub fn records(store: &Path, session: &str) -> Result<Vec<(usize, Value)>> {
    let rows = query(
        store,
        &format!(
            "select id, type, seq, time_created, data from session_message where session_id = '{}' order by seq, id",
            session.replace('\'', "''")
        ),
    )?;
    rows.into_iter()
        .map(|row| {
            let seq = row["seq"]
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .context("a message without a valid seq")?;
            let raw = text(&row, "data")?;
            let mut record = serde_json::json!({
                "type": row["type"],
                "id": row["id"],
                "seq": seq,
                "timestamp": row["time_created"],
            });
            match serde_json::from_str::<Value>(&raw) {
                Ok(data) => record["data"] = data,
                Err(error) => {
                    record["data"] = Value::String(raw);
                    record["malformed"] = Value::String(error.to_string());
                }
            }
            Ok((seq, record))
        })
        .collect()
}

/// The rows `sql` selects, through `sqlite3 -readonly -json`. No rows print
/// nothing, which is an empty list.
fn query(store: &Path, sql: &str) -> Result<Vec<Value>> {
    let output = Command::new("sqlite3")
        .args(["-readonly", "-bail", "-json", "-cmd", ".timeout 5000"])
        .arg(store)
        .arg(sql)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => {
                anyhow!("sqlite3 not found on PATH; Vessa reads opencode's store with it")
            }
            _ => anyhow!("sqlite3 could not run: {error}"),
        })?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        bail!(
            "sqlite3 could not read it: {}",
            said.lines().next().unwrap_or("no reason given").trim()
        );
    }
    if output.stdout.iter().all(u8::is_ascii_whitespace) {
        return Ok(Vec::new());
    }
    serde_json::from_slice(&output.stdout).context("sqlite3 printed rows Vessa cannot read")
}

fn text(row: &Value, column: &str) -> Result<String> {
    row[column]
        .as_str()
        .map(str::to_owned)
        .with_context(|| format!("column {column} is not text"))
}
