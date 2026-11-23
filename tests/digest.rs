//! `vessa collect` and `vessa digest`: the snapshot, and what happened
//! since the person last looked.

use std::path::PathBuf;
use std::process::{Command, Output};

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
    // A counter apart from the clock: two tests that make a folder in the
    // same nanosecond must not share it.
    static CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let call = CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    std::env::temp_dir().join(format!(
        "vessa-digest-test-{}-{}-{call}-{name}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

/// `vessa` as a person's shell runs it: no agent marker set, so a digest
/// may move its cursor.
fn vessa(args: &[&str], state: &std::path::Path, input: &str) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_vessa"));
    command
        .args(args)
        .arg("--input")
        .arg(input)
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

/// A Claude-shaped transcript: one session, started `hours_ago`, one usage
/// record `minutes_after` its start.
fn transcript(dir: &std::path::Path, id: &str, hours_ago: i64, input: u64, output: u64) -> PathBuf {
    let start = now().timestamp() - hours_ago * 3_600;
    let at = chrono::DateTime::from_timestamp(start, 0)
        .unwrap()
        .to_rfc3339();
    let later = chrono::DateTime::from_timestamp(start + 60, 0)
        .unwrap()
        .to_rfc3339();
    let user = serde_json::json!({
        "type": "user", "uuid": format!("u-{id}"), "sessionId": id,
        "cwd": format!("/work/{id}"), "timestamp": at,
        "message": {"role": "user", "content": "hi"}
    });
    let assistant = serde_json::json!({
        "type": "assistant", "uuid": format!("a-{id}"), "requestId": format!("r-{id}"), "sessionId": id,
        "timestamp": later,
        "message": {
            "id": format!("m-{id}"), "role": "assistant", "model": "demo-model",
            "content": [{"type": "text", "text": "done"}],
            "usage": {"input_tokens": input, "output_tokens": output}
        }
    });
    let file = dir.join(format!("{id}.jsonl"));
    std::fs::write(&file, format!("{user}\n{assistant}\n")).unwrap();
    file
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[test]
fn collect_writes_the_snapshot_and_digest_reads_it() {
    let state = temp("state");
    let given = temp("given");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(&given).unwrap();
    transcript(&given, "fresh", 1, 1_400_000, 302_000);
    // Three days old: kept in the snapshot, outside a first digest's day.
    transcript(&given, "older", 72, 900, 100);

    let out = vessa(&["collect"], &state, given.to_str().unwrap());
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout(&out).starts_with("vessa: collected 2 sessions"),
        "{}",
        stdout(&out)
    );
    let snapshot: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(state.join("vessa/collected.json")).unwrap())
            .unwrap();
    assert_eq!(snapshot["schema"], "vessa-collected.v1");
    assert_eq!(snapshot["sessions"].as_array().unwrap().len(), 2);
    assert_eq!(snapshot["unplaced"], 0);

    // A first digest: the last day only, so the older session waits.
    let out = vessa(&["digest", "--no-collect"], &state, given.to_str().unwrap());
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let said = stdout(&out);
    assert!(said.contains("fresh  1.4M in · 302k out"), "{said}");
    assert!(!said.contains("older"), "{said}");
    assert!(
        said.contains("by agent: claude: 1 session · 1.4M in · 302k out"),
        "{said}"
    );
    // A piped stdout is not a person at a terminal: no cursor moves.
    assert!(!state.join("vessa/digest.json").exists());

    // Asked with --since, the whole week answers, and nothing moves.
    let out = vessa(
        &["digest", "--no-collect", "--since", "7d"],
        &state,
        given.to_str().unwrap(),
    );
    let said = stdout(&out);
    assert!(said.contains("older  900 in · 100 out"), "{said}");
    assert!(!state.join("vessa/digest.json").exists());
    std::fs::remove_dir_all(state).unwrap();
    std::fs::remove_dir_all(given).unwrap();
}

#[test]
fn a_digest_reads_its_window_from_its_cursor() {
    let state = temp("state");
    let given = temp("given");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(&given).unwrap();
    transcript(&given, "fresh", 1, 1_400_000, 302_000);
    transcript(&given, "older", 72, 900, 100);
    vessa(&["collect"], &state, given.to_str().unwrap());
    // A cursor from three days ago: the older session is in the window.
    std::fs::write(
        state.join("vessa/digest.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "schema": "vessa-digest-cursor.v1",
            "at": chrono::DateTime::from_timestamp(now().timestamp() - 96 * 3_600, 0)
                .unwrap()
                .to_rfc3339(),
            "shown_at": "2001-01-01T00:00:00+00:00"
        }))
        .unwrap(),
    )
    .unwrap();
    let out = vessa(
        &["digest", "--no-collect", "--json"],
        &state,
        given.to_str().unwrap(),
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let record: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(record["schema"], "vessa-digest.v1");
    assert_eq!(record["sessions"].as_array().unwrap().len(), 2);
    std::fs::remove_dir_all(state).unwrap();
    std::fs::remove_dir_all(given).unwrap();
}

