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
    // Real Claude transcripts never name a provider; the source does.
    assert_eq!(v["entries"][0]["provider"], "anthropic");
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
        include_str!("../examples/claude.jsonl")
            .replace("demo-model", "<script>untrusted</script>"),
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
fn report_explains_partial_spending_and_command_evidence() {
    let path = temp("spending.html");
    let output = cli(&[
        "report",
        "--input",
        "examples/codex.jsonl",
        "--input",
        "examples/pi.jsonl",
        "--output",
        path.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let html = std::fs::read_to_string(&path).unwrap();
    assert!(html.contains("<strong>$0.002200</strong> known estimate subtotal"));
    assert!(html.contains("<strong>2/4</strong> usage records priced"));
    assert!(html.contains("<strong>2</strong> unpriced"));
    let ranking = html
        .split("Sessions by known estimate subtotal")
        .nth(1)
        .unwrap();
    assert!(ranking.find("pi-demo").unwrap() < ranking.find("codex-demo").unwrap());
    assert!(html.contains("Unpriced sessions can cost more"));
    assert!(html.contains("Command 1 ·"));
    assert!(html.contains("Uncached input</th>"));
    assert!(html.contains("Cached input read</th>"));
    assert!(html.contains("Savings are not measured here."));
    assert!(html.contains("Do not nest a second wrapper."));
    assert!(!html.contains("Please run the existing local checks"));
    std::fs::remove_file(path).unwrap();
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
    assert_eq!(v["sessions"][0]["totals"][0]["input_tokens"], 1200);
    assert!(v["sessions"][0]["totals"][0]["estimated_usd_micros"].is_null());
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
    let payload = serde_json::json!({"session_id":"demo","model":{"display_name":"Model\u{1b}[31m"},"cost":{"total_cost_usd":4.451911},"context_window":{"current_usage":{"input_tokens":120,"output_tokens":35,"cache_read_input_tokens":400,"cache_creation_input_tokens":null}},"rate_limits":{"five_hour":{"used_percentage":88}}});
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
    assert!(text.starts_with("session estimate $4.45"));
    assert!(text
        .contains("Last API call: input 120 · output 35 · cache read 400 · cache write unknown"));
    assert!(!text.contains('\u{1b}'));
    assert!(!text.contains("88") && !text.contains("limit"));
}

#[test]
fn native_directory_metadata_is_not_misread_as_an_export() {
    let dir = temp("native-dir");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(
        dir.join("session.jsonl"),
        include_str!("../examples/pi.jsonl"),
    )
    .unwrap();
    std::fs::write(dir.join("session.meta.json"), "{\"agentType\":\"worker\"}").unwrap();
    let v = value(cli(&["stats", "--input", dir.to_str().unwrap(), "--json"]));
    assert!(v["warnings"].as_array().unwrap().is_empty());
    assert_eq!(v["buckets"][0]["known_estimated_usd_micros"], 2200);
    assert_eq!(
        cli(&[
            "usage",
            "--input",
            dir.join("session.meta.json").to_str().unwrap()
        ])
        .status
        .code(),
        Some(4)
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn command_change_and_waste_windows_preserve_totals_unknowns_and_privacy() {
    let v = value(cli(&["status", "--input", "examples/work.jsonl", "--json"]));
    assert_eq!(v["entries"][0]["estimated_usd_micros"], 7000);
    let insights = &v["insights"];
    assert_eq!(insights["initial_request"][0]["estimated_usd_micros"], 2000);
    assert_eq!(insights["commands"].as_array().unwrap().len(), 2);
    let first = &insights["commands"][0];
    assert_eq!(first["entries"][0]["estimated_usd_micros"], 3000);
    assert_eq!(first["elapsed_seconds"], 20);
    assert_eq!(first["failed_tools"], 1);
    assert_eq!(
        first["changes"][0]["entries"][0]["estimated_usd_micros"],
        3000
    );
    assert_eq!(first["waste_upper_bound_usd_micros"], 3000);
    assert!(insights["commands"][1]["waste_upper_bound_usd_micros"].is_null());
    assert!(insights["unattributed"].as_array().unwrap().is_empty());
    let export = value(cli(&["usage", "--input", "examples/work.jsonl", "--json"]));
    assert!(!export.to_string().contains("PRIVATE"));
    let input = temp("unpriced-work.jsonl");
    std::fs::write(
        &input,
        include_str!("../examples/work.jsonl").replace(",\"cost\":{\"total\":0.002}", ""),
    )
    .unwrap();
    let v = value(cli(&[
        "status",
        "--input",
        input.to_str().unwrap(),
        "--json",
    ]));
    assert!(v["insights"]["commands"][0]["waste_upper_bound_usd_micros"].is_null());
    std::fs::remove_file(input).unwrap();
    let html = temp("work.html");
    assert!(cli(&[
        "report",
        "--input",
        "examples/work.jsonl",
        "--output",
        html.to_str().unwrap()
    ])
    .status
    .success());
    let report = std::fs::read_to_string(&html).unwrap();
    assert!(report.contains("Command 1") && report.contains("Change 1"));
    assert!(report.contains("$0 to $0.003000"));
    assert!(!report.contains("PRIVATE"));
    std::fs::remove_file(html).unwrap();
}

#[test]
fn native_tool_results_are_not_user_commands_and_unknown_results_are_not_changes() {
    let input = temp("claude-work.jsonl");
    let mut rows: Vec<Value> = include_str!("../examples/claude.jsonl")
        .lines()
        .take(2)
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    rows[1]["message"]["content"] = serde_json::json!([
        {"type":"tool_use","id":"edit1","name":"Edit"}
    ]);
    rows.push(serde_json::json!({"type":"user","sessionId":"claude-demo","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"edit1","content":"PRIVATE RESULT"}]}}));
    // Repeated native result must not double-count a change.
    rows.push(rows[2].clone());
    std::fs::write(
        &input,
        rows.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    let v = value(cli(&[
        "status",
        "--input",
        input.to_str().unwrap(),
        "--json",
    ]));
    assert_eq!(v["insights"]["commands"].as_array().unwrap().len(), 1);
    assert_eq!(
        v["insights"]["commands"][0]["changes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    std::fs::remove_file(input).unwrap();

    let input = temp("codex-work.jsonl");
    let original = include_str!("../examples/codex.jsonl");
    let call = serde_json::json!({"type":"response_item","payload":{"type":"custom_tool_call","call_id":"edit1","name":"apply_patch","input":"PRIVATE PATCH"}});
    for (output, expected) in [
        (serde_json::json!({"metadata":{"exit_code":0}}), 1),
        (serde_json::json!("unrecognized result"), 0),
    ] {
        let result = serde_json::json!({"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"edit1","output":output.to_string()}});
        std::fs::write(&input, format!("{original}\n{call}\n{result}\n")).unwrap();
        let v = value(cli(&[
            "status",
            "--input",
            input.to_str().unwrap(),
            "--json",
        ]));
        assert_eq!(
            v["insights"]["commands"][0]["changes"]
                .as_array()
                .unwrap()
                .len(),
            expected
        );
        assert_eq!(v["entries"][0]["input_tokens"], 800);
    }
    std::fs::remove_file(input).unwrap();
}

fn write_lines(path: &PathBuf, rows: &[Value]) {
    std::fs::write(
        path,
        rows.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
}

#[test]
fn claude_subagent_snapshots_keep_the_final_output_count() {
    // Claude Code writes one line per content block. In subagent transcripts
    // the output count grows across the lines of one request (3, 3, 644).
    let output = cli(&[
        "usage",
        "--input",
        "examples/claude-subagent.jsonl",
        "--json",
    ]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let v: Value = serde_json::from_slice(&output.stdout).unwrap();
    let session = &v["sessions"][0];
    assert_eq!(session["id"], "claude-demo/subagent-demo");
    assert_eq!(session["totals"][0]["output_tokens"], 644 + 731);
    assert_eq!(session["totals"][0]["input_tokens"], 6);
    assert_eq!(session["totals"][0]["cache_read_tokens"], 2500);
    assert_eq!(session["totals"][0]["cache_write_tokens"], 80);
    // One record per request, pointing at the line that carries its count.
    let usage = session["usage"].as_array().unwrap();
    assert_eq!(usage.len(), 2);
    assert_eq!(usage[0]["line"], 4);
    assert_eq!(usage[1]["line"], 7);
    assert!(session["limitations"].as_array().unwrap().is_empty());

    // A copy holding only an earlier, smaller snapshot is not a conflict.
    let copy = temp("subagent-copy.jsonl");
    std::fs::write(
        &copy,
        include_str!("../examples/claude-subagent.jsonl")
            .lines()
            .take(2)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    let v = value(cli(&[
        "usage",
        "--input",
        "examples/claude-subagent.jsonl",
        "--input",
        copy.to_str().unwrap(),
        "--json",
    ]));
    assert_eq!(v["sessions"][0]["totals"][0]["output_tokens"], 1375);
    std::fs::remove_file(copy).unwrap();
}

#[test]
fn disagreeing_claude_snapshots_are_flagged_not_merged() {
    // Same request, but the inputs differ: that is a true conflict, not growth.
    let path = temp("conflict.jsonl");
    let mut rows: Vec<Value> = include_str!("../examples/claude-subagent.jsonl")
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    rows[3]["message"]["usage"]["cache_read_input_tokens"] = 901.into();
    write_lines(&path, &rows);
    let output = cli(&["usage", "--input", path.to_str().unwrap(), "--json"]);
    assert_eq!(output.status.code(), Some(4));
    let v: Value = serde_json::from_slice(&output.stdout).unwrap();
    let session = &v["sessions"][0];
    assert!(session["limitations"][0]
        .as_str()
        .unwrap()
        .contains("conflicting usage request-sub-1"));
    // The first record is retained; the conflicting one is not summed.
    assert_eq!(session["totals"][0]["output_tokens"], 3 + 731);
    assert_eq!(session["totals"][0]["cache_read_tokens"], 2500);

    // A shrinking count is not growth either when the inputs also change.
    rows[3]["message"]["usage"]["cache_read_input_tokens"] = 900.into();
    rows[2]["message"]["usage"]["input_tokens"] = 5.into();
    write_lines(&path, &rows);
    assert_eq!(
        cli(&["usage", "--input", path.to_str().unwrap()])
            .status
            .code(),
        Some(4)
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn codex_provider_is_inferred_and_unused_rate_cards_warn() {
    // Without model_provider, a Codex transcript is still OpenAI's.
    let path = temp("codex-no-provider.jsonl");
    std::fs::write(
        &path,
        include_str!("../examples/codex.jsonl").replace(",\"model_provider\":\"openai\"", ""),
    )
    .unwrap();
    let v = value(cli(&[
        "receipt",
        "--input",
        path.to_str().unwrap(),
        "--rates",
        "examples/rates.json",
    ]));
    assert_eq!(v["entries"][0]["provider"], "openai");
    assert_eq!(v["entries"][0]["estimated_usd_micros"], 3740);
    std::fs::remove_file(path).unwrap();

    // A rate card that prices nothing is named, not silently ignored.
    let rates = temp("unused-rates.json");
    let mut cards: Vec<Value> =
        serde_json::from_str(include_str!("../examples/rates.json")).unwrap();
    let mut unused = cards[0].clone();
    unused["provider"] = "nobody".into();
    cards.push(unused);
    std::fs::write(&rates, serde_json::to_vec(&cards).unwrap()).unwrap();
    let output = cli(&[
        "usage",
        "--input",
        "examples/claude.jsonl",
        "--rates",
        rates.to_str().unwrap(),
        "--json",
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let v: Value = serde_json::from_slice(&output.stdout).unwrap();
    let warnings = v["warnings"].as_array().unwrap();
    // The Claude card priced the session; the OpenAI and "nobody" cards did not.
    assert_eq!(v["sessions"][0]["totals"][0]["estimated_usd_micros"], 8715);
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings.iter().any(|w| w
        .as_str()
        .unwrap()
        .starts_with("rate card nobody/demo-model")));
    assert!(warnings
        .iter()
        .all(|w| w.as_str().unwrap().contains("priced no usage records")));
    assert!(stderr.contains("rate card nobody/demo-model"));
    // A pricing notice does not make the usage evidence partial.
    assert_eq!(output.status.code(), Some(0));
    std::fs::remove_file(rates).unwrap();
}

#[test]
fn full_runs_are_partial_only_for_real_gaps() {
    // Every realistic fixture together, as a full run reads them: complete.
    let output = cli(&["usage", "--input", "examples", "--json"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    let v: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(v["warnings"].as_array().unwrap().is_empty());
    assert_eq!(v["sessions"].as_array().unwrap().len(), 5);
    for session in v["sessions"].as_array().unwrap() {
        assert!(session["limitations"].as_array().unwrap().is_empty());
    }
    assert!(stderr.is_empty(), "{stderr}");

    // A real gap: Claude falls back to another model inside one request and
    // repeats the earlier counts, so the fallback's usage is unknown.
    let path = temp("fallback.jsonl");
    let mut rows: Vec<Value> = include_str!("../examples/claude-subagent.jsonl")
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    rows[2]["message"]["model"] = "fallback-model".into();
    write_lines(&path, &rows);
    let output = cli(&["usage", "--input", path.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(4));
    // The limitation says what disagreed, and the run says why it is partial.
    assert!(
        stderr.contains("conflicting usage request-sub-1 (first seen in claude-demo/subagent-demo): model demo-model then fallback-model"),
        "{stderr}"
    );
    assert!(
        stderr.trim_end().ends_with(
            "vessa: partial usage evidence (exit 4): 1 session with limitations, 0 source warnings"
        ),
        "{stderr}"
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn codex_cache_writes_are_read_when_present() {
    // Newer Codex counters also report cache writes. Like cached reads, they
    // are part of the inclusive input count.
    let path = temp("codex-cache-write.jsonl");
    let total = |input: u64, cached: u64, written: u64, output: u64| serde_json::json!({"input_tokens":input,"cached_input_tokens":cached,"cache_write_input_tokens":written,"output_tokens":output,"reasoning_output_tokens":0,"total_tokens":input + output});
    let rows = [
        serde_json::json!({"type":"session_meta","payload":{"id":"codex-writes","cwd":"/demo/widget","model_provider":"openai","source":"cli"}}),
        serde_json::json!({"type":"turn_context","payload":{"model":"demo-codex"}}),
        serde_json::json!({"timestamp":"2026-09-30T10:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":total(1000, 400, 100, 100)}}}),
        serde_json::json!({"timestamp":"2026-09-30T10:02:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":total(1500, 700, 150, 200)}}}),
    ];
    write_lines(&path, &rows);
    let v = value(cli(&["receipt", "--input", path.to_str().unwrap()]));
    assert_eq!(v["entries"][0]["input_tokens"], 1500 - 700 - 150);
    assert_eq!(v["entries"][0]["cache_read_tokens"], 700);
    assert_eq!(v["entries"][0]["cache_write_tokens"], 150);
    assert_eq!(v["entries"][0]["output_tokens"], 200);

    // A child session's first counter excludes inherited writes as well.
    let mut child = rows.to_vec();
    child[0]["payload"]["source"] = serde_json::json!({"subagent":"review"});
    child[2]["payload"]["info"]["last_token_usage"] = total(100, 40, 10, 10);
    child.truncate(3);
    write_lines(&path, &child);
    let v = value(cli(&["receipt", "--input", path.to_str().unwrap()]));
    assert_eq!(v["entries"][0]["input_tokens"], 100 - 40 - 10);
    assert_eq!(v["entries"][0]["cache_write_tokens"], 10);

    // Older Codex omits the field; OpenAI caching has no separate write.
    let v = value(cli(&["receipt", "--input", "examples/codex.jsonl"]));
    assert_eq!(v["entries"][0]["cache_write_tokens"], 0);
    std::fs::remove_file(path).unwrap();
}
