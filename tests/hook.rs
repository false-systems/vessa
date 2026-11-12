//! `vessa hook` and `vessa messages --commit` against real git: clones with
//! commits at set times, and synthetic sessions of every agent vessa reads,
//! active before or after a commit's parent, inside or outside the clone.

mod support;

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A folder of clones, with git reading no configuration of this machine's.
struct World {
    root: PathBuf,
}

impl World {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "vessa-hook-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        std::fs::write(root.join("gitconfig"), "[init]\n\tdefaultBranch = main\n").unwrap();
        World { root }
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
            "XDG_DATA_HOME",
        ] {
            command.env_remove(name);
        }
        command
    }

    /// git in `dir`, its commits made at `at` (UTC, on the fixtures' day).
    fn git_at(&self, dir: &Path, at: &str, args: &[&str]) -> Output {
        let date = format!("2026-10-01T{at}Z");
        self.command("git", dir)
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .args(args)
            .output()
            .unwrap()
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let out = self.git_at(dir, "10:00:00", args);
        assert!(
            out.status.success(),
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    /// A clone whose one commit was made at 10:00.
    fn clone(&self, name: &str) -> PathBuf {
        let dir = self.root.join(name);
        self.git(&self.root, &["init", "-q", name]);
        self.git(&dir, &["config", "user.name", "Ada Lovelace"]);
        self.git(&dir, &["config", "user.email", "ada@example.org"]);
        std::fs::write(dir.join("README.md"), "# Widget\n").unwrap();
        self.git(&dir, &["add", "README.md"]);
        self.git(&dir, &["commit", "-q", "-m", "initial"]);
        dir
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
            let path = folder.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, lines).unwrap();
        }
        folder
    }

    /// `hook prepare-commit-msg` on `message`, written to a file in `dir`,
    /// reading the sessions in `folder`: the message after, and the output.
    fn prepare(
        &self,
        dir: &Path,
        folder: &Path,
        message: &str,
        source: &[&str],
    ) -> (String, Output) {
        let file = self.root.join("COMMIT_EDITMSG");
        std::fs::write(&file, message).unwrap();
        let mut args = vec![
            "--input",
            text(folder),
            "hook",
            "prepare-commit-msg",
            text(&file),
        ];
        args.extend(source);
        let out = self.vessa(dir, &args);
        (std::fs::read_to_string(&file).unwrap(), out)
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

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// `HH:MM:SS` on the fixtures' day, 2026-10-01, in UTC.
fn at(time: &str) -> String {
    format!("2026-10-01T{time}Z")
}

fn millis(time: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(&at(time))
        .unwrap()
        .timestamp_millis()
}

/// Half a minute after `time`, when an agent answers.
fn later(time: &str) -> String {
    let t =
        chrono::DateTime::parse_from_rfc3339(&at(time)).unwrap() + chrono::TimeDelta::seconds(30);
    t.to_utc().format("%H:%M:%S").to_string()
}

/// A Claude Code session started from `entrypoint` in `cwd`: at each time,
/// a person's message, and the agent's reply half a minute later.
fn claude_from(id: &str, entrypoint: &str, cwd: &Path, said: &[(&str, &str)]) -> String {
    let mut lines = String::new();
    for (n, (time, words)) in said.iter().enumerate() {
        let user = json!({"type":"user","uuid":format!("{id}-u{n}"),"sessionId":id,"cwd":text(cwd),
            "timestamp":at(time),"entrypoint":entrypoint,"isSidechain":false,
            "message":{"role":"user","content":words}});
        let reply = json!({"type":"assistant","uuid":format!("{id}-a{n}"),"requestId":format!("req_{id}_{n}"),
            "sessionId":id,"cwd":text(cwd),"timestamp":at(&later(time)),"isSidechain":false,
            "message":{"id":format!("msg_01{id}{n}"),"role":"assistant","model":"demo-model",
                "content":[{"type":"text","text":"Done."}],
                "usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}});
        lines += &format!("{user}\n{reply}\n");
    }
    lines
}

fn claude(id: &str, cwd: &Path, said: &[(&str, &str)]) -> String {
    claude_from(id, "cli", cwd, said)
}

/// A Claude Code session in `cwd` that, at `time`, was asked to fix
/// something and edited `file`.
fn claude_edit(id: &str, cwd: &Path, time: &str, file: &Path) -> String {
    let user = json!({"type":"user","uuid":format!("{id}-u"),"sessionId":id,"cwd":text(cwd),
        "timestamp":at(time),"entrypoint":"cli","isSidechain":false,
        "message":{"role":"user","content":"Fix the heading."}});
    let call = json!({"type":"assistant","uuid":format!("{id}-a"),"requestId":format!("req_{id}"),
        "sessionId":id,"cwd":text(cwd),"timestamp":at(&later(time)),"isSidechain":false,
        "message":{"id":format!("msg_01{id}"),"role":"assistant","model":"demo-model",
            "content":[{"type":"tool_use","id":format!("toolu_{id}"),"name":"Edit","input":{"file_path":text(file)}}],
            "usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}});
    format!("{user}\n{call}\n")
}

/// A Codex session in `cwd`, started at `start`, with a person's message at
/// `time`.
fn codex(id: &str, cwd: &Path, start: &str, time: &str, words: &str) -> String {
    let meta = json!({"timestamp":at(start),"type":"session_meta","payload":{"id":id,"cwd":text(cwd),
        "source":"cli","originator":"codex-tui","timestamp":at(start)}});
    let turn = json!({"type":"turn_context","payload":{"model":"demo-codex","cwd":text(cwd)}});
    let user = json!({"timestamp":at(time),"type":"response_item","payload":{"type":"message","role":"user",
        "content":[{"type":"input_text","text":words}]}});
    let reply = json!({"timestamp":at(&later(time)),"type":"response_item","payload":{"type":"message","role":"assistant",
        "content":[{"type":"output_text","text":"Done."}]}});
    format!("{meta}\n{turn}\n{user}\n{reply}\n")
}

/// A pi session in `cwd`, with a person's message at `time`.
fn pi(id: &str, cwd: &Path, time: &str, words: &str) -> String {
    let header = json!({"type":"session","version":3,"id":id,"timestamp":at(time),"cwd":text(cwd)});
    let user = json!({"type":"message","id":format!("{id}-1"),"timestamp":at(time),
        "message":{"role":"user","content":[{"type":"text","text":words}]}});
    let reply = json!({"type":"message","id":format!("{id}-2"),"timestamp":at(&later(time)),
        "message":{"role":"assistant","provider":"zai","model":"demo-glm","content":[{"type":"text","text":"Done."}],
            "usage":{"input":10,"output":5,"cacheRead":0,"cacheWrite":0}}});
    format!("{header}\n{user}\n{reply}\n")
}

/// An opencode store at `db`: a session in `cwd` with a person's message at
/// `time` and its reply, and a subagent it started elsewhere a minute on.
fn opencode(db: &Path, cwd: &Path, elsewhere: &Path, time: &str, words: &str) {
    let user =
        |text: &str| json!({"text":text,"time":{"created":0},"files":[],"agents":[]}).to_string();
    let prompt = |text: &str| json!({"text":text,"time":{"created":0}}).to_string();
    let reply = json!({"agent":"build","content":[{"type":"text","text":"Done."}],
        "model":{"id":"demo","providerID":"demo"},"tokens":{"input":1,"output":1,"reasoning":0,"cache":{"read":0,"write":0}},
        "finish":"stop","time":{"created":0}})
    .to_string();
    let t = millis(time);
    support::store(
        db,
        support::LAYOUT,
        &[
            ("ses_widget", None, text(cwd), t),
            ("ses_child", Some("ses_widget"), text(elsewhere), t + 60_000),
        ],
        &[
            ("msg_1", "ses_widget", "user", 0, t, user(words)),
            (
                "msg_2",
                "ses_widget",
                "assistant",
                1,
                t + 30_000,
                reply.clone(),
            ),
            (
                "msg_3",
                "ses_child",
                "user",
                0,
                t + 60_000,
                prompt("Count the tests."),
            ),
            ("msg_4", "ses_child", "assistant", 1, t + 90_000, reply),
        ],
    );
}

fn trailers(message: &str) -> Vec<&str> {
    message
        .lines()
        .filter(|line| line.starts_with("Agent-Session:"))
        .collect()
}

#[test]
fn a_commit_points_to_each_session_of_this_clone_active_since_its_parent() {
    let world = World::new("points");
    let widget = world.clone("widget");
    std::fs::create_dir_all(widget.join("src")).unwrap();
    let side = world.root.join("widget-side");
    world.git(
        &widget,
        &["worktree", "add", "-q", "-b", "side", text(&side)],
    );
    let elsewhere = world.root.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let folder = world.sessions(
        "sessions",
        &[
            (
                "in-repo.jsonl",
                claude(
                    "in-repo",
                    &widget.join("src"),
                    &[
                        ("10:05:00", "Run the checks."),
                        ("10:20:00", "Then commit."),
                    ],
                ),
            ),
            (
                "in-side.jsonl",
                claude(
                    "in-side",
                    &side,
                    &[("10:10:00", "Look at the side branch.")],
                ),
            ),
            // Active only before the parent, at 10:00.
            (
                "before.jsonl",
                claude(
                    "before",
                    &widget,
                    &[("09:00:00", "Old."), ("09:30:00", "Older.")],
                ),
            ),
            // Active across it: only what came after counts.
            (
                "straddle.jsonl",
                claude(
                    "straddle",
                    &widget,
                    &[("09:50:00", "Before."), ("10:30:00", "After.")],
                ),
            ),
            (
                "outside.jsonl",
                claude("outside", &elsewhere, &[("10:15:00", "Elsewhere.")]),
            ),
            (
                "by-files.jsonl",
                claude_edit(
                    "by-files",
                    &elsewhere,
                    "10:25:00",
                    &widget.join("README.md"),
                ),
            ),
            // `claude -p`: active, with no message a person typed.
            (
                "headless.jsonl",
                claude_from("headless", "sdk-cli", &widget, &[("10:40:00", "Lint it.")]),
            ),
            (
                "codex.jsonl",
                codex(
                    "codex-widget",
                    &widget,
                    "10:17:00",
                    "10:18:00",
                    "Rename it.",
                ),
            ),
            (
                "pi.jsonl",
                pi("pi-widget", &widget, "10:16:00", "Explain it."),
            ),
            // Active after the parent, by what it holds, but last written
            // before it: never opened.
            (
                "stale.jsonl",
                claude("stale", &widget, &[("10:50:00", "Stale.")]),
            ),
        ],
    );
    opencode(
        &folder.join("opencode.db"),
        &widget,
        &elsewhere,
        "10:12:00",
        "Tidy the docs.",
    );
    let stale = std::fs::File::options()
        .write(true)
        .open(folder.join("stale.jsonl"))
        .unwrap();
    let parent = std::time::UNIX_EPOCH
        + std::time::Duration::from_millis(u64::try_from(millis("09:59:00")).unwrap());
    stale.set_modified(parent).unwrap();
    drop(stale);

    let started = std::time::Instant::now();
    let (message, out) = world.prepare(&widget, &folder, "Fix the widget.\n", &["message"]);
    let took = started.elapsed();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        message,
        "Fix the widget.\n\n\
         Agent-Session: claude by-files (1 message, 2026-10-01 10:25 UTC)\n\
         Agent-Session: claude headless (0 messages, 2026-10-01 10:40 UTC)\n\
         Agent-Session: claude in-repo (2 messages, 2026-10-01 10:05–10:20 UTC)\n\
         Agent-Session: claude in-side (1 message, 2026-10-01 10:10 UTC)\n\
         Agent-Session: claude straddle (1 message, 2026-10-01 10:30 UTC)\n\
         Agent-Session: codex codex-widget (1 message, 2026-10-01 10:17–10:18 UTC)\n\
         Agent-Session: opencode ses_widget (1 message, 2026-10-01 10:12–10:13 UTC)\n\
         Agent-Session: pi pi-widget (1 message, 2026-10-01 10:16 UTC)\n",
        "{}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains("vessa: this commit points to 8 agent sessions"),
        "{}",
        stderr(&out)
    );
    // Pointers only: no words, no path, no name.
    for line in trailers(&message) {
        for private in [text(&world.root), "Run the checks", "Ada", "widget-side"] {
            assert!(!line.contains(private), "{line}");
        }
    }
    assert!(took < std::time::Duration::from_secs(3), "{took:?}");

    // Run again, as on an amend or a rebase: nothing is added twice.
    let (again, out) = world.prepare(&widget, &folder, &message, &["message"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(again, message);
    assert!(!stderr(&out).contains("points to"), "{}", stderr(&out));
}

#[test]
fn the_hook_never_stops_a_commit_and_says_why_it_added_nothing() {
    let world = World::new("errors");
    let widget = world.clone("widget");
    let folder = world.sessions(
        "sessions",
        &[(
            "in-repo.jsonl",
            claude("in-repo", &widget, &[("10:05:00", "Run the checks.")]),
        )],
    );
    // Outside any clone.
    let (message, out) = world.prepare(&world.root, &folder, "Fix it.\n", &["message"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(message, "Fix it.\n");
    let said = stderr(&out);
    assert_eq!(said.lines().count(), 1, "{said}");
    assert!(
        said.starts_with("vessa: no Agent-Session pointers added: "),
        "{said}"
    );
    // No message file.
    let out = world.vessa(
        &widget,
        &[
            "--input",
            text(&folder),
            "hook",
            "prepare-commit-msg",
            text(&world.root.join("absent")),
            "message",
        ],
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stderr(&out).lines().count(), 1, "{}", stderr(&out));
    // A message yet to be written, empty or a template's: git aborts a
    // commit left so only while it is, so nothing is added.
    for source in [&[][..], &["template"]] {
        let (message, out) = world.prepare(&widget, &folder, "\n# Write the message.\n", source);
        assert_eq!(out.status.code(), Some(0));
        assert_eq!(message, "\n# Write the message.\n");
        assert!(
            stderr(&out).contains("no Agent-Session pointers"),
            "{}",
            stderr(&out)
        );
    }
    // A source that cannot be read is said; what could be read still
    // counts.
    let missing = world.root.join("missing");
    let file = world.root.join("COMMIT_EDITMSG");
    std::fs::write(&file, "Fix it.\n").unwrap();
    let out = world.vessa(
        &widget,
        &[
            "--input",
            text(&folder),
            "--input",
            text(&missing),
            "hook",
            "prepare-commit-msg",
            text(&file),
            "message",
        ],
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        trailers(&std::fs::read_to_string(&file).unwrap()),
        ["Agent-Session: claude in-repo (1 message, 2026-10-01 10:05 UTC)"]
    );
    assert!(stderr(&out).contains(text(&missing)), "{}", stderr(&out));
    // A transcript with a record vessa cannot read may hold more messages
    // than it counts; an id that reads as a secret is never written.
    let key = ["gh", "p_", "Ab3dE5gH7jK9mN1pQ3sT5vW7yZ9bC1dF3hJ5"].concat();
    let folder = world.sessions(
        "damaged",
        &[
            (
                "damaged.jsonl",
                claude("damaged", &widget, &[("10:05:00", "Run the checks.")]) + "{not json\n",
            ),
            (
                "secret.jsonl",
                claude(&key, &widget, &[("10:06:00", "Hide it.")]),
            ),
        ],
    );
    let (message, out) = world.prepare(&widget, &folder, "Fix it.\n", &["message"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        trailers(&message),
        ["Agent-Session: claude damaged (at least 1 message, 2026-10-01 10:05 UTC)"]
    );
    assert!(!message.contains(&key));
    assert!(
        stderr(&out)
            .contains("1 agent session of this clone has an id that reads as a secret or a path"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_merge_points_to_the_work_after_every_commit_it_joins() {
    let world = World::new("merge");
    let widget = world.clone("widget");
    world.git(&widget, &["checkout", "-q", "-b", "side"]);
    std::fs::write(widget.join("side.txt"), "side\n").unwrap();
    world.git(&widget, &["add", "side.txt"]);
    assert!(world
        .git_at(&widget, "10:30:00", &["commit", "-q", "-m", "side"])
        .status
        .success());
    world.git(&widget, &["checkout", "-q", "main"]);
    std::fs::write(widget.join("main.txt"), "main\n").unwrap();
    world.git(&widget, &["add", "main.txt"]);
    assert!(world
        .git_at(&widget, "10:20:00", &["commit", "-q", "-m", "main"])
        .status
        .success());
    world.git(&widget, &["merge", "-q", "--no-commit", "--no-ff", "side"]);
    let folder = world.sessions(
        "sessions",
        &[
            // After main's last commit, before side's.
            (
                "between.jsonl",
                claude("between", &widget, &[("10:25:00", "Between.")]),
            ),
            (
                "after.jsonl",
                claude("after", &widget, &[("10:35:00", "Resolve it.")]),
            ),
        ],
    );
    let (message, out) = world.prepare(&widget, &folder, "Merge branch 'side'\n", &["merge"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        trailers(&message),
        ["Agent-Session: claude after (1 message, 2026-10-01 10:35 UTC)"]
    );
}

#[test]
fn a_squash_carries_the_pointers_of_the_commits_it_squashes() {
    let world = World::new("squash");
    let widget = world.clone("widget");
    world.git(&widget, &["checkout", "-q", "-b", "side"]);
    std::fs::write(widget.join("side.txt"), "side\n").unwrap();
    world.git(&widget, &["add", "side.txt"]);
    let pointed = "claude earlier (2 messages, 2026-10-01 09:05–09:20 UTC)";
    let said = format!("Side work.\n\nAgent-Session: {pointed}\n");
    assert!(world
        .git_at(&widget, "10:30:00", &["commit", "-q", "-m", &said])
        .status
        .success());
    world.git(&widget, &["checkout", "-q", "main"]);
    world.git(&widget, &["merge", "-q", "--squash", "side"]);
    let squash = std::fs::read_to_string(widget.join(".git/SQUASH_MSG")).unwrap();
    let folder = world.sessions(
        "sessions",
        &[(
            "now.jsonl",
            claude("now", &widget, &[("10:40:00", "Squash it.")]),
        )],
    );
    let (message, out) = world.prepare(&widget, &folder, &squash, &["squash"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(message.starts_with(&squash), "{message}");
    assert_eq!(
        trailers(&message),
        [
            format!("Agent-Session: {pointed}").as_str(),
            "Agent-Session: claude now (1 message, 2026-10-01 10:40 UTC)"
        ]
    );
}

#[test]
fn the_installed_hook_points_each_commit_and_a_replay_keeps_its_own() {
    let world = World::new("installed");
    let widget = world.clone("widget");
    let side = world.root.join("widget-side");
    world.git(
        &widget,
        &["worktree", "add", "-q", "-b", "side", text(&side)],
    );
    // The agents' own folders, under a home of the test's.
    let home = world.root.join("home");
    let projects = home.join(".claude/projects/widget");
    std::fs::create_dir_all(&projects).unwrap();
    std::fs::write(
        projects.join("one.jsonl"),
        claude("one", &widget, &[("10:05:00", "Fix the heading.")]),
    )
    .unwrap();
    let out = world.vessa(&widget, &["hook", "install"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let common = PathBuf::from(world.git(
        &widget,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ));
    let hook = common.join("hooks/prepare-commit-msg");
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("installed: {}\n", hook.display())
    );
    // Every worktree of the clone has it.
    let out = world.vessa(&side, &["hook", "install"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("unchanged: {}\n", hook.display())
    );
    let bin = Path::new(env!("CARGO_BIN_EXE_vessa")).parent().unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let commit = |dir: &Path, at: &str, args: &[&str]| {
        let out = world
            .command("git", dir)
            .env("HOME", &home)
            .env("PATH", &path)
            .env("GIT_AUTHOR_DATE", self::at(at))
            .env("GIT_COMMITTER_DATE", self::at(at))
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", stderr(&out));
        out
    };
    std::fs::write(widget.join("README.md"), "# Widget, fixed\n").unwrap();
    world.git(&widget, &["add", "README.md"]);
    commit(
        &widget,
        "10:30:00",
        &["commit", "-q", "-m", "Fix the heading."],
    );
    let one = "Agent-Session: claude one (1 message, 2026-10-01 10:05 UTC)";
    let body = |rev: &str| world.git(&widget, &["log", "-1", "--format=%B", rev]);
    assert_eq!(trailers(&body("HEAD")), [one]);
    // An amend keeps the message's pointer, and adds none twice.
    commit(
        &widget,
        "10:35:00",
        &["commit", "-q", "--amend", "--no-edit"],
    );
    assert_eq!(trailers(&body("HEAD")), [one]);
    // A session active after the base a cherry-pick replays onto: the
    // replayed commit keeps its own pointer, and gets no new one.
    std::fs::write(
        projects.join("two.jsonl"),
        claude("two", &side, &[("10:40:00", "Pick it.")]),
    )
    .unwrap();
    commit(&side, "10:45:00", &["cherry-pick", "main"]);
    assert_eq!(
        trailers(&world.git(&side, &["log", "-1", "--format=%B"])),
        [one]
    );
    // A message written in the editor gets none, and says so.
    let editor = world.root.join("editor.sh");
    std::fs::write(&editor, "#!/bin/sh\nprintf 'Edited.\\n' > \"$1\"\n").unwrap();
    std::process::Command::new("chmod")
        .args(["+x", text(&editor)])
        .status()
        .unwrap();
    std::fs::write(side.join("more.txt"), "more\n").unwrap();
    world.git(&side, &["add", "more.txt"]);
    let mut edit = world.command("git", &side);
    let out = edit
        .env("HOME", &home)
        .env("PATH", &path)
        .env("GIT_EDITOR", &editor)
        .args(["commit", "-q"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("no Agent-Session pointers"),
        "{}",
        stderr(&out)
    );
    assert_eq!(world.git(&side, &["log", "-1", "--format=%B"]), "Edited.");
    // Without vessa on PATH, the commit still goes on.
    std::fs::write(side.join("last.txt"), "last\n").unwrap();
    world.git(&side, &["add", "last.txt"]);
    let out = world
        .command("git", &side)
        .env("PATH", "/usr/bin:/bin")
        .args(["commit", "-q", "-m", "Last."])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn install_leaves_another_hook_alone_and_refuses_a_hooks_path() {
    let world = World::new("install");
    let widget = world.clone("widget");
    let hook = widget.join(".git/hooks/prepare-commit-msg");
    std::fs::write(&hook, "#!/bin/sh\necho mine\n").unwrap();
    let out = world.vessa(&widget, &["hook", "install"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("exists, and vessa did not write it"),
        "{}",
        stderr(&out)
    );
    assert_eq!(
        std::fs::read_to_string(&hook).unwrap(),
        "#!/bin/sh\necho mine\n"
    );
    std::fs::remove_file(&hook).unwrap();
    world.git(&widget, &["config", "core.hooksPath", "elsewhere"]);
    let out = world.vessa(&widget, &["hook", "install"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("core.hooksPath"), "{}", stderr(&out));
    assert!(!hook.exists());
}

fn messages(world: &World, dir: &Path, args: &[&str]) -> (Value, Output) {
    let out = world.vessa(dir, args);
    let value = serde_json::from_slice(&out.stdout).unwrap_or_else(|error| {
        panic!(
            "{error}: {} {}",
            String::from_utf8_lossy(&out.stdout),
            stderr(&out)
        )
    });
    (value, out)
}

#[test]
fn messages_of_a_commit_are_its_sessions_in_its_window() {
    let world = World::new("commit");
    let widget = world.clone("widget");
    let folder = world.sessions(
        "sessions",
        &[(
            "in-repo.jsonl",
            claude(
                "in-repo",
                &widget,
                &[
                    ("09:50:00", "Before the parent."),
                    ("10:05:00", "Run the checks."),
                    ("10:20:00", "Then commit."),
                    ("10:50:00", "After the commit."),
                ],
            ),
        )],
    );
    std::fs::write(widget.join("README.md"), "# Widget, fixed\n").unwrap();
    world.git(&widget, &["add", "README.md"]);
    let pointed = "claude in-repo (2 messages, 2026-10-01 10:05–10:20 UTC)";
    let away = "codex not-here (1 message, 2026-10-01 10:10 UTC)";
    let said = format!("Fix.\n\nAgent-Session: {pointed}\nAgent-Session: {away}\n");
    assert!(world
        .git_at(&widget, "10:30:00", &["commit", "-q", "-m", &said])
        .status
        .success());
    let head = world.git(&widget, &["rev-parse", "HEAD"]);

    let (v, out) = messages(
        &world,
        &widget,
        &[
            "--input",
            text(&folder),
            "messages",
            "--commit",
            "HEAD",
            "--json",
        ],
    );
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    assert_eq!(v["schema"], "vessa-messages.v1");
    let texts: Vec<&str> = v["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts, ["Run the checks.", "Then commit."]);
    assert_eq!(v["messages"][1]["reply"]["text"], "Done.");
    assert_eq!(v["coverage"]["since"], "2026-10-01T10:00:00+00:00");
    assert_eq!(v["coverage"]["until"], "2026-10-01T10:30:00+00:00");
    assert_eq!(v["coverage"]["earlier"], 1);
    assert_eq!(v["coverage"]["later"], 1);
    assert_eq!(
        v["commit"],
        json!({"id": head, "after": "2026-10-01T10:00:00+00:00", "until": "2026-10-01T10:30:00+00:00",
        "sessions": [
            {"agent":"claude","id":"in-repo","trailer":pointed,"found":true,"messages":2},
            {"agent":"codex","id":"not-here","trailer":away,"found":false,"messages":0},
        ]})
    );
    assert!(
        stderr(&out).contains("codex not-here: named by commit"),
        "{}",
        stderr(&out)
    );

    let out = world.vessa(
        &widget,
        &["--input", text(&folder), "messages", "--commit", "HEAD"],
    );
    assert_eq!(out.status.code(), Some(4));
    let plain = String::from_utf8(out.stdout).unwrap();
    assert!(
        plain.starts_with(&format!(
            "Commit {}: 2 agent sessions named; their messages after 2026-10-01T10:00:00+00:00, up to 2026-10-01T10:30:00+00:00\n",
            &head[..12]
        )),
        "{plain}"
    );
    assert!(plain.contains("\n  Run the checks.\n"), "{plain}");
    assert!(!plain.contains("After the commit."), "{plain}");
    assert!(
        plain.contains(&format!(
            "claude in-repo: 2 messages in this window · committed as: {pointed}\n"
        )),
        "{plain}"
    );
    assert!(
        plain.contains(&format!(
            "codex not-here: not on this machine · committed as: {away}\n"
        )),
        "{plain}"
    );

    // A commit that names no session lists none: not every session.
    let (v, out) = messages(
        &world,
        &widget,
        &[
            "--input",
            text(&folder),
            "messages",
            "--commit",
            "HEAD~1",
            "--json",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(v["messages"], json!([]));
    assert_eq!(v["commit"]["sessions"], json!([]));
    // The window and the selection are the commit's.
    let out = world.vessa(&widget, &["messages", "--commit", "HEAD", "--since", "1d"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let out = world.vessa(
        &widget,
        &["messages", "--commit", "HEAD", "--session", "in-repo"],
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
}

#[test]
fn trailers_go_where_git_keeps_them_and_no_configured_command_runs() {
    let world = World::new("placed");
    let widget = world.clone("widget");
    let folder = world.sessions(
        "sessions",
        &[(
            "in-repo.jsonl",
            claude("in-repo", &widget, &[("10:05:00", "Run the checks.")]),
        )],
    );
    let ours = "Agent-Session: claude in-repo (1 message, 2026-10-01 10:05 UTC)";
    // A trailer command git is configured with would hold the commit and
    // add its own words: the hook never runs one.
    world.git(
        &widget,
        &["config", "trailer.held.command", "sleep 5; echo held"],
    );
    let diff = "# ------------------------ >8 ------------------------\n# Do not modify or remove the line above.\ndiff --git a/x b/x\n Agent-Session: claude in-repo (9 messages, 2026-10-01 10:05 UTC)\n";
    for (message, expected) in [
        ("Fix.".to_owned(), format!("Fix.\n\n{ours}\n")),
        (
            "Fix.\n\nWhy.\n\nSigned-off-by: Ada <ada@example.org>\n".to_owned(),
            format!("Fix.\n\nWhy.\n\nSigned-off-by: Ada <ada@example.org>\n{ours}\n"),
        ),
        (
            format!("Fix.\n\nWhy.\n\n# Please enter the commit message.\n{diff}"),
            format!("Fix.\n\nWhy.\n\n{ours}\n\n# Please enter the commit message.\n{diff}"),
        ),
        (
            "Fix.\n\n    Agent-Session: claude other (1 message)\n".to_owned(),
            format!("Fix.\n\n    Agent-Session: claude other (1 message)\n\n{ours}\n"),
        ),
    ] {
        let started = std::time::Instant::now();
        let (after, out) = world.prepare(&widget, &folder, &message, &["message"]);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(out.status.code(), Some(0));
        assert_eq!(after, expected, "{}", stderr(&out));
    }
}

#[test]
fn an_opencode_store_whose_log_was_written_since_is_read() {
    let world = World::new("wal");
    let widget = world.clone("widget");
    let elsewhere = world.root.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let folder = world.sessions("sessions", &[]);
    let db = folder.join("opencode.db");
    opencode(&db, &widget, &elsewhere, "10:12:00", "Tidy the docs.");
    // SQLite writes to the store's log, `opencode.db-wal`, first: the store's
    // own file can be older than the parent while its log is not.
    let store = std::fs::File::options().write(true).open(&db).unwrap();
    let parent = std::time::UNIX_EPOCH
        + std::time::Duration::from_millis(u64::try_from(millis("09:59:00")).unwrap());
    store.set_modified(parent).unwrap();
    drop(store);
    std::fs::write(folder.join("opencode.db-wal"), "").unwrap();
    let (message, out) = world.prepare(&widget, &folder, "Fix.\n", &["message"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        trailers(&message),
        ["Agent-Session: opencode ses_widget (1 message, 2026-10-01 10:12–10:13 UTC)"],
        "{}",
        stderr(&out)
    );
}

#[test]
fn what_a_commit_says_of_its_sessions_is_printed_scrubbed() {
    let world = World::new("scrubbed");
    let widget = world.clone("widget");
    // Anyone can write a trailer: one whose id reads as a key is printed
    // redacted, as everything `messages` prints is.
    let key = ["gh", "p_", "Ab3dE5gH7jK9mN1pQ3sT5vW7yZ9bC1dF3hJ5"].concat();
    let said = format!("Fix.\n\nAgent-Session: claude {key} (1 message, 2026-10-01 10:10 UTC)\n");
    std::fs::write(widget.join("README.md"), "# Widget, fixed\n").unwrap();
    world.git(&widget, &["add", "README.md"]);
    assert!(world
        .git_at(&widget, "10:30:00", &["commit", "-q", "-m", &said])
        .status
        .success());
    let folder = world.sessions("sessions", &[]);
    for json in [true, false] {
        let mut args = vec!["--input", text(&folder), "messages", "--commit", "HEAD"];
        if json {
            args.push("--json");
        }
        let out = world.vessa(&widget, &args);
        assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
        let printed = format!("{}{}", String::from_utf8_lossy(&out.stdout), stderr(&out));
        assert!(!printed.contains(&key), "{printed}");
        assert!(printed.contains("[redacted]"), "{printed}");
    }
}
