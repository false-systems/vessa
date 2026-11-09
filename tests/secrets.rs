mod support;

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "vessa-secrets-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str, records: &[Value]) -> PathBuf {
        let path = self.0.join(name);
        let text = records.iter().map(|v| format!("{v}\n")).collect::<String>();
        std::fs::write(&path, text).unwrap();
        path
    }
    /// The scan of `path`, with [`key`] one of the credentials this machine
    /// holds, as a secret-named variable.
    fn run(&self, path: &Path, args: &[&str]) -> Output {
        self.run_with(path, args, &[("FIXTURE_API_KEY", &key())])
    }
    /// The scan in an emptied environment: the fixture is the home folder,
    /// there is no PATH (so no gh), and `env` is all that is set besides.
    /// Never the person's own credentials.
    fn run_with(&self, path: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_vessa"));
        command
            .env_clear()
            .env("HOME", &self.0)
            .args(["secrets", "--input"])
            .arg(path)
            .args(args);
        for (name, value) in env {
            command.env(name, value);
        }
        command.output().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn key() -> String {
    ["s", "k-", &"a1".repeat(12)].concat()
}

#[test]
fn explicit_secret_labels_survive_the_native_scan_without_changing_redaction() {
    let f = Fixture::new();
    let value = ["Ab3dE5gH7jK9mN1pQ3sT", "5vW7yZ9bC1dF3hJ5kL7mN9p"].concat();
    let path = f.file("labelled-codex.jsonl", &[
        json!({"type":"session_meta","payload":{"id":"labelled-codex","cwd":"/fixture"}}),
        json!({"type":"event_msg","timestamp":"2026-10-01T00:00:01Z","payload":{"type":"user_message","message":format!("password: {value}")}}),
        json!({"type":"event_msg","timestamp":"2026-10-01T00:00:02Z","payload":{"type":"user_message","message":value}})
    ]);
    let output = f.run_with(&path, &["--json"], &[("FIXTURE_PASSWORD", &value)]);
    assert!(!String::from_utf8_lossy(&output.stdout).contains(&value));
    let doc = document(&output, 0);
    let rows = doc["findings"].as_array().unwrap();
    for rule in ["named_secret", "mixed_token"] {
        assert_eq!(
            rows.iter()
                .filter(|row| row["rule"] == rule)
                .map(|row| row["count"].as_u64().unwrap())
                .sum::<u64>(),
            1,
            "{doc}"
        );
    }
    assert_eq!(rows.len(), 2, "{doc}");
}

#[test]
fn opaque_transport_matches_are_counted_without_hiding_key_shaped_values() {
    let f = Fixture::new();
    let at = "2026-10-01T00:00:01Z";
    let opaque = "Q7hB4mR9vT2kN8pS6xA3dF5jL1cE0uWz";
    let claude = f.file("opaque-claude.jsonl", &[
        json!({"type":"assistant","sessionId":"opaque-claude","cwd":"/fixture","timestamp":at,"message":{"role":"assistant","content":[{"type":"thinking","thinking":key(),"signature":opaque}]}})
    ]);
    let codex = f.file("opaque-codex.jsonl", &[
        json!({"type":"session_meta","payload":{"id":"opaque-codex","cwd":"/fixture"}}),
        json!({"type":"response_item","timestamp":at,"payload":{"type":"reasoning","summary":[],"encrypted_content":opaque}}),
        json!({"type":"response_item","timestamp":at,"payload":{"type":"reasoning","summary":[],"encrypted_content":key()}})
    ]);
    for path in [claude, codex] {
        let value = document(
            &f.run_with(
                &path,
                &["--json"],
                &[("FIXTURE_API_KEY", &key()), ("FIXTURE_SIGNING_KEY", opaque)],
            ),
            0,
        );
        let rows = value["findings"].as_array().unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| row["count"].as_u64().unwrap())
                .sum::<u64>(),
            2
        );
        let opaque_count: u64 = rows
            .iter()
            .map(|row| {
                row["opaque_count"]
                    .as_u64()
                    .expect("every new finding reports context coverage")
            })
            .sum();
        assert!(
            rows.iter().all(|row| row["opaque_native"] == true),
            "decoded context is explicitly native-position verified: {value}"
        );
        assert_eq!(
            opaque_count,
            if path.ends_with("opaque-claude.jsonl") {
                1
            } else {
                2
            }
        );
        assert!(rows.iter().any(|row| row["rule"] == "openai"));
        assert!(!String::from_utf8_lossy(
            &f.run_with(&path, &["--json"], &[("FIXTURE_SIGNING_KEY", opaque)])
                .stdout
        )
        .contains(opaque));
    }
}

