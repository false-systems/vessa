mod import;
mod metrics;
mod model;
mod render;
mod scrub;
mod work;

use anyhow::{ensure, Context, Result};
use clap::{Parser, Subcommand};
use std::io::Write;
use std::path::PathBuf;

#[derive(Parser, Clone)]
#[command(
    name = "vessa",
    version,
    about = "Local agent token usage and conversation evidence."
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
    /// Native JSONL file/directory, or a wrapper's ohjaa-sessions.v1 JSON
    /// export; repeatable.
    #[arg(long, global = true)]
    input: Vec<PathBuf>,
    #[arg(long, global = true)]
    agent: Option<String>,
    /// Native session ID, not wrapper session ID.
    #[arg(long, global = true)]
    session: Option<String>,
    #[arg(long, global = true)]
    json: bool,
    /// Refresh in the foreground; for status, usage or stats. Ctrl-C stops it.
    #[arg(long, global = true, value_parser = clap::value_parser!(u64).range(1..))]
    watch: Option<u64>,
    /// Stop after this many refreshes; requires --watch.
    #[arg(long, global = true, requires = "watch", value_parser = clap::value_parser!(u64).range(1..))]
    samples: Option<u64>,
}

#[derive(Subcommand, Clone)]
enum Cmd {
    /// List native sessions and source coverage.
    Sessions {
        /// Each session's header alone (identity, lineage, start, folder,
        /// origin), read from a transcript's opening records only. With
        /// --json: vessa-session-headers.v1.
        #[arg(long)]
        headers: bool,
    },
    /// Token categories by session/provider/model.
    Usage {
        /// Sum deduplicated usage across native sessions for each agent/model.
        #[arg(long)]
        by_agent: bool,
    },
    /// Write a standalone, private HTML report. Refuses to overwrite.
    Report {
        #[arg(long)]
        output: PathBuf,
    },
    /// Emit one session's usage as an agent-usage.v1 receipt, for a wrapper
    /// to import.
    Receipt,
    /// Current recorded usage for exactly one explicitly selected session.
    Status,
    /// Calendar usage statistics, in UTC. Weeks use ISO week years.
    Stats {
        #[arg(long, value_enum, default_value = "month")]
        period: metrics::Period,
    },
    /// Render Claude Code's native status-line JSON from stdin.
    ClaudeStatusline,
    /// The messages people typed into agent sessions, each with the agent
    /// reply it follows; harness, tool, subagent and headless turns are
    /// counted by rule, not listed. Every secret-looking token in what is
    /// printed becomes [redacted], and how many were is said. With --json:
    /// vessa-messages.v1.
    Messages {
        /// Only messages from this time on: RFC 3339, a UTC date
        /// (YYYY-MM-DD), or an age such as 7d or 12h.
        #[arg(long)]
        since: Option<String>,
        /// Print secret-looking tokens as written, not redacted; said on
        /// stderr. Nothing else turns the scrub off.
        #[arg(long)]
        raw: bool,
    },
}

fn run(cli: Cli) -> Result<i32> {
    if let Some(seconds) = cli.watch {
        ensure!(
            matches!(
                cli.command,
                Cmd::Usage { .. } | Cmd::Status | Cmd::Stats { .. }
            ),
            "--watch supports status, usage or stats only"
        );
        let mut sample = 0u64;
        loop {
            let code = run_once(cli.clone())?;
            sample += 1;
            if cli.samples.is_some_and(|n| sample >= n) {
                return Ok(code);
            }
            std::thread::sleep(std::time::Duration::from_secs(seconds));
        }
    }
    run_once(cli)
}

