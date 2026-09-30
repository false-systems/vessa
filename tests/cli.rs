use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vessa"))
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap()
}
fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "vessa-test-{}-{}-{name}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}
fn value(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn native_usage_is_not_double_counted_and_costs_are_not_bills() {
    let v = value(cli(&[
        "receipt",
        "--input",
        "examples/claude.jsonl",
        "--rates",
        "examples/rates.json",
    ]));
    assert_eq!(v["entries"][0]["input_tokens"], 1200);
    assert_eq!(v["entries"][0]["estimated_usd_micros"], 8715);
    assert!(v["entries"][0]["billed"].is_null());
    let v = value(cli(&["receipt", "--input", "examples/codex.jsonl"]));
    assert_eq!(v["entries"][0]["input_tokens"], 800);
    assert_eq!(v["entries"][0]["cache_read_tokens"], 700);
    assert_eq!(v["entries"][0]["output_tokens"], 200);
    assert!(v["entries"][0]["estimated_usd_micros"].is_null());
    let v = value(cli(&["receipt", "--input", "examples/pi.jsonl"]));
    assert_eq!(v["entries"][0]["input_tokens"], 220);
    assert_eq!(v["entries"][0]["estimated_usd_micros"], 2200);
}

#[test]
fn malformed_data_is_partial_not_empty_success() {
    let path = temp("broken.jsonl");
    std::fs::write(
        &path,
        format!("{}\n{{broken\n", include_str!("../examples/pi.jsonl")),
    )
    .unwrap();
    let output = cli(&["usage", "--input", path.to_str().unwrap(), "--json"]);
    assert_eq!(output.status.code(), Some(4));
    let v: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v["sessions"][0]["usage"].as_array().unwrap().len(), 2);
    assert!(v["sessions"][0]["limitations"][0]
        .as_str()
        .unwrap()
        .contains("malformed"));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn html_is_private_escaped_and_evidence_links_resolve() {
    let path = temp("report.html");
    let args = [
        "report",
        "--input",
        "examples/claude.jsonl",
        "--input",
        "examples/codex.jsonl",
        "--output",
        path.to_str().unwrap(),
    ];
    let output = cli(&args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let html = std::fs::read_to_string(&path).unwrap();
    assert!(!html.contains("<script>"));
    assert!(html.contains("&lt;script&gt;"));
    assert!(html.contains("possible correction"));
    assert!(html.contains("repeated user text"));
    for part in html.split("href=\"#").skip(1) {
        let target = part.split('"').next().unwrap();
        assert!(
            html.contains(&format!("id=\"{target}\"")),
            "broken link: {target}"
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(!cli(&args).status.success());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn forked_pi_history_is_not_new_spending() {
    let path = temp("fork.jsonl");
    let original = include_str!("../examples/pi.jsonl");
    let fork = original.replacen(
        "\"id\":\"pi-demo\"",
        "\"id\":\"pi-child\",\"parentSession\":\"pi-demo\"",
        1,
    );
    std::fs::write(&path, fork).unwrap();
    let output = cli(&[
        "usage",
        "--input",
        "examples/pi.jsonl",
        "--input",
        path.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(4));
    let v: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        v["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["usage"].as_array().unwrap().len())
            .sum::<usize>(),
        2
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn missing_counts_are_not_free_and_rates_cannot_overlap() {
    let path = temp("missing.jsonl");
    std::fs::write(
        &path,
        include_str!("../examples/pi.jsonl").replace("\"cacheRead\":400,", ""),
    )
    .unwrap();
    let v = value(cli(&["receipt", "--input", path.to_str().unwrap()]));
    assert!(v["entries"][0]["cache_read_tokens"].is_null());
    std::fs::remove_file(path).unwrap();
    let path = temp("rates.json");
    let rates: Value = serde_json::from_str(include_str!("../examples/rates.json")).unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec(&vec![&rates[0], &rates[0]]).unwrap(),
    )
    .unwrap();
    assert!(!cli(&[
        "usage",
        "--input",
        "examples/claude.jsonl",
        "--rates",
        path.to_str().unwrap()
    ])
    .status
    .success());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn child_counters_exclude_inherited_context() {
    let path = temp("child.jsonl");
    let records = [
        serde_json::json!({"type":"session_meta","payload":{"id":"child","timestamp":"2026-09-30T12:00:00Z","source":{"subagent":"review"},"model_provider":"openai"}}),
        serde_json::json!({"type":"turn_context","payload":{"model":"test"}}),
        serde_json::json!({"timestamp":"2026-09-30T12:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":400,"output_tokens":100},"last_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":10}}}}),
    ];
    std::fs::write(
        &path,
        records
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    let v = value(cli(&["receipt", "--input", path.to_str().unwrap()]));
    assert_eq!(v["entries"][0]["input_tokens"], 60);
    assert_eq!(v["entries"][0]["output_tokens"], 10);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn unrelated_pi_sessions_can_reuse_short_message_ids() {
    let path = temp("independent.jsonl");
    std::fs::write(
        &path,
        include_str!("../examples/pi.jsonl").replacen("pi-demo", "pi-independent", 1),
    )
    .unwrap();
    let v = value(cli(&[
        "usage",
        "--by-agent",
        "--input",
        "examples/pi.jsonl",
        "--input",
        path.to_str().unwrap(),
        "--json",
    ]));
    assert_eq!(v["pi"][0]["input_tokens"], 440);
    assert_eq!(v["pi"][0]["estimated_usd_micros"], 4400);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn ohjaa_links_require_explicit_identity_and_agent_totals_stay_deduplicated() {
    let receipt = value(cli(&["receipt", "--input", "examples/pi.jsonl"]));
    let path = temp("ohjaa.json");
    let export = serde_json::json!({"schema":"ohjaa-sessions.v1","sessions":[{"id":"oh-fixture","agent":"pi","intent":"verify a parser","usage":[receipt]}],"unreadable":[]});
    std::fs::write(&path, serde_json::to_vec(&export).unwrap()).unwrap();
    let v = value(cli(&[
        "sessions",
        "--input",
        "examples/pi.jsonl",
        "--input",
        path.to_str().unwrap(),
        "--json",
    ]));
    assert_eq!(v["sessions"][0]["ohjaa"][0]["id"], "oh-fixture");
    let v = value(cli(&[
        "usage",
        "--by-agent",
        "--input",
        "examples/pi.jsonl",
        "--input",
        "examples/pi.jsonl",
        "--json",
    ]));
    assert_eq!(v["pi"][0]["estimated_usd_micros"], 2200);
    std::fs::remove_file(path).unwrap();
}