#[test]
fn user_supplied_or_unknown_blocks_cannot_claim_native_opaque_context() {
    let f = Fixture::new();
    let opaque = "Q7hB4mR9vT2kN8pS6xA3dF5jL1cE0uWz";
    let at = "2026-10-01T00:00:01Z";
    let claude = f.file("user-claude.jsonl", &[
        json!({"type":"user","sessionId":"user-claude","cwd":"/fixture","timestamp":at,"message":{"role":"user","content":[{"type":"thinking","signature":opaque}]}})
    ]);
    let codex = f.file("user-codex.jsonl", &[
        json!({"type":"session_meta","payload":{"id":"user-codex","cwd":"/fixture"}}),
        json!({"type":"response_item","timestamp":at,"payload":{"type":"message","role":"user","content":[{"type":"reasoning","encrypted_content":opaque}]}}),
        json!({"type":"reasoning","timestamp":at,"encrypted_content":opaque})
    ]);
    for path in [claude, codex] {
        let value = document(
            &f.run_with(&path, &["--json"], &[("FIXTURE_SIGNING_KEY", opaque)]),
            0,
        );
        let rows = value["findings"].as_array().unwrap();
        assert!(!rows.is_empty());
        assert!(
            rows.iter()
                .all(|row| row["count"] == 1 && row["opaque_count"] == 0),
            "{value}"
        );
    }
}