fn run_once(cli: Cli) -> Result<i32> {
    if matches!(cli.command, Cmd::ClaudeStatusline) {
        // A status line that fails goes blank: input that cannot be read is
        // said on stderr, and every field shows as unknown.
        use std::io::Read;
        let mut bytes = Vec::new();
        let input = match std::io::stdin().take(1_048_577).read_to_end(&mut bytes) {
            Err(error) => Err(format!("cannot read stdin: {error}")),
            Ok(_) if bytes.len() > 1_048_576 => Err("exceeds 1 MiB".into()),
            Ok(_) => serde_json::from_slice(&bytes).map_err(|error| error.to_string()),
        }
        .unwrap_or_else(|error| {
            eprintln!("vessa: status-line input unreadable ({error}); every field is unknown");
            serde_json::Value::Null
        });
        println!("{}", metrics::claude_statusline(&input));
        return Ok(0);
    }
    if let Cmd::Messages { since, raw } = &cli.command {
        return messages(&cli, since.as_deref(), *raw);
    }
    if matches!(cli.command, Cmd::Status) {
        ensure!(
            !cli.input.is_empty() || cli.session.is_some(),
            "status requires --input or --session; never guesses the latest session"
        );
    }
    let headers = matches!(cli.command, Cmd::Sessions { headers: true });
    let (paths, not_installed) = inputs(&cli)?;
    let mut report = if headers {
        import::headers(&paths)?
    } else {
        import::read(&paths)?
    };
    report.warnings.extend(not_installed);
    if let Some(agent) = cli.agent {
        supported(&agent)?;
        report.sessions.retain(|s| s.agent == agent);
    }
    if let Some(id) = cli.session {
        // A session's subagents are part of it: its id selects them too.
        report
            .sessions
            .retain(|s| s.id == id || s.subagent_of() == Some(id.as_str()));
        // Never answer for a session with its subagents alone: its own usage
        // would be unknown, and a subagent is not the session.
        let subagents = report.sessions.iter().filter(|s| s.id != id).count();
        // A main transcript some of whose records say they are a sidechain
        // was read, but as one of its subagents.
        let taken = report.sessions.iter().find(|s| s.partly_sidechain);
        ensure!(
            report.sessions.len() > subagents,
            "native session {id} not found{}",
            match taken {
                Some(s) => format!(
                    "; its transcript {} was read as its subagent {}, because some of its records say they are a sidechain and some that they are not",
                    s.source, s.id
                ),
                None if subagents > 0 => format!(
                    "; its main transcript was not read, only {subagents} of its subagents"
                ),
                None => String::new(),
            }
        );
    }
    // Partial means a source warning or session limitation.
    let source_warnings = report.warnings.len();
    let limited = report
        .sessions
        .iter()
        .filter(|s| !s.limitations.is_empty())
        .count();
    let incomplete = source_warnings > 0 || limited > 0;
    // Every command below prints the report's warnings and limitations, and
    // the report and exports its wrapper records: words from transcripts and
    // a wrapper, scrubbed of secrets as `messages` scrubs its own.
    let found = scrub::report(&mut report);
    // Imported names are untrusted terminal content, on stderr too.
    use render::for_terminal as clean;
    for warning in &report.warnings {
        eprintln!("vessa: {}", clean(warning));
    }
    match cli.command {
        Cmd::ClaudeStatusline | Cmd::Messages { .. } => unreachable!(),
        Cmd::Status => {
            let families = model::families(&report.sessions);
            ensure!(
                families.len() == 1,
                "status needs exactly one native session; select --session"
            );
            let family = &families[0];
            let s = family.main;
            let last_recorded = last_recorded_at(family.usage());
            if cli.json {
                let subagents = family
                    .subagents
                    .iter()
                    .map(|sub| {
                        Ok(serde_json::json!({"id":sub.id,"source":sub.source,"last_recorded_at":last_recorded_at(sub.usage.iter()),"entries":model::aggregate(sub)?}))
                    })
                    .collect::<Result<Vec<_>>>()?;
                json(
                    &serde_json::json!({"schema":"vessa-status.v2", "agent":s.agent,"session_id":s.id,"last_recorded_at":last_recorded,"entries":family.totals()?,"main_entries":model::aggregate(s)?,"subagents":subagents,"insights":work::summarize(s)?,"limitations":family.limitations(),"warnings":report.warnings}),
                )?;
            } else {
                let n = family.subagents.len();
                // pi writes no subagents.
                let scope = if s.agent == "pi" {
                    String::new()
                } else if let Some(parent) = s.subagent_of() {
                    format!(" · subagent of {parent}")
                } else {
                    match n {
                        0 => " · no subagent transcripts read".into(),
                        1 => " · totals include 1 subagent".into(),
                        n => format!(" · totals include {n} subagents"),
                    }
                };
                println!(
                    "{} {} · last recorded {}{}",
                    clean(&s.agent),
                    clean(&s.id),
                    last_recorded.as_deref().unwrap_or("unknown"),
                    clean(&scope),
                );
                let print = |indent: &str, entries: Vec<model::Entry>| {
                    if entries.is_empty() {
                        println!("{indent}usage unknown");
                    }
                    for e in entries {
                        println!("{indent}{}", render::usage_line(&e));
                    }
                };
                print("  ", family.totals()?);
                if n > 0 {
                    println!("  main transcript");
                    print("    ", model::aggregate(s)?);
                    for sub in &family.subagents {
                        println!(
                            "  subagent {} · last recorded {}",
                            clean(&sub.id),
                            last_recorded_at(sub.usage.iter())
                                .as_deref()
                                .unwrap_or("unknown")
                        );
                        if sub.usage.is_empty() && sub.counted_elsewhere > 0 {
                            println!(
                                "    no requests of its own; {} counted in another transcript",
                                sub.counted_elsewhere
                            );
                        } else {
                            print("    ", model::aggregate(sub)?);
                        }
                    }
                    println!("Commands below are the main transcript's; subagent usage is in the totals above.");
                }
                println!("{}", work::text(&work::summarize(s)?));
            }
        }
        Cmd::Stats { period } => {
            let buckets = metrics::history(&report, period)?;
            if cli.json {
                json(
                    &serde_json::json!({"schema":"vessa-statistics.v2","timezone":"UTC","buckets":buckets,"warnings":report.warnings,"limitations":report.sessions.iter().flat_map(|s| s.limitations.iter().map(move |l| format!("{}/{}: {l}",s.agent,s.id))).collect::<Vec<_>>() }),
                )?;
            } else {
                println!("Calendar usage · UTC · selected local records");
                for b in buckets {
                    println!(
                        "{} {} · {}{} · {} usage records",
                        b.period,
                        clean(&b.agent),
                        render::many(b.sessions, "session"),
                        if b.subagents > 0 {
                            format!(" · {}", render::many(b.subagents, "subagent"))
                        } else {
                            String::new()
                        },
                        b.records,
                    );
                    for e in b.entries {
                        println!("  {}", render::usage_line(&e));
                    }
                }
            }
        }
        Cmd::Report { output } => {
            let html = render::html(&report)?;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&output).with_context(|| {
                format!(
                    "cannot create {}; reports never overwrite existing files",
                    output.display()
                )
            })?;
            file.write_all(html.as_bytes())?;
            println!(
                "report: {} · {}",
                output.display(),
                model::counted(&report.sessions)
            );
        }
        Cmd::Receipt => {
            let families = model::families(&report.sessions);
            ensure!(
                families.len() == 1,
                "receipt needs exactly one session; select with --session"
            );
            json(&model::receipt(&families[0])?)?;
        }
        Cmd::Usage { by_agent: true } => {
            let mut groups = std::collections::BTreeMap::<&str, Vec<&model::Usage>>::new();
            for session in &report.sessions {
                groups
                    .entry(&session.agent)
                    .or_default()
                    .extend(&session.usage);
            }
            let totals = groups
                .into_iter()
                .map(|(agent, usage)| Ok((agent, model::aggregate_usage(usage.into_iter())?)))
                .collect::<Result<std::collections::BTreeMap<_, _>>>()?;
            if cli.json {
                json(&totals)?;
            } else {
                println!("Deduplicated agent totals");
                for (agent, entries) in totals {
                    println!("{}", clean(agent));
                    if entries.is_empty() {
                        println!("  usage unknown");
                    }
                    for entry in entries {
                        println!("  {}", render::usage_line(&entry));
                    }
                }
            }
        }
        Cmd::Sessions { headers: true } => {
            if cli.json {
                json(&serde_json::json!({
                    "schema": report.schema,
                    "sessions": report.sessions.iter().map(model::Session::header).collect::<Vec<_>>(),
                    "warnings": report.warnings,
                }))?;
            } else {
                for s in &report.sessions {
                    println!(
                        "{} {} · started {} · {}",
                        clean(&s.agent),
                        clean(&s.id),
                        clean(s.started_at.as_deref().unwrap_or("unknown")),
                        clean(&s.cwd)
                    );
                }
            }
        }
        Cmd::Sessions { .. } | Cmd::Usage { .. } if cli.json => {
            let mut export = serde_json::to_value(&report)?;
            // `totals` stays each transcript's own; a session's subagents are
            // named and summed with it in additive fields.
            let families: std::collections::HashMap<_, _> = model::families(&report.sessions)
                .into_iter()
                .filter(|f| !f.subagents.is_empty())
                .map(|f| ((f.main.agent.as_str(), f.main.id.as_str()), f))
                .collect();
            for (value, session) in export["sessions"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .zip(&report.sessions)
            {
                let totals = model::aggregate(session)?;
                let family = families.get(&(session.agent.as_str(), session.id.as_str()));
                let (subagents, with_subagents) = match family {
                    Some(f) => (
                        f.subagents.iter().map(|s| s.id.as_str()).collect(),
                        f.totals()?,
                    ),
                    None => (Vec::new(), totals.clone()),
                };
                value["totals"] = serde_json::to_value(totals)?;
                value["subagents"] = serde_json::to_value(subagents)?;
                value["totals_with_subagents"] = serde_json::to_value(with_subagents)?;
                value["synthetic_records"] = session.synthetic.len().into();
                value["insights"] = serde_json::to_value(work::summarize(session)?)?;
            }
            json(&export)?;
        }
        Cmd::Sessions { .. } => {
            for s in &report.sessions {
                println!(
                    "{} {} · {} usage records{} · {}",
                    clean(&s.agent),
                    clean(&s.id),
                    s.usage.len(),
                    match s.synthetic.len() {
                        0 => String::new(),
                        n => format!(" · {n} synthetic records (no usage)"),
                    },
                    clean(&s.cwd)
                );
            }
        }
        Cmd::Usage { .. } => {
            println!("Input excludes cached tokens. Unknown is not zero.");
            for s in &report.sessions {
                println!("{} {}", clean(&s.agent), clean(&s.id));
                let entries = model::aggregate(s)?;
                if entries.is_empty() {
                    println!("  usage unknown");
                }
                for e in entries {
                    println!("  {}", render::usage_line(&e));
                }
            }
        }
    }
    for s in &report.sessions {
        for limitation in &s.limitations {
            eprintln!(
                "vessa: {} {}: {}",
                clean(&s.agent),
                clean(&s.id),
                clean(limitation)
            );
        }
    }
    if let Some(said) = found.said() {
        eprintln!("vessa: {said}");
    }
    if incomplete && headers {
        // Headers read no usage, so they carry no limitations: only a
        // transcript that could not be read leaves the list partial.
        eprintln!(
            "vessa: partial session list (exit 4): {source_warnings} source {}",
            if source_warnings == 1 {
                "warning"
            } else {
                "warnings"
            },
        );
        return Ok(4);
    }
    if incomplete {
        // Last, so a long list of limitations still ends with why exit is 4.
        eprintln!(
            "vessa: partial usage evidence (exit 4): {limited} {} with limitations, {source_warnings} source {}",
            if limited == 1 { "session" } else { "sessions" },
            if source_warnings == 1 { "warning" } else { "warnings" },
        );
        return Ok(4);
    }
    Ok(0)
}

