//! `vessa line`: what a terminal's opening says, from the session folders.

use std::path::PathBuf;
use std::process::{Command, Output};

/// Now in UTC, as the clock-less chrono build gives it.
fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64,
        0,
    )
    .unwrap()
}

fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "vessa-line-test-{}-{}-{name}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

/// The markers an agent's own shell carries; the tests take them all out,
/// because the harness itself may run inside an agent session.
const MARKERS: [&str; 16] = [
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
];

/// `vessa line`, as a person's shell runs it: no agent marker set.
fn line(args: &[&str], state: &std::path::Path, input: &str) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_vessa"));
    command
        .args(args)
        .arg("--input")
        .arg(input)
        .env("XDG_STATE_HOME", state)
        .env_remove("HOME");
    for marker in MARKERS {
        command.env_remove(marker);
    }
    command.output().unwrap()
}

/// A Claude-shaped transcript whose session started `hours_ago`.
fn transcript(dir: &std::path::Path, hours_ago: i64, input: u64, output: u64) -> PathBuf {
    let at = chrono::DateTime::from_timestamp(now().timestamp() - hours_ago * 3_600, 0)
        .unwrap()
        .to_rfc3339();
    let id = format!("s{input}");
    let user = serde_json::json!({
        "type": "user", "uuid": "u1", "sessionId": id,
        "cwd": "/demo", "timestamp": at,
        "message": {"role": "user", "content": "hi"}
    });
    let assistant = serde_json::json!({
        "type": "assistant", "uuid": "a1", "requestId": "r1", "sessionId": id,
        "timestamp": at,
        "message": {
            "id": "m1", "role": "assistant", "model": "demo-model",
            "content": [{"type": "text", "text": "done"}],
            "usage": {"input_tokens": input, "output_tokens": output}
        }
    });
    let file = dir.join(format!("{hours_ago}-{input}.jsonl"));
    std::fs::write(&file, format!("{user}\n{assistant}\n")).unwrap();
    file
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[test]
fn a_day_is_said_with_its_sessions_and_usage_and_kept_in_the_log() {
    let state = temp("state");
    std::fs::create_dir_all(&state).unwrap();
    let given = temp("given");
    std::fs::create_dir_all(&given).unwrap();
    transcript(&given, 0, 1_400_000, 302_000);

    // --json inspects: the whole record, and nothing is kept.
    let out = line(&["line", "--json"], &state, given.to_str().unwrap());
    assert!(out.status.success());
    let record: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(record["schema"], "vessa-line.v1");
    assert_eq!(record["sessions"], 1);
    assert_eq!(record["subagents"], 0);
    assert_eq!(record["input_tokens"], 1_400_000);
    assert_eq!(record["output_tokens"], 302_000);
    assert_eq!(record["text"], "1 session today · 1.4M in · 302k out");
    assert!(!state.join("vessa/line.json").exists());

    // --always says it to a person, and keeps the cursor and the log.
    let out = line(&["line", "--always"], &state, given.to_str().unwrap());
    assert!(out.status.success());
    assert_eq!(stdout(&out), "1 session today · 1.4M in · 302k out");
    let cursor: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(state.join("vessa/line.json")).unwrap())
            .unwrap();
    assert_eq!(cursor["schema"], "vessa-line-cursor.v1");
    assert_eq!(cursor["line"], "1 session today · 1.4M in · 302k out");
    assert_eq!(cursor["day"], now().date_naive().to_string());
    let log = std::fs::read_to_string(state.join("vessa/line-log.jsonl")).unwrap();
    assert_eq!(log.lines().count(), 1);
    assert!(log.contains("1 session today"));

    // The day changed under the person: the line is due again, the log grows.
    std::fs::write(
        state.join("vessa/line.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "schema": "vessa-line-cursor.v1",
            "day": "2001-01-01",
            "line": "1 session today · 1.4M in · 302k out",
            "shown_at": "2001-01-01T00:00:00+00:00"
        }))
        .unwrap(),
    )
    .unwrap();
    let out = line(&["line", "--always"], &state, given.to_str().unwrap());
    assert_eq!(stdout(&out), "1 session today · 1.4M in · 302k out");
    let log = std::fs::read_to_string(state.join("vessa/line-log.jsonl")).unwrap();
    assert_eq!(log.lines().count(), 2);
    std::fs::remove_dir_all(state).unwrap();
    std::fs::remove_dir_all(given).unwrap();
}