/// A record without a time of its own takes the time of the record before
/// it, said as such: a transcript is appended to, so it was written at or
/// after that one. A nested time is honored only where it is native.
#[test]
fn a_record_without_a_time_takes_the_time_of_the_record_before_said_as_inferred() {
    let f = Fixture::new();
    let path = f.file("history.jsonl", &[
        json!({"type":"user","sessionId":"history","cwd":"/fixture","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"Synthetic header"}}),
        json!({"type":"file-history-snapshot","snapshot":{"timestamp":"2026-09-30T00:00:00Z","data":key()}}),
        json!({"type":"file-history-snapshot","snapshot":{"timestamp":"2026-10-01T00:00:01Z","data":key()}}),
        json!({"type":"future_record","snapshot":{"timestamp":"2026-10-01T00:00:01Z"},"data":key()})
    ]);
    let value = document(
        &f.run(&path, &["--json", "--since", "2026-10-01T00:00:00Z"]),
        0,
    );
    let rows = value["findings"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{value}");
    let inferred: Vec<&Value> = rows.iter().filter(|r| r["at_inferred"] == true).collect();
    assert_eq!(inferred.len(), 1, "{value}");
    assert_eq!(inferred[0]["line"], 4);
    assert_eq!(inferred[0]["at"], "2026-10-01T00:00:01Z");
    assert_eq!(value["undated_records"], 0);
    assert_eq!(value["complete"], true);
}

/// The record before is the window's edge or earlier: the one without a
/// time may be either side of `--since`, so it stays unplaced.
#[test]
fn a_record_after_one_before_the_window_stays_unplaced() {
    let f = Fixture::new();
    let path = f.file("edge.jsonl", &[
        json!({"type":"session_meta","payload":{"id":"edge","cwd":"/fixture"}}),
        json!({"type":"response_item","timestamp":"2026-09-30T00:00:00Z","payload":{"type":"message","role":"user","content":"Synthetic"}}),
        json!({"type":"response_item","payload":{"type":"function_call_output","output":key()}}),
    ]);
    let value = document(
        &f.run(&path, &["--json", "--since", "2026-10-01T00:00:00Z"]),
        4,
    );
    assert!(value["findings"][0]["at"].is_null(), "{value}");
    assert_eq!(value["undated_records"], 1);
    assert_eq!(value["complete"], false);
}

#[test]
fn nested_history_times_are_not_borrowed_by_other_native_formats() {
    let f = Fixture::new();
    let path = f.file("codex-unknown-history.jsonl", &[
        json!({"type":"session_meta","payload":{"id":"unknown-history","cwd":"/fixture"}}),
        json!({"type":"file-history-snapshot","snapshot":{"timestamp":"2026-09-30T00:00:00Z","data":key()}})
    ]);
    let value = document(
        &f.run(&path, &["--json", "--since", "2026-10-01T00:00:00Z"]),
        4,
    );
    assert_eq!(value["findings"].as_array().unwrap().len(), 1);
    assert!(value["findings"][0]["at"].is_null());
    assert_eq!(value["undated_records"], 1);
}
fn document(out: &Output, expected: i32) -> Value {
    assert_eq!(
        out.status.code(),
        Some(expected),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !text.contains(&key()),
        "matched values must never leave the reader"
    );
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["schema"], "vessa-secrets.v1");
    value
}

#[test]
fn secrets_cover_native_person_agent_and_both_tool_directions_without_values() {
    let f = Fixture::new();
    let at = "2026-10-01T00:00:01Z";
    let k = key();
    let cases = [
        (
            "claude",
            vec![
                json!({"type":"user","sessionId":"claude-scan","cwd":"/fixture","timestamp":at,"message":{"role":"user","content":k}}),
                json!({"type":"assistant","sessionId":"claude-scan","timestamp":at,"message":{"role":"assistant","content":[{"type":"text","text":k},{"type":"tool_use","input":{"authorization":k}}]}}),
                json!({"type":"user","sessionId":"claude-scan","timestamp":at,"message":{"role":"user","content":[{"type":"tool_result","content":k}]}}),
            ],
        ),
        (
            "codex",
            vec![
                json!({"type":"session_meta","payload":{"id":"codex-scan","cwd":"/fixture","source":"cli"}}),
                json!({"type":"response_item","timestamp":at,"payload":{"type":"message","role":"user","content":[{"type":"input_text","text":k}]}}),
                json!({"type":"response_item","timestamp":at,"payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":k}]}}),
                json!({"type":"response_item","timestamp":at,"payload":{"type":"function_call","arguments":json!({"authorization":k}).to_string()}}),
                json!({"type":"response_item","timestamp":at,"payload":{"type":"function_call_output","output":k}}),
            ],
        ),
        (
            "pi",
            vec![
                json!({"type":"session","id":"pi-scan","cwd":"/fixture","timestamp":at}),
                json!({"type":"message","timestamp":at,"message":{"role":"user","content":[{"type":"text","text":k}]}}),
                json!({"type":"message","timestamp":at,"message":{"role":"assistant","content":[{"type":"text","text":k},{"type":"toolCall","arguments":{"authorization":k}}]}}),
                json!({"type":"message","timestamp":at,"message":{"role":"toolResult","content":[{"type":"text","text":k}]}}),
            ],
        ),
    ];
    for (agent, records) in cases {
        let path = f.file(&format!("{agent}.jsonl"), &records);
        let value = document(&f.run(&path, &["--json"]), 0);
        assert_eq!(value["complete"], true);
        assert_eq!(value["unreadable"], json!([]));
        let findings = value["findings"].as_array().unwrap();
        assert_eq!(findings.len(), 4, "{value}");
        for place in ["person", "agent", "tool_input", "tool_output"] {
            let found = findings.iter().find(|v| v["where"] == place).unwrap();
            assert_eq!(found["agent"], agent);
            assert_eq!(found["session"], format!("{agent}-scan"));
            assert_eq!(found["at"], at);
            assert_eq!(found["rule"], "openai");
            assert_eq!(found["count"], 1);
            assert!(found["source"]
                .as_str()
                .unwrap()
                .ends_with(&format!("{agent}.jsonl")));
            assert!(found["line"].as_u64().unwrap() > 0);
            assert!(found.get("value").is_none());
            assert!(found.get("fingerprint").is_none());
        }
        let plain = f.run(&path, &[]);
        assert!(plain.status.success());
        let text = String::from_utf8(plain.stdout).unwrap();
        assert!(
            text.contains("4 occurrence(s) of this machine's credentials"),
            "{text}"
        );
        assert!(
            text.contains("environment variable FIXTURE_API_KEY"),
            "{text}"
        );
        assert!(!text.contains(&k));
    }
}

