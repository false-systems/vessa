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
        .contains("conflicting usage req_011CdemoSub1"));
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
        stderr.contains("conflicting usage req_011CdemoSub1 (first seen in claude-demo/subagent-demo): model demo-model then fallback-model"),
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
    // Newer Codex counters also report cache writes. Vessa assumes that, like
    // cached reads, they are part of the inclusive input count. Every real
    // value seen so far is zero, so a non-zero one is reported as unconfirmed.
    let path = temp("codex-cache-write.jsonl");
    let total = |input: u64, cached: u64, written: u64, output: u64| serde_json::json!({"input_tokens":input,"cached_input_tokens":cached,"cache_write_input_tokens":written,"output_tokens":output,"reasoning_output_tokens":0,"total_tokens":input + output});
    let rows = [
        serde_json::json!({"type":"session_meta","payload":{"id":"codex-writes","cwd":"/demo/widget","model_provider":"openai","source":"cli"}}),
        serde_json::json!({"type":"turn_context","payload":{"model":"demo-codex"}}),
        serde_json::json!({"timestamp":"2026-09-30T10:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":total(1000, 400, 100, 100)}}}),
        serde_json::json!({"timestamp":"2026-09-30T10:02:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":total(1500, 700, 150, 200)}}}),
    ];
    let unconfirmed = |path: &PathBuf| {
        let output = cli(&["receipt", "--input", path.to_str().unwrap()]);
        assert_eq!(output.status.code(), Some(4));
        let v: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(
            v["limitations"]
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l.as_str().unwrap().contains("cache_write_input_tokens")),
            "{v}"
        );
        v
    };
    write_lines(&path, &rows);
    let v = unconfirmed(&path);
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
    let v = unconfirmed(&path);
    assert_eq!(v["entries"][0]["input_tokens"], 100 - 40 - 10);
    assert_eq!(v["entries"][0]["cache_write_tokens"], 10);

    // Zero writes, as every real counter reports so far, are complete.
    let mut zero = rows.to_vec();
    zero[2]["payload"]["info"]["total_token_usage"] = total(1000, 400, 0, 100);
    zero[3]["payload"]["info"]["total_token_usage"] = total(1500, 700, 0, 200);
    write_lines(&path, &zero);
    let v = value(cli(&["receipt", "--input", path.to_str().unwrap()]));
    assert_eq!(v["entries"][0]["input_tokens"], 800);
    assert_eq!(v["entries"][0]["cache_write_tokens"], 0);

    // Older Codex omits the field; OpenAI caching has no separate write.
    let v = value(cli(&["receipt", "--input", "examples/codex.jsonl"]));
    assert_eq!(v["entries"][0]["cache_write_tokens"], 0);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn claude_provider_comes_from_response_ids_not_the_source() {
    // Claude Code also runs on Bedrock, Vertex and Anthropic-compatible
    // gateways with the same transcript shape. Only the ids tell them apart.
    let path = temp("claude-provider.jsonl");
    for (message_id, request_id, expected) in [
        (
            "msg_011CdemoMessage1",
            Some("req_011CdemoRequest1"),
            "anthropic",
        ),
        ("msg_011CdemoMessage1", None, "anthropic"),
        ("msg_bdrk_01demoMessage1", None, "bedrock"),
        ("msg_vrtx_01demoMessage1", None, "vertex"),
        ("chatcmpl-demo1", None, "unknown"),
        ("gen-demo1", Some("gateway-request-1"), "unknown"),
    ] {
        let mut row = serde_json::json!({"type":"assistant","uuid":"a1","sessionId":"claude-demo","timestamp":"2026-09-30T09:01:00Z","message":{"id":message_id,"role":"assistant","model":"demo-model","content":[{"type":"text","text":"Done."}],"usage":{"input_tokens":1200,"output_tokens":300,"cache_read_input_tokens":800,"cache_creation_input_tokens":100}}});
        if let Some(request_id) = request_id {
            row["requestId"] = request_id.into();
        }
        write_lines(&path, &[row]);
        let output = cli(&[
            "receipt",
            "--input",
            path.to_str().unwrap(),
            "--rates",
            "examples/rates.json",
        ]);
        let v: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(v["entries"][0]["provider"], expected, "{message_id}");
        // Only Anthropic's own usage meets the anthropic rate card.
        assert_eq!(
            v["entries"][0]["estimated_usd_micros"].is_null(),
            expected != "anthropic",
            "{message_id}"
        );
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn non_openai_codex_providers_are_never_priced_as_openai() {
    // Codex can run against other providers; their usage must not meet an
    // OpenAI rate card just because Codex is OpenAI's agent.
    let path = temp("codex-ollama.jsonl");
    std::fs::write(
        &path,
        include_str!("../examples/codex.jsonl").replace(
            "\"model_provider\":\"openai\"",
            "\"model_provider\":\"ollama\"",
        ),
    )
    .unwrap();
    let output = cli(&[
        "usage",
        "--input",
        path.to_str().unwrap(),
        "--rates",
        "examples/rates.json",
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&output.stdout).unwrap();
    let totals = &v["sessions"][0]["totals"][0];
    assert_eq!(totals["provider"], "ollama");
    assert_eq!(totals["model"], "demo-codex");
    assert!(totals["estimated_usd_micros"].is_null());
    // The OpenAI card matched nothing here, and says so.
    assert!(v["warnings"].as_array().unwrap().iter().any(|w| w
        .as_str()
        .unwrap()
        .starts_with("rate card openai/demo-codex")));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn a_session_says_how_it_was_started_and_is_dated_past_untimed_header_records() {
    // Current Claude Code transcripts open with records that carry the
    // session id but no time and no folder; `claude -p` marks its records
    // `entrypoint: sdk-cli`. `codex exec` names itself as the originator.
    let claude = temp("headless.jsonl");
    std::fs::write(
        &claude,
        concat!(
            r#"{"type":"last-prompt","sessionId":"claude-headless","leafUuid":"x"}"#,
            "\n",
            r#"{"type":"mode","sessionId":"claude-headless","mode":"normal"}"#,
            "\n",
            r#"{"type":"user","uuid":"u1","sessionId":"claude-headless","cwd":"/demo/widget","entrypoint":"sdk-cli","timestamp":"2026-09-30T09:00:00Z","message":{"role":"user","content":"draft"}}"#,
            "\n",
        ),
    )
    .unwrap();
    let codex = temp("exec.jsonl");
    std::fs::write(
        &codex,
        concat!(
            r#"{"type":"session_meta","payload":{"id":"codex-exec","cwd":"/demo/widget","originator":"codex_exec","source":"exec","timestamp":"2026-09-30T10:00:00Z"}}"#,
            "\n",
        ),
    )
    .unwrap();
    let output = cli(&[
        "sessions",
        "--json",
        "--input",
        claude.to_str().unwrap(),
        "--input",
        codex.to_str().unwrap(),
    ]);
    let v: Value = serde_json::from_slice(&output.stdout).unwrap();
    let session = |id: &str| {
        v["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == id)
            .unwrap()
            .clone()
    };
    let claude_session = session("claude-headless");
    assert_eq!(claude_session["started_at"], "2026-09-30T09:00:00Z");
    assert_eq!(claude_session["cwd"], "/demo/widget");
    assert_eq!(claude_session["entrypoint"], "sdk-cli");
    assert!(claude_session["originator"].is_null());
    let codex_session = session("codex-exec");
    assert_eq!(codex_session["originator"], "codex_exec");
    assert!(codex_session["entrypoint"].is_null());
    // Unknown is said as null, never left out: a reader can tell an older
    // vessa from a session that did not say.
    let v = value(cli(&["sessions", "--json", "--input", "examples/pi.jsonl"]));
    let pi = v["sessions"][0].as_object().unwrap();
    assert!(pi.contains_key("entrypoint") && pi.contains_key("originator"));
}

/// The record Claude Code writes in the main transcript when it launches a
/// subagent: a tool result whose `toolUseResult` names the subagent's id.
const LAUNCH: &str = r#"{"type":"user","uuid":"u4","sessionId":"claude-demo","timestamp":"2026-09-30T09:10:30Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"task1","content":"Agent launched."}]},"toolUseResult":{"isAsync":true,"status":"async_launched","agentId":"subagent-demo"}}"#;

/// A Claude session laid out as Claude Code writes it: the main transcript at
/// `<dir>/claude-demo.jsonl`, and each subagent's transcript beside it in
/// `<dir>/claude-demo/subagents/agent-<id>.jsonl`.
fn claude_family(name: &str, with_subagent: bool) -> PathBuf {
    let dir = temp(name);
    std::fs::create_dir_all(dir.join("claude-demo/subagents")).unwrap();
    std::fs::write(
        dir.join("claude-demo.jsonl"),
        format!("{}{LAUNCH}\n", include_str!("../examples/claude.jsonl")),
    )
    .unwrap();
    if with_subagent {
        std::fs::write(
            dir.join("claude-demo/subagents/agent-subagent-demo.jsonl"),
            include_str!("../examples/claude-subagent.jsonl"),
        )
        .unwrap();
    }
    dir
}

#[test]
fn a_claude_receipt_includes_the_sessions_subagents_and_says_so() {
    // Main transcript: in 1200, out 300, cache read 800, cache write 100.
    // Its subagent: in 6, out 1375, cache read 2500, cache write 80.
    let dir = claude_family("family-receipt", true);
    let main = dir.join("claude-demo.jsonl");
    for args in [
        // Looked up by the parent id, as consumers join on it.
        vec!["--input", dir.to_str().unwrap(), "--session", "claude-demo"],
        // The main transcript alone: its subagents live beside it.
        vec!["--input", main.to_str().unwrap()],
    ] {
        let mut argv = vec!["receipt", "--rates", "examples/rates.json"];
        argv.extend(&args);
        let output = cli(&argv);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let v: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(v["session_id"], "claude-demo", "{args:?}");
        let entries = v["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1, "{args:?}");
        assert_eq!(entries[0]["input_tokens"], 1200 + 6);
        assert_eq!(entries[0]["output_tokens"], 300 + 1375);
        assert_eq!(entries[0]["cache_read_tokens"], 800 + 2500);
        assert_eq!(entries[0]["cache_write_tokens"], 100 + 80);
        // 8715 for the main transcript, 10130 + 11564 for the subagent.
        assert_eq!(entries[0]["estimated_usd_micros"], 8715 + 10130 + 11564);
        assert!(
            v["limitations"].as_array().unwrap().iter().any(|l| {
                let l = l.as_str().unwrap();
                l.contains("1 Claude subagent") && l.contains("claude-demo/subagent-demo")
            }),
            "{args:?}: {v}"
        );
        // ohjaa refuses a receipt with a field it does not know.
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "agent",
                "complete",
                "entries",
                "limitations",
                "observed_at_ms",
                "schema",
                "session_id",
                "source"
            ]
        );
    }
    // A subagent can still be asked for on its own.
    let v = value(cli(&[
        "receipt",
        "--input",
        dir.to_str().unwrap(),
        "--session",
        "claude-demo/subagent-demo",
    ]));
    assert_eq!(v["session_id"], "claude-demo/subagent-demo");
    assert_eq!(v["entries"][0]["output_tokens"], 1375);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn claude_status_totals_include_subagents_with_a_breakdown() {
    let dir = claude_family("family-status", true);
    let args = [
        "status",
        "--input",
        dir.to_str().unwrap(),
        "--session",
        "claude-demo",
    ];
    let mut json_args = args.to_vec();
    json_args.push("--json");
    let v = value(cli(&json_args));
    assert_eq!(v["session_id"], "claude-demo");
    assert_eq!(v["entries"][0]["input_tokens"], 1206);
    assert_eq!(v["entries"][0]["output_tokens"], 1675);
    assert_eq!(v["main_entries"][0]["output_tokens"], 300);
    let subagents = v["subagents"].as_array().unwrap();
    assert_eq!(subagents.len(), 1);
    assert_eq!(subagents[0]["id"], "claude-demo/subagent-demo");
    assert_eq!(subagents[0]["entries"][0]["output_tokens"], 1375);
    assert_eq!(
        subagents[0]["last_recorded_at"],
        "2026-09-30T09:10:20+00:00"
    );
    // The last record of the session is the subagent's, after the main one's.
    assert_eq!(v["last_recorded_at"], "2026-09-30T09:10:20+00:00");

    let out = cli(&args);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("totals include 1 subagent"), "{text}");
    assert!(text.contains("in=1206 out=1675"), "{text}");
    assert!(text.contains("main transcript"), "{text}");
    assert!(
        text.contains("subagent claude-demo/subagent-demo"),
        "{text}"
    );
    assert!(text.contains("in=6 out=1375"), "{text}");

    // A Claude session without subagents says how many it counted.
    let out = cli(&["status", "--input", "examples/claude.jsonl"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("no subagent transcripts read"));
    // A subagent asked for by its own id is shown alone, and says whose it is.
    let out = cli(&[
        "status",
        "--input",
        dir.to_str().unwrap(),
        "--session",
        "claude-demo/subagent-demo",
    ]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("subagent of claude-demo"), "{text}");
    assert!(text.contains("in=6 out=1375"), "{text}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_launched_subagent_whose_transcript_was_not_read_is_partial() {
    let dir = claude_family("family-missing", false);
    let output = cli(&["receipt", "--input", dir.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(4));
    let v: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v["entries"][0]["output_tokens"], 300);
    assert!(
        v["limitations"].as_array().unwrap().iter().any(|l| {
            let l = l.as_str().unwrap();
            l.contains("subagent claude-demo/subagent-demo") && l.contains("not read")
        }),
        "{v}"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn usage_export_links_subagents_to_their_session_additively() {
    let dir = claude_family("family-usage", true);
    let v = value(cli(&["usage", "--input", dir.to_str().unwrap(), "--json"]));
    let sessions = v["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2);
    let main = sessions.iter().find(|s| s["id"] == "claude-demo").unwrap();
    // `totals` is still this transcript's own records.
    assert_eq!(main["totals"][0]["input_tokens"], 1200);
    assert_eq!(
        main["subagents"],
        serde_json::json!(["claude-demo/subagent-demo"])
    );
    assert_eq!(main["totals_with_subagents"][0]["input_tokens"], 1206);
    assert_eq!(main["totals_with_subagents"][0]["output_tokens"], 1675);
    let sub = sessions
        .iter()
        .find(|s| s["id"] == "claude-demo/subagent-demo")
        .unwrap();
    assert_eq!(sub["parent"], "claude-demo");
    assert_eq!(sub["subagents"], serde_json::json!([]));
    assert_eq!(sub["totals_with_subagents"], sub["totals"]);
    // Every session says it, so an older vessa is told apart from none.
    let v = value(cli(&["usage", "--input", "examples/pi.jsonl", "--json"]));
    assert_eq!(v["sessions"][0]["subagents"], serde_json::json!([]));
    assert_eq!(
        v["sessions"][0]["totals_with_subagents"],
        v["sessions"][0]["totals"]
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn stats_count_top_level_sessions_and_subagents_separately() {
    let dir = claude_family("family-stats", true);
    for input in [dir.to_str().unwrap(), "examples"] {
        let v = value(cli(&[
            "stats", "--period", "month", "--agent", "claude", "--input", input, "--json",
        ]));
        let bucket = &v["buckets"][0];
        assert_eq!(bucket["period"], "2026-09", "{input}");
        assert_eq!(bucket["sessions"], 1, "{input}");
        assert_eq!(bucket["subagents"], 1, "{input}");
        // The subagent's usage is still counted in the period's totals.
        assert_eq!(bucket["entries"][0]["output_tokens"], 1675, "{input}");
    }
    let out = cli(&[
        "stats",
        "--period",
        "month",
        "--input",
        dir.to_str().unwrap(),
    ]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("claude · 1 sessions · 1 subagents ·"),
        "{text}"
    );
    // A subagent read without its main transcript still counts one session.
    let v = value(cli(&[
        "stats",
        "--input",
        "examples/claude-subagent.jsonl",
        "--json",
    ]));
    assert_eq!(v["buckets"][0]["sessions"], 1);
    assert_eq!(v["buckets"][0]["subagents"], 1);
    let v = value(cli(&["stats", "--input", "examples/pi.jsonl", "--json"]));
    assert_eq!(v["buckets"][0]["subagents"], 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_session_whose_main_transcript_was_not_read_is_not_found() {
    // Only the subagent's transcript was read: `--session` for its parent
    // must not answer with the subagent as if it were the session.
    let dir = claude_family("family-no-main", true);
    std::fs::remove_file(dir.join("claude-demo.jsonl")).unwrap();
    for command in ["receipt", "status"] {
        let output = cli(&[
            command,
            "--input",
            dir.to_str().unwrap(),
            "--session",
            "claude-demo",
        ]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{command}: {stderr}");
        assert!(output.stdout.is_empty(), "{command}");
        assert!(
            stderr.contains("native session claude-demo not found")
                && stderr.contains("main transcript was not read")
                && stderr.contains("1 of its subagents"),
            "{command}: {stderr}"
        );
    }
    // The subagent is still there by its own id.
    let v = value(cli(&[
        "receipt",
        "--input",
        dir.to_str().unwrap(),
        "--session",
        "claude-demo/subagent-demo",
    ]));
    assert_eq!(v["session_id"], "claude-demo/subagent-demo");
    std::fs::remove_dir_all(dir).unwrap();

    // A main transcript holding a sidechain record is read as a subagent
    // transcript; its session id alone still does not select it.
    let path = temp("main-with-sidechain.jsonl");
    std::fs::write(
        &path,
        format!(
            "{}{}\n",
            include_str!("../examples/claude.jsonl"),
            r#"{"type":"user","uuid":"x1","isSidechain":true,"agentId":"stray","sessionId":"claude-demo","timestamp":"2026-09-30T09:04:00Z","message":{"role":"user","content":"Look."}}"#
        ),
    )
    .unwrap();
    let output = cli(&[
        "receipt",
        "--input",
        path.to_str().unwrap(),
        "--session",
        "claude-demo",
    ]);
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn a_subagent_named_from_its_file_is_the_one_its_launch_names() {
    // Without an agentId in its records, a subagent is named from its file,
    // `agent-<id>.jsonl`: the same id the launch record gives.
    let dir = claude_family("family-no-agent-id", true);
    let path = dir.join("claude-demo/subagents/agent-subagent-demo.jsonl");
    let rows: Vec<Value> = include_str!("../examples/claude-subagent.jsonl")
        .lines()
        .map(|s| {
            let mut row: Value = serde_json::from_str(s).unwrap();
            row.as_object_mut().unwrap().remove("agentId");
            row
        })
        .collect();
    write_lines(&path, &rows);
    let output = cli(&["receipt", "--input", dir.to_str().unwrap()]);
    let v: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(output.status.code(), Some(0), "{v}");
    assert_eq!(v["entries"][0]["output_tokens"], 300 + 1375);
    assert!(v["limitations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l.as_str().unwrap().contains("claude-demo/subagent-demo")));
    assert!(!v.to_string().contains("not read"), "{v}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_subagent_whose_requests_are_all_counted_with_its_session_is_not_unknown() {
    // Every request in this subagent transcript is one its session already
    // holds: its usage is counted once, with the session, not unknown.
    let dir = claude_family("family-all-dedup", true);
    let mut rows = vec![
        serde_json::json!({"type":"user","uuid":"s1","isSidechain":true,"agentId":"subagent-demo","sessionId":"claude-demo","timestamp":"2026-09-30T09:00:30Z","message":{"role":"user","content":"Check."}}),
    ];
    for line in include_str!("../examples/claude.jsonl")
        .lines()
        .skip(1)
        .take(2)
    {
        let mut row: Value = serde_json::from_str(line).unwrap();
        row["isSidechain"] = true.into();
        row["agentId"] = "subagent-demo".into();
        rows.push(row);
    }
    write_lines(
        &dir.join("claude-demo/subagents/agent-subagent-demo.jsonl"),
        &rows,
    );
    let output = cli(&["receipt", "--input", dir.to_str().unwrap()]);
    let v: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(output.status.code(), Some(0), "{v}");
    assert_eq!(v["entries"][0]["input_tokens"], 1200);
    assert_eq!(v["entries"][0]["output_tokens"], 300);
    assert!(!v.to_string().contains("unknown, not zero"), "{v}");
    let out = cli(&["status", "--input", dir.to_str().unwrap()]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("no requests of its own; 1 counted in another transcript"),
        "{text}"
    );
    std::fs::remove_dir_all(dir).unwrap();
}