/// The paths given with `--input`, else the agents' own session folders.
/// A default folder that does not exist is an agent not installed here, or
/// one that has kept no session yet: nothing to read, and no gap. A path
/// given with `--input` that does not exist is a warning, and so is the
/// folder of an agent asked for with `--agent`: what was asked for cannot
/// be read. Returns the paths to read, and those warnings.
fn inputs(cli: &Cli) -> Result<(Vec<PathBuf>, Vec<String>)> {
    if !cli.input.is_empty() {
        return Ok((cli.input.clone(), Vec::new()));
    }
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME unavailable; pass --input")?);
    let mut paths = Vec::new();
    let mut warnings = Vec::new();
    for (agent, folder) in [
        ("claude", ".claude/projects"),
        ("codex", ".codex/sessions"),
        ("pi", ".pi/agent/sessions"),
    ] {
        let path = home.join(folder);
        // Any other failure to look, such as a folder that cannot be read,
        // is still read and reported.
        let missing = std::fs::symlink_metadata(&path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
        if !missing {
            paths.push(path);
        } else if cli.agent.as_deref() == Some(agent) {
            warnings.push(format!(
                "{agent}: not installed here ({} does not exist), so its sessions cannot be read",
                path.display()
            ));
        }
    }
    Ok((paths, warnings))
}

fn supported(agent: &str) -> Result<()> {
    ensure!(
        agent != "opencode",
        "unsupported agent opencode: it keeps its sessions in SQLite, which Vessa does not read"
    );
    ensure!(
        ["claude", "codex", "pi"].contains(&agent),
        "unsupported agent {agent}"
    );
    Ok(())
}

/// opencode's session store, when it was written in the window: a known
/// source Vessa does not read. SQLite writes through its `-wal` file, so
/// either may be the newer; a time that cannot be read counts as written.
fn opencode(since: Option<chrono::DateTime<chrono::FixedOffset>>) -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    let db = data.join("opencode/opencode.db");
    if !db.exists() {
        return None;
    }
    let Some(since) = since else {
        return Some(db);
    };
    let wal = db.with_file_name("opencode.db-wal");
    let written = |path: &std::path::Path| {
        path.exists()
            && std::fs::metadata(path)
                .and_then(|m| m.modified())
                .map_or(true, |t| chrono::DateTime::<chrono::Utc>::from(t) > since)
    };
    (written(&db) || written(&wal)).then_some(db)
}