#[test]
fn secrets_keep_good_records_and_unknown_times_without_claiming_complete_coverage() {
    let f = Fixture::new();
    let k = key();
    let path = f.file("partial.jsonl", &[
        json!({"type":"session_meta","payload":{"id":"partial","cwd":"/fixture"}}),
        json!({"type":"response_item","timestamp":"2026-10-01T00:00:00Z","payload":{"type":"message","role":"user","content":k}}),
        json!({"type":"response_item","timestamp":"2026-10-01T00:00:01Z","payload":{"type":"message","role":"assistant","content":k}}),
        json!({"type":"response_item","payload":{"type":"function_call_output","output":k}}),
    ]);
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(file, "{{broken {k}").unwrap();
    file.write_all(b"\xff\n").unwrap();
    drop(file);
    let value = document(
        &f.run(&path, &["--json", "--since", "2026-10-01T00:00:00Z"]),
        4,
    );
    assert_eq!(value["complete"], false);
    assert_eq!(value["findings"].as_array().unwrap().len(), 3, "{value}");
    // The two damaged lines; the record without a time follows one inside
    // the window, and takes its time.
    assert_eq!(value["unreadable"].as_array().unwrap().len(), 2, "{value}");
    let damaged = value["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["line"] == 5)
        .unwrap();
    assert!(
        damaged["opaque_count"].is_null(),
        "damaged records have unknown structured context: {value}"
    );
    assert!(value["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["where"] == "tool_output" && v["at_inferred"] == true));
    assert!(!value["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["where"] == "person"));
    assert_eq!(
        document(&f.run(&f.0.join("missing.jsonl"), &["--json"]), 4)["complete"],
        false
    );
}

#[test]
fn secrets_respect_filters_and_scrub_metadata_too() {
    let f = Fixture::new();
    let k = key();
    let path = f.file("identity.jsonl", &[
        json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":{"id":k,"cwd":"/fixture"}}),
        json!({"type":"response_item","timestamp":"2026-10-01T00:00:01Z","payload":{"type":"function_call_output","output":k}}),
    ]);
    let value = document(&f.run(&path, &["--json", "--session", &k]), 0);
    assert!(value["findings"]
        .as_array()
        .unwrap()
        .iter()
        .all(|v| v["session"] == "[redacted]"));
    let other = document(&f.run(&path, &["--json", "--agent", "pi"]), 0);
    assert_eq!(other["findings"], json!([]));
    assert_eq!(other["complete"], true);
    let absent = document(&f.run(&path, &["--json", "--session", "absent"]), 4);
    assert_eq!(absent["complete"], false);
    assert_eq!(f.run(&path, &["--raw"]).status.code(), Some(2));
}

#[test]
fn secrets_never_echo_a_matched_value_inside_source_paths_or_folders() {
    let f = Fixture::new();
    let k = key();
    let path = f.file(&format!("{k}.jsonl"), &[
        json!({"type":"session_meta","payload":{"id":"path-scan","cwd":format!("/fixture/{k}")}}),
        json!({"type":"response_item","timestamp":"2026-10-01T00:00:01Z","payload":{"type":"function_call_output","output":k}}),
    ]);
    let value = document(&f.run(&path, &["--json"]), 0);
    assert!(value["findings"][0]["source"]
        .as_str()
        .unwrap()
        .contains("[redacted]"));
    assert_eq!(value["findings"][0]["cwd"], "/fixture/[redacted]");
}

#[test]
fn secrets_report_unsupported_and_empty_sources_as_partial() {
    let f = Fixture::new();
    let wrapper = f.file(
        "wrapper.json",
        &[json!({"schema":"ohjaa-sessions.v1","sessions":[]})],
    );
    assert_eq!(
        document(&f.run(&wrapper, &["--json"]), 4)["complete"],
        false
    );
    let empty = f.file("empty.jsonl", &[]);
    assert_eq!(document(&f.run(&empty, &["--json"]), 4)["complete"], false);
    let arbitrary = f.file(
        "unsupported.txt",
        &[json!({"text":"not a native transcript"})],
    );
    assert_eq!(
        document(&f.run(&arbitrary, &["--json"]), 4)["complete"],
        false
    );
    let db = f.0.join("data/opencode/opencode.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    std::fs::write(&db, []).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_vessa"))
        .args(["secrets", "--json"])
        .env_clear()
        .env("HOME", &f.0)
        .env("XDG_DATA_HOME", f.0.join("data"))
        .output()
        .unwrap();
    let value = document(&out, 4);
    assert_eq!(value["unreadable"].as_array().unwrap().len(), 1);
    assert!(value["unreadable"][0]["why"]
        .as_str()
        .unwrap()
        .contains("not a SQLite database"));
}

#[test]
fn secrets_are_found_in_opencode_sessions() {
    let f = Fixture::new();
    let db = f.0.join("opencode.db");
    let tool = json!({"type":"tool","id":"call_1","name":"bash","executed":true,"state":{
        "status":"completed",
        "input":{"command":format!("curl -H 'X-Api-Key: {}' https://example.invalid", key())},
        "content":[{"type":"text","text":format!("FIXTURE_API_KEY={}", key())}],
        "metadata":{"exit":0}
    }});
    support::store(
        &db,
        support::LAYOUT,
        &[("ses_main", None, "/fixture", support::at(0))],
        &[
            (
                "msg_1",
                "ses_main",
                "user",
                0,
                support::at(1),
                json!({"text":format!("Use {} for the API.", key())}).to_string(),
            ),
            (
                "msg_2",
                "ses_main",
                "assistant",
                1,
                support::at(2),
                json!({"content":[{"type":"text","text":"Calling it."}, tool]}).to_string(),
            ),
            (
                "msg_3",
                "ses_main",
                "synthetic",
                2,
                support::at(3),
                json!({"text":"Loaded AGENTS.md"}).to_string(),
            ),
            (
                "msg_4",
                "ses_main",
                "assistant",
                3,
                support::at(4),
                format!("{{\"content\": [\"{}\"", key()),
            ),
        ],
    );
    let sqlite3 = support::sqlite3_dir();
    let env = [
        ("FIXTURE_API_KEY", key()),
        ("PATH", sqlite3.display().to_string()),
    ];
    let env: Vec<(&str, &str)> = env.iter().map(|(n, v)| (*n, v.as_str())).collect();
    // The damaged message is a gap; what it holds is still found.
    let v = document(&f.run_with(&db, &["--json"], &env), 4);
    assert_eq!(v["transcripts"], 1);
    let store = db.canonicalize().unwrap().display().to_string();
    let found: Vec<(u64, &str, u64, &str)> = v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            assert_eq!(f["agent"], "opencode");
            assert_eq!(f["session"], "ses_main");
            assert_eq!(f["cwd"], "/fixture");
            assert_eq!(f["source"], store);
            assert_eq!(f["held"], "environment variable FIXTURE_API_KEY");
            (
                f["line"].as_u64().unwrap(),
                f["where"].as_str().unwrap(),
                f["count"].as_u64().unwrap(),
                f["at"].as_str().unwrap_or("none"),
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            (0, "person", 1, "2026-09-30T09:00:01+00:00"),
            (1, "tool_input", 1, "2026-09-30T09:00:02+00:00"),
            (1, "tool_output", 1, "2026-09-30T09:00:02+00:00"),
            (3, "other", 1, "none"),
        ]
    );
    let gaps = v["unreadable"].as_array().unwrap();
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0]["line"], 3);
    assert!(gaps[0]["why"]
        .as_str()
        .unwrap()
        .contains("opencode session ses_main: malformed JSON"));
    // Each message is placed in the window by its own time.
    let v = document(
        &f.run_with(&db, &["--json", "--since", "2026-09-30T09:00:01Z"], &env),
        4,
    );
    let lines: Vec<u64> = v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["line"].as_u64().unwrap())
        .collect();
    assert_eq!(lines, [1, 1, 3]);
    // Without sqlite3 the store cannot be read: a gap, never an all-clear.
    // (With no PATH at all, the system's default search path still finds
    // /usr/bin's; an empty one finds nothing.)
    let v = document(
        &f.run_with(
            &db,
            &["--json"],
            &[("FIXTURE_API_KEY", &key()), ("PATH", "")],
        ),
        4,
    );
    assert_eq!(v["transcripts"], 0);
    assert!(v["unreadable"][0]["why"]
        .as_str()
        .unwrap()
        .contains("sqlite3 not found on PATH"));
}

