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
    // Vessa describes itself, naming no other tool, in what it renders. The
    // wrapper export it imports is named by its schema alone.
    let help = [
        &["--help"][..],
        &["report", "--help"],
        &["receipt", "--help"],
    ]
    .iter()
    .map(|args| String::from_utf8(cli(args).stdout).unwrap())
    .collect::<String>();
    for (what, text) in [("report", &html), ("help", &help)] {
        let text = text.to_lowercase().replace("ohjaa-sessions.v1", "");
        for name in ["ohjaa", "toimija", "seula"] {
            assert!(!text.contains(name), "{what} names {name}");
        }
    }
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

/// `vessa claude-statusline` given `input` on stdin.
fn statusline(input: Vec<u8>) -> Output {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new(env!("CARGO_BIN_EXE_vessa"))
        .arg("claude-statusline")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    // It may stop reading early, as it does past 1 MiB.
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let out = child.wait_with_output().unwrap();
    writer.join().unwrap();
    out
}

#[test]
fn claude_statusline_always_prints_and_says_unknown_for_what_it_cannot_read() {
    let realistic = serde_json::json!({"hook_event_name":"Status","session_id":"0a1b2c3d-demo","transcript_path":"/demo/t.jsonl","cwd":"/demo/widget","model":{"id":"demo-model-id","display_name":"Demo"},"workspace":{"current_dir":"/demo/widget","project_dir":"/demo/widget"},"version":"2.0.0","output_style":{"name":"default"},"cost":{"total_cost_usd":0.01234,"total_duration_ms":45000,"total_api_duration_ms":2300,"total_lines_added":156,"total_lines_removed":23},"context_window":{"current_usage":{"input_tokens":8500,"output_tokens":1200,"cache_creation_input_tokens":5000,"cache_read_input_tokens":2000}}});
    let out = statusline(realistic.to_string().into_bytes());
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "session estimate $0.01 · billed unknown · Demo · 0a1b2c3d-demo\n\
         Last API call: input 8500 · output 1200 · cache read 2000 · cache write 5000\n"
    );

    // Each field it cannot read is unknown, and the others still show.
    let broken = serde_json::json!({"session_id":"demo\u{7}\u{1b}]0;title","model":{"display_name":17,"id":"id\u{1b}[2J"},"cost":{"total_cost_usd":-1.5},"context_window":{"current_usage":{"input_tokens":"8500","output_tokens":-3,"cache_read_input_tokens":2.5,"cache_creation_input_tokens":40}}});
    let out = statusline(broken.to_string().into_bytes());
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "session estimate unknown · billed unknown · id[2J · demo]0;title\n\
         Last API call: input unknown · output unknown · cache read unknown · cache write 40\n"
    );
    for input in [
        br#"{"cost":{"total_cost_usd":"4.45"}}"#.to_vec(),
        br#"{"session_id":null,"model":null,"cost":7,"context_window":[]}"#.to_vec(),
        b"[1, 2]".to_vec(),
        b"null".to_vec(),
        b"not json".to_vec(),
        Vec::new(),
        vec![0xff, 0xfe],
        vec![b' '; 1_048_577],
    ] {
        let out = statusline(input.clone());
        let label = String::from_utf8_lossy(&input[..input.len().min(40)]).into_owned();
        assert_eq!(out.status.code(), Some(0), "{label}");
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            "session estimate unknown · billed unknown · unknown model · unknown session\n\
             Last API call: input unknown · output unknown · cache read unknown · cache write unknown\n",
            "{label}"
        );
    }
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
fn command_elapsed_time_is_said_in_plain_words() {
    // The first command has usage; the later two have none, so no end.
    let out = cli(&["status", "--input", "examples/claude.jsonl"]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("Command 1 · line 1 · usage records=1 tools=0 recorded changes=0 · elapsed 60s (to its last usage record)\n"),
        "{text}"
    );
    assert!(
        text.contains(
            "Command 2 · line 4 · usage records=0 tools=0 recorded changes=0 · elapsed unknown\n"
        ),
        "{text}"
    );
    assert!(!text.contains("unknowns") && !text.contains("observed elapsed"));
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
fn claude_synthetic_records_are_counted_as_no_usage_not_conflicts() {
    // Claude Code writes its own `<synthetic>` assistant message, with zero
    // usage, for an API error or a refusal. One may carry the request id of
    // the real request it replaces; another has an id of its own.
    let mut rows: Vec<Value> = include_str!("../examples/claude.jsonl")
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    let synthetic = |uuid: &str, request: Option<&str>, message: &str| {
        let mut row = serde_json::json!({"type":"assistant","uuid":uuid,"sessionId":"claude-demo","timestamp":"2026-09-30T09:01:30Z","isApiErrorMessage":true,"message":{"id":message,"role":"assistant","model":"<synthetic>","stop_reason":"stop_sequence","content":[{"type":"text","text":"Synthetic API error."}],"usage":{"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}});
        if let Some(request) = request {
            row["requestId"] = request.into();
        }
        row
    };
    rows.insert(
        3,
        synthetic(
            "s1",
            Some("req_011CdemoRequest1"),
            "00000000-0000-4000-8000-000000000001",
        ),
    );
    rows.push(synthetic(
        "s2",
        None,
        "00000000-0000-4000-8000-000000000002",
    ));
    let path = temp("synthetic.jsonl");
    write_lines(&path, &rows);
    // A copy of the session elsewhere holds the same records: counted once.
    let copy = temp("synthetic-copy.jsonl");
    write_lines(&copy, &rows);
    for inputs in [vec![&path], vec![&path, &copy]] {
        let mut args = vec!["usage", "--json"];
        for input in &inputs {
            args.extend(["--input", input.to_str().unwrap()]);
        }
        let output = cli(&args);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(0), "{stderr}");
        let v: Value = serde_json::from_slice(&output.stdout).unwrap();
        let session = &v["sessions"][0];
        assert!(session["limitations"].as_array().unwrap().is_empty());
        // Known, and no usage: not a record, not a model, not a price gap.
        assert_eq!(session["synthetic_records"], 2);
        assert_eq!(session["usage"].as_array().unwrap().len(), 1);
        assert_eq!(session["totals"].as_array().unwrap().len(), 1);
        assert_eq!(session["totals"][0]["model"], "demo-model");
        assert_eq!(session["totals"][0]["input_tokens"], 1200);
    }
    let text = cli(&["sessions", "--input", path.to_str().unwrap()]);
    assert!(String::from_utf8_lossy(&text.stdout)
        .contains(" · 1 usage records · 2 synthetic records (no usage) · "));
    // A session without them says nothing of them in its text line, and 0.
    let v = value(cli(&[
        "usage",
        "--input",
        "examples/claude.jsonl",
        "--json",
    ]));
    assert_eq!(v["sessions"][0]["synthetic_records"], 0);

    // One that reports usage is no known non-usage record: it is read as
    // usage, and here conflicts with the request it shares an id with.
    rows[3]["message"]["usage"]["output_tokens"] = 5.into();
    write_lines(&path, &rows);
    let output = cli(&["usage", "--input", path.to_str().unwrap(), "--json"]);
    assert_eq!(output.status.code(), Some(4));
    let v: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v["sessions"][0]["synthetic_records"], 1);
    assert!(v["sessions"][0]["limitations"][0]
        .as_str()
        .unwrap()
        .contains("model demo-model then <synthetic>"));
    std::fs::remove_file(path).unwrap();
    std::fs::remove_file(copy).unwrap();
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
fn an_agent_that_is_not_installed_is_no_gap() {
    // Only Claude Code is installed: no `~/.codex`, no `~/.pi`.
    let home = temp("claude-only-home");
    std::fs::create_dir_all(home.join(".claude/projects/demo")).unwrap();
    std::fs::copy(
        "examples/claude.jsonl",
        home.join(".claude/projects/demo/claude-demo.jsonl"),
    )
    .unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_vessa"))
            .args(args)
            .env("HOME", &home)
            .env_remove("XDG_DATA_HOME")
            .output()
            .unwrap()
    };
    for args in [
        &["usage", "--json"][..],
        &["sessions", "--headers", "--json"],
        &["messages", "--json"],
    ] {
        let out = run(args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(0), "{args:?}: {stderr}");
        assert!(stderr.is_empty(), "{args:?}: {stderr}");
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert!(v["warnings"].as_array().unwrap().is_empty(), "{args:?}");
    }
    // A path asked for by name that is not there is a gap.
    let missing = home.join(".pi/agent/sessions");
    let out = run(&["usage", "--json", "--input", missing.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(4));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["warnings"][0]
        .as_str()
        .unwrap()
        .starts_with(missing.to_str().unwrap()));
    std::fs::remove_dir_all(home).unwrap();
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

/// A session header's fields: `vessa-session-headers.v1` has these and no
/// others, each as `vessa-usage.v1` has it.
const HEADER: [&str; 8] = [
    "agent",
    "id",
    "parent",
    "started_at",
    "cwd",
    "source",
    "entrypoint",
    "originator",
];

/// Asserts that `sessions --headers --json` lists, for `args`, the sessions
/// and warnings `sessions --json` lists, in its order, each with the full
/// listing's header fields and only those. Returns how many it compared.
fn assert_headers_match(args: &[&str]) -> usize {
    let mut full = vec!["sessions", "--json"];
    full.extend(args);
    let mut light = full.clone();
    light.push("--headers");
    let (full, light) = (cli(&full), cli(&light));
    let stderr = String::from_utf8_lossy(&light.stderr).into_owned();
    let f: Value = serde_json::from_slice(&full.stdout).unwrap();
    let h: Value = serde_json::from_slice(&light.stdout).unwrap();
    assert_eq!(h["schema"], "vessa-session-headers.v1");
    assert_eq!(h["warnings"], f["warnings"], "{args:?}");
    // Headers read no usage: only a source warning makes them partial.
    let partial = !f["warnings"].as_array().unwrap().is_empty();
    assert_eq!(
        light.status.code(),
        Some(if partial { 4 } else { 0 }),
        "{args:?}: {stderr}"
    );
    let (fs, hs) = (
        f["sessions"].as_array().unwrap(),
        h["sessions"].as_array().unwrap(),
    );
    assert_eq!(fs.len(), hs.len(), "{args:?}");
    for (a, b) in fs.iter().zip(hs) {
        let fields = b.as_object().unwrap();
        assert_eq!(fields.len(), HEADER.len(), "{args:?}: {b}");
        for key in HEADER {
            assert!(fields.contains_key(key), "{args:?}: {key} missing: {b}");
            assert_eq!(a[key], b[key], "{args:?}: {key} of {}", a["id"]);
        }
    }
    fs.len()
}

/// Synthetic transcripts covering each rule a header is read by.
fn header_cases() -> PathBuf {
    let dir = claude_family("header-cases", true);
    let write = |name: &str, rows: &[&str]| {
        std::fs::write(dir.join(name), rows.join("\n") + "\n").unwrap();
    };
    // Claude: untimed, unplaced opening records; time, folder and origin come
    // from later records, and not all from the same one.
    write(
        "headless.jsonl",
        &[
            r#"{"type":"last-prompt","sessionId":"claude-headless","leafUuid":"x"}"#,
            r#"{"type":"mode","sessionId":"claude-headless","mode":"normal"}"#,
            r#"{"type":"user","uuid":"u1","isSidechain":false,"sessionId":"claude-headless","timestamp":"2026-09-30T09:00:00Z","message":{"role":"user","content":"draft"}}"#,
            r#"{"type":"user","uuid":"u2","isSidechain":false,"sessionId":"claude-headless","cwd":"/demo/widget","timestamp":"2026-09-30T09:00:05Z","message":{"role":"user","content":"more"}}"#,
            r#"{"type":"user","uuid":"u3","isSidechain":false,"sessionId":"claude-headless","cwd":"/demo/other","entrypoint":"sdk-cli","timestamp":"2026-09-30T09:00:09Z","message":{"role":"user","content":"last"}}"#,
        ],
    );
    // Claude, placed only by a record after every other fact.
    write(
        "unplaced.jsonl",
        &[
            r#"{"type":"user","uuid":"p1","isSidechain":false,"sessionId":"claude-unplaced","entrypoint":"cli","timestamp":"2026-09-30T08:30:00Z","message":{"role":"user","content":"a"}}"#,
            r#"{"type":"user","uuid":"p2","isSidechain":false,"sessionId":"claude-unplaced","cwd":"/demo/late","message":{"role":"user","content":"b"}}"#,
        ],
    );
    // Claude, saying neither its origin nor whether it is a sidechain: read
    // to its end, unknown.
    write(
        "silent.jsonl",
        &[
            r#"{"type":"user","uuid":"u1","sessionId":"claude-silent","cwd":"/demo/widget","timestamp":"2026-09-30T08:00:00Z","message":{"role":"user","content":"hi"}}"#,
            r#"{"type":"user","uuid":"u2","sessionId":"claude-silent","message":{"role":"user","content":"again"}}"#,
        ],
    );
    // A Claude subagent whose records omit its agentId: named from its file.
    write(
        "claude-demo/subagents/agent-unnamed.jsonl",
        &[
            r#"{"type":"user","uuid":"n1","isSidechain":true,"sessionId":"claude-demo","cwd":"/demo/widget","entrypoint":"cli","timestamp":"2026-09-30T09:20:00Z","message":{"role":"user","content":"look"}}"#,
        ],
    );
    // A Claude subagent that says it is one, and its agentId, only after an
    // opening record that has every other fact.
    write(
        "claude-demo/subagents/agent-late.jsonl",
        &[
            r#"{"type":"user","uuid":"l0","sessionId":"claude-demo","cwd":"/demo/widget","entrypoint":"cli","timestamp":"2026-09-30T09:30:00Z","message":{"role":"user","content":"start"}}"#,
            r#"{"type":"user","uuid":"l1","isSidechain":true,"sessionId":"claude-demo","timestamp":"2026-09-30T09:30:01Z","message":{"role":"user","content":"look"}}"#,
            r#"{"type":"user","uuid":"l2","isSidechain":true,"agentId":"late-agent","sessionId":"claude-demo","timestamp":"2026-09-30T09:30:02Z","message":{"role":"user","content":"more"}}"#,
        ],
    );
    // Two copies of one session: the first path's header is kept.
    for (name, cwd) in [("copy-a.jsonl", "/demo/a"), ("copy-b.jsonl", "/demo/b")] {
        write(
            name,
            &[&format!(
                r#"{{"type":"user","uuid":"c1","isSidechain":false,"sessionId":"claude-copy","cwd":"{cwd}","entrypoint":"cli","timestamp":"2026-09-30T07:00:00Z","message":{{"role":"user","content":"copy"}}}}"#
            )],
        );
    }
    // A malformed opening line is skipped, as the full reader skips it.
    write(
        "malformed.jsonl",
        &[
            "{broken",
            r#"{"type":"user","uuid":"m1","isSidechain":false,"sessionId":"claude-malformed","cwd":"/demo/widget","entrypoint":"cli","timestamp":"2026-09-30T06:00:00Z","message":{"role":"user","content":"x"}}"#,
        ],
    );
    // Codex: a fork, a subagent with no parent named, `codex exec`, and a
    // session_meta with no time and no folder of its own.
    write(
        "fork.jsonl",
        &[
            r#"{"timestamp":"2026-09-30T10:05:00Z","type":"session_meta","payload":{"id":"codex-fork","forked_from_id":"codex-demo","cwd":"/demo/widget","originator":"codex-tui","source":"cli","timestamp":"2026-09-30T10:05:00Z"}}"#,
        ],
    );
    write(
        "codex-sub.jsonl",
        &[
            r#"{"timestamp":"2026-09-30T10:06:00Z","type":"session_meta","payload":{"id":"codex-sub","cwd":"/demo/widget","originator":"codex-tui","source":{"subagent":{"other":"review"}},"timestamp":"2026-09-30T10:06:00Z"}}"#,
        ],
    );
    write(
        "exec.jsonl",
        &[
            r#"{"type":"session_meta","payload":{"id":"codex-exec","cwd":"/demo/widget","originator":"codex_exec","source":"exec","timestamp":"2026-09-30T10:00:00Z"}}"#,
        ],
    );
    write(
        "untimed.jsonl",
        &[
            r#"{"type":"session_meta","payload":{"id":"codex-untimed","source":"cli"}}"#,
            r#"{"type":"turn_context","payload":{"model":"demo-codex"}}"#,
            r#"{"timestamp":"2026-09-30T10:07:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[]}}"#,
        ],
    );
    // Pi: a child session naming its parent.
    write(
        "pi-child.jsonl",
        &[
            r#"{"type":"session","version":3,"id":"pi-child","parentSession":"pi-demo","timestamp":"2026-09-30T11:30:00Z","cwd":"/demo/widget"}"#,
        ],
    );
    // What is not a session is a warning in both listings.
    write("note.jsonl", &[r#"{"type":"note"}"#]);
    write(
        "no-id.jsonl",
        &[r#"{"type":"session_meta","payload":{"id":"","cwd":"/demo/widget"}}"#],
    );
    write("blank.jsonl", &[""]);
    dir
}

#[test]
fn session_headers_are_the_full_listings_headers() {
    let cases = header_cases();
    let dir = cases.to_str().unwrap();
    let main = cases.join("claude-demo.jsonl");
    let receipt = value(cli(&["receipt", "--input", "examples/pi.jsonl"]));
    let export = temp("headers-ohjaa.json");
    let wrappers = serde_json::json!({"schema":"ohjaa-sessions.v1","sessions":[{"id":"oh-fixture","agent":"pi","usage":[receipt]},{"id":"oh-unmatched","agent":"pi","usage":[]}],"unreadable":["oh-gone"]});
    std::fs::write(&export, serde_json::to_vec(&wrappers).unwrap()).unwrap();
    let mut compared = 0;
    for args in [
        vec!["--input", "examples"],
        vec!["--input", "examples/claude.jsonl"],
        vec!["--input", "examples/claude-subagent.jsonl"],
        vec!["--input", "examples/codex.jsonl"],
        vec!["--input", "examples/pi.jsonl"],
        vec!["--input", "examples/work.jsonl"],
        vec!["--input", dir],
        vec!["--input", dir, "--agent", "codex"],
        vec!["--input", dir, "--session", "claude-demo"],
        // A transcript given alone brings its subagents along.
        vec!["--input", main.to_str().unwrap()],
        vec![
            "--input",
            "examples/pi.jsonl",
            "--input",
            export.to_str().unwrap(),
        ],
        vec!["--input", "/nonexistent/vessa-headers"],
    ] {
        compared += assert_headers_match(&args);
    }
    // Every case above was listed, and the rules each one tests applied.
    assert!(compared >= 30, "{compared}");
    // Partial (exit 4): its three files that are not sessions are warnings.
    let output = cli(&["sessions", "--json", "--headers", "--input", dir]);
    assert_eq!(output.status.code(), Some(4));
    let v: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v["warnings"].as_array().unwrap().len(), 3, "{v}");
    let header = |id: &str| {
        v["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == id)
            .unwrap_or_else(|| panic!("{id} not listed: {v}"))
            .clone()
    };
    let headless = header("claude-headless");
    assert_eq!(headless["started_at"], "2026-09-30T09:00:00Z");
    assert_eq!(headless["cwd"], "/demo/widget");
    assert_eq!(headless["entrypoint"], "sdk-cli");
    assert!(header("claude-silent")["entrypoint"].is_null());
    assert_eq!(header("claude-unplaced")["cwd"], "/demo/late");
    assert_eq!(header("claude-demo/subagent-demo")["parent"], "claude-demo");
    assert_eq!(header("claude-demo/unnamed")["parent"], "claude-demo");
    assert_eq!(header("claude-demo/late-agent")["parent"], "claude-demo");
    assert_eq!(header("claude-copy")["cwd"], "/demo/a");
    assert_eq!(header("codex-fork")["parent"], "codex-demo");
    assert_eq!(
        header("codex-sub")["parent"],
        "parent identity unavailable (subagent source)"
    );
    assert_eq!(header("codex-exec")["originator"], "codex_exec");
    assert_eq!(
        header("codex-untimed")["started_at"],
        "2026-09-30T10:07:00Z"
    );
    assert_eq!(header("pi-child")["parent"], "pi-demo");
    std::fs::remove_dir_all(cases).unwrap();
    std::fs::remove_file(export).unwrap();
}

/// `sessions --json` for `args`, with its exit status, partial or not.
fn listing(args: &[&str]) -> (Value, Option<i32>) {
    let output = cli(args);
    let v = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("{args:?}: {e}: {}", String::from_utf8_lossy(&output.stderr)));
    (v, output.status.code())
}

#[test]
fn session_headers_differ_from_the_full_listing_only_where_documented() {
    // The header reader stops at a transcript's header, trusting how the
    // agents write. Where a file breaks that, the modes differ, as the
    // README says, and these pin how.
    let dir = temp("header-differences");
    std::fs::create_dir_all(&dir).unwrap();
    let opening = |id: &str| {
        format!(
            r#"{{"type":"user","uuid":"o1","isSidechain":false,"sessionId":"{id}","cwd":"/demo/widget","entrypoint":"cli","timestamp":"2026-09-30T09:00:00Z","message":{{"role":"user","content":"a"}}}}"#
        )
    };
    let write = |name: &str, rows: &[&str]| {
        let path = dir.join(name);
        std::fs::write(&path, rows.join("\n") + "\n").unwrap();
        path.to_str().unwrap().to_owned()
    };
    let both = |input: &str| {
        let (full, _) = listing(&["sessions", "--json", "--input", input]);
        let (headers, code) = listing(&["sessions", "--json", "--headers", "--input", input]);
        (full, headers, code)
    };
    // A Claude transcript that says it is no sidechain, then that it is one:
    // the full listing makes it a subagent of itself, headers keep the first.
    let flip = write(
        "flip.jsonl",
        &[
            &opening("claude-flip"),
            r#"{"type":"user","uuid":"o2","isSidechain":true,"agentId":"late","sessionId":"claude-flip","message":{"role":"user","content":"b"}}"#,
        ],
    );
    let (full, headers, code) = both(&flip);
    assert_eq!(full["sessions"][0]["id"], "claude-flip/late");
    assert_eq!(full["sessions"][0]["parent"], "claude-flip");
    assert_eq!(headers["sessions"][0]["id"], "claude-flip");
    assert!(headers["sessions"][0]["parent"].is_null());
    assert_eq!(code, Some(0));
    // A file holding two agents' records: the full listing takes the higher
    // ranked (Codex, then pi), headers the first.
    for (name, later, agent, id) in [
        (
            "then-codex.jsonl",
            r#"{"type":"session_meta","payload":{"id":"codex-mixed","cwd":"/demo/widget","timestamp":"2026-09-30T10:00:00Z"}}"#,
            "codex",
            "codex-mixed",
        ),
        (
            "then-pi.jsonl",
            r#"{"type":"session","version":3,"id":"pi-mixed","timestamp":"2026-09-30T11:00:00Z","cwd":"/demo/widget"}"#,
            "pi",
            "pi-mixed",
        ),
    ] {
        let mixed = write(name, &[&opening("claude-mixed"), later]);
        let (full, headers, _) = both(&mixed);
        assert_eq!(full["sessions"][0]["agent"], agent, "{name}");
        assert_eq!(full["sessions"][0]["id"], id, "{name}");
        assert_eq!(headers["sessions"][0]["agent"], "claude", "{name}");
        assert_eq!(headers["sessions"][0]["id"], "claude-mixed", "{name}");
    }
    // Damage after the header: the full listing drops the transcript with a
    // warning, headers list it, since they never read that far.
    let damaged = |name: &str, opening: &str| {
        let path = dir.join(name);
        let mut bytes = opening.as_bytes().to_vec();
        bytes.extend(b"\n{\"type\":\"user\",\"sessionId\":\"x\",\"note\":\"\xff\xfe\"}\n");
        std::fs::write(&path, bytes).unwrap();
        path.to_str().unwrap().to_owned()
    };
    let tail = damaged("damaged-tail.jsonl", &opening("claude-damaged"));
    let (full, headers, code) = both(&tail);
    assert!(full["sessions"].as_array().unwrap().is_empty(), "{full}");
    assert_eq!(full["warnings"].as_array().unwrap().len(), 2, "{full}");
    assert_eq!(headers["sessions"][0]["id"], "claude-damaged");
    assert_eq!(headers["warnings"], serde_json::json!([]));
    assert_eq!(code, Some(0));
    // So of two copies of one session, a damaged first copy gives the
    // header; the full listing has only the second.
    let copies = dir.join("copies");
    std::fs::create_dir_all(&copies).unwrap();
    std::fs::rename(
        damaged("a.jsonl", &opening("claude-copied")),
        copies.join("a.jsonl"),
    )
    .unwrap();
    std::fs::write(
        copies.join("b.jsonl"),
        opening("claude-copied")
            .replace("/demo/widget", "/demo/other")
            .replace("09:00:00", "09:30:00"),
    )
    .unwrap();
    let (full, headers, _) = both(copies.to_str().unwrap());
    assert_eq!(full["sessions"][0]["cwd"], "/demo/other");
    assert_eq!(full["sessions"][0]["started_at"], "2026-09-30T09:30:00Z");
    assert_eq!(full["warnings"].as_array().unwrap().len(), 1, "{full}");
    assert_eq!(headers["sessions"][0]["cwd"], "/demo/widget");
    assert_eq!(headers["sessions"][0]["started_at"], "2026-09-30T09:00:00Z");
    assert_eq!(headers["warnings"], serde_json::json!([]));
    let source = |v: &Value| v["sessions"][0]["source"].as_str().unwrap().to_owned();
    assert!(source(&full).ends_with("b.jsonl"), "{full}");
    assert!(source(&headers).ends_with("a.jsonl"), "{headers}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn session_headers_print_a_line_each_and_refuse_rates_wherever_given() {
    let text = cli(&["sessions", "--headers", "--input", "examples/pi.jsonl"]);
    assert_eq!(
        String::from_utf8(text.stdout).unwrap(),
        "pi pi-demo · started 2026-09-30T11:00:00Z · /demo/widget\n"
    );
    // Headers read no usage to price; a rate card would only add notices
    // that a caller takes for gaps.
    for args in [
        vec!["sessions", "--headers", "--rates", "examples/rates.json"],
        vec!["--rates", "examples/rates.json", "sessions", "--headers"],
    ] {
        let mut argv = args.clone();
        argv.extend(["--json", "--input", "examples/pi.jsonl"]);
        let output = cli(&argv);
        assert!(!output.status.success(), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("--rates"), "{args:?}: {stderr}");
    }
}

// ---- messages ---------------------------------------------------------------

/// `vessa messages --json` with `args`, and its exit code.
fn messages(args: &[&str]) -> (Value, Option<i32>) {
    let output = cli(&[&["messages", "--json"], args].concat());
    let v = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!("{e}: {}", String::from_utf8_lossy(&output.stderr));
    });
    (v, output.status.code())
}
fn texts(v: &Value) -> Vec<&str> {
    v["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["text"].as_str().unwrap())
        .collect()
}
fn excluded(v: &Value) -> &serde_json::Map<String, Value> {
    v["coverage"]["excluded"].as_object().unwrap()
}
/// A synthetic transcript in a file of its own.
fn transcript(name: &str, rows: &[Value]) -> PathBuf {
    let path = temp(name);
    write_lines(&path, rows);
    path
}
fn claude_user(uuid: &str, minute: u32, content: Value) -> Value {
    serde_json::json!({"type":"user","uuid":uuid,"sessionId":"claude-demo","cwd":"/demo/widget","entrypoint":"cli","isSidechain":false,"timestamp":format!("2026-09-30T09:{minute:02}:00Z"),"message":{"role":"user","content":content}})
}
fn claude_reply(uuid: &str, minute: u32, text: &str) -> Value {
    serde_json::json!({"type":"assistant","uuid":uuid,"sessionId":"claude-demo","timestamp":format!("2026-09-30T09:{minute:02}:30Z"),"message":{"role":"assistant","content":[{"type":"text","text":text}]}})
}
fn codex_meta(id: &str, extra: Value) -> Value {
    let mut payload = serde_json::json!({"id":id,"cwd":"/demo/widget","source":"cli","originator":"codex-tui","timestamp":"2026-09-30T10:00:00Z"});
    payload
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    serde_json::json!({"timestamp":"2026-09-30T10:00:00Z","type":"session_meta","payload":payload})
}
fn codex_turn(role: &str, minute: u32, text: &str) -> Value {
    let kind = if role == "user" {
        "input_text"
    } else {
        "output_text"
    };
    serde_json::json!({"timestamp":format!("2026-09-30T10:{minute:02}:00Z"),"type":"response_item","payload":{"type":"message","role":role,"content":[{"type":kind,"text":text}]}})
}

#[test]
fn messages_lists_what_a_person_typed_with_the_reply_it_follows() {
    let (v, code) = messages(&["--input", "examples/claude.jsonl"]);
    assert_eq!(code, Some(0));
    assert_eq!(v["schema"], "vessa-messages.v1");
    let m = &v["messages"][1];
    for (key, expected) in [
        ("agent", "claude"),
        ("session", "claude-demo"),
        ("id", "u2"),
        ("at", "2026-09-30T09:02:00Z"),
        ("cwd", "/demo/widget"),
        ("text", "I already told you to run the local checks."),
    ] {
        assert_eq!(m[key], expected, "{key}");
    }
    assert!(m["parent"].is_null());
    assert_eq!(m["line"], 4);
    assert!(m["source"]
        .as_str()
        .unwrap()
        .ends_with("examples/claude.jsonl"));
    assert_eq!(m["reply"]["text"], "The implementation is ready.");
    assert_eq!(m["reply"]["line"], 3);
    assert_eq!(m["reply"]["failed"], false);
    assert_eq!(m["reply"]["truncated"], false);
    // The first message follows no reply.
    assert!(v["messages"][0]["reply"].is_null());
    assert_eq!(v["coverage"]["human"], 3);
    assert_eq!(v["coverage"]["user_turns"], 3);
    assert_eq!(v["sessions"][0]["messages"], 3);
    assert_eq!(v["sessions"][0]["headless"], false);

    // A long reply travels as its last 600 characters.
    let long = format!("{}END", "x".repeat(700));
    let path = transcript(
        "long-reply.jsonl",
        &[
            claude_user("u1", 0, "Explain the parser.".into()),
            claude_reply("a1", 0, &long),
            claude_user("u2", 1, "Shorter, please.".into()),
        ],
    );
    let (v, _) = messages(&["--input", path.to_str().unwrap()]);
    let reply = &v["messages"][1]["reply"];
    assert_eq!(reply["text"].as_str().unwrap().chars().count(), 600);
    assert!(reply["text"].as_str().unwrap().ends_with("END"));
    assert_eq!(reply["truncated"], true);
    std::fs::remove_file(path).unwrap();

    // The plain form prints each message under the place it was said.
    let out = cli(&["messages", "--input", "examples/claude.jsonl"]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.starts_with("claude claude-demo · 2026-09-30T09:00:00Z · /demo/widget · "));
    assert!(text.contains("\n  Please run the existing local checks before finishing.\n"));
    assert!(text.contains("3 messages from 1 sessions (0 headless)"));
}

#[test]
fn harness_envelopes_and_prompts_are_counted_not_listed() {
    let notice = serde_json::json!({"type":"user","uuid":"n1","sessionId":"claude-demo","timestamp":"2026-09-30T09:20:00Z","origin":{"kind":"task-notification"},"promptSource":"system","message":{"role":"user","content":"<task-notification>\n<status>completed</status>\n</task-notification>"}});
    let mut meta = claude_user("m1", 21, "Base directory for this skill: /demo".into());
    meta["isMeta"] = true.into();
    let mut summary = claude_user("c1", 22, "Summary of the work so far.".into());
    summary["isCompactSummary"] = true.into();
    let blob = serde_json::json!({"type":"api-request-blob","sessionId":"claude-demo","hash":"h1","message":{"role":"user","content":[{"type":"text","text":"Keep the parser strict."}]}});
    let path = transcript(
        "envelopes.jsonl",
        &[
            claude_user(
                "e1",
                1,
                "<command-name>/model</command-name>\n<command-message>model</command-message>\n<command-args></command-args>".into(),
            ),
            claude_user("e2", 2, "[Request interrupted by user]".into()),
            claude_user("e3", 3, "<local-command-stdout>Set model</local-command-stdout>".into()),
            claude_user(
                "e4",
                4,
                "This session is being continued from a previous conversation that ran out of context. The summary follows.".into(),
            ),
            claude_user(
                "e5",
                5,
                "Your toimija session packet is below. It is your workset contract: work from it.\n\n# Toimija Context\n- branch: main".into(),
            ),
            notice,
            meta,
            summary,
            blob,
            claude_user(
                "h1",
                6,
                "<system-reminder>Injected context.</system-reminder>\nKeep the parser strict.".into(),
            ),
            // A person may start a message as a harness would.
            claude_user("h2", 7, "Caveat: the fixture is synthetic.".into()),
            claude_user("h3", 8, "<command-name> is the tag I meant".into()),
        ],
    );
    let (v, code) = messages(&["--input", path.to_str().unwrap()]);
    assert_eq!(code, Some(0));
    assert_eq!(
        texts(&v),
        [
            "Keep the parser strict.",
            "Caveat: the fixture is synthetic.",
            "<command-name> is the tag I meant"
        ]
    );
    let rules = excluded(&v);
    for (rule, n) in [
        ("envelope", 3),
        ("compaction", 2),
        ("harness_prompt", 1),
        ("harness_origin", 1),
        ("meta", 1),
        ("request_record", 1),
    ] {
        assert_eq!(rules[rule], n, "{rule}");
    }
    assert_eq!(v["coverage"]["user_turns"], 12);
    std::fs::remove_file(path).unwrap();

    // Codex: startup context, the AGENTS.md heading, `/init`, and the answer
    // to the agent's question tool.
    let path = transcript(
        "codex-envelopes.jsonl",
        &[
            codex_meta("codex-envelopes", serde_json::json!({})),
            codex_turn(
                "user",
                1,
                "<environment_context>\n  <cwd>/demo/widget</cwd>\n</environment_context>",
            ),
            codex_turn(
                "user",
                2,
                "# AGENTS.md instructions for /demo/widget\n\n<INSTRUCTIONS>\nRust only.\n</INSTRUCTIONS>",
            ),
            codex_turn(
                "user",
                3,
                "Generate a file named AGENTS.md that serves as a contributor guide for this repository.\nYour goal is a clear guide.",
            ),
            codex_question("q1", &[("Proceed?", Some(&["Yes", "No"]))]),
            codex_turn(
                "user",
                4,
                &codex_reply(&[("q1", 0, "Proceed?", "Yes")]),
            ),
            codex_turn("user", 5, "Please run the existing local checks."),
        ],
    );
    let (v, _) = messages(&["--input", path.to_str().unwrap()]);
    assert_eq!(texts(&v), ["Please run the existing local checks."]);
    let rules = excluded(&v);
    assert_eq!(rules["envelope"], 2);
    assert_eq!(rules["harness_prompt"], 1);
    assert_eq!(rules["question_reply"], 1);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn tool_results_are_not_messages() {
    // Any tool result makes the turn a tool's, even with text beside it.
    let path = transcript(
        "tool-results.jsonl",
        &[
            claude_user("u1", 0, "Run the tests.".into()),
            claude_user(
                "t1",
                1,
                serde_json::json!([{"type":"tool_result","tool_use_id":"x1","content":"ok"}]),
            ),
            claude_user(
                "t2",
                2,
                serde_json::json!([{"type":"tool_result","tool_use_id":"x2","content":"ok"},{"type":"text","text":"Tool output follows."}]),
            ),
        ],
    );
    let (v, _) = messages(&["--input", path.to_str().unwrap()]);
    assert_eq!(texts(&v), ["Run the tests."]);
    assert_eq!(excluded(&v)["tool_result"], 2);
    std::fs::remove_file(path).unwrap();

    // pi and Codex write tool results as records of their own, not user turns.
    let pi = transcript(
        "pi-tools.jsonl",
        &[
            serde_json::json!({"type":"session","version":3,"id":"pi-tools","timestamp":"2026-09-30T11:00:00Z","cwd":"/demo/widget"}),
            serde_json::json!({"type":"message","id":"p1","timestamp":"2026-09-30T11:00:00Z","message":{"role":"user","content":[{"type":"text","text":"Run the tests."}]}}),
            serde_json::json!({"type":"message","id":"p2","timestamp":"2026-09-30T11:00:10Z","message":{"role":"toolResult","toolCallId":"c1","content":[{"type":"text","text":"ok"}]}}),
        ],
    );
    let codex = transcript(
        "codex-tools.jsonl",
        &[
            codex_meta("codex-tools", serde_json::json!({})),
            codex_turn("user", 1, "Run the tests."),
            serde_json::json!({"timestamp":"2026-09-30T10:02:00Z","type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"ok"}}),
        ],
    );
    let (v, _) = messages(&[
        "--input",
        pi.to_str().unwrap(),
        "--input",
        codex.to_str().unwrap(),
    ]);
    assert_eq!(texts(&v), ["Run the tests.", "Run the tests."]);
    assert_eq!(v["coverage"]["user_turns"], 2);
    std::fs::remove_file(pi).unwrap();
    std::fs::remove_file(codex).unwrap();
}

#[test]
fn a_codex_fork_lists_only_its_own_turns() {
    let dir = temp("codex-fork");
    std::fs::create_dir_all(&dir).unwrap();
    let history = [
        codex_turn("user", 1, "Add the parser."),
        codex_turn("assistant", 2, "Added the parser."),
        codex_turn("user", 3, "Now test it."),
        codex_turn("assistant", 4, "The tests pass."),
    ];
    let parent = dir.join("a-parent.jsonl");
    let mut rows = vec![codex_meta("codex-parent", serde_json::json!({}))];
    rows.extend(history.iter().cloned());
    write_lines(&parent, &rows);
    // A fork copies its parent's transcript, header included, behind its own
    // header, stamped with the fork time; then come its own turns.
    let fork = dir.join("b-fork.jsonl");
    let mut rows = vec![
        codex_meta(
            "codex-fork",
            serde_json::json!({"forked_from_id":"codex-parent","timestamp":"2026-09-30T10:30:00Z"}),
        ),
        codex_meta("codex-parent", serde_json::json!({})),
    ];
    rows.extend(history.iter().cloned().map(|mut r| {
        r["timestamp"] = "2026-09-30T10:30:00Z".into();
        r
    }));
    rows.push(codex_turn("user", 31, "Try a different parser."));
    write_lines(&fork, &rows);

    let (v, code) = messages(&["--input", dir.to_str().unwrap()]);
    assert_eq!(code, Some(0));
    assert_eq!(
        texts(&v),
        ["Add the parser.", "Now test it.", "Try a different parser."]
    );
    let own = &v["messages"][2];
    assert_eq!(own["session"], "codex-fork");
    assert_eq!(own["parent"], "codex-parent");
    // Its reply is the copied history's last, which the person saw.
    assert_eq!(own["reply"]["text"], "The tests pass.");
    assert_eq!(excluded(&v)["fork_copy"], 2);

    // Selected alone, the fork still needs its parent's transcript.
    let (v, _) = messages(&["--input", dir.to_str().unwrap(), "--session", "codex-fork"]);
    assert_eq!(texts(&v), ["Try a different parser."]);
    assert_eq!(v["sessions"].as_array().unwrap().len(), 1);

    // Without it, copied history cannot be told from the fork's own turns.
    let (v, code) = messages(&["--input", fork.to_str().unwrap()]);
    assert_eq!(code, Some(4));
    assert!(texts(&v).is_empty());
    assert_eq!(excluded(&v)["fork_copy"], 3);
    assert!(v["coverage"]["limitations"][0]
        .as_str()
        .unwrap()
        .contains("parent transcript not read"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_subagents_prompts_are_its_parents_and_a_person_can_write_to_it() {
    let dir = claude_family("messages-subagent", true);
    // A person writes to the running subagent: Claude Code records it in the
    // subagent's transcript, as meta, with a person's origin.
    let subagent = dir.join("claude-demo/subagents/agent-subagent-demo.jsonl");
    let mut rows: Vec<Value> = include_str!("../examples/claude-subagent.jsonl")
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    rows.push(serde_json::json!({"parentUuid":"s3","isSidechain":true,"isMeta":true,"agentId":"subagent-demo","type":"user","uuid":"s9","sessionId":"claude-demo","cwd":"/demo/widget","entrypoint":"cli","timestamp":"2026-09-30T09:12:00Z","origin":{"kind":"human"},"message":{"role":"user","content":"Only the parser tests, not the fixtures."}}));
    // A subagent copies no history: an undated message to it is still its own.
    rows.push(serde_json::json!({"parentUuid":"s9","isSidechain":true,"isMeta":true,"agentId":"subagent-demo","type":"user","uuid":"s10","sessionId":"claude-demo","cwd":"/demo/widget","entrypoint":"cli","origin":{"kind":"human"},"message":{"role":"user","content":"Skip the slow tests."}}));
    write_lines(&subagent, &rows);
    let codex = dir.join("codex-subagent.jsonl");
    write_lines(
        &codex,
        &[
            codex_meta(
                "codex-subagent",
                serde_json::json!({"source":{"subagent":{"thread_spawn":{"parent_thread_id":"codex-demo"}}}}),
            ),
            codex_turn("user", 1, "Review the parser change."),
        ],
    );
    let (v, code) = messages(&["--input", dir.to_str().unwrap()]);
    assert_eq!(code, Some(0));
    // Codex subagents are not read; Claude's are, for what a person sent.
    assert_eq!(v["coverage"]["subagent_transcripts"], 1);
    assert!(!texts(&v).contains(&"Review the parser change."));
    // The parent agent's prompt to its subagent is not a person's.
    assert!(!texts(&v).contains(&"Find the parser tests and report their names."));
    assert_eq!(excluded(&v)["subagent"], 1);
    assert_eq!(texts(&v).len(), 5);
    assert!(texts(&v).contains(&"Skip the slow tests."));
    assert!(v["coverage"]["limitations"].as_array().unwrap().is_empty());
    let sent = v["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["session"] != "claude-demo")
        .unwrap();
    assert_eq!(sent["text"], "Only the parser tests, not the fixtures.");
    assert_eq!(sent["session"], "claude-demo/subagent-demo");
    assert_eq!(sent["parent"], "claude-demo");
    assert_eq!(
        sent["reply"]["text"],
        "The parser tests live in tests/parser.rs."
    );
    let subagents: Vec<&Value> = v["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["subagent"] == true)
        .collect();
    assert_eq!(subagents.len(), 1);
    assert_eq!(subagents[0]["messages"], 2);
    // A subagent's own id selects it alone.
    let (v, code) = messages(&[
        "--input",
        dir.to_str().unwrap(),
        "--session",
        "claude-demo/subagent-demo",
    ]);
    assert_eq!(code, Some(0));
    assert_eq!(
        texts(&v),
        [
            "Only the parser tests, not the fixtures.",
            "Skip the slow tests."
        ]
    );
    // A session whose main transcript was not read is not found.
    std::fs::remove_file(dir.join("claude-demo.jsonl")).unwrap();
    let out = cli(&[
        "messages",
        "--input",
        dir.to_str().unwrap(),
        "--session",
        "claude-demo",
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("its main transcript was not read"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn headless_prompts_are_not_listed_and_their_sessions_say_so() {
    let mut sdk = claude_user("s1", 0, "Summarise the diff.".into());
    sdk["entrypoint"] = "sdk-cli".into();
    let headless = transcript("headless.jsonl", &[sdk.clone()]);
    // An interactive session later resumed by a script keeps its own turns.
    let mut resumed = sdk;
    resumed["uuid"] = "s2".into();
    resumed["timestamp"] = "2026-09-30T09:05:00Z".into();
    let mixed = temp("mixed");
    std::fs::create_dir_all(&mixed).unwrap();
    write_lines(
        &mixed.join("claude-mixed.jsonl"),
        &[
            claude_user("u1", 0, "Fix the parser.".into()),
            claude_reply("a1", 0, "Fixed."),
            resumed,
        ],
    );
    let exec = transcript(
        "codex-exec.jsonl",
        &[
            codex_meta(
                "codex-exec",
                serde_json::json!({"source":"exec","originator":"codex_exec"}),
            ),
            codex_turn("user", 1, "Summarise the widget changelog."),
        ],
    );
    let (v, _) = messages(&[
        "--input",
        headless.to_str().unwrap(),
        "--input",
        exec.to_str().unwrap(),
    ]);
    assert!(texts(&v).is_empty());
    assert_eq!(excluded(&v)["headless"], 2);
    let sessions = v["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2);
    assert!(sessions.iter().all(|s| s["headless"] == true));
    let (v, _) = messages(&["--input", mixed.to_str().unwrap()]);
    assert_eq!(texts(&v), ["Fix the parser."]);
    assert_eq!(excluded(&v)["headless"], 1);
    assert_eq!(v["sessions"][0]["headless"], false);
    std::fs::remove_file(headless).unwrap();
    std::fs::remove_file(exec).unwrap();
    std::fs::remove_dir_all(mixed).unwrap();
}

#[test]
fn a_resent_message_is_a_retry_and_one_after_a_failure_says_so() {
    let mut failure = claude_reply("a1", 0, "API Error: 500 server error");
    failure["isApiErrorMessage"] = true.into();
    let path = transcript(
        "retry.jsonl",
        &[
            claude_user("u1", 0, "Run the checks.".into()),
            failure,
            // The same request sent again, unanswered: not a new message.
            claude_user("u2", 1, "Run the checks.".into()),
            claude_user("u3", 2, "continue".into()),
            claude_reply("a2", 2, "The checks pass."),
            claude_user("u4", 3, "yes".into()),
            claude_reply("a3", 3, "Shall I commit?"),
            // Said again after a reply, it is a message of its own.
            claude_user("u5", 4, "yes".into()),
        ],
    );
    let (v, _) = messages(&["--input", path.to_str().unwrap()]);
    assert_eq!(texts(&v), ["Run the checks.", "continue", "yes", "yes"]);
    assert_eq!(excluded(&v)["retry"], 1);
    // "continue" answers a failure, not a reply.
    let after = &v["messages"][1]["reply"];
    assert_eq!(after["failed"], true);
    assert_eq!(after["text"], "API Error: 500 server error");
    assert_eq!(v["messages"][2]["reply"]["failed"], false);
    std::fs::remove_file(path).unwrap();

    // A Codex error event is a failed turn too.
    let path = transcript(
        "codex-error.jsonl",
        &[
            codex_meta("codex-error", serde_json::json!({})),
            codex_turn("user", 1, "Run the checks."),
            serde_json::json!({"timestamp":"2026-09-30T10:02:00Z","type":"event_msg","payload":{"type":"error","message":"stream disconnected"}}),
            codex_turn("user", 3, "try again"),
        ],
    );
    let (v, _) = messages(&["--input", path.to_str().unwrap()]);
    assert_eq!(v["messages"][1]["reply"]["failed"], true);
    assert_eq!(v["messages"][1]["reply"]["text"], "stream disconnected");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn a_pi_session_lists_its_messages_and_a_fork_only_its_own() {
    let (v, code) = messages(&["--input", "examples/pi.jsonl"]);
    assert_eq!(code, Some(0));
    assert_eq!(texts(&v), ["Find the error and show the fix working."]);
    assert_eq!(v["messages"][0]["id"], "p1");
    assert_eq!(v["messages"][0]["cwd"], "/demo/widget");

    let session = |id: &str, parent: Option<&str>, at: &str| {
        let mut row = serde_json::json!({"type":"session","version":3,"id":id,"timestamp":at,"cwd":"/demo/widget"});
        if let Some(parent) = parent {
            row["parentSession"] = parent.into();
        }
        row
    };
    let user = |id: &str, at: &str, text: &str| serde_json::json!({"type":"message","id":id,"timestamp":at,"message":{"role":"user","content":[{"type":"text","text":text}]}});
    let path = transcript(
        "pi-fork.jsonl",
        &[
            session("pi-fork", Some("pi-demo"), "2026-09-30T12:00:00Z"),
            // Copied from the parent, before the fork.
            user(
                "p1",
                "2026-09-30T11:00:00Z",
                "Find the error and show the fix working.",
            ),
            serde_json::json!({"type":"message","id":"p9","timestamp":"2026-09-30T12:01:00Z","message":{"role":"assistant","content":[],"stopReason":"error","errorMessage":"rate limited"}}),
            user("f1", "2026-09-30T12:02:00Z", "/model"),
            user("f2", "2026-09-30T12:03:00Z", "Try the smaller model."),
        ],
    );
    let (v, _) = messages(&["--input", path.to_str().unwrap()]);
    assert_eq!(texts(&v), ["Try the smaller model."]);
    assert_eq!(v["messages"][0]["parent"], "pi-demo");
    assert_eq!(v["messages"][0]["reply"]["text"], "rate limited");
    assert_eq!(v["messages"][0]["reply"]["failed"], true);
    let rules = excluded(&v);
    assert_eq!(rules["inherited"], 1);
    assert_eq!(rules["envelope"], 1);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn messages_select_by_time_session_and_agent() {
    let all = [
        "--input",
        "examples/claude.jsonl",
        "--input",
        "examples/codex.jsonl",
        "--input",
        "examples/pi.jsonl",
    ];
    let (v, _) = messages(&[&all[..], &["--since", "2026-09-30T09:01:00Z"]].concat());
    assert_eq!(texts(&v).len(), 4);
    assert_eq!(v["coverage"]["earlier"], 1);
    assert_eq!(v["coverage"]["since"], "2026-09-30T09:01:00+00:00");
    let (v, _) = messages(&[&all[..], &["--since", "2026-10-01"]].concat());
    assert!(texts(&v).is_empty());
    assert_eq!(v["coverage"]["earlier"], 5);
    let (v, _) = messages(&[&all[..], &["--since", "100000d"]].concat());
    assert_eq!(texts(&v).len(), 5);
    let (v, _) = messages(&[&all[..], &["--agent", "codex"]].concat());
    assert_eq!(
        texts(&v),
        ["Please run the existing local checks before finishing."]
    );
    let (v, _) = messages(&[&all[..], &["--session", "pi-demo"]].concat());
    assert_eq!(texts(&v), ["Find the error and show the fix working."]);

    // A message with no time cannot be placed after `--since`.
    let mut undated = claude_user("u9", 0, "When was this?".into());
    undated.as_object_mut().unwrap().remove("timestamp");
    let path = transcript("undated.jsonl", &[undated]);
    let (v, _) = messages(&["--input", path.to_str().unwrap(), "--since", "7d"]);
    assert_eq!(v["coverage"]["undated"], 1);
    // A transcript last written before `--since` is not read.
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000))
        .unwrap();
    let (v, _) = messages(&["--input", path.to_str().unwrap(), "--since", "7d"]);
    assert_eq!(v["coverage"]["unmodified_transcripts"], 1);
    assert_eq!(v["coverage"]["transcripts"], 0);
    std::fs::remove_file(path).unwrap();

    for (args, error) in [
        (
            &["--session", "missing"][..],
            "native session missing not found",
        ),
        (&["--agent", "opencode"][..], "SQLite"),
        (&["--since", "soon"][..], "expected RFC 3339"),
        (
            &["--rates", "examples/rates.json"][..],
            "--rates cannot be used",
        ),
    ] {
        let out = cli(&[&["messages"], &all[..], args].concat());
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(error),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn opencode_is_a_known_gap_in_the_default_sources() {
    let home = temp("home");
    for dir in [
        ".claude/projects/demo",
        ".codex/sessions",
        ".pi/agent/sessions",
    ] {
        std::fs::create_dir_all(home.join(dir)).unwrap();
    }
    std::fs::copy(
        "examples/claude.jsonl",
        home.join(".claude/projects/demo/claude-demo.jsonl"),
    )
    .unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_vessa"))
            .args(args)
            .env("HOME", &home)
            .env_remove("XDG_DATA_HOME")
            .output()
            .unwrap()
    };
    let out = run(&["messages", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    let db = home.join(".local/share/opencode");
    std::fs::create_dir_all(&db).unwrap();
    std::fs::write(db.join("opencode.db"), "").unwrap();
    let out = run(&["messages", "--json"]);
    assert_eq!(out.status.code(), Some(4));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(texts(&v).len(), 3);
    assert!(v["coverage"]["gaps"][0]
        .as_str()
        .unwrap()
        .starts_with("opencode: "));
    // Asked for one agent, the others are no gap.
    assert_eq!(
        run(&["messages", "--agent", "claude"]).status.code(),
        Some(0)
    );
    // A store last written before the window holds nothing from it.
    let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    let age = |path: &std::path::Path| {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(old)
            .unwrap()
    };
    age(&db.join("opencode.db"));
    let out = run(&["messages", "--json", "--since", "7d"]);
    assert_eq!(out.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["coverage"]["gaps"].as_array().unwrap().is_empty());
    // SQLite writes through its WAL: a recent one is a gap.
    std::fs::write(db.join("opencode.db-wal"), "").unwrap();
    assert_eq!(
        run(&["messages", "--json", "--since", "7d"]).status.code(),
        Some(4)
    );
    age(&db.join("opencode.db-wal"));
    assert_eq!(
        run(&["messages", "--json", "--since", "7d"]).status.code(),
        Some(0)
    );
    std::fs::remove_dir_all(home).unwrap();
}

/// Codex's question tool asking `questions` (title, options), as item `id`.
fn codex_question(id: &str, questions: &[(&str, Option<&[&str]>)]) -> Value {
    let questions: Vec<Value> = questions
        .iter()
        .map(|(title, options)| serde_json::json!({"title":title,"options":options}))
        .collect();
    serde_json::json!({"timestamp":"2026-09-30T10:03:30Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"AgentMessage","id":id,"delivery":"async","content":[],"questions":questions}}})
}
/// Codex's reply to its question tool: (item, index, question, answer), with
/// `questionItemId` a JSON string, as Codex writes it.
fn codex_reply(answers: &[(&str, u64, &str, &str)]) -> String {
    let answers: Vec<Value> = answers
        .iter()
        .map(|(item, index, question, answer)| serde_json::json!({"answer":answer,"question":question,"questionItemId":serde_json::json!(["request_user_input_async",item,index]).to_string()}))
        .collect();
    format!(
        "<send_user_message_question_reply>\n{}\n</send_user_message_question_reply>",
        Value::from(answers)
    )
}

#[test]
fn a_persons_own_answers_rejections_and_tasks_are_listed_and_picks_are_not() {
    // Codex: one question offers options and gets a pick; one offers them and
    // gets an answer of the person's own; one offers none.
    let path = transcript(
        "codex-answers.jsonl",
        &[
            codex_meta("codex-answers", serde_json::json!({})),
            codex_turn("assistant", 1, "Which parser should I keep?"),
            codex_question(
                "q1",
                &[
                    ("Which parser?", Some(&["Strict", "Lenient"])),
                    ("Keep the fixtures?", Some(&["Yes", "No"])),
                ],
            ),
            codex_question("q2", &[("Anything else?", None)]),
            // An answer worded like the message before it is still an answer.
            codex_turn("user", 3, "Name the error type."),
            codex_turn(
                "user",
                4,
                &codex_reply(&[
                    ("q1", 0, "Which parser?", "Strict"),
                    ("q1", 1, "Keep the fixtures?", "Neither; regenerate them."),
                    ("q2", 0, "Anything else?", "Name the error type."),
                ]),
            ),
            // Every answer a pick: nobody's own words.
            codex_turn(
                "user",
                5,
                &codex_reply(&[("q1", 0, "Which parser?", "Lenient")]),
            ),
        ],
    );
    let (v, code) = messages(&["--input", path.to_str().unwrap()]);
    assert_eq!(code, Some(0));
    assert_eq!(
        texts(&v),
        [
            "Name the error type.",
            "Neither; regenerate them.",
            "Name the error type."
        ]
    );
    let answer = &v["messages"][1];
    assert_eq!(answer["kind"], "answer");
    assert_eq!(v["messages"][2]["kind"], "answer");
    assert_eq!(answer["question"], "Keep the fixtures?");
    assert_eq!(answer["reply"]["text"], "Which parser should I keep?");
    assert_eq!(excluded(&v)["question_reply"], 1);
    std::fs::remove_file(path).unwrap();

    // Claude: a free-text answer to its question tool, a pick (one, and
    // several joined), and the reason given for rejecting a tool call.
    let question = |uuid: &str, answers: Value| {
        let mut row = claude_user(
            uuid,
            1,
            serde_json::json!([{"type":"tool_result","tool_use_id":uuid,"content":"Answered."}]),
        );
        row["toolUseResult"] = serde_json::json!({"questions":[
            {"question":"Which parser?","header":"Parser","multiSelect":false,"options":[{"label":"Strict","description":"s"},{"label":"Lenient","description":"l"}]},
            {"question":"Which checks?","header":"Checks","multiSelect":true,"options":[{"label":"fmt","description":"f"},{"label":"clippy","description":"c"}]}
        ],"answers":answers,"annotations":{}});
        row
    };
    let rejected = claude_user(
        "r1",
        3,
        serde_json::json!([{"type":"tool_result","tool_use_id":"x1","is_error":true,"content":"The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). To tell you how to proceed, the user said:\nEdit the parser, not the tests."}]),
    );
    let refused = claude_user(
        "r2",
        4,
        serde_json::json!([{"type":"tool_result","tool_use_id":"x2","is_error":true,"content":"The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). STOP what you are doing and wait for the user to tell you how to proceed."}]),
    );
    let path = transcript(
        "claude-answers.jsonl",
        &[
            question(
                "q1",
                serde_json::json!({"Which parser?":"Strict","Which checks?":"fmt, clippy"}),
            ),
            question(
                "q2",
                serde_json::json!({"Which parser?":"Write one by hand.","Which checks?":"fmt"}),
            ),
            rejected,
            refused,
            // The task a person typed into a launch prompt follows its packet.
            claude_user(
                "l1",
                5,
                "You are in a toimija session outside any Git repository (/demo). There is no session packet.\n\nTask: Write the release notes.\n".into(),
            ),
        ],
    );
    let (v, _) = messages(&["--input", path.to_str().unwrap()]);
    assert_eq!(
        texts(&v),
        [
            "Write one by hand.",
            "Edit the parser, not the tests.",
            "Write the release notes."
        ]
    );
    let kinds: Vec<&str> = v["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["answer", "rejection", "task"]);
    assert_eq!(v["messages"][0]["question"], "Which parser?");
    assert_eq!(excluded(&v)["tool_result"], 2);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn a_message_typed_while_the_agent_works_is_listed_once() {
    let queued = |uuid: &str, minute: u32, text: &str| serde_json::json!({"type":"attachment","uuid":uuid,"sessionId":"claude-demo","cwd":"/demo/widget","entrypoint":"cli","isSidechain":false,"timestamp":format!("2026-09-30T09:{minute:02}:10Z"),"attachment":{"type":"queued_command","commandMode":"prompt","prompt":text,"origin":{"kind":"human"}}});
    let path = transcript(
        "queued.jsonl",
        &[
            claude_user("u1", 0, "Refactor the parser.".into()),
            // Absorbed mid-turn: the attachment is its only record.
            queued("q1", 1, "Keep the public API."),
            // Queued, then delivered as a message of its own.
            queued("q2", 2, "Also update the docs."),
            claude_reply("a1", 3, "Refactored."),
            claude_user("u2", 4, "Also update the docs.".into()),
            // A notification the harness queued is no person's.
            serde_json::json!({"type":"attachment","uuid":"q3","sessionId":"claude-demo","timestamp":"2026-09-30T09:05:00Z","attachment":{"type":"queued_command","commandMode":"task-notification","prompt":"<task-notification>done</task-notification>"}}),
        ],
    );
    let (v, _) = messages(&["--input", path.to_str().unwrap()]);
    assert_eq!(
        texts(&v),
        [
            "Refactor the parser.",
            "Keep the public API.",
            "Also update the docs."
        ]
    );
    assert_eq!(v["messages"][1]["kind"], "queued");
    assert_eq!(v["messages"][2]["kind"], "message");
    assert_eq!(excluded(&v)["duplicate"], 1);
    assert_eq!(excluded(&v)["harness_origin"], 1);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn each_structural_rule_excludes_what_it_names() {
    // A message read again from a copy of its session counts once.
    let dir = temp("copies");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy("examples/claude.jsonl", dir.join("a.jsonl")).unwrap();
    std::fs::copy("examples/claude.jsonl", dir.join("b.jsonl")).unwrap();
    let (v, _) = messages(&["--input", dir.to_str().unwrap()]);
    assert_eq!(texts(&v).len(), 3);
    assert_eq!(excluded(&v)["duplicate"], 3);
    std::fs::remove_dir_all(dir).unwrap();

    let mut sidechain = claude_user("c1", 3, "Find the tests.".into());
    sidechain["isSidechain"] = true.into();
    let mut sdk = claude_user("c2", 4, "Summarise.".into());
    sdk["promptSource"] = "sdk".into();
    let mut peer = claude_user("c3", 5, "Review done.".into());
    peer["origin"] = serde_json::json!({"kind":"peer"});
    let mut system = claude_user("c4", 6, "Scheduled check.".into());
    system["promptSource"] = "system".into();
    let mut other = claude_user("c5", 7, "From elsewhere.".into());
    other["sessionId"] = "claude-other".into();
    let path = transcript(
        "structural.jsonl",
        &[
            claude_user("u1", 0, "Fix the parser.".into()),
            // An image alone has no text to read.
            claude_user(
                "e1",
                1,
                serde_json::json!([{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AA=="}}]),
            ),
            sidechain,
            sdk,
            peer,
            system,
            other,
        ],
    );
    let (v, code) = messages(&["--input", path.to_str().unwrap()]);
    assert_eq!(texts(&v), ["Fix the parser."]);
    let rules = excluded(&v);
    for (rule, n) in [
        ("empty", 1),
        ("subagent", 1),
        ("headless", 1),
        ("harness_origin", 2),
        ("other_session", 1),
    ] {
        assert_eq!(rules[rule], n, "{rule}");
    }
    // A record of another session leaves this one's list partial.
    assert_eq!(code, Some(4));
    assert!(v["coverage"]["limitations"][0]
        .as_str()
        .unwrap()
        .contains("a record of another session"));
    std::fs::remove_file(path).unwrap();

    // Codex: the bare AGENTS.md heading, and a `codex exec` run in which a
    // person submitted a turn interactively.
    let path = transcript(
        "codex-exec-interactive.jsonl",
        &[
            codex_meta(
                "codex-exec-interactive",
                serde_json::json!({"source":"exec","originator":"codex_exec"}),
            ),
            codex_turn(
                "user",
                1,
                "# AGENTS.md instructions\n<INSTRUCTIONS>\nRust only.\n</INSTRUCTIONS>",
            ),
            {
                let mut turn = codex_turn("user", 2, "Now check the parser.");
                turn["payload"]["id"] = "msg_demo".into();
                turn
            },
            serde_json::json!({"timestamp":"2026-09-30T10:02:00Z","type":"event_msg","payload":{"type":"user_message","message":"Now check the parser."}}),
        ],
    );
    let (v, _) = messages(&["--input", path.to_str().unwrap()]);
    assert_eq!(texts(&v), ["Now check the parser."]);
    assert_eq!(v["messages"][0]["id"], "msg_demo");
    assert_eq!(excluded(&v)["envelope"], 1);
    assert_eq!(v["sessions"][0]["headless"], false);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn a_fork_of_a_fork_is_read_after_its_parents_whatever_their_paths() {
    // Path order would read the grandchild first, then the child, then the
    // parent; each needs its parent's transcript first.
    let dir = temp("fork-chain");
    std::fs::create_dir_all(&dir).unwrap();
    let parent = vec![
        codex_meta("codex-parent", serde_json::json!({})),
        codex_turn("user", 1, "Add the parser."),
        codex_turn("assistant", 2, "Added the parser."),
    ];
    let fork = |id: &str, from: &str, copied: &[Value], own: &str, minute: u32| {
        let mut rows = vec![codex_meta(
            id,
            serde_json::json!({"forked_from_id":from,"timestamp":format!("2026-09-30T10:{minute:02}:00Z")}),
        )];
        rows.extend(copied.iter().cloned());
        rows.push(codex_turn("user", minute + 1, own));
        rows.push(codex_turn("assistant", minute + 2, "Done."));
        rows
    };
    let child = fork(
        "codex-child",
        "codex-parent",
        &parent,
        "Try the strict parser.",
        20,
    );
    let grandchild = fork(
        "codex-grandchild",
        "codex-child",
        &child,
        "Try the lenient one.",
        40,
    );
    write_lines(&dir.join("a-grandchild.jsonl"), &grandchild);
    write_lines(&dir.join("b-child.jsonl"), &child);
    write_lines(&dir.join("c-parent.jsonl"), &parent);
    let (v, code) = messages(&["--input", dir.to_str().unwrap()]);
    assert_eq!(code, Some(0), "{}", v["coverage"]["limitations"]);
    let mut listed = texts(&v);
    listed.sort();
    assert_eq!(
        listed,
        [
            "Add the parser.",
            "Try the lenient one.",
            "Try the strict parser."
        ]
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn since_is_exclusive_like_intakes_window_and_errors_leave_no_partial_json() {
    // A reader passes the time of the last message it handled: that message
    // is not listed again.
    let (v, _) = messages(&[
        "--input",
        "examples/claude.jsonl",
        "--since",
        "2026-09-30T09:02:00Z",
    ]);
    assert_eq!(
        texts(&v),
        ["Please run the existing local checks before finishing."]
    );
    assert_eq!(v["messages"][0]["id"], "u3");
    assert_eq!(v["coverage"]["earlier"], 2);
    let (v, _) = messages(&[
        "--input",
        "examples/claude.jsonl",
        "--since",
        "2026-09-30T09:01:59.999Z",
    ]);
    assert_eq!(texts(&v).len(), 2);

    // A run that fails before its first message writes nothing to stdout.
    let out = cli(&[
        "messages",
        "--json",
        "--input",
        "examples/claude.jsonl",
        "--session",
        "missing",
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
}

/// A launch prompt as toimija composes it for `ohjaa run`: the launcher's
/// alerts, if any, then the fixed framing and the packet, then `Task:` with
/// the task, if any, then ohjaa's briefing, if any.
fn launch_prompt(alerts: bool, task: Option<&str>, briefing: Option<&str>) -> String {
    let mut prompt = String::from(
        "Your toimija session packet is below. It is your workset contract: work from it, \
         and declare any expansion (`toimija intent \"<why>\" --scope <path>`) before reading \
         outside it.\nLive packet (re-read after HEAD changes): /demo/widget/.toimija/current.md\n\n\
         # Toimija Context\n- branch: main\n- dirty: false",
    );
    prompt.push('\n');
    if let Some(task) = task {
        prompt.push_str(&format!("\nTask: {task}\n"));
    }
    if let Some(briefing) = briefing {
        prompt.push_str(&format!("\n{briefing}\n"));
    }
    if alerts {
        prompt.insert_str(
            0,
            "## Before anything else (from the launcher)\nThe gate on main is unproven.\n\n",
        );
    }
    prompt
}

/// ohjaa's briefing: its parts joined by a newline, each opening as ohjaa
/// writes it.
fn ohjaa_briefing(knowledge: bool) -> String {
    let mut parts = Vec::new();
    if knowledge {
        parts.push(
            "Knowledge (ohjaa, knowledge-node.v1): what the people you work for know and\n\
             have decided, as of the date on each entry. Follow it.\n\n\
             Ours: decided by the team, binding everywhere:\n- [rule] Rust only.\n"
                .to_owned(),
        );
    }
    parts.push(format!(
        "How this session works (ohjaa):\nIntent: demo.\nIf no concrete task is assigned, \
         acknowledge and wait.\n{}",
        "1. Before your first change, read back what you know.\n".repeat(80)
    ));
    parts.push(
        "System perimeter (derived by ohjaa at launch; re-derive with\n`ohjaa system`, \
         session observations via `ohjaa fleet`):\n\nRepositories:\n- widget — demo\n"
            .to_owned(),
    );
    parts.join("\n")
}

#[test]
fn a_launch_task_is_what_the_person_typed_without_the_briefing() {
    let briefing = ohjaa_briefing(true);
    let prompts = [
        // The briefing alone, with no task or an empty one.
        launch_prompt(false, None, Some(&briefing)),
        launch_prompt(false, Some(""), Some(&briefing)),
        launch_prompt(false, None, None),
        // A short task, alone and before a long briefing.
        launch_prompt(false, Some("fix the parser"), None),
        launch_prompt(false, Some("tidy the docs"), Some(&briefing)),
        // A task of several paragraphs keeps them, up to the briefing.
        launch_prompt(
            false,
            Some("Rename the parser.\n\nKeep the old name as an alias."),
            Some(&ohjaa_briefing(false)),
        ),
        // The launcher's alerts come before the framing.
        launch_prompt(true, Some("check the gate"), Some(&briefing)),
        // Before ohjaa passed its briefing apart, it was the task: alone, or
        // after the typed task and a blank line.
        launch_prompt(false, Some(&briefing), None),
        launch_prompt(false, Some(&format!("ship it\n\n{briefing}")), None),
    ];
    let rows: Vec<Value> = prompts
        .iter()
        .enumerate()
        .flat_map(|(i, prompt)| {
            [
                claude_user(&format!("l{i}"), i as u32 * 2, prompt.clone().into()),
                claude_reply(&format!("a{i}"), i as u32 * 2, "Ready."),
            ]
        })
        .collect();
    let path = transcript("launch-tasks.jsonl", &rows);
    let (v, code) = messages(&["--input", path.to_str().unwrap()]);
    assert_eq!(code, Some(0));
    assert_eq!(
        texts(&v),
        [
            "fix the parser",
            "tidy the docs",
            "Rename the parser.\n\nKeep the old name as an alias.",
            "check the gate",
            "ship it"
        ]
    );
    assert!(v["messages"]
        .as_array()
        .unwrap()
        .iter()
        .all(|m| m["kind"] == "task"));
    assert_eq!(excluded(&v)["harness_prompt"], 4);
    std::fs::remove_file(path).unwrap();
}