/// `--since`: RFC 3339, a UTC date, or an age in days or hours.
fn since(text: &str) -> Result<chrono::DateTime<chrono::FixedOffset>> {
    if let Ok(time) = chrono::DateTime::parse_from_rfc3339(text) {
        return Ok(time);
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return Ok(date
            .and_time(chrono::NaiveTime::MIN)
            .and_utc()
            .fixed_offset());
    }
    let unit = |suffix: char, seconds: i64| {
        text.strip_suffix(suffix)
            .and_then(|n| n.parse::<i64>().ok())
            .filter(|n| *n >= 0)
            .and_then(|n| n.checked_mul(seconds))
    };
    let age = unit('d', 86_400)
        .or_else(|| unit('h', 3_600))
        .with_context(|| {
            format!("--since {text}: expected RFC 3339, YYYY-MM-DD, or an age such as 7d or 12h")
        })?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
    let at = i64::try_from(now.as_secs())?.saturating_sub(age);
    Ok(chrono::DateTime::from_timestamp(at, 0)
        .context("--since is out of range")?
        .fixed_offset())
}

/// `vessa-messages.v1`, written as messages arrive, one a line. The document
/// opens with its first message, so a run that fails before one leaves
/// nothing on stdout, and one that fails after ends it with its `error`.
struct Document<W: Write> {
    out: W,
    written: usize,
}