#[test]
fn a_day_without_sessions_says_so_and_yesterday_does_not_count() {
    let state = temp("state");
    std::fs::create_dir_all(&state).unwrap();
    let given = temp("given");
    std::fs::create_dir_all(&given).unwrap();
    // Started 48 hours ago: not today's.
    transcript(&given, 48, 10, 10);
    let out = line(&["line", "--always"], &state, given.to_str().unwrap());
    assert!(out.status.success());
    assert_eq!(stdout(&out), "no agent sessions recorded today");
    std::fs::remove_dir_all(state).unwrap();
    std::fs::remove_dir_all(given).unwrap();
}

#[test]
fn what_cannot_be_read_is_in_the_line_never_an_all_clear() {
    let state = temp("state");
    std::fs::create_dir_all(&state).unwrap();
    let given = temp("given");
    std::fs::create_dir_all(&given).unwrap();
    transcript(&given, 0, 10, 10);
    // A second --input that is not there: a source warning the line carries.
    let missing = given.join("missing.jsonl");
    let mut command = Command::new(env!("CARGO_BIN_EXE_vessa"));
    command
        .args(["line", "--always", "--input", given.to_str().unwrap()])
        .arg("--input")
        .arg(missing.to_str().unwrap())
        .env("XDG_STATE_HOME", &state)
        .env_remove("HOME");
    for marker in MARKERS {
        command.env_remove(marker);
    }
    let out = command.output().unwrap();
    assert!(out.status.success());
    let said = stdout(&out);
    assert!(
        said.ends_with("; not all read: 1 source warning"),
        "the line must say its gap: {said}"
    );
    assert!(said.contains("1 session today"), "{said}");
    std::fs::remove_dir_all(state).unwrap();
    std::fs::remove_dir_all(given).unwrap();
}

#[test]
fn an_agents_shell_never_sees_the_persons_line() {
    let state = temp("state");
    std::fs::create_dir_all(&state).unwrap();
    let given = temp("given");
    std::fs::create_dir_all(&given).unwrap();
    transcript(&given, 0, 10, 10);
    let mut command = Command::new(env!("CARGO_BIN_EXE_vessa"));
    command
        .args(["line", "--always"])
        .arg("--input")
        .arg(given.to_str().unwrap())
        .env("XDG_STATE_HOME", &state)
        .env_remove("HOME")
        .env("PI_SESSION_ID", "the agent's own shell");
    for marker in MARKERS {
        command.env_remove(marker);
    }
    command.env("PI_SESSION_ID", "the agent's own shell");
    let out = command.output().unwrap();
    assert!(out.status.success());
    assert_eq!(stdout(&out), "");
    assert!(
        !state.join("vessa/line.json").exists(),
        "an agent's shell moves no cursor"
    );
    std::fs::remove_dir_all(state).unwrap();
    std::fs::remove_dir_all(given).unwrap();
}

#[test]
fn a_piped_terminal_without_always_prints_nothing() {
    let state = temp("state");
    std::fs::create_dir_all(&state).unwrap();
    let given = temp("given");
    std::fs::create_dir_all(&given).unwrap();
    transcript(&given, 0, 10, 10);
    let out = line(&["line"], &state, given.to_str().unwrap());
    assert!(out.status.success());
    assert_eq!(stdout(&out), "");
    assert!(!state.join("vessa/line.json").exists());
    std::fs::remove_dir_all(state).unwrap();
    std::fs::remove_dir_all(given).unwrap();
}