#[test]
fn a_stale_snapshot_is_collected_by_the_digest_itself() {
    let state = temp("state");
    let given = temp("given");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(&given).unwrap();
    transcript(&given, "fresh", 0, 1_400_000, 302_000);
    // A snapshot from an hour ago that holds nothing.
    std::fs::create_dir_all(state.join("vessa")).unwrap();
    std::fs::write(
        state.join("vessa/collected.json"),
        serde_json::to_string(&serde_json::json!({
            "schema": "vessa-collected.v1",
            "at": chrono::DateTime::from_timestamp(now().timestamp() - 3_600, 0)
                .unwrap()
                .to_rfc3339(),
            "sessions": [],
            "unplaced": 0,
            "warnings": []
        }))
        .unwrap(),
    )
    .unwrap();
    let out = vessa(&["digest"], &state, given.to_str().unwrap());
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let said = stdout(&out);
    assert!(said.contains("fresh  1.4M in · 302k out"), "{said}");
    // The digest's collect refreshed the snapshot.
    let snapshot: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(state.join("vessa/collected.json")).unwrap())
            .unwrap();
    let ids: Vec<&str> = snapshot["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|session| session["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        vec!["fresh"],
        "the snapshot holds exactly the given sessions"
    );
    std::fs::remove_dir_all(state).unwrap();
    std::fs::remove_dir_all(given).unwrap();
}

#[test]
fn nothing_collected_yet_is_said_and_nothing_is_invented() {
    let state = temp("state");
    let given = temp("given");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(&given).unwrap();
    let out = vessa(&["digest", "--no-collect"], &state, given.to_str().unwrap());
    assert_eq!(out.status.code(), Some(4));
    assert!(
        stdout(&out).contains("nothing was collected yet"),
        "{}",
        stdout(&out)
    );
    std::fs::remove_dir_all(state).unwrap();
    std::fs::remove_dir_all(given).unwrap();
}

#[test]
fn one_collect_at_a_time_and_a_dead_lock_is_taken_over() {
    let state = temp("state");
    let given = temp("given");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(&given).unwrap();
    transcript(&given, "fresh", 0, 10, 10);
    std::fs::create_dir_all(state.join("vessa")).unwrap();
    // A lock held now: said, and no snapshot is written over it.
    std::fs::write(state.join("vessa/collect.lock"), b"").unwrap();
    let out = vessa(&["collect"], &state, given.to_str().unwrap());
    assert_eq!(out.status.code(), Some(0));
    assert!(
        stdout(&out).is_empty()
            && String::from_utf8_lossy(&out.stderr).contains("a collect is running already"),
        "{} / {}",
        stdout(&out),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!state.join("vessa/collected.json").exists());
    // A lock whose holder died hours ago is taken over: the collect runs.
    let stale = state.join("vessa/collect.lock");
    let old = chrono::DateTime::from_timestamp(now().timestamp() - 3_600, 0).unwrap();
    std::fs::write(&stale, b"").unwrap();
    let stamp = old.format("%Y%m%d%H%M.%S").to_string();
    std::process::Command::new("touch")
        .arg("-t")
        .arg(&stamp)
        .arg(&stale)
        .status()
        .unwrap();
    let out = vessa(&["collect"], &state, given.to_str().unwrap());
    assert_eq!(out.status.code(), Some(0));
    assert!(state.join("vessa/collected.json").exists());
    // And the lock is gone: the collect released it.
    assert!(!stale.exists());
    std::fs::remove_dir_all(state).unwrap();
    std::fs::remove_dir_all(given).unwrap();
}

#[test]
fn what_cannot_be_read_is_counted_never_hidden() {
    let state = temp("state");
    let given = temp("given");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(&given).unwrap();
    transcript(&given, "fresh", 0, 10, 10);
    // A session no record places in time, and a source that is not there.
    std::fs::write(
        given.join("unplaced.jsonl"),
        concat!(
            "{\"type\":\"user\",\"uuid\":\"u9\",\"sessionId\":\"s-none\",\"cwd\":\"/work/x\",",
            "\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n"
        ),
    )
    .unwrap();
    let missing = given.join("missing.jsonl");
    let mut command = Command::new(env!("CARGO_BIN_EXE_vessa"));
    command
        .args(["collect", "--input", given.to_str().unwrap()])
        .arg("--input")
        .arg(missing.to_str().unwrap())
        .env("XDG_STATE_HOME", &state)
        .env_remove("HOME");
    for marker in [
        "PI_SESSION_ID",
        "CLAUDECODE",
        "CODEX_SESSION_ID",
        "AI_AGENT",
    ] {
        command.env_remove(marker);
    }
    let out = command.output().unwrap();
    assert_eq!(out.status.code(), Some(4));
    let out = vessa(
        &["digest", "--no-collect", "--json"],
        &state,
        given.to_str().unwrap(),
    );
    assert_eq!(out.status.code(), Some(4));
    let record: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(record["unplaced"], 1);
    assert_eq!(record["warnings"].as_array().unwrap().len(), 1);
    let out = vessa(&["digest", "--no-collect"], &state, given.to_str().unwrap());
    assert!(stdout(&out).contains("not all read:"), "{}", stdout(&out));
    std::fs::remove_dir_all(state).unwrap();
    std::fs::remove_dir_all(given).unwrap();
}

