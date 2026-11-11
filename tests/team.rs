//! `vessa share` and `vessa team` against real git: a bare remote, clones
//! with their own identities, and synthetic sessions whose recorded folder
//! is inside, or outside, each clone.

use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// A bare `origin.git` whose `main` holds one commit, and clones of it.
struct World {
    root: PathBuf,
}

impl World {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "vessa-team-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        // Git reads no configuration of this machine's, only this.
        std::fs::write(root.join("gitconfig"), "[init]\n\tdefaultBranch = main\n").unwrap();
        let world = World { root };
        world.git(&world.root, &["init", "-q", "--bare", "origin.git"]);
        let seed = world.root.join("seed");
        world.git(&world.root, &["init", "-q", "seed"]);
        world.git(&seed, &["config", "user.name", "Seed"]);
        world.git(&seed, &["config", "user.email", "seed@example.org"]);
        std::fs::write(seed.join("README.md"), "# Widget\n").unwrap();
        world.git(&seed, &["add", "README.md"]);
        world.git(&seed, &["commit", "-q", "-m", "initial"]);
        let origin = world.origin();
        world.git(&seed, &["push", "-q", origin.to_str().unwrap(), "main"]);
        world
    }

    fn origin(&self) -> PathBuf {
        self.root.join("origin.git")
    }

    /// A clone of origin, as `name <email>`.
    fn clone(&self, dir: &str, name: &str, email: &str) -> PathBuf {
        let origin = self.origin();
        self.git(&self.root, &["clone", "-q", origin.to_str().unwrap(), dir]);
        let clone = self.root.join(dir);
        self.git(&clone, &["config", "user.name", name]);
        self.git(&clone, &["config", "user.email", email]);
        clone
    }

    fn command(&self, program: &str, dir: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", self.root.join("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1");
        for name in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_COMMON_DIR",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        ] {
            command.env_remove(name);
        }
        command
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let out = self.command("git", dir).args(args).output().unwrap();
        assert!(
            out.status.success(),
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    fn git_in(&self, dir: &Path, args: &[&str], input: &str) -> String {
        let mut child = self
            .command("git", dir)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    fn vessa(&self, dir: &Path, args: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_vessa"), dir)
            .args(args)
            .output()
            .unwrap()
    }

    /// A folder of session files, each given as its name and lines.
    fn sessions(&self, dir: &str, files: &[(&str, String)]) -> PathBuf {
        let folder = self.root.join(dir);
        std::fs::create_dir_all(&folder).unwrap();
        for (name, lines) in files {
            std::fs::write(folder.join(name), lines).unwrap();
        }
        folder
    }

    /// `share` reading the sessions in `folder`, with `more` arguments.
    fn share(&self, clone: &Path, folder: &Path, more: &[&str]) -> Output {
        let mut args = vec!["share", "--input", folder.to_str().unwrap()];
        args.extend(more);
        self.vessa(clone, &args)
    }

    /// The person's ref in `dir`, the one under refs/vessa/usage/.
    fn usage_ref(&self, dir: &Path) -> String {
        let refs = self.git(
            dir,
            &["for-each-ref", "--format=%(refname)", "refs/vessa/usage/"],
        );
        let refs: Vec<&str> = refs.lines().collect();
        assert_eq!(refs.len(), 1, "{refs:?}");
        refs[0].to_owned()
    }

    fn record_at(&self, dir: &Path, rev: &str) -> String {
        self.git(dir, &["cat-file", "blob", &format!("{rev}:usage.json")])
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// A Claude Code session that recorded `cwd` as its folder: one request of
/// `input` uncached input tokens, and a person's words around it.
fn claude(id: &str, cwd: &Path, branch: Option<&str>, input: u64) -> String {
    let mut first = json!({"type":"user","uuid":format!("{id}-u1"),"sessionId":id,"cwd":text(cwd),
        "timestamp":"2026-09-30T09:00:00Z","entrypoint":"cli","isSidechain":false,
        "message":{"role":"user","content":"Please run the local checks."}});
    if let Some(branch) = branch {
        first["gitBranch"] = branch.into();
    }
    let answer = json!({"type":"assistant","uuid":format!("{id}-a1"),"requestId":format!("req_{id}"),
        "sessionId":id,"cwd":text(cwd),"timestamp":"2026-09-30T09:05:00Z","isSidechain":false,
        "message":{"id":format!("msg_01{id}"),"role":"assistant","model":"demo-model",
            "content":[{"type":"text","text":"The checks pass."}],
            "usage":{"input_tokens":input,"output_tokens":50,"cache_read_input_tokens":200,"cache_creation_input_tokens":10}}});
    format!("{first}\n{answer}\n")
}

/// A Codex session that recorded `cwd` and `branch`.
fn codex(id: &str, cwd: &Path, branch: &str, input: u64) -> String {
    let meta = json!({"type":"session_meta","timestamp":"2026-10-01T10:00:00Z","payload":{
        "id":id,"cwd":text(cwd),"model_provider":"openai","source":"cli","originator":"codex-tui",
        "timestamp":"2026-10-01T10:00:00Z",
        "git":{"branch":branch,"commit_hash":"0123456789abcdef","repository_url":"git@git.example.net:team/widget.git"}}});
    let turn = json!({"type":"turn_context","payload":{"model":"demo-codex","cwd":text(cwd)}});
    let count = json!({"timestamp":"2026-10-01T10:04:00Z","type":"event_msg","payload":{"type":"token_count",
        "info":{"total_token_usage":{"input_tokens":input,"cached_input_tokens":100,"output_tokens":40}}}});
    format!("{meta}\n{turn}\n{count}\n")
}

fn record(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|error| {
        panic!(
            "{error}: stdout {} stderr {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn ids(record: &Value) -> BTreeSet<String> {
    record["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_owned())
        .collect()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn share_selects_the_sessions_recorded_inside_this_clone_and_its_worktrees() {
    let world = World::new("select");
    let alice = world.clone("alice", "Ada Lovelace", "ada@example.org");
    std::fs::create_dir_all(alice.join("src")).unwrap();
    let side = world.root.join("alice-side");
    world.git(
        &alice,
        &["worktree", "add", "-q", "-b", "side", text(&side)],
    );
    let elsewhere = world.root.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    // A sibling whose name starts with the clone's: inside by string, not
    // by path.
    let sibling = world.root.join("alice-other");
    std::fs::create_dir_all(&sibling).unwrap();
    let mut files = vec![
        (
            "main.jsonl",
            claude("in-main", &alice.join("src"), Some("main"), 100),
        ),
        (
            "side.jsonl",
            claude("in-worktree", &side, Some("side"), 200),
        ),
        // A folder since deleted, inside the clone.
        (
            "gone.jsonl",
            claude("in-deleted-folder", &alice.join("gone/deeper"), None, 300),
        ),
        (
            "out.jsonl",
            claude("outside", &elsewhere, Some("main"), 400),
        ),
        (
            "sibling.jsonl",
            claude("sibling", &sibling, Some("main"), 500),
        ),
        // A folder that walks out of the clone.
        (
            "escape.jsonl",
            claude("escape", &alice.join("nothing/../../elsewhere"), None, 600),
        ),
        (
            "relative.jsonl",
            claude("relative", Path::new("src"), None, 700),
        ),
    ];
    // The same clone reached through a symbolic link: the same folder.
    #[cfg(unix)]
    {
        let link = world.root.join("link");
        std::os::unix::fs::symlink(&alice, &link).unwrap();
        files.push((
            "link.jsonl",
            claude("through-link", &link.join("src"), None, 800),
        ));
    }
    let folder = world.sessions("sessions", &files);

    let out = world.share(&alice, &folder, &["--dry-run"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let mut expected: BTreeSet<String> = ["in-main", "in-worktree", "in-deleted-folder"]
        .map(String::from)
        .into();
    if cfg!(unix) {
        expected.insert("through-link".into());
    }
    assert_eq!(ids(&record(&out)), expected);

    // Any worktree of the clone selects the same sessions.
    let out = world.share(&side, &folder, &["--dry-run"]);
    assert_eq!(ids(&record(&out)), expected);

    // A dry run changes nothing.
    assert_eq!(
        world.git(&alice, &["for-each-ref", "refs/vessa/"]),
        "",
        "a dry run wrote a ref"
    );
}

#[test]
fn the_record_holds_numbers_only() {
    let world = World::new("privacy");
    let alice = world.clone("alice", "Ada Lovelace", "ada@example.org");
    let cwd = alice.join("src").join("hidden");
    std::fs::create_dir_all(&cwd).unwrap();
    let secret = ["gh", "p_", "Zq8Lm3Np5Rt7Vx9Bz1Cd3Fg5Hj7Kl9Mn1Pq"].concat();
    let windows = r"C:\Users\ada\private\key.pem";
    let id = "privacy-session";
    let lines = [
        json!({"type":"user","uuid":"p-u1","sessionId":id,"cwd":text(&cwd),"gitBranch":"feat/privacy",
            "timestamp":"2026-09-30T09:00:00Z","entrypoint":"cli","isSidechain":false,
            "message":{"role":"user","content":format!("Deploy with key {secret} from /Users/ada/secret-project/notes.txt and src/hidden/plan.rs")}}),
        json!({"type":"assistant","uuid":"p-a1","requestId":"req_privacy1","sessionId":id,"cwd":text(&cwd),
            "gitBranch":"feat/privacy","timestamp":"2026-09-30T09:01:00Z","isSidechain":false,
            "message":{"id":"msg_01privacy1","role":"assistant","model":"demo-model",
                "content":[{"type":"thinking","thinking":"I am weighing the private plan."},
                    {"type":"text","text":"The frobnicator is fixed."},
                    {"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"git commit -m 'Fix the frobnicator in widget'"}}],
                "usage":{"input_tokens":120,"output_tokens":30,"cache_read_input_tokens":400,"cache_creation_input_tokens":20}}}),
        json!({"type":"user","uuid":"p-u2","sessionId":id,"cwd":text(&cwd),"gitBranch":"feat/privacy",
            "timestamp":"2026-09-30T09:02:00Z","isSidechain":false,
            "toolUseResult":{"stdout":format!("[main 1a2b3c4] Fix the frobnicator in widget, see {windows}"),"filePath":"/Users/ada/secret-project/notes.txt"},
            "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":format!("committed; token {secret}")}]}}),
        json!({"type":"summary","summary":"Ada fixed the frobnicator in /Users/ada/secret-project","leafUuid":"p-a1"}),
        // A launch whose subagent was not read: vessa's limitation quotes
        // the subagent's id, a path here.
        json!({"type":"user","uuid":"p-u3","sessionId":id,"cwd":text(&cwd),"timestamp":"2026-09-30T09:03:00Z",
            "isSidechain":false,"toolUseResult":{"agentId":"from-/Users/ada/secret-project"},
            "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_2","content":"started"}]}}),
        // A local model named by its file.
        json!({"type":"assistant","uuid":"p-a2","requestId":"req_privacy2","sessionId":id,"cwd":text(&cwd),
            "timestamp":"2026-09-30T09:04:00Z","isSidechain":false,
            "message":{"id":"msg_01privacy2","role":"assistant","model":"/Users/ada/secret-project/model.gguf",
                "content":[{"type":"text","text":"Local answer."}],
                "usage":{"input_tokens":1,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}),
        // Local models named by a relative path, and by a Windows one.
        json!({"type":"assistant","uuid":"p-a3","requestId":"req_privacy3","sessionId":id,"cwd":text(&cwd),
            "timestamp":"2026-09-30T09:05:00Z","isSidechain":false,
            "message":{"id":"msg_01privacy3","role":"assistant","model":"models/private.gguf","content":[],
                "usage":{"input_tokens":2,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}),
        json!({"type":"assistant","uuid":"p-a4","requestId":"req_privacy4","sessionId":id,"cwd":text(&cwd),
            "timestamp":"2026-09-30T09:06:00Z","isSidechain":false,
            "message":{"id":"msg_01privacy4","role":"assistant","model":r"weights\private-model","content":[],
                "usage":{"input_tokens":4,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}),
        // A model named in the provider/model shape vessa itself writes.
        json!({"type":"assistant","uuid":"p-a5","requestId":"req_privacy5","sessionId":id,"cwd":text(&cwd),
            "timestamp":"2026-09-30T09:07:00Z","isSidechain":false,
            "message":{"id":"msg_01privacy5","role":"assistant","model":"anthropic/demo-opus","content":[],
                "usage":{"input_tokens":8,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}),
    ];
    let session: String = lines.iter().map(|l| format!("{l}\n")).collect();
    // A Codex session beside it, which records its repository's address
    // and commit; and two sessions whose ids are relative paths, which
    // cannot be shared at all.
    let folder = world.sessions(
        "sessions",
        &[
            ("privacy.jsonl", session),
            (
                "codex.jsonl",
                codex("privacy-codex", &cwd, "feat/privacy", 500),
            ),
            (
                "drafts.jsonl",
                claude("drafts/private-plan.md", &cwd, None, 9),
            ),
            (
                "windows.jsonl",
                claude(r"sessions\private-plan", &cwd, None, 9),
            ),
        ],
    );

    // Partial: the subagent's transcript was not read.
    let dry = world.share(&alice, &folder, &["--dry-run"]);
    assert_eq!(dry.status.code(), Some(4), "{}", stderr(&dry));
    let shared = world.share(&alice, &folder, &["--no-push", "--json"]);
    assert_eq!(shared.status.code(), Some(4), "{}", stderr(&shared));
    let said = stderr(&shared);
    assert!(said.contains("2 sessions of this repository"), "{said}");
    let reference = world.usage_ref(&alice);
    let stored = world.record_at(&alice, &reference);
    // `--json` prints exactly the bytes written.
    assert_eq!(stored.as_bytes(), shared.stdout.trim_ascii_end());

    let root = text(&world.root).to_owned();
    let machine = Command::new("hostname")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default();
    let mut forbidden: Vec<String> = [
        secret.as_str(),
        "secret-project",
        "notes.txt",
        "plan.rs",
        "hidden",
        "key.pem",
        r"C:\Users",
        "frobnicator",
        "Deploy",
        "weighing",
        "Fix the",
        "committed",
        "git.example.net",
        "0123456789abcdef",
        "toolu_1",
        "p-u1",
        "model.gguf",
        "Local answer",
        "private.gguf",
        "models/",
        "weights",
        "private-model",
        "private-plan",
        "drafts",
        // A branch with a separator reads as a path too.
        "feat/privacy",
    ]
    .map(String::from)
    .into();
    // The clone's folder in every spelling it could take.
    for spelling in [
        root.clone(),
        root.replace('\\', "/"),
        root.replace('\\', "\\\\"),
        text(&cwd).to_owned(),
        text(&alice).to_owned(),
        world.root.file_name().unwrap().to_str().unwrap().to_owned(),
    ] {
        forbidden.push(spelling);
    }
    if machine.len() >= 4 {
        forbidden.push(machine);
    }
    for bytes in [&dry.stdout[..], stored.as_bytes()] {
        let written = String::from_utf8_lossy(bytes);
        for word in &forbidden {
            assert!(
                !written.contains(word.as_str()),
                "{word:?} reached the record:\n{written}"
            );
        }
    }

    // Every field is one the format names, and every string one of these.
    let v: Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(ids(&v), [id, "privacy-codex"].map(String::from).into());
    let keys = |v: &Value| {
        v.as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>()
    };
    let set = |names: &[&str]| names.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>();
    assert_eq!(
        keys(&v),
        set(&[
            "schema",
            "person",
            "shared_at",
            "entries_include_subagents",
            "source_warnings",
            "sessions"
        ])
    );
    assert_eq!(keys(&v["person"]), set(&["name", "email"]));
    let s = &v["sessions"][0];
    assert_eq!(
        keys(s),
        set(&[
            "agent",
            "id",
            "started_at",
            "ended_at",
            "branch",
            "subagents",
            "partial",
            "limitations",
            "entries"
        ])
    );
    assert_eq!(
        keys(&s["entries"][0]),
        set(&[
            "provider",
            "model",
            "input_tokens",
            "output_tokens",
            "cache_read_tokens",
            "cache_write_tokens"
        ])
    );
    let allowed = set(&[
        "vessa-team-usage.v1",
        "Ada Lovelace",
        "ada@example.org",
        "claude",
        id,
        "anthropic",
        "demo-model",
        "anthropic/demo-opus",
        // The local model's file, withheld.
        "[path]",
        // vessa's limitation, by kind.
        "subagent_not_read",
        "codex",
        "privacy-codex",
        "openai",
        "demo-codex",
    ]);
    fn strings(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => out.push(s.clone()),
            Value::Array(items) => items.iter().for_each(|i| strings(i, out)),
            Value::Object(fields) => fields.values().for_each(|f| strings(f, out)),
            _ => {}
        }
    }
    let mut all = Vec::new();
    strings(&v, &mut all);
    for string in all {
        assert!(
            allowed.contains(&string) || chrono_like(&string),
            "unexpected string {string:?} in the record"
        );
    }
    let entry = |model: &str| {
        s["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["model"] == model)
            .cloned()
            .unwrap()
    };
    assert_eq!(entry("demo-model")["input_tokens"], 120);
    assert_eq!(entry("anthropic/demo-opus")["input_tokens"], 8);
    // The three local models, withheld alike, are one entry.
    assert_eq!(entry("[path]")["input_tokens"], 7);
    assert_eq!(
        s["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["model"] == "[path]")
            .count(),
        1
    );
    assert_eq!(s["branch"], "[path]");
    assert_eq!(v["sessions"][1]["branch"], "[path]");
    assert_eq!(v["source_warnings"], 1);
    assert_eq!(s["limitations"], json!({"subagent_not_read": 1}));

    // The commit holds the one file, and its message and author no path.
    let commit = world.git(&alice, &["cat-file", "commit", &reference]);
    for word in &forbidden {
        assert!(
            !commit.contains(word.as_str()),
            "{word:?} reached the commit"
        );
    }
    assert_eq!(
        world.git(&alice, &["ls-tree", "--name-only", &reference]),
        "usage.json"
    );
}

/// An RFC 3339 time in UTC, as the record writes its times.
fn chrono_like(s: &str) -> bool {
    s.len() >= 20
        && s.ends_with('Z')
        && s.as_bytes()[4] == b'-'
        && s.as_bytes()[10] == b'T'
        && s.chars().all(|c| c.is_ascii_digit() || "-:TZ.".contains(c))
}

#[test]
fn sharing_again_keeps_each_record_and_a_history_of_shares() {
    let world = World::new("again");
    let alice = world.clone("alice", "Ada Lovelace", "Ada@Example.org");
    let folder = world.sessions(
        "sessions",
        &[
            ("one.jsonl", claude("one", &alice, Some("main"), 100)),
            ("two.jsonl", claude("two", &alice, Some("main"), 200)),
        ],
    );
    let first = world.share(&alice, &folder, &["--no-push"]);
    assert_eq!(first.status.code(), Some(0), "{}", stderr(&first));
    let reference = world.usage_ref(&alice);
    // The key is a short digest of the lower-cased email, which the ref
    // does not spell.
    let digest = world.git_in(&alice, &["hash-object", "--stdin"], "ada@example.org");
    assert_eq!(reference, format!("refs/vessa/usage/{}", &digest[..16]));
    let before: Value = serde_json::from_str(&world.record_at(&alice, &reference)).unwrap();
    let first_commit = world.git(&alice, &["rev-parse", &reference]);

    let second = world.share(&alice, &folder, &["--no-push"]);
    assert_eq!(second.status.code(), Some(0), "{}", stderr(&second));
    let after: Value = serde_json::from_str(&world.record_at(&alice, &reference)).unwrap();
    assert_eq!(before["sessions"], after["sessions"]);
    assert_eq!(after["sessions"].as_array().unwrap().len(), 2);
    // A new commit for each share, on the one before.
    assert_eq!(
        world.git(&alice, &["rev-parse", &format!("{reference}^")]),
        first_commit
    );
    assert_eq!(world.git(&alice, &["rev-list", "--count", &reference]), "2");

    // A session no longer found here keeps the record it was shared with;
    // a session whose numbers changed is shared with its new ones.
    std::fs::remove_file(folder.join("two.jsonl")).unwrap();
    std::fs::write(
        folder.join("one.jsonl"),
        claude("one", &alice, Some("main"), 150),
    )
    .unwrap();
    let third = world.share(&alice, &folder, &["--no-push"]);
    assert_eq!(third.status.code(), Some(0), "{}", stderr(&third));
    let last: Value = serde_json::from_str(&world.record_at(&alice, &reference)).unwrap();
    let by_id = |v: &Value, id: &str| {
        v["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == id)
            .cloned()
            .unwrap()
    };
    assert_eq!(by_id(&last, "two"), by_id(&before, "two"));
    assert_eq!(by_id(&last, "one")["entries"][0]["input_tokens"], 150);
}

#[test]
fn two_people_share_and_team_shows_both() {
    let world = World::new("two");
    let alice = world.clone("alice", "Ada Lovelace", "ada@example.org");
    let bob = world.clone("bob", "Grace Hopper", "grace@example.org");
    let side = world.root.join("alice-side");
    world.git(
        &alice,
        &["worktree", "add", "-q", "-b", "side", text(&side)],
    );
    let theirs = world.sessions(
        "sessions-alice",
        &[
            ("a1.jsonl", claude("alice-1", &alice, Some("main"), 100)),
            ("a2.jsonl", claude("alice-2", &side, Some("side"), 200)),
        ],
    );
    // Grace's Claude session recorded no cache writes: unknown, not zero.
    let unrecorded = claude("bob-2", &bob, Some("feat/parser"), 300)
        .replace("\"cache_creation_input_tokens\":10,", "");
    assert!(!unrecorded.contains("cache_creation"));
    let his = world.sessions(
        "sessions-bob",
        &[
            (
                "b1.jsonl",
                codex("bob-1", &bob.join("src"), "feat/parser", 1000),
            ),
            ("b2.jsonl", unrecorded),
        ],
    );
    let out = world.share(&alice, &theirs, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let out = world.share(&bob, &his, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let shared = world.git(
        &world.origin(),
        &["for-each-ref", "--format=%(refname)", "refs/vessa/"],
    );
    assert_eq!(shared.lines().count(), 2, "{shared}");

    let out = world.vessa(&alice, &["team", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let v = record(&out);
    assert_eq!(v["schema"], "vessa-team.v1");
    assert_eq!(v["fetched"], true);
    let people = v["people"].as_array().unwrap();
    assert_eq!(people.len(), 2);
    assert_eq!(people[0]["name"], "Ada Lovelace");
    assert_eq!(people[1]["name"], "Grace Hopper");
    assert_eq!(people[1]["sessions"].as_array().unwrap().len(), 2);
    assert_eq!(v["total"]["people"], 2);
    assert_eq!(v["total"]["sessions"], 4);
    assert_eq!(v["total"]["partial_sessions"], 0);
    let line = |agent: &str| {
        v["total"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["agent"] == agent)
            .cloned()
            .unwrap()
    };
    assert_eq!(line("claude")["input_tokens"], 600);
    assert_eq!(line("claude")["sessions"], 3);
    assert_eq!(line("claude")["cache_write_tokens"], Value::Null);
    assert_eq!(
        people[0]["sessions"][0]["entries"][0]["cache_write_tokens"],
        10
    );
    assert_eq!(line("codex")["input_tokens"], 900);
    assert_eq!(line("codex")["cache_write_tokens"], 0);

    let plain = world.vessa(&alice, &["team"]);
    assert_eq!(plain.status.code(), Some(0), "{}", stderr(&plain));
    let plain = String::from_utf8(plain.stdout).unwrap();
    assert!(plain.contains("2 people have shared"), "{plain}");
    assert!(plain.contains("Ada Lovelace <ada@example.org>"), "{plain}");
    assert!(
        plain.contains("Grace Hopper <grace@example.org>"),
        "{plain}"
    );
    assert!(plain.contains("Team total"), "{plain}");

    // Regrouped by branch: what each branch used, across the team.
    let out = world.vessa(&alice, &["team", "--by", "branch", "--json", "--no-fetch"]);
    let v = record(&out);
    let groups: Vec<(Value, Value)> = v["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| (g["key"].clone(), g["sessions"].clone()))
        .collect();
    assert_eq!(
        groups,
        vec![
            // A branch name with a separator is withheld as a path.
            (json!("[path]"), json!(2)),
            (json!("main"), json!(1)),
            (json!("side"), json!(1)),
        ]
    );
    for by in ["agent", "model", "week"] {
        let out = world.vessa(&alice, &["team", "--by", by, "--no-fetch"]);
        assert_eq!(out.status.code(), Some(0), "--by {by}: {}", stderr(&out));
    }
    let out = world.vessa(&alice, &["team", "--by", "week", "--json", "--no-fetch"]);
    let weeks: Vec<Value> = record(&out)["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["key"].clone())
        .collect();
    assert_eq!(weeks, vec![json!("2026-W40")]);
}

#[test]
fn a_push_the_remote_refuses_overwrites_nothing() {
    let world = World::new("refused");
    let alice = world.clone("alice", "Ada Lovelace", "ada@example.org");
    // The same person in a second clone, or someone else with that email.
    let again = world.clone("again", "Ada Lovelace", "Ada@Example.org");
    let first = world.sessions(
        "sessions-alice",
        &[("a.jsonl", claude("alice-1", &alice, None, 100))],
    );
    let second = world.sessions(
        "sessions-again",
        &[("b.jsonl", claude("again-1", &again, None, 200))],
    );
    let out = world.share(&alice, &first, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let reference = world.usage_ref(&alice);
    let on_origin = world.git(&world.origin(), &["rev-parse", &reference]);

    let out = world.share(&again, &second, &[]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let said = stderr(&out);
    assert!(said.contains("Nothing on origin was overwritten"), "{said}");
    assert!(said.contains("run `vessa share` again"), "{said}");
    assert_eq!(
        world.git(&world.origin(), &["rev-parse", &reference]),
        on_origin
    );
    // The share is kept in that clone.
    assert_eq!(world.usage_ref(&again), reference);

    // Shared again, it keeps the sessions of both and pushes.
    let out = world.share(&again, &second, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let merged = world.git(&world.origin(), &["rev-parse", &reference]);
    assert_ne!(merged, on_origin);
    let parents = world.git(
        &world.origin(),
        &["rev-list", "--parents", "-n", "1", &reference],
    );
    assert_eq!(parents.split(' ').count(), 3, "{parents}");
    let v: Value = serde_json::from_str(&world.record_at(&world.origin(), &reference)).unwrap();
    assert_eq!(ids(&v), ["again-1", "alice-1"].map(String::from).into());
}

#[test]
fn team_without_fetch_reads_only_this_clone() {
    let world = World::new("no-fetch");
    let alice = world.clone("alice", "Ada Lovelace", "ada@example.org");
    let bob = world.clone("bob", "Grace Hopper", "grace@example.org");
    let theirs = world.sessions(
        "sessions-alice",
        &[("a.jsonl", claude("alice-1", &alice, None, 100))],
    );
    let his = world.sessions(
        "sessions-bob",
        &[("b.jsonl", claude("bob-1", &bob, None, 300))],
    );
    assert_eq!(world.share(&alice, &theirs, &[]).status.code(), Some(0));
    assert_eq!(world.share(&bob, &his, &[]).status.code(), Some(0));

    let names = |out: Output| -> Vec<String> {
        assert!(out.status.success(), "{}", stderr(&out));
        record(&out)["people"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap().to_owned())
            .collect()
    };
    // Alice has not fetched Grace's share yet.
    assert_eq!(
        names(world.vessa(&alice, &["team", "--json", "--no-fetch"])),
        ["Ada Lovelace"]
    );
    // A clone that never fetched holds no shares: nobody, never zero.
    let carol = world.clone("carol", "Carol", "carol@example.org");
    let out = world.vessa(&carol, &["team", "--json", "--no-fetch"]);
    assert_eq!(names(out), Vec::<String>::new());
    let out = world.vessa(&carol, &["team", "--no-fetch"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("Nobody has shared"));

    // A share kept in this clone only is never replaced by a fetch.
    let more = world.sessions(
        "sessions-alice-more",
        &[
            ("a.jsonl", claude("alice-1", &alice, None, 100)),
            ("c.jsonl", claude("alice-2", &alice, None, 50)),
        ],
    );
    assert_eq!(
        world.share(&alice, &more, &["--no-push"]).status.code(),
        Some(0)
    );
    let reference = world.usage_ref(&alice);
    let kept = world.git(&alice, &["rev-parse", &reference]);
    let out = world.vessa(&alice, &["team", "--json"]);
    assert_eq!(world.git(&alice, &["rev-parse", &reference]), kept);
    let v = record(&out);
    assert_eq!(v["people"].as_array().unwrap().len(), 2);
    let ada = &v["people"][0];
    assert_eq!(ada["sessions"].as_array().unwrap().len(), 2);
    assert_eq!(ada["pushed"], false);
    assert_eq!(v["people"][1]["pushed"], true);

    // `team` reads only what was shared, never session files.
    let out = world.vessa(&alice, &["team", "--input", text(&theirs)]);
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn a_partial_session_is_shared_as_partial() {
    let world = World::new("partial");
    let alice = world.clone("alice", "Ada Lovelace", "ada@example.org");
    let id = "partial-1";
    let cwd = text(&alice);
    let lines = [
        json!({"type":"user","uuid":"u1","sessionId":id,"cwd":cwd,"timestamp":"2026-09-30T09:00:00Z",
            "entrypoint":"cli","isSidechain":false,"message":{"role":"user","content":"Go."}})
        .to_string(),
        "not a record".to_owned(),
        json!({"type":"user","uuid":"u2","sessionId":id,"cwd":cwd,"timestamp":"2026-09-30T09:01:00Z",
            "isSidechain":false,"toolUseResult":{"agentId":"ghost"},
            "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_9","content":"started"}]}})
        .to_string(),
        json!({"type":"assistant","uuid":"a1","requestId":"req_partial1","sessionId":id,"cwd":cwd,
            "timestamp":"2026-09-30T09:02:00Z","isSidechain":false,
            "message":{"id":"msg_01partial1","role":"assistant","model":"demo-model","content":[],
                "usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}})
        .to_string(),
    ];
    let folder = world.sessions(
        "sessions",
        &[
            ("partial.jsonl", lines.join("\n") + "\n"),
            ("whole.jsonl", claude("whole-1", &alice, None, 100)),
        ],
    );
    let out = world.share(&alice, &folder, &["--json"]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    let v = record(&out);
    let partial = v["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id)
        .unwrap();
    assert_eq!(partial["partial"], true);
    assert_eq!(
        partial["limitations"],
        json!({"malformed_record": 1, "subagent_not_read": 1})
    );
    let whole = v["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == "whole-1")
        .unwrap();
    assert_eq!(whole["partial"], false);
    assert_eq!(whole["limitations"], json!({}));

    let out = world.vessa(&alice, &["team", "--json"]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    assert_eq!(record(&out)["total"]["partial_sessions"], 1);
    let out = world.vessa(&alice, &["team"]);
    assert_eq!(out.status.code(), Some(4));
    let plain = String::from_utf8_lossy(&out.stdout);
    assert!(plain.contains("partial-1"), "{plain}");
    assert!(plain.contains("subagent_not_read"), "{plain}");
}

/// A Claude Code subagent `agent` of session `session`, recorded in `cwd`.
fn subagent(session: &str, agent: &str, cwd: &Path, input: u64) -> String {
    let task = json!({"type":"user","uuid":format!("{agent}-u1"),"sessionId":session,"agentId":agent,
        "isSidechain":true,"cwd":text(cwd),"timestamp":"2026-09-30T09:02:00Z","entrypoint":"cli",
        "message":{"role":"user","content":"Look into it."}});
    let answer = json!({"type":"assistant","uuid":format!("{agent}-a1"),"requestId":format!("req_{agent}"),
        "sessionId":session,"agentId":agent,"isSidechain":true,"cwd":text(cwd),"timestamp":"2026-09-30T09:03:00Z",
        "message":{"id":format!("msg_01{agent}"),"role":"assistant","model":"demo-model","content":[],
            "usage":{"input_tokens":input,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}});
    format!("{task}\n{answer}\n")
}

#[test]
fn a_subagent_is_shared_once_with_or_without_its_session() {
    let world = World::new("subagent");
    let alice = world.clone("alice", "Ada Lovelace", "ada@example.org");
    let folder = world.root.join("sessions");
    let subagents = folder.join("main-1").join("subagents");
    std::fs::create_dir_all(&subagents).unwrap();
    std::fs::write(
        subagents.join("agent-helper.jsonl"),
        subagent("main-1", "helper", &alice, 70),
    )
    .unwrap();
    let shared = |world: &World| {
        let out = world.share(&alice, &folder, &["--json", "--no-push"]);
        assert!(out.status.code() == Some(0), "{}", stderr(&out));
        record(&out)
    };
    // Its session's transcript is not here: the subagent stands alone.
    let first = shared(&world);
    assert_eq!(ids(&first), ["main-1/helper"].map(String::from).into());

    // Its session appears: the subagent is counted with it, and only there.
    std::fs::write(
        folder.join("main-1.jsonl"),
        claude("main-1", &alice, None, 100),
    )
    .unwrap();
    let second = shared(&world);
    assert_eq!(ids(&second), ["main-1"].map(String::from).into());
    // The record names the subagent, with its own numbers.
    let named = &second["sessions"][0]["subagents"];
    assert_eq!(named.as_array().unwrap().len(), 1);
    assert_eq!(named[0]["id"], "main-1/helper");
    assert_eq!(named[0]["entries"][0]["input_tokens"], 70);
    assert_eq!(second["sessions"][0]["entries"][0]["input_tokens"], 170);

    // The session's transcript goes, the subagent's stays: the session
    // keeps its record, and the subagent is not added beside it.
    std::fs::remove_file(folder.join("main-1.jsonl")).unwrap();
    let third = shared(&world);
    assert_eq!(third["sessions"], second["sessions"]);

    // A subagent the session's record does not hold is shared alone, even
    // though that session was shared.
    let late = folder
        .join("main-1")
        .join("subagents")
        .join("agent-late.jsonl");
    std::fs::write(&late, subagent("main-1", "late", &alice, 30)).unwrap();
    let fourth = shared(&world);
    assert_eq!(
        ids(&fourth),
        ["main-1", "main-1/late"].map(String::from).into()
    );
}

#[test]
fn shares_that_diverged_count_a_subagent_once() {
    let world = World::new("diverged");
    let alice = world.clone("alice", "Ada Lovelace", "ada@example.org");
    let again = world.clone("again", "Ada Lovelace", "ada@example.org");
    // One clone read the session with its subagent, and shared first.
    let first = world.root.join("sessions-alice");
    let subagents = first.join("main-1").join("subagents");
    std::fs::create_dir_all(&subagents).unwrap();
    std::fs::write(
        first.join("main-1.jsonl"),
        claude("main-1", &alice, None, 100),
    )
    .unwrap();
    std::fs::write(
        subagents.join("agent-helper.jsonl"),
        subagent("main-1", "helper", &alice, 70),
    )
    .unwrap();
    let out = world.share(&alice, &first, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    // The other read the subagent alone; its push is refused.
    let second = world.root.join("sessions-again");
    let subagents = second.join("main-1").join("subagents");
    std::fs::create_dir_all(&subagents).unwrap();
    std::fs::write(
        subagents.join("agent-helper.jsonl"),
        subagent("main-1", "helper", &again, 70),
    )
    .unwrap();
    let out = world.share(&again, &second, &[]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    // Shared again, it follows both: the subagent is counted with its
    // session, and not beside it as well.
    let out = world.share(&again, &second, &["--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let v = record(&out);
    assert_eq!(ids(&v), ["main-1"].map(String::from).into());
    assert_eq!(v["sessions"][0]["entries"][0]["input_tokens"], 170);
}

#[test]
fn a_subagent_that_recorded_more_after_its_session_was_shared_makes_it_partial() {
    let world = World::new("grew");
    let alice = world.clone("alice", "Ada Lovelace", "ada@example.org");
    let folder = world.root.join("sessions");
    let subagents = folder.join("main-1").join("subagents");
    std::fs::create_dir_all(&subagents).unwrap();
    std::fs::write(
        folder.join("main-1.jsonl"),
        claude("main-1", &alice, None, 100),
    )
    .unwrap();
    let helper = subagents.join("agent-helper.jsonl");
    std::fs::write(&helper, subagent("main-1", "helper", &alice, 70)).unwrap();
    let out = world.share(&alice, &folder, &["--no-push"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));

    // The session's transcript goes; its subagent holds a request more than
    // when the session was shared, dated before the session's last one, so
    // no time can tell it is new.
    std::fs::remove_file(folder.join("main-1.jsonl")).unwrap();
    let later = json!({"type":"assistant","uuid":"helper-a2","requestId":"req_helper2","sessionId":"main-1",
        "agentId":"helper","isSidechain":true,"cwd":text(&alice),"timestamp":"2026-09-30T09:04:00Z",
        "message":{"id":"msg_01helper2","role":"assistant","model":"demo-model","content":[],
            "usage":{"input_tokens":30,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}});
    std::fs::write(
        &helper,
        subagent("main-1", "helper", &alice, 70) + &format!("{later}\n"),
    )
    .unwrap();
    let out = world.share(&alice, &folder, &["--json", "--no-push"]);
    // Counting the subagent beside its session would count its first
    // request twice; leaving the later one out leaves the session partial.
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    let v = record(&out);
    assert_eq!(ids(&v), ["main-1"].map(String::from).into());
    let s = &v["sessions"][0];
    assert_eq!(s["entries"][0]["input_tokens"], 170);
    assert_eq!(s["partial"], true);
    assert_eq!(s["limitations"], json!({"subagent_changed_after_share": 1}));
    // Shared again, the same.
    let again = world.share(&alice, &folder, &["--json", "--no-push"]);
    assert_eq!(record(&again)["sessions"], v["sessions"]);
}

#[test]
fn a_subagent_shared_alone_stays_when_its_session_is_not_shared() {
    let world = World::new("elsewhere");
    let alice = world.clone("alice", "Ada Lovelace", "ada@example.org");
    let outside = world.root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let folder = world.root.join("sessions");
    let subagents = folder.join("main-1").join("subagents");
    std::fs::create_dir_all(&subagents).unwrap();
    std::fs::write(
        subagents.join("agent-helper.jsonl"),
        subagent("main-1", "helper", &alice, 70),
    )
    .unwrap();
    std::fs::write(
        folder.join("other-1.jsonl"),
        claude("other-1", &alice, None, 5),
    )
    .unwrap();
    let out = world.share(&alice, &folder, &["--no-push"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    // Its session appears, recorded outside this clone: it is not this
    // repository's, so the subagent's record stays.
    std::fs::write(
        folder.join("main-1.jsonl"),
        claude("main-1", &outside, None, 100),
    )
    .unwrap();
    let out = world.share(&alice, &folder, &["--json", "--no-push"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let v = record(&out);
    assert_eq!(
        ids(&v),
        ["main-1/helper", "other-1"].map(String::from).into()
    );
    assert_eq!(v["sessions"][0]["entries"][0]["input_tokens"], 70);
}

#[test]
fn a_share_stays_partial_while_it_keeps_sessions_of_one_that_was() {
    let world = World::new("coverage");
    let alice = world.clone("alice", "Ada Lovelace", "ada@example.org");
    // A file that is no session vessa can read: a source warning.
    let folder = world.sessions(
        "sessions",
        &[
            ("a.jsonl", claude("a", &alice, None, 1)),
            ("unreadable.jsonl", "{\"no\":\"header\"}\n".into()),
        ],
    );
    let out = world.share(&alice, &folder, &["--json"]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    assert_eq!(record(&out)["source_warnings"], 1);

    // Read again without it, the share keeps the sessions of the one that
    // may have missed some, so it is still partial.
    std::fs::remove_file(folder.join("unreadable.jsonl")).unwrap();
    let out = world.share(&alice, &folder, &["--json"]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    assert_eq!(record(&out)["source_warnings"], 1);
    let out = world.vessa(&alice, &["team", "--json"]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    assert_eq!(record(&out)["people"][0]["source_warnings"], 1);
}

#[test]
fn a_session_whose_id_cannot_be_shared_is_left_out_and_said() {
    let world = World::new("ids");
    let alice = world.clone("alice", "Ada Lovelace", "ada@example.org");
    let secret = ["gh", "p_", "Zq8Lm3Np5Rt7Vx9Bz1Cd3Fg5Hj7Kl9Mn1Pq"].concat();
    // Two ids that read as paths, and one that holds a secret: each would
    // be written alike, so none can be told apart from another.
    let folder = world.sessions(
        "sessions",
        &[
            ("a.jsonl", claude("/tmp/one", &alice, None, 100)),
            ("b.jsonl", claude("/tmp/two", &alice, None, 200)),
            (
                "c.jsonl",
                claude(&format!("id-{secret}"), &alice, None, 300),
            ),
            ("d.jsonl", claude("plain", &alice, None, 400)),
        ],
    );
    let out = world.share(&alice, &folder, &["--json", "--no-push"]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    let v = record(&out);
    assert_eq!(ids(&v), ["plain"].map(String::from).into());
    assert_eq!(v["source_warnings"], 1);
    let said = stderr(&out);
    assert!(said.contains("3 sessions of this repository"), "{said}");
    assert!(!said.contains(&secret), "{said}");
    // Shared again, the same.
    let again = world.share(&alice, &folder, &["--json", "--no-push"]);
    assert_eq!(record(&again)["sessions"], v["sessions"]);
}

#[test]
fn team_fetches_no_tags() {
    let world = World::new("tags");
    let alice = world.clone("alice", "Ada Lovelace", "ada@example.org");
    // A tag made after the clone, and a remote set to fetch every tag.
    let seed = world.root.join("seed");
    world.git(&seed, &["tag", "v9"]);
    world.git(&seed, &["push", "-q", text(&world.origin()), "v9"]);
    world.git(&alice, &["config", "remote.origin.tagOpt", "--tags"]);
    // A tag of this clone's only, and a setting that prunes tags.
    world.git(&alice, &["tag", "mine"]);
    world.git(&alice, &["config", "fetch.pruneTags", "true"]);
    let folder = world.sessions("sessions", &[("a.jsonl", claude("a", &alice, None, 1))]);
    assert_eq!(world.share(&alice, &folder, &[]).status.code(), Some(0));
    let out = world.vessa(&alice, &["team", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(record(&out)["fetched"], true);
    assert_eq!(world.git(&alice, &["tag", "--list"]), "mine");
}

#[test]
fn share_needs_a_repository_and_a_person() {
    let world = World::new("needs");
    let outside = world.root.join("not-a-clone");
    std::fs::create_dir_all(&outside).unwrap();
    let folder = world.sessions("sessions", &[("a.jsonl", claude("a", &outside, None, 1))]);
    let out = world.share(&outside, &folder, &["--dry-run"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("git repository"), "{}", stderr(&out));

    let nobody = world.root.join("nobody");
    world.git(
        &world.root,
        &["clone", "-q", text(&world.origin()), "nobody"],
    );
    let out = world.share(&nobody, &folder, &["--dry-run"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("user.email"), "{}", stderr(&out));

    let alice = world.clone("alice", "Ada Lovelace", "ada@example.org");
    // A remote that git would take for an option.
    let out = world.share(&alice, &folder, &["--remote=--upload-pack=x"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("is not a remote name"),
        "{}",
        stderr(&out)
    );
    let out = world.share(&alice, &folder, &["--agent", "claude"]);
    assert_eq!(out.status.code(), Some(1));
}