impl<W: Write> Document<W> {
    fn new(out: W) -> Self {
        Document { out, written: 0 }
    }

    fn message(&mut self, m: &model::Message) -> Result<()> {
        if self.written == 0 {
            write!(
                self.out,
                "{{\"schema\":\"vessa-messages.v1\",\"messages\":["
            )?;
        } else {
            write!(self.out, ",")?;
        }
        writeln!(self.out)?;
        serde_json::to_writer(&mut self.out, m)?;
        self.written += 1;
        Ok(())
    }

    /// Ends the document with what was read, and what was redacted (`found`;
    /// None under `--raw`).
    fn finish(
        &mut self,
        listing: &import::Listing,
        found: Option<&scrub::Redactions>,
    ) -> Result<()> {
        if self.written == 0 {
            write!(
                self.out,
                "{{\"schema\":\"vessa-messages.v1\",\"messages\":["
            )?;
        }
        write!(self.out, "\n],\n\"sessions\":")?;
        serde_json::to_writer(&mut self.out, &listing.sessions)?;
        write!(self.out, ",\n\"coverage\":")?;
        serde_json::to_writer(&mut self.out, &listing.coverage)?;
        write!(self.out, ",\n\"warnings\":")?;
        serde_json::to_writer(&mut self.out, &listing.warnings)?;
        self.redacted(found)?;
        writeln!(self.out, "}}")?;
        self.out.flush()?;
        Ok(())
    }

