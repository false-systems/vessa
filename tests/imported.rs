//! `vessa import`: usage receipts from elsewhere, kept strictly and
//! counted with what the transcripts here say.

use std::path::PathBuf;
use std::process::{Command, Output};

fn temp(name: &str) -> PathBuf {
    static CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let call = CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    std::env::temp_dir().join(format!(
        "vessa-import-test-{}-{}-{call}-{name}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn vessa(args: &[&str], state: &std::path::Path) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_vessa"));
    command
        .args(args)
        .env("XDG_STATE_HOME", state)
        .env_remove("HOME");
    for marker in [
        "TOIMIJA_SESSION",
        "TOIMIJA_RUN",
        "CLAUDECODE",
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_CODE_SESSION_ID",
        "CODEX_THREAD_ID",
        "CODEX_SESSION_ID",
        "CODEX_CI",
        "CODEX_SANDBOX",
        "CODEX_SANDBOX_NETWORK_DISABLED",
        "CODEX_MANAGED_BY_NPM",
        "PI_SESSION_ID",
        "PI_CODING_AGENT",
        "OPENCODE",
        "OPENCODE_SESSION_ID",
        "AI_AGENT",
    ] {
        command.env_remove(marker);
    }
    command.output().unwrap()
}

/// A receipt as `vessa receipt` writes one, with distinctive numbers.
fn receipt(dir: &std::path::Path, input: u64) -> PathBuf {
    let file = dir.join(format!("receipt-{input}.json"));
    std::fs::write(
        &file,
        serde_json::to_string_pretty(&serde_json::json!({
            "schema": "agent-usage.v1",
            "agent": "claude",
            "session_id": "elsewhere-1",
            "source": "/elsewhere/session.jsonl",
            "observed_at_ms": 1_760_000_000_000i64,
            "complete": false,
            "limitations": ["Local transcript snapshot; unlogged requests are not verified"],
            "entries": [{
                "provider": "anthropic", "model": "demo-model",
                "input_tokens": input, "output_tokens": 222,
                "cache_read_tokens": 0, "cache_write_tokens": 0
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    file
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[test]
fn a_receipt_is_kept_once_replaced_when_it_changes_and_never_money() {
    let state = temp("state");
    let given = temp("given");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(&given).unwrap();
    let first = receipt(&given, 11_111);

    let out = vessa(&["import", "--file", first.to_str().unwrap()], &state);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout(&out).contains("claude-elsewhere-1.json: claude session elsewhere-1, 1 entry"),
        "{}",
        stdout(&out)
    );

    // The same bytes again: already there.
    let out = vessa(&["import", "--file", first.to_str().unwrap()], &state);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        stdout(&out).contains("already imported"),
        "{}",
        stdout(&out)
    );

    // Different bytes for the same session: the earlier receipt is replaced.
    let second = receipt(&given, 22_222);
    let out = vessa(&["import", "--file", second.to_str().unwrap()], &state);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        stdout(&out).contains("replaced an earlier receipt"),
        "{}",
        stdout(&out)
    );
    assert_eq!(
        std::fs::read_to_string(state.join("vessa/imported/claude-elsewhere-1.json")).unwrap(),
        std::fs::read_to_string(&second).unwrap()
    );

    // Money, in any spelling the old world knew, is refused whole.
    let money = given.join("money.json");
    std::fs::write(
        &money,
        serde_json::to_string(&serde_json::json!({
            "schema": "agent-usage.v1", "agent": "claude", "session_id": "m",
            "entries": [{
                "provider": "anthropic", "model": "m",
                "input_tokens": 1, "output_tokens": 1,
                "cache_read_tokens": 0, "cache_write_tokens": 0,
                "estimated_usd_micros": 12
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    let out = vessa(&["import", "--file", money.to_str().unwrap()], &state);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("fields the agent-usage.v1 schema does not have"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!state.join("vessa/imported/claude-session m.json").exists());
    std::fs::remove_dir_all(state).unwrap();
    std::fs::remove_dir_all(given).unwrap();
}

#[test]
fn a_receipt_that_is_not_what_vessa_writes_is_refused() {
    let state = temp("state");
    let given = temp("given");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(&given).unwrap();

    let refuse = |name: &str, body: serde_json::Value| {
        let file = given.join(name);
        std::fs::write(&file, body.to_string()).unwrap();
        let out = vessa(&["import", "--file", file.to_str().unwrap()], &state);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{name}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !state
                .join("vessa/imported")
                .join(format!("{name}.json"))
                .exists(),
            "{name} kept something"
        );
    };

    // Another schema.
    refuse(
        "wrong-schema.json",
        serde_json::json!({
            "schema": "agent-usage.v2", "agent": "claude", "session_id": "x",
            "entries": []
        }),
    );
    // An unknown field of its own.
    refuse(
        "extra-field.json",
        serde_json::json!({
            "schema": "agent-usage.v1", "agent": "claude", "session_id": "x",
            "observed_at_ms": 1i64, "entries": [], "billed": {"usd": 1}
        }),
    );
    // No agent, no session.
    refuse(
        "no-session.json",
        serde_json::json!({"schema": "agent-usage.v1", "agent": "claude", "entries": []}),
    );
    // A negative count.
    refuse(
        "negative.json",
        serde_json::json!({
            "schema": "agent-usage.v1", "agent": "claude", "session_id": "x",
            "entries": [{
                "provider": "p", "model": "m",
                "input_tokens": -5, "output_tokens": 1,
                "cache_read_tokens": 0, "cache_write_tokens": 0
            }]
        }),
    );
    // Not even JSON.
    let file = given.join("not-json.json");
    std::fs::write(&file, "{").unwrap();
    let out = vessa(&["import", "--file", file.to_str().unwrap()], &state);
    assert_eq!(out.status.code(), Some(2));
    assert!(state
        .join("vessa/imported")
        .read_dir()
        .unwrap()
        .next()
        .is_none());
    std::fs::remove_dir_all(state).unwrap();
    std::fs::remove_dir_all(given).unwrap();
}

#[test]
fn the_counting_commands_count_what_was_imported() {
    let state = temp("state");
    let given = temp("given");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(&given).unwrap();
    let file = receipt(&given, 11_111);
    let out = vessa(&["import", "--file", file.to_str().unwrap()], &state);
    assert_eq!(out.status.code(), Some(0));

    // An empty native input: the imported session is the usage there is.
    let empty = temp("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let out = vessa(
        &[
            "usage",
            "--by-agent",
            "--json",
            "--input",
            empty.to_str().unwrap(),
        ],
        &state,
    );
    // The receipt carries its own limitations, so the count is partial:
    // exit 4, with the numbers still there.
    assert_eq!(out.status.code(), Some(4));
    let totals: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(totals["claude"][0]["input_tokens"], 11_111);
    assert_eq!(totals["claude"][0]["output_tokens"], 222);

    // And in the calendar, at the receipt's observation time.
    let out = vessa(
        &["stats", "--json", "--input", empty.to_str().unwrap()],
        &state,
    );
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(said.contains("11111"), "{said}");
    std::fs::remove_dir_all(state).unwrap();
    std::fs::remove_dir_all(given).unwrap();
    std::fs::remove_dir_all(empty).unwrap();
}

#[test]
fn a_session_read_here_is_never_counted_twice() {
    let state = temp("state");
    let given = temp("given");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(&given).unwrap();
    // A native transcript of session elsewhere-1.
    let at = chrono::DateTime::from_timestamp(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            - 3_600,
        0,
    )
    .unwrap()
    .to_rfc3339();
    let transcript = given.join("native.jsonl");
    let user = serde_json::json!({
        "type": "user", "uuid": "u1", "sessionId": "elsewhere-1",
        "cwd": "/w", "timestamp": at,
        "message": {"role": "user", "content": "hi"}
    });
    std::fs::write(&transcript, format!("{user}\n")).unwrap();
    let file = receipt(&given, 11_111);
    let out = vessa(&["import", "--file", file.to_str().unwrap()], &state);
    assert_eq!(out.status.code(), Some(0));

    let out = vessa(
        &[
            "usage",
            "--by-agent",
            "--json",
            "--input",
            transcript.to_str().unwrap(),
        ],
        &state,
    );
    let totals: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    // The transcript holds no usage records; the imported receipt is the
    // same session, so it is not counted beside it.
    let counted = totals["claude"]
        .as_array()
        .map(|entries| entries.iter().any(|entry| entry["input_tokens"] == 11_111))
        .unwrap_or(false);
    assert!(!counted, "the imported receipt was counted twice: {totals}");
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("is not counted; its transcript is read here"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::remove_dir_all(state).unwrap();
    std::fs::remove_dir_all(given).unwrap();
}
