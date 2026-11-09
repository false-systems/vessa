//! opencode stores for the tests: synthetic SQLite databases in opencode's
//! own layout, made with the sqlite3 command that Vessa reads them with.
#![allow(dead_code)]
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The folder of the sqlite3 command on this PATH. opencode's store can be
/// neither made nor read without it, so these tests require it.
pub fn sqlite3_dir() -> PathBuf {
    std::env::var_os("PATH")
        .and_then(|path| std::env::split_paths(&path).find(|dir| dir.join("sqlite3").is_file()))
        .expect("the opencode tests need the sqlite3 command on PATH")
}

/// opencode's tables for its sessions and their messages, as opencode
/// creates them.
pub const LAYOUT: &str = r#"
CREATE TABLE `session_v2` (
  `id` text PRIMARY KEY, `project_id` text NOT NULL, `workspace_id` text,
  `parent_id` text, `fork_session_id` text, `fork_boundary` text,
  `slug` text NOT NULL, `directory` text NOT NULL, `path` text, `title` text,
  `version` text NOT NULL, `share_url` text, `summary_additions` integer,
  `summary_deletions` integer, `summary_files` integer, `summary_diffs` text,
  `metadata` text, `cost` real DEFAULT 0 NOT NULL,
  `tokens_input` integer DEFAULT 0 NOT NULL, `tokens_output` integer DEFAULT 0 NOT NULL,
  `tokens_reasoning` integer DEFAULT 0 NOT NULL, `tokens_cache_read` integer DEFAULT 0 NOT NULL,
  `tokens_cache_write` integer DEFAULT 0 NOT NULL, `revert` text, `permission` text,
  `agent` text, `model` text, `time_created` integer NOT NULL, `time_updated` integer NOT NULL,
  `time_compacting` integer, `time_archived` integer, `time_suspended` integer,
  `resume_attempts` integer DEFAULT 0 NOT NULL, `time_idle` integer, `time_viewed` integer,
  `idle_outcome` text
);
CREATE TABLE "session_message" (
  `id` text PRIMARY KEY, `session_id` text NOT NULL, `type` text NOT NULL,
  `seq` integer NOT NULL, `time_created` integer NOT NULL, `time_updated` integer NOT NULL,
  `data` text NOT NULL,
  CONSTRAINT `fk_session_message_session_id_session_v2_id_fk` FOREIGN KEY (`session_id`) REFERENCES `session_v2`(`id`) ON DELETE CASCADE
);
"#;

/// Older opencode's session table, which newer opencode copies into
/// `session_v2`.
pub const OLDER: &str =
    "CREATE TABLE `session` (`id` text PRIMARY KEY, `directory` text NOT NULL, `time_created` integer NOT NULL);";

/// A session: its id, the session that started it, its folder and when it
/// was created, in milliseconds.
pub type Session<'a> = (&'a str, Option<&'a str>, &'a str, i64);
/// A message: its id, session, type, seq, creation in milliseconds, and its
/// data as written.
pub type Message<'a> = (&'a str, &'a str, &'a str, i64, i64, String);

/// Writes a store at `db`: `layout` first, then `sessions` and
/// `messages`.
pub fn store(db: &Path, layout: &str, sessions: &[Session], messages: &[Message]) {
    let quote = |text: &str| format!("'{}'", text.replace('\'', "''"));
    let mut script = layout.to_owned();
    for (id, parent, directory, at) in sessions {
        script += &format!(
            "INSERT INTO session_v2 (id, project_id, parent_id, slug, directory, version, time_created, time_updated) VALUES ({}, 'project', {}, 'slug', {}, '1.0.0', {at}, {at});\n",
            quote(id),
            parent.map_or("NULL".into(), quote),
            quote(directory),
        );
    }
    for (id, session, kind, seq, at, data) in messages {
        script += &format!(
            "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data) VALUES ({}, {}, {}, {seq}, {at}, {at}, {});\n",
            quote(id),
            quote(session),
            quote(kind),
            quote(data),
        );
    }
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    sql(db, &script);
}

/// Runs `script` against the store at `db`.
pub fn sql(db: &Path, script: &str) {
    let mut child = Command::new(sqlite3_dir().join("sqlite3"))
        .arg(db)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "sqlite3: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A time in the fixtures' day, 2026-09-30, `seconds` after 09:00 UTC, in
/// milliseconds.
pub fn at(seconds: i64) -> i64 {
    1_790_758_800_000 + seconds * 1000
}