    /// Ends a document the failure left open. Its messages are those read
    /// before it; without coverage, it says nothing of the rest.
    fn fail(&mut self, error: &anyhow::Error, mut found: Option<&mut scrub::Redactions>) {
        if self.written > 0 {
            let error = format!("{error:#}");
            let error = match found.as_deref_mut() {
                Some(found) => found.text(&error),
                None => error,
            };
            let _ = write!(self.out, "\n],\n\"error\":");
            let _ = serde_json::to_writer(&mut self.out, &error);
            let _ = self.redacted(found.as_deref());
            let _ = writeln!(self.out, "}}");
        }
        let _ = self.out.flush();
    }

    /// How many secret-looking tokens the document's text had redacted, in
    /// all and by rule; both null under `--raw`, which redacts nothing.
    fn redacted(&mut self, found: Option<&scrub::Redactions>) -> Result<()> {
        write!(self.out, ",\n\"redacted\":")?;
        serde_json::to_writer(&mut self.out, &found.map(scrub::Redactions::total))?;
        write!(self.out, ",\n\"redacted_by_rule\":")?;
        serde_json::to_writer(&mut self.out, &found.map(scrub::Redactions::by_rule))?;
        Ok(())
    }
}

/// One message in the plain form: where it was said, then its text,
/// scrubbed of secrets into `found` unless `--raw` (None). The reply and
/// question are not printed here, so not counted.
fn plain(
    out: &mut impl Write,
    m: &model::Message,
    found: Option<&mut scrub::Redactions>,
) -> Result<()> {
    let text = match found {
        Some(found) => std::borrow::Cow::Owned(found.text(&m.text)),
        None => std::borrow::Cow::Borrowed(m.text.as_str()),
    };
    let mut notes = String::new();
    if m.kind != "message" {
        notes.push_str(" · ");
        notes.push_str(m.kind);
    }
    if m.reply.as_ref().is_some_and(|r| r.failed) {
        notes.push_str(" · after a failed reply");
    }
    // Imported text is untrusted terminal content, where it was said too.
    use render::for_terminal as clean;
    writeln!(
        out,
        "{} {} · {} · {} · {}:{}{notes}",
        clean(&m.agent),
        clean(&m.session),
        clean(m.at.as_deref().unwrap_or("undated")),
        clean(&m.cwd),
        clean(&m.source),
        m.line,
    )?;
    for line in text.lines() {
        writeln!(out, "  {}", clean(line))?;
    }
    Ok(())
}