#[test]
fn the_digest_names_the_work_and_what_waits() {
    let state = temp("state");
    let given = temp("given");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(&given).unwrap();
    transcript(&given, "fresh", 0, 1_400_000, 302_000);
    // A git repository the session ran in: sykli says its main is proven.
    let repo = temp("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&repo)
        .output()
        .unwrap();
    let cwd = repo.display().to_string();
    let at = chrono::DateTime::from_timestamp(now().timestamp(), 0)
        .unwrap()
        .to_rfc3339();
    let user = serde_json::json!({
        "type": "user", "uuid": "u-r", "sessionId": "in-a-repo",
        "cwd": cwd, "timestamp": at,
        "message": {"role": "user", "content": "hi"}
    });
    std::fs::write(given.join("repo.jsonl"), format!("{user}\n")).unwrap();
    // The sykli of the test: one fixed answer, whatever it is asked.
    let fake = temp("bin");
    std::fs::create_dir_all(&fake).unwrap();
    let answer = serde_json::json!({
        "schema": "sykli-branch-status.v1",
        "base": {"branch": "main", "commit": "0000000000000000000000000000000000000000"},
        "branches": [{
            "branch": "main", "current": true,
            "proof": "proven", "review": "-",
            "ahead": 0, "behind": 0
        }],
        "warnings": []
    });
    std::fs::write(
        fake.join("sykli"),
        format!("#!/bin/sh\necho '{}'\n", answer),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(fake.join("sykli"), std::fs::Permissions::from_mode(0o755)).unwrap();
    // Three proposals wait for the person.
    std::fs::create_dir_all(state.join("ohjaa/proposals/ours")).unwrap();
    for name in ["a.md", "b.md", "c.md"] {
        std::fs::write(state.join("ohjaa/proposals/ours").join(name), "x").unwrap();
    }

    let path = std::env::var_os("PATH").unwrap();
    let run = |args: &[&str]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_vessa"));
        command
            .args(args)
            .arg("--input")
            .arg(given.to_str().unwrap())
            .env("XDG_STATE_HOME", &state)
            .env("PATH", {
                let mut paths = std::env::split_paths(&path).collect::<Vec<_>>();
                paths.insert(0, fake.clone());
                std::env::join_paths(paths).unwrap()
            })
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
    };

    let out = run(&["collect"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = run(&["digest", "--no-collect"]);
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        said.contains(&format!(
            "{}  main proven",
            repo.file_name().unwrap().to_string_lossy()
        )),
        "the digest must name the repository and its proof: {said}"
    );
    assert!(
        said.contains("waiting for you: 3 proposals in ohjaa"),
        "the digest must count what waits: {said}"
    );
    // The line carries what waits, from the fresh snapshot.
    let out = run(&["line", "--always"]);
    let line = String::from_utf8_lossy(&out.stdout);
    assert!(
        line.contains("3 waiting for you (vessa digest)"),
        "the line must carry what waits: {line}"
    );
    std::fs::remove_dir_all(state).unwrap();
    std::fs::remove_dir_all(given).unwrap();
    std::fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_missing_sykli_is_said_not_guessed() {
    let state = temp("state");
    let given = temp("given");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(&given).unwrap();
    let repo = temp("repo2");
    std::fs::create_dir_all(&repo).unwrap();
    std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&repo)
        .output()
        .unwrap();
    let at = chrono::DateTime::from_timestamp(now().timestamp(), 0)
        .unwrap()
        .to_rfc3339();
    let user = serde_json::json!({
        "type": "user", "uuid": "u-r2", "sessionId": "in-a-repo2",
        "cwd": repo.display().to_string(), "timestamp": at,
        "message": {"role": "user", "content": "hi"}
    });
    std::fs::write(given.join("repo2.jsonl"), format!("{user}\n")).unwrap();
    // A PATH with git but no sykli: the repository is found, its judge is
    // not, and that is said.
    let fake = temp("bin2");
    std::fs::create_dir_all(&fake).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/usr/bin/git", fake.join("git")).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_vessa"))
        .args(["collect"])
        .arg("--input")
        .arg(given.to_str().unwrap())
        .env("XDG_STATE_HOME", &state)
        .env("PATH", &fake)
        .env_remove("HOME")
        .env_remove("PI_SESSION_ID")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let snapshot: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(state.join("vessa/collected.json")).unwrap())
            .unwrap();
    let repos = snapshot["repos"].as_array().unwrap();
    assert!(!repos.is_empty(), "the repository is still named");
    assert!(
        repos[0]["warning"]
            .as_str()
            .is_some_and(|why| why.contains("sykli")),
        "the missing sykli must be said: {repos:?}"
    );
    std::fs::remove_dir_all(state).unwrap();
    std::fs::remove_dir_all(given).unwrap();
    std::fs::remove_dir_all(repo).unwrap();
}
