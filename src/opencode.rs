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
/// else `~/.local/share/opencode/opencode.db`. As the XDG specification
/// says, an `XDG_DATA_HOME` that is empty or relative is ignored.
pub fn default_store() -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(data.join("opencode/opencode.db"))
}

/// Whether `path` is a file given as a store: a SQLite database, by its
/// header, or a `.db` file, which is read as one and said to be unreadable
/// when it is not.
pub fn is_store(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
        && (path.extension().is_some_and(|e| e == "db") || sqlite(path))
}

fn sqlite(path: &Path) -> bool {
    let mut magic = [0u8; 16];
    std::fs::File::open(path)
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

/// A session in a store, with what the store says of it besides.
pub struct Stored {
    /// Its header, no usage read. A subagent's `root` is the session that
    /// started it, which [`crate::import`] follows to the top of the chain.
    pub session: Session,
    /// When its newest message was created, in milliseconds; none when it
    /// has none.
    pub last: Option<i64>,
    /// The session it was forked from.
    pub fork: Option<String>,
}

/// The sessions in the store, with what the store holds that Vessa does not
/// read. A store whose layout is not the one Vessa knows is an error: it
/// cannot say what the store holds.
pub fn sessions(store: &Path) -> Result<(Vec<Stored>, Vec<String>)> {
    ensure!(
        sqlite(store),
        "not a SQLite database, so not an opencode store"
    );
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
                "{n} session(s) in opencode's older layout (table session) not read"
            ));
        }
    }
    let fork = if tables["session_v2"].contains("fork_session_id") {
        "fork_session_id"
    } else {
        "null"
    };
    let mut sessions = Vec::new();
    for row in query(
        store,
        &format!(
            "select s.id, s.parent_id, s.directory, s.time_created, {fork} as fork, (select max(m.time_created) from session_message m where m.session_id = s.id) as last from session_v2 s order by s.time_created, s.id"
        ),
    )? {
        let id = text(&row, "id")?;
        ensure!(!id.is_empty(), "a session without an id");
        let parent = row["parent_id"].as_str().map(str::to_owned);
        sessions.push(Stored {
            session: Session {
                agent: "opencode".into(),
                subagent: parent.is_some(),
                root: parent.clone(),
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
                partly_sidechain: false,
            },
            last: row["last"].as_i64(),
            fork: row["fork"].as_str().map(str::to_owned),
        });
    }
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
            let raw = text(&row, "data")?;
            let mut record = message(&row)?;
            match serde_json::from_str::<Value>(&raw) {
                Ok(data) => record.1["data"] = data,
                Err(error) => {
                    record.1["data"] = Value::String(raw);
                    record.1["malformed"] = Value::String(error.to_string());
                }
            }
            Ok(record)
        })
        .collect()
}

/// Each session's records, by session id.
pub type BySession = BTreeMap<String, Vec<(usize, Value)>>;

/// Every session's model responses (`assistant` and `compaction` messages),
/// by session, as [`records`] gives them but with only the `tokens` and
/// `model` of their data: what usage reads, in one query for the whole
/// store. A response whose data is not JSON comes with `malformed`.
pub fn responses(store: &Path) -> Result<BySession> {
    let rows = query(
        store,
        "select session_id, id, type, seq, time_created, json_valid(data) as valid, case when json_valid(data) then json_extract(data, '$.tokens') end as tokens, case when json_valid(data) then json_extract(data, '$.model') end as model from session_message where type in ('assistant', 'compaction') order by session_id, seq, id",
    )?;
    let mut by_session = BySession::new();
    for row in rows {
        let mut record = message(&row)?;
        if row["valid"].as_i64() == Some(1) {
            let mut data = serde_json::Map::new();
            for field in ["tokens", "model"] {
                let value = match &row[field] {
                    Value::String(json) => serde_json::from_str(json)
                        .with_context(|| format!("{field} is not JSON"))?,
                    other => other.clone(),
                };
                data.insert(field.into(), value);
            }
            record.1["data"] = Value::Object(data);
        } else {
            record.1["malformed"] = "its data is not JSON".into();
        }
        by_session
            .entry(text(&row, "session_id")?)
            .or_default()
            .push(record);
    }
    Ok(by_session)
}

/// A message row as a record, its data not yet added.
fn message(row: &Value) -> Result<(usize, Value)> {
    let seq = row["seq"]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .context("a message without a valid seq")?;
    Ok((
        seq,
        serde_json::json!({
            "type": row["type"],
            "id": row["id"],
            "seq": seq,
            "timestamp": row["time_created"],
        }),
    ))
}

/// The rows `sql` selects, through `sqlite3 -readonly -json`. No rows print
/// nothing, which is an empty list. `-init /dev/null` keeps the person's
/// `~/.sqliterc` (`.timer on`, `.echo on`) out of the output.
fn query(store: &Path, sql: &str) -> Result<Vec<Value>> {
    let output = Command::new("sqlite3")
        .args([
            "-init",
            "/dev/null",
            "-batch",
            "-readonly",
            "-bail",
            "-json",
        ])
        .args(["-cmd", ".timeout 5000"])
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
        let said = said.lines().next().unwrap_or("no reason given").trim();
        // `-json` came with sqlite3 3.33.
        if said.contains("-json") {
            bail!("this sqlite3 cannot print JSON ({said}); Vessa needs sqlite3 3.33 or newer");
        }
        bail!("sqlite3 could not read it: {said}");
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