/// `vessa messages`: each message is written as its transcript is read,
/// with every secret-looking token in its words redacted unless `raw`.
fn messages(cli: &Cli, from: Option<&str>, raw: bool) -> Result<i32> {
    if let Some(agent) = &cli.agent {
        supported(agent)?;
    }
    let select = import::Selection {
        agent: cli.agent.clone(),
        session: cli.session.clone(),
        since: from.map(since).transpose()?,
    };
    let (paths, not_installed) = inputs(cli)?;
    // The default sources hold every agent Vessa reads; one it knows of and
    // does not read, written in the window, leaves the list partial.
    let gap = if cli.input.is_empty() && cli.agent.is_none() {
        opencode(select.since)
    } else {
        None
    };
    let gaps = gap.into_iter().map(|db| {
        format!(
            "opencode: {} not read; opencode keeps its sessions in SQLite, which Vessa does not read",
            db.display()
        )
    });
    // What is redacted, by rule; None under `--raw`, which redacts nothing.
    // Each message is scrubbed as it is printed, after it was read, so the
    // rules that tell a person's messages apart see the words as written.
    let mut found = (!raw).then(scrub::Redactions::default);
    if raw {
        eprintln!("vessa: --raw: secret-looking tokens are printed as written, not redacted");
    }
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    let listing = if cli.json {
        let mut doc = Document::new(&mut out);
        let read = import::messages(&paths, &select, !raw, &mut |m| match found.as_mut() {
            Some(found) => doc.message(&found.message(m)),
            None => doc.message(m),
        });
        match read {
            Ok(mut listing) => {
                listing.coverage.gaps.extend(gaps);
                listing.warnings.extend(not_installed);
                scrubbed(&mut listing, found.as_mut());
                doc.finish(&listing, found.as_ref())?;
                listing
            }
            Err(error) => {
                doc.fail(&error, found.as_mut());
                return Err(error);
            }
        }
    } else {
        let mut listing = import::messages(&paths, &select, !raw, &mut |m| {
            plain(&mut out, m, found.as_mut())
        })?;
        listing.coverage.gaps.extend(gaps);
        listing.warnings.extend(not_installed);
        scrubbed(&mut listing, found.as_mut());
        let c = &listing.coverage;
        let rules = c
            .excluded
            .iter()
            .map(|(rule, n)| format!("{rule} {n}"))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            out,
            "{} from {} ({} headless) · {} other user turns not listed{} · {} Codex subagent transcripts not read",
            render::many(c.human, "message"),
            render::many(
                listing.sessions.iter().filter(|s| s.messages > 0).count(),
                "session"
            ),
            listing.sessions.iter().filter(|s| s.headless).count(),
            c.excluded.values().sum::<usize>(),
            if rules.is_empty() { String::new() } else { format!(": {rules}") },
            c.subagent_transcripts,
        )?;
        if let Some(since) = &c.since {
            writeln!(
                out,
                "after {since}: {} earlier and {} undated messages not listed; {} transcripts unmodified since, not read",
                c.earlier, c.undated, c.unmodified_transcripts
            )?;
        }
        out.flush()?;
        listing
    };
    drop(out);
    let c = &listing.coverage;
    use render::for_terminal as clean;
    for warning in listing.warnings.iter().chain(&c.gaps) {
        eprintln!("vessa: {}", clean(warning));
    }
    for limitation in &c.limitations {
        eprintln!("vessa: {}", clean(limitation));
    }
    // The document says it in `redacted`; the plain form, here.
    if let Some(said) = found.as_ref().and_then(scrub::Redactions::said) {
        if !cli.json {
            eprintln!("vessa: {said}");
        }
    }
    let gaps = listing.warnings.len() + c.gaps.len();
    if gaps + c.limitations.len() > 0 {
        eprintln!(
            "vessa: partial message list (exit 4): {gaps} source {}, {} {}",
            if gaps == 1 { "warning" } else { "warnings" },
            c.limitations.len(),
            if c.limitations.len() == 1 {
                "limitation"
            } else {
                "limitations"
            },
        );
        return Ok(4);
    }
    Ok(0)
}

