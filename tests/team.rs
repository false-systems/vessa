//! `vessa share` and `vessa team` against real git: a bare remote, clones
//! with their own identities, and synthetic sessions whose recorded folder,
//! or the files they touched, are inside or outside each clone.

mod support;

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

/// A Claude Code session that recorded `cwd` and made one `tool` call with
/// `input`, in a request of `tokens` uncached input tokens.
fn claude_tool(id: &str, cwd: &Path, tool: &str, input: Value, tokens: u64) -> String {
    let first = json!({"type":"user","uuid":format!("{id}-u1"),"sessionId":id,"cwd":text(cwd),
        "timestamp":"2026-09-30T09:00:00Z","entrypoint":"cli","isSidechain":false,
        "message":{"role":"user","content":"Fix it."}});
    let call = json!({"type":"assistant","uuid":format!("{id}-a1"),"requestId":format!("req_{id}"),
        "sessionId":id,"cwd":text(cwd),"timestamp":"2026-09-30T09:05:00Z","isSidechain":false,
        "message":{"id":format!("msg_01{id}"),"role":"assistant","model":"demo-model",
            "content":[{"type":"tool_use","id":format!("toolu_{id}"),"name":tool,"input":input}],
            "usage":{"input_tokens":tokens,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}});
    format!("{first}\n{call}\n")
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
    // A session started outside the clone, found by the file it edited in
    // it: the file's path decides, and is written nowhere.
    let workspace = world.root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let edited = cwd.join("touched-secret.rs");
    let touched = claude_tool(
        "privacy-touched",
        &workspace,
        "Edit",
        json!({"file_path":text(&edited),"old_string":"a","new_string":"b"}),
        6,
    );
    // A Codex session beside it, which records its repository's address
    // and commit; two sessions whose ids are relative paths, which cannot
    // be shared at all; and a branch name that is no branch.
    let folder = world.sessions(
        "sessions",
        &[
            ("privacy.jsonl", session),
            ("touched.jsonl", touched),
            (
                "badbranch.jsonl",
                claude("privacy-badbranch", &cwd, Some("~/secret-branch"), 3),
            ),
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
        "touched-secret",
        "workspace",
        "secret-branch",
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
    assert_eq!(
        ids(&v),
        [id, "privacy-codex", "privacy-touched", "privacy-badbranch"]
            .map(String::from)
            .into()
    );
    let session = |wanted: &str| {
        v["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == wanted)
            .cloned()
            .unwrap()
    };
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
    let s = &session(id);
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
        // A valid branch name is kept.
        "feat/privacy",
        "privacy-touched",
        "privacy-badbranch",
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
    assert_eq!(s["branch"], "feat/privacy");
    assert_eq!(session("privacy-codex")["branch"], "feat/privacy");
    assert_eq!(session("privacy-badbranch")["branch"], "[path]");
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
            (json!("feat/parser"), json!(2)),
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

    // Shared again, the two copies are joined by a commit on both, and
    // pushed; reading both histories gives the sessions of both.
    let out = world.share(&again, &second, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let merged = world.git(&world.origin(), &["rev-parse", &reference]);
    assert_ne!(merged, on_origin);
    let parents = world.git(
        &world.origin(),
        &["rev-list", "--parents", "-n", "1", &reference],
    );
    assert_eq!(parents.split(' ').count(), 3, "{parents}");
    let out = world.vessa(&alice, &["team", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let v = record(&out);
    assert_eq!(v["people"].as_array().unwrap().len(), 1);
    assert_eq!(
        ids(&v["people"][0]),
        ["again-1", "alice-1"].map(String::from).into()
    );
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
    // Shared again, nothing is new: no commit, and nothing printed.
    let again = world.share(&alice, &folder, &["--json", "--no-push"]);
    assert_eq!(again.status.code(), Some(4), "{}", stderr(&again));
    assert!(again.stdout.is_empty());
    let reference = world.usage_ref(&alice);
    assert_eq!(world.git(&alice, &["rev-list", "--count", &reference]), "1");
}

/// A Codex session that recorded `cwd`, made `calls` (response items), and
/// counted `tokens` input tokens.
fn codex_calls(id: &str, cwd: &Path, calls: &[Value], tokens: u64) -> String {
    let mut lines = vec![
        json!({"type":"session_meta","timestamp":"2026-10-01T10:00:00Z","payload":{
            "id":id,"cwd":text(cwd),"model_provider":"openai","source":"cli","timestamp":"2026-10-01T10:00:00Z"}}),
        json!({"type":"turn_context","payload":{"model":"demo-codex","cwd":text(cwd)}}),
    ];
    for call in calls {
        lines.push(
            json!({"timestamp":"2026-10-01T10:01:00Z","type":"response_item","payload":call}),
        );
    }
    lines.push(json!({"timestamp":"2026-10-01T10:04:00Z","type":"event_msg","payload":{"type":"token_count",
        "info":{"total_token_usage":{"input_tokens":tokens,"cached_input_tokens":0,"output_tokens":4}}}}));
    lines.iter().map(|l| format!("{l}\n")).collect()
}

#[test]
fn a_session_is_found_by_the_files_it_touched_never_by_its_commands() {
    let world = World::new("touched");
    let alice = world.clone("alice", "Ada Lovelace", "ada@example.org");
    // Agents started in the folder that holds the clones.
    let parent = world.root.clone();
    let inside = alice.join("src").join("lib.rs");
    let elsewhere = world.root.join("other").join("x.rs");
    let run = |cmd: String| {
        json!({"type":"function_call","name":"exec_command","call_id":"c0",
        "arguments":json!({"cmd":cmd}).to_string()})
    };
    let pi = [
        json!({"type":"session","version":3,"id":"pi-edit","timestamp":"2026-09-30T11:00:00Z","cwd":text(&parent)}),
        json!({"type":"message","id":"q1","timestamp":"2026-09-30T11:01:00Z","message":{"role":"assistant",
            "provider":"zai","model":"demo-glm",
            "content":[{"type":"toolCall","id":"t1","name":"edit","arguments":{"path":"alice/src/lib.rs","edits":[]}}],
            "usage":{"input":5,"output":1,"cacheRead":0,"cacheWrite":0}}}),
    ]
    .iter()
    .map(|l| format!("{l}\n"))
    .collect::<String>();
    let folder = world.sessions(
        "sessions",
        &[
            (
                "claude-edit.jsonl",
                claude_tool("claude-edit", &parent, "Edit",
                    json!({"file_path":text(&inside),"old_string":"a","new_string":"b"}), 1),
            ),
            (
                "claude-relative.jsonl",
                claude_tool("claude-relative", &parent, "Read", json!({"file_path":"alice/README.md"}), 2),
            ),
            (
                "claude-grep.jsonl",
                claude_tool("claude-grep", &parent, "Grep", json!({"pattern":"fn","path":text(&alice)}), 3),
            ),
            (
                "claude-elsewhere.jsonl",
                claude_tool("claude-elsewhere", &parent, "Write",
                    json!({"file_path":text(&elsewhere),"content":"x"}), 4),
            ),
            // A shell command that names the clone says nothing vessa reads.
            (
                "claude-shell.jsonl",
                claude_tool("claude-shell", &parent, "Bash",
                    json!({"command":format!("cd {} && cargo test", text(&alice))}), 5),
            ),
            (
                "codex-workdir.jsonl",
                codex_calls("codex-workdir", &parent, &[json!({"type":"function_call","name":"exec_command",
                    "call_id":"c1","arguments":json!({"cmd":"cargo test","workdir":text(&alice)}).to_string()})], 6),
            ),
            (
                "codex-patch.jsonl",
                codex_calls("codex-patch", &parent, &[json!({"type":"custom_tool_call","name":"apply_patch",
                    "call_id":"c2","input":"*** Begin Patch\n*** Update File: alice/src/lib.rs\n@@\n-a\n+b\n*** End Patch\n"})], 7),
            ),
            (
                "codex-shell.jsonl",
                codex_calls("codex-shell", &parent, &[run(format!("cd {} && cargo test", text(&alice)))], 8),
            ),
            ("pi-edit.jsonl", pi),
        ],
    );
    support::store(
        &folder.join("opencode.db"),
        support::LAYOUT,
        &[("ses_edit", None, text(&parent), support::at(0))],
        &[(
            "msg_1",
            "ses_edit",
            "assistant",
            1,
            support::at(1),
            json!({"model":{"providerID":"demo","id":"demo-oc"},
                "tokens":{"input":9,"output":1,"reasoning":0,"cache":{"read":0,"write":0}},
                // Content elements that are no object are passed over.
                "content":["a bare string", 3, null, {"type":"tool","tool":"edit","state":{"status":"completed",
                    "input":{"path":"alice/src/lib.rs","oldString":"a","newString":"b"}}}]})
            .to_string(),
        )],
    );
    let out = world.share(&alice, &folder, &["--dry-run"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        ids(&record(&out)),
        [
            "claude-edit",
            "claude-relative",
            "claude-grep",
            "codex-workdir",
            "codex-patch",
            "pi-edit",
            "ses_edit",
        ]
        .map(String::from)
        .into()
    );
    let said = stderr(&out);
    assert!(
        said.contains("7 sessions of this repository: 0 by the folder they recorded, 7 by the files they touched"),
        "{said}"
    );
    // The store reads whole for every other command too.
    let store = folder.join("opencode.db");
    let out = world.vessa(&alice, &["usage", "--input", text(&store)]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
}

#[test]
fn a_session_that_touched_two_clones_counts_in_both() {
    let world = World::new("two-clones");
    let alice = world.clone("alice", "Ada Lovelace", "ada@example.org");
    let other = world.clone("other", "Ada Lovelace", "ada@example.org");
    let id = "both";
    let edit = |n: u32, file: &Path| {
        json!({"type":"assistant","uuid":format!("a{n}"),"requestId":format!("req_both{n}"),
        "sessionId":id,"cwd":text(&world.root),"timestamp":"2026-09-30T09:05:00Z","isSidechain":false,
        "message":{"id":format!("msg_01both{n}"),"role":"assistant","model":"demo-model",
            "content":[{"type":"tool_use","id":format!("toolu_{n}"),"name":"Write","input":{"file_path":text(file),"content":"x"}}],
            "usage":{"input_tokens":10,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}})
    };
    let lines = [
        json!({"type":"user","uuid":"u1","sessionId":id,"cwd":text(&world.root),"timestamp":"2026-09-30T09:00:00Z",
            "entrypoint":"cli","isSidechain":false,"message":{"role":"user","content":"Both."}}),
        edit(1, &alice.join("a.rs")),
        edit(2, &other.join("b.rs")),
    ];
    let folder = world.sessions(
        "sessions",
        &[(
            "both.jsonl",
            lines.iter().map(|l| format!("{l}\n")).collect(),
        )],
    );
    for clone in [&alice, &other] {
        let out = world.share(clone, &folder, &["--dry-run"]);
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        let v = record(&out);
        assert_eq!(ids(&v), [id].map(String::from).into());
        assert_eq!(v["sessions"][0]["repositories_touched"], 2);
    }
    // A session of one repository says nothing of others.
    let single = world.sessions("single", &[("s.jsonl", claude("single", &alice, None, 1))]);
    let out = world.share(&alice, &single, &["--dry-run"]);
    assert!(record(&out)["sessions"][0]
        .get("repositories_touched")
        .is_none());

    let out = world.share(&alice, &folder, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let out = world.vessa(&alice, &["team", "--json"]);
    assert_eq!(record(&out)["total"]["sessions_in_several_repositories"], 1);
    let out = world.vessa(&alice, &["team"]);
    let plain = String::from_utf8_lossy(&out.stdout);
    assert!(plain.contains("more than once"), "{plain}");
}

#[test]
fn a_share_adds_only_what_changed_and_history_keeps_the_rest() {
    let world = World::new("append");
    let alice = world.clone("alice", "Ada Lovelace", "Ada@Example.org");
    let folder = world.sessions(
        "sessions",
        &[
            ("one.jsonl", claude("one", &alice, None, 100)),
            ("two.jsonl", claude("two", &alice, None, 200)),
        ],
    );
    let out = world.share(&alice, &folder, &["--no-push"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let reference = world.usage_ref(&alice);
    // The key is a short digest of the lower-cased email, which the ref
    // does not spell.
    let digest = world.git_in(&alice, &["hash-object", "--stdin"], "ada@example.org");
    assert_eq!(reference, format!("refs/vessa/usage/{}", &digest[..16]));
    let first = world.git(&alice, &["rev-parse", &reference]);

    // Nothing new: no commit, and it says so.
    let out = world.share(&alice, &folder, &["--no-push"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(String::from_utf8_lossy(&out.stdout).contains("Nothing new"));
    assert_eq!(world.git(&alice, &["rev-parse", &reference]), first);

    // One session changes and the other's transcript goes: the share
    // holds the changed one alone, on the one before.
    std::fs::write(folder.join("one.jsonl"), claude("one", &alice, None, 150)).unwrap();
    std::fs::remove_file(folder.join("two.jsonl")).unwrap();
    let out = world.share(&alice, &folder, &["--no-push"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let v: Value = serde_json::from_str(&world.record_at(&alice, &reference)).unwrap();
    assert_eq!(ids(&v), ["one"].map(String::from).into());
    assert_eq!(
        world.git(&alice, &["rev-parse", &format!("{reference}^")]),
        first
    );
    // The first share is as it was.
    let old: Value = serde_json::from_str(&world.record_at(&alice, &first)).unwrap();
    assert_eq!(ids(&old), ["one", "two"].map(String::from).into());

    // History is the record: the latest of each session.
    let out = world.vessa(&alice, &["team", "--json", "--no-fetch"]);
    let v = record(&out);
    let ada = &v["people"][0];
    assert_eq!(ids(ada), ["one", "two"].map(String::from).into());
    let one = ada["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == "one")
        .unwrap();
    assert_eq!(one["entries"][0]["input_tokens"], 150);
    assert_eq!(ada["shares"], 2);
}

#[test]
fn a_subagent_shared_alone_gives_way_to_its_session_when_read() {
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
    // Its session's transcript is not here: the subagent is shared alone.
    let out = world.share(&alice, &folder, &["--json", "--no-push"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        ids(&record(&out)),
        ["main-1/helper"].map(String::from).into()
    );
    // Its session appears: shared with the subagent, which it names.
    std::fs::write(
        folder.join("main-1.jsonl"),
        claude("main-1", &alice, None, 100),
    )
    .unwrap();
    let out = world.share(&alice, &folder, &["--json", "--no-push"]);
    let v = record(&out);
    assert_eq!(ids(&v), ["main-1"].map(String::from).into());
    let named = &v["sessions"][0]["subagents"];
    assert_eq!(named[0]["id"], "main-1/helper");
    assert_eq!(named[0]["entries"][0]["input_tokens"], 70);

    // Reading, the subagent's own record gives way, and that is said.
    let out = world.vessa(&alice, &["team", "--json", "--no-fetch"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let v = record(&out);
    let ada = &v["people"][0];
    assert_eq!(ids(ada), ["main-1"].map(String::from).into());
    assert_eq!(ada["subagents_counted_with_their_session"], 1);
    assert_eq!(v["total"]["entries"][0]["input_tokens"], 170);
    let out = world.vessa(&alice, &["team", "--no-fetch"]);
    let plain = String::from_utf8_lossy(&out.stdout);
    assert!(
        plain.contains("1 subagent counted with its session"),
        "{plain}"
    );
}

#[test]
fn a_person_is_partial_by_their_latest_share() {
    let world = World::new("latest");
    let alice = world.clone("alice", "Ada Lovelace", "ada@example.org");
    // A file that is no session vessa can read: a source warning.
    let folder = world.sessions(
        "sessions",
        &[
            ("a.jsonl", claude("a", &alice, None, 1)),
            ("unreadable.jsonl", "{\"no\":\"header\"}\n".into()),
        ],
    );
    let out = world.share(&alice, &folder, &["--json", "--no-push"]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    assert_eq!(record(&out)["source_warnings"], 1);
    let out = world.vessa(&alice, &["team", "--json", "--no-fetch"]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    assert_eq!(record(&out)["people"][0]["partial"], true);

    // The next share reads every source; its own warnings are what count.
    std::fs::remove_file(folder.join("unreadable.jsonl")).unwrap();
    std::fs::write(folder.join("a.jsonl"), claude("a", &alice, None, 2)).unwrap();
    let out = world.share(&alice, &folder, &["--json", "--no-push"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(record(&out)["source_warnings"], 0);
    let out = world.vessa(&alice, &["team", "--json", "--no-fetch"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let v = record(&out);
    assert_eq!(v["people"][0]["partial"], false);
    assert_eq!(v["people"][0]["source_warnings"], 0);

    // A source goes unreadable while no session changes: the warnings are
    // new, so they are shared, with no session.
    std::fs::write(folder.join("unreadable.jsonl"), "{\"no\":\"header\"}\n").unwrap();
    let out = world.share(&alice, &folder, &["--json", "--no-push"]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    let v = record(&out);
    assert_eq!(v["source_warnings"], 1);
    assert_eq!(v["sessions"], json!([]));
    let out = world.vessa(&alice, &["team", "--json", "--no-fetch"]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    assert_eq!(record(&out)["people"][0]["partial"], true);
}

#[test]
fn a_subagent_newer_than_its_session_s_record_makes_the_person_partial() {
    let world = World::new("newer");
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
    // The session's transcript goes; its subagent records one request more,
    // and is shared alone.
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
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        ids(&record(&out)),
        ["main-1/helper"].map(String::from).into()
    );
    // Reading, the session counts its subagent as it was; the newer usage
    // is not counted, and the person is partial.
    let out = world.vessa(&alice, &["team", "--json", "--no-fetch"]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    let ada = &record(&out)["people"][0];
    assert_eq!(ids(ada), ["main-1"].map(String::from).into());
    assert_eq!(ada["subagents_newer_than_their_session"], 1);
    assert_eq!(ada["partial"], true);
}

#[test]
fn a_subagent_partial_after_its_session_was_shared_makes_the_person_partial() {
    let world = World::new("newer-partial");
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
    assert_eq!(
        world.share(&alice, &folder, &["--no-push"]).status.code(),
        Some(0)
    );
    // The session's transcript goes; its subagent's gains a damaged record,
    // its numbers the same.
    std::fs::remove_file(folder.join("main-1.jsonl")).unwrap();
    std::fs::write(
        &helper,
        subagent("main-1", "helper", &alice, 70) + "not a record\n",
    )
    .unwrap();
    let out = world.share(&alice, &folder, &["--json", "--no-push"]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    assert_eq!(
        ids(&record(&out)),
        ["main-1/helper"].map(String::from).into()
    );
    let out = world.vessa(&alice, &["team", "--json", "--no-fetch"]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    let ada = &record(&out)["people"][0];
    assert_eq!(ada["subagents_newer_than_their_session"], 1);
    assert_eq!(ada["partial"], true);
}

#[test]
fn a_first_share_with_warnings_alone_is_written() {
    let world = World::new("only-warnings");
    let alice = world.clone("alice", "Ada Lovelace", "ada@example.org");
    // A file that is no session vessa can read, and a session whose id
    // cannot be shared.
    let folder = world.sessions(
        "sessions",
        &[
            ("unreadable.jsonl", "{\"no\":\"header\"}\n".into()),
            ("a.jsonl", claude("/tmp/id", &alice, None, 1)),
        ],
    );
    let out = world.share(&alice, &folder, &["--json", "--no-push"]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    let v = record(&out);
    assert_eq!(v["sessions"], json!([]));
    assert_eq!(v["source_warnings"], 2);
    let out = world.vessa(&alice, &["team", "--json", "--no-fetch"]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    let ada = &record(&out)["people"][0];
    assert_eq!(ada["partial"], true);
    assert_eq!(ada["sessions"], json!([]));
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
