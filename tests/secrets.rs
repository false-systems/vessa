use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "vessa-secrets-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
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