/// What `messages` says of its sources, which quotes paths, ids and model
/// names from transcripts, scrubbed into `found` as the messages are;
/// untouched under `--raw` (None).
fn scrubbed(listing: &mut import::Listing, found: Option<&mut scrub::Redactions>) {
    if let Some(found) = found {
        let c = &mut listing.coverage;
        found.lines(
            listing
                .warnings
                .iter_mut()
                .chain(&mut c.gaps)
                .chain(&mut c.limitations),
        );
    }
}

/// The time of the latest usage record, so an old transcript is not taken
/// for a live balance.
fn last_recorded_at<'a>(usage: impl Iterator<Item = &'a model::Usage>) -> Option<String> {
    usage
        .filter_map(|u| u.at.as_deref())
        .filter_map(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .max()
        .map(|t| t.to_rfc3339())
}

fn json(value: &impl serde::Serialize) -> Result<()> {
    serde_json::to_writer_pretty(std::io::stdout().lock(), value)?;
    println!();
    Ok(())
}

fn main() {
    let cli = Cli::parse();
    // An error can quote what was read, or typed: scrubbed of secrets as
    // the rest is, unless `messages --raw` asked for the words as written.
    let raw = matches!(cli.command, Cmd::Messages { raw: true, .. });
    match run(cli) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            let error = format!("{e:#}");
            let error = if raw {
                error
            } else {
                scrub::Redactions::default().text(&error)
            };
            eprintln!("vessa: {}", render::for_terminal(&error));
            std::process::exit(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(text: &str) -> model::Message {
        model::Message {
            agent: "claude".into(),
            session: "claude-demo".into(),
            parent: None,
            kind: "message",
            id: Some("u1".into()),
            at: Some("2026-09-30T09:00:00Z".into()),
            cwd: "/demo/widget".into(),
            text: text.into(),
            question: None,
            reply: None,
            source: "/demo/claude-demo.jsonl".into(),
            line: 1,
        }
    }

    #[test]
    fn a_failure_mid_stream_still_leaves_one_json_document() {
        let mut out = Vec::new();
        let mut doc = Document::new(&mut out);
        doc.message(&message("Run the checks.")).unwrap();
        doc.message(&message("Then commit.")).unwrap();
        let mut found = scrub::Redactions::default();
        let key = ["gh", "p_", "Ab3dE5gH7jK9mN1pQ3sT5vW7yZ9bC1dF3hJ5"].concat();
        doc.fail(
            &anyhow::anyhow!("transcript vanished after {key}"),
            Some(&mut found),
        );
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["schema"], "vessa-messages.v1");
        assert_eq!(v["messages"].as_array().unwrap().len(), 2);
        assert_eq!(v["error"], "transcript vanished after [redacted]");
        assert_eq!(v["redacted"], 1);
        assert_eq!(v["redacted_by_rule"], serde_json::json!({"github": 1}));
        assert!(v.get("coverage").is_none());
        // Under --raw, nothing is redacted, and the document says it counted
        // nothing.
        let mut out = Vec::new();
        let mut doc = Document::new(&mut out);
        doc.message(&message("Run the checks.")).unwrap();
        doc.fail(&anyhow::anyhow!("transcript vanished"), None);
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert!(v["redacted"].is_null());
        assert!(v["redacted_by_rule"].is_null());
    }

    #[test]
    fn a_failure_before_any_message_leaves_nothing() {
        let mut out = Vec::new();
        Document::new(&mut out).fail(&anyhow::anyhow!("no such session"), None);
        assert!(out.is_empty());
    }
}