#[test]
fn secrets_keep_subagent_evidence_without_claiming_an_unread_parent_was_scanned() {
    let f = Fixture::new();
    let parent = f.file("parent.jsonl", &[json!({"type":"user","sessionId":"parent","cwd":"/fixture","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"Synthetic parent."}})]);
    let children = parent.with_extension("").join("subagents");
    std::fs::create_dir_all(&children).unwrap();
    let child = children.join("agent-child.jsonl");
    std::fs::write(&child, format!("{}\n", json!({"type":"user","sessionId":"parent","agentId":"child","isSidechain":true,"timestamp":"2026-10-01T00:00:01Z","message":{"role":"user","content":[{"type":"tool_result","content":key()}]}}))).unwrap();
    let found = document(&f.run(&parent, &["--json", "--session", "parent"]), 0);
    assert_eq!(found["transcripts"], 2);
    assert_eq!(found["findings"][0]["session"], "parent/child");
    assert_eq!(found["findings"][0]["where"], "tool_output");
    let partial = document(&f.run(&child, &["--json", "--session", "parent"]), 4);
    assert_eq!(partial["findings"].as_array().unwrap().len(), 1);
    assert_eq!(partial["complete"], false);
}

#[test]
fn secrets_scan_unknown_records_and_content_variants_instead_of_silently_dropping_them() {
    let f = Fixture::new();
    let path = f.file("future.jsonl", &[
        json!({"type":"session_meta","payload":{"id":"future","cwd":"/fixture"}}),
        json!({"timestamp":"2026-10-01T00:00:01Z","type":"future_record","data":key()}),
        json!({"timestamp":"2026-10-01T00:00:02Z","type":"response_item","payload":{"type":"message","role":"user","text":key()}}),
    ]);
    let found = document(&f.run(&path, &["--json"]), 0);
    assert_eq!(found["findings"].as_array().unwrap().len(), 2, "{found}");
}

/// A random-looking key of a known shape, not [`key`]: no placeholder.
fn other_key() -> String {
    ["gh", "p_", "Q7hB4mR9vT2kN8pS6xA3dF5jL1cE0uWzY4tR"].concat()
}

/// A finding is one of this machine's credentials in a transcript: every
/// other secret-shaped string is counted by rule, plain placeholders apart,
/// and never listed.
#[test]
fn only_this_machines_credentials_are_findings_and_the_rest_are_counted() {
    let f = Fixture::new();
    let mine = ["Zq8", "Wn3Rt6Yp1Ks4Vb7Xm2Lc5Hd9Fg0"].concat();
    let path = f.file("mixed.jsonl", &[
        json!({"type":"session_meta","payload":{"id":"mixed","cwd":"/fixture"}}),
        json!({"type":"response_item","timestamp":"2026-10-01T00:00:01Z","payload":{"type":"function_call_output","output":format!("token: {mine}")}}),
        json!({"type":"response_item","timestamp":"2026-10-01T00:00:02Z","payload":{"type":"function_call_output","output":other_key()}}),
        json!({"type":"response_item","timestamp":"2026-10-01T00:00:03Z","payload":{"type":"function_call_output","output":key()}}),
    ]);
    let out = f.run_with(&path, &["--json"], &[("DEPLOY_TOKEN", &mine)]);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    for value in [&mine, &other_key()] {
        assert!(
            !text.contains(value.as_str()),
            "a value left the reader: {text}"
        );
    }
    let doc = document(&out, 0);
    let rows = doc["findings"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{doc}");
    assert_eq!(rows[0]["held"], "environment variable DEPLOY_TOKEN");
    assert_eq!(rows[0]["line"], 2);
    assert_eq!(
        doc["candidates"],
        json!({"github": 1, "openai": 1}),
        "{doc}"
    );
    assert_eq!(doc["placeholders"], 1, "sk-a1a1… is a placeholder: {doc}");
    let read = doc["compared_with"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["source"] == "environment variable DEPLOY_TOKEN")
        .unwrap();
    assert_eq!(
        (read["status"].clone(), read["credentials"].clone()),
        (json!("read"), json!(1))
    );
    let plain = f.run_with(&path, &[], &[("DEPLOY_TOKEN", &mine)]);
    let said = String::from_utf8(plain.stdout).unwrap();
    assert!(
        said.contains("1 occurrence(s) of this machine's credentials"),
        "{said}"
    );
    assert!(
        said.contains("Not alarms: 2 other secret-shaped string(s)"),
        "{said}"
    );
    assert!(said.contains("(1 plainly placeholders)"), "{said}");
}

/// Nothing this machine holds appears: an all-clear, said as one, however
/// many other secret-shaped strings were counted.
#[test]
fn no_held_credential_in_the_transcripts_is_said_as_such() {
    let f = Fixture::new();
    let path = f.file("clear.jsonl", &[
        json!({"type":"session_meta","payload":{"id":"clear","cwd":"/fixture"}}),
        json!({"type":"response_item","timestamp":"2026-10-01T00:00:01Z","payload":{"type":"function_call_output","output":other_key()}}),
    ]);
    let doc = document(&f.run_with(&path, &["--json"], &[]), 0);
    assert_eq!(doc["findings"], json!([]));
    assert_eq!(doc["candidates"], json!({"github": 1}));
    let said = String::from_utf8(f.run_with(&path, &[], &[]).stdout).unwrap();
    assert!(
        said.starts_with(
            "None of this machine's credentials appears in 1 transcript(s) · scan complete"
        ),
        "{said}"
    );
}

/// The credential files under the home folder are compared; one that exists
/// but cannot be read leaves the scan partial, never an all-clear.
#[cfg(unix)]
#[test]
fn credential_files_are_compared_and_an_unreadable_one_leaves_the_scan_partial() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    std::fs::write(
        f.0.join(".git-credentials"),
        format!("https://x:{}@github.com\n", other_key()),
    )
    .unwrap();
    let path = f.file("files.jsonl", &[
        json!({"type":"session_meta","payload":{"id":"files","cwd":"/fixture"}}),
        json!({"type":"response_item","timestamp":"2026-10-01T00:00:01Z","payload":{"type":"function_call_output","output":other_key()}}),
    ]);
    let doc = document(&f.run_with(&path, &["--json"], &[]), 0);
    assert_eq!(doc["findings"][0]["held"], "~/.git-credentials", "{doc}");
    std::fs::write(f.0.join(".netrc"), "machine x password y\n").unwrap();
    std::fs::set_permissions(f.0.join(".netrc"), std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(f.0.join(".netrc")).is_ok() {
        return; // root reads it anyway: nothing to show here
    }
    let partial = document(&f.run_with(&path, &["--json"], &[]), 4);
    assert_eq!(partial["complete"], false);
    assert!(
        partial["unreadable"]
            .as_array()
            .unwrap()
            .iter()
            .any(|gap| gap["path"] == "~/.netrc"),
        "{partial}"
    );
}

/// The GitHub CLI's login is compared when gh is on PATH.
#[cfg(unix)]
#[test]
fn the_github_cli_login_is_compared_when_gh_is_on_path() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let bin = f.0.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let gh = bin.join("gh");
    std::fs::write(
        &gh,
        format!(
            "#!/bin/sh\n[ \"$1 $2\" = \"auth token\" ] && echo {}\n",
            other_key()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = f.file("gh.jsonl", &[
        json!({"type":"session_meta","payload":{"id":"gh","cwd":"/fixture"}}),
        json!({"type":"response_item","timestamp":"2026-10-01T00:00:01Z","payload":{"type":"message","role":"user","content":other_key()}}),
    ]);
    let doc = document(
        &f.run_with(&path, &["--json"], &[("PATH", bin.to_str().unwrap())]),
        0,
    );
    assert_eq!(
        doc["findings"][0]["held"], "GitHub CLI login (gh auth token)",
        "{doc}"
    );
    assert_eq!(doc["findings"][0]["where"], "person");
}
