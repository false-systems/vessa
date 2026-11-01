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
    fn run(&self, path: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_vessa"))
            .args(["secrets", "--input"])
            .arg(path)
            .args(args)
            .output()
            .unwrap()
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
        let value = document(&f.run(&path, &["--json"]), 0);
        let rows = value["findings"].as_array().unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| row["count"].as_u64().unwrap())
                .sum::<u64>(),
            if value["transcripts"] == 1 {
                2
            } else {
                unreachable!()
            }
        );
        let opaque_count: u64 = rows
            .iter()
            .map(|row| {
                row["opaque_count"]
                    .as_u64()
                    .expect("every new finding reports context coverage")
            })
            .sum();
        assert_eq!(
            opaque_count,
            if path.ends_with("opaque-claude.jsonl") {
                1
            } else {
                2
            }
        );
        assert!(rows.iter().any(|row| row["rule"] == "openai"));
        assert!(!String::from_utf8_lossy(&f.run(&path, &["--json"]).stdout).contains(opaque));
    }
}

#[test]
fn nested_file_history_times_are_honored_without_inventing_times_for_unknown_records() {
    let f = Fixture::new();
    let path = f.file("history.jsonl", &[
        json!({"type":"user","sessionId":"history","cwd":"/fixture","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"Synthetic header"}}),
        json!({"type":"file-history-snapshot","snapshot":{"timestamp":"2026-09-30T00:00:00Z","data":key()}}),
        json!({"type":"file-history-snapshot","snapshot":{"timestamp":"2026-10-01T00:00:01Z","data":key()}}),
        json!({"type":"future_record","snapshot":{"timestamp":"2026-10-01T00:00:01Z"},"data":key()})
    ]);
    let value = document(
        &f.run(&path, &["--json", "--since", "2026-10-01T00:00:00Z"]),
        4,
    );
    let rows = value["findings"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{value}");
    assert_eq!(rows.iter().filter(|row| row["at"].is_null()).count(), 1);
    assert_eq!(value["undated_records"], 1);
    assert_eq!(value["unreadable"].as_array().unwrap().len(), 1);
    assert_eq!(value["complete"], false);
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
        assert!(text.contains("4 secret-shaped occurrence(s)"), "{text}");
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
    assert_eq!(value["unreadable"].as_array().unwrap().len(), 3, "{value}");
    assert!(value["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["where"] == "tool_output" && v["at"].is_null()));
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
        .env("HOME", &f.0)
        .env("XDG_DATA_HOME", f.0.join("data"))
        .output()
        .unwrap();
    let value = document(&out, 4);
    assert_eq!(value["unreadable"].as_array().unwrap().len(), 1);
    assert!(value["unreadable"][0]["why"]
        .as_str()
        .unwrap()
        .contains("SQLite"));
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
