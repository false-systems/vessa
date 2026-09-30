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
    let input = temp("escaped.jsonl");
    std::fs::write(
        &input,
        include_str!("../examples/claude.jsonl").replace("anthropic", "<script>untrusted</script>"),
    )
    .unwrap();
    let args = [
        "report",
        "--input",
        input.to_str().unwrap(),
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
    assert!(!html.contains("I already told you"));
    assert!(!html.contains("pattern candidates"));
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
    std::fs::remove_file(input).unwrap();
}

#[test]
fn usage_export_has_no_conversation_payload() {
    let v = value(cli(&[
        "usage",
        "--input",
        "examples/claude.jsonl",
        "--json",
    ]));
    assert_eq!(v["schema"], "vessa-usage.v1");
    assert!(v["sessions"][0].get("messages").is_none());
    assert!(!v.to_string().contains("I already told you"));
    assert!(!cli(&["patterns"]).status.success());
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

#[test]
fn calendar_statistics_use_usage_time_utc_and_preserve_unknowns() {
    let p = temp("calendar.jsonl");
    let records = [
        serde_json::json!({"type":"session","version":3,"id":"calendar"}),
        serde_json::json!({"type":"message","id":"one","timestamp":"2026-01-01T00:30:00+02:00","message":{"role":"assistant","provider":"test","model":"model","usage":{"input":10,"output":2,"cacheRead":0,"cacheWrite":0,"cost":{"total":1.25}}}}),
        serde_json::json!({"type":"message","id":"two","timestamp":"2026-01-01T01:00:00Z","message":{"role":"assistant","provider":"test","model":"model","usage":{"input":20,"output":3,"cacheRead":0,"cacheWrite":0}}}),
        serde_json::json!({"type":"message","id":"three","message":{"role":"assistant","provider":"test","model":"model","usage":{"input":30,"output":4,"cacheRead":0,"cacheWrite":0,"cost":{"total":0.5}}}}),
    ];
    std::fs::write(
        &p,
        records
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    let v = value(cli(&[
        "stats",
        "--period",
        "month",
        "--input",
        p.to_str().unwrap(),
        "--input",
        p.to_str().unwrap(),
        "--json",
    ]));
    assert_eq!(v["buckets"][0]["period"], "2025-12");
    assert_eq!(v["buckets"][0]["known_estimated_usd_micros"], 1_250_000);
    assert_eq!(v["buckets"][1]["period"], "2026-01");
    assert_eq!(v["buckets"][1]["unpriced_records"], 1);
    assert!(v["buckets"][1]["entries"][0]["estimated_usd_micros"].is_null());
    assert_eq!(v["buckets"][2]["period"], "undated");
    let v = value(cli(&[
        "stats",
        "--period",
        "week",
        "--input",
        p.to_str().unwrap(),
        "--json",
    ]));
    assert_eq!(v["buckets"][0]["period"], "2026-W01");
    assert_eq!(v["buckets"][0]["entries"][0]["input_tokens"], 30);
    let v = value(cli(&[
        "stats",
        "--period",
        "year",
        "--input",
        p.to_str().unwrap(),
        "--json",
    ]));
    assert_eq!(v["buckets"][0]["period"], "2025");
    assert_eq!(v["buckets"][1]["period"], "2026");
    std::fs::remove_file(p).unwrap();
}

#[test]
fn live_status_requires_identity_and_watch_is_bounded_when_requested() {
    assert!(!cli(&["status"]).status.success());
    let v = value(cli(&["status", "--input", "examples/pi.jsonl", "--json"]));
    assert_eq!(v["session_id"], "pi-demo");
    assert_eq!(v["entries"][0]["estimated_usd_micros"], 2200);
    let out = cli(&[
        "status",
        "--input",
        "examples/pi.jsonl",
        "--watch",
        "1",
        "--samples",
        "2",
    ]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout)
            .matches("last recorded")
            .count(),
        2
    );
    assert!(
        !cli(&["receipt", "--input", "examples/pi.jsonl", "--watch", "1"])
            .status
            .success()
    );
}

#[test]
fn native_claude_status_uses_estimate_not_limits_and_sanitizes_terminal_text() {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new(env!("CARGO_BIN_EXE_vessa"))
        .arg("claude-statusline")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let payload = serde_json::json!({"session_id":"demo","model":{"display_name":"Model\u{1b}[31m"},"cost":{"total_cost_usd":4.451911},"rate_limits":{"five_hour":{"used_percentage":88}}});
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("session estimate $4.45"));
    assert!(text.contains("billed unknown"));
    assert!(!text.contains('\u{1b}'));
    assert!(!text.contains("88") && !text.contains("limit"));
}
