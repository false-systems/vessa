mod analysis;
mod import;
mod metrics;
mod model;
mod render;
mod work;

use anyhow::{ensure, Context, Result};
use clap::{Parser, Subcommand};
use std::io::Write;
use std::path::PathBuf;

#[derive(Parser, Clone)]
#[command(about = "Local agent usage and spending evidence. Estimates are not bills.")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
    /// Native JSONL file/directory or ohjaa JSON export; repeatable.
    #[arg(long, global = true)]
    input: Vec<PathBuf>,
    #[arg(long, global = true)]
    agent: Option<String>,
    /// Native session ID, not wrapper session ID.
    #[arg(long, global = true)]
    session: Option<String>,
    /// Dated rate-card JSON array. No baked-in prices.
    #[arg(long, global = true)]
    rates: Option<PathBuf>,
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
    /// Token categories and estimates by session/provider/model.
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
    /// Emit one agent-usage.v1 snapshot for `ohjaa usage import`.
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
    /// counted by rule, not listed. With --json: vessa-messages.v1.
    Messages {
        /// Only messages from this time on: RFC 3339, a UTC date
        /// (YYYY-MM-DD), or an age such as 7d or 12h.
        #[arg(long)]
        since: Option<String>,
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
        use std::io::Read;
        let mut bytes = Vec::new();
        std::io::stdin().take(1_048_577).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 1_048_576, "statusline input exceeds 1 MiB");
        let input = serde_json::from_slice(&bytes)?;
        println!("{}", metrics::claude_statusline(&input)?);
        return Ok(0);
    }
    if let Cmd::Messages { since } = &cli.command {
        ensure!(
            cli.rates.is_none(),
            "--rates cannot be used with messages, which reads no usage to price"
        );
        return messages(&cli, since.as_deref());
    }
    if matches!(cli.command, Cmd::Status) {
        ensure!(
            !cli.input.is_empty() || cli.session.is_some(),
            "status requires --input or --session; never guesses the latest session"
        );
    }
    let headers = matches!(cli.command, Cmd::Sessions { headers: true });
    // Checked here, not by clap: `--rates` is global, and given before the
    // subcommand a clap conflict does not see it. Headers read no usage to
    // price, and rate-card notices would read as source warnings.
    ensure!(
        !headers || cli.rates.is_none(),
        "--rates cannot be used with sessions --headers, which reads no usage to price"
    );
    let paths = inputs(&cli)?;
    let mut report = if headers {
        import::headers(&paths)?
    } else {
        import::read(&paths)?
    };
    if let Some(agent) = cli.agent {
        supported(&agent)?;
        report.sessions.retain(|s| s.agent == agent);
    }
    if let Some(id) = cli.session {
        // A Claude session's subagents are part of it: its id selects them too.
        report
            .sessions
            .retain(|s| s.id == id || s.subagent_of() == Some(id.as_str()));
        // Never answer for a session with its subagents alone: its own usage
        // would be unknown, and a subagent is not the session.
        let subagents = report.sessions.iter().filter(|s| s.id != id).count();
        ensure!(
            report.sessions.len() > subagents,
            "native session {id} not found{}",
            if subagents > 0 {
                format!("; its main transcript was not read, only {subagents} of its subagents")
            } else {
                String::new()
            }
        );
    }
    // Partial means the usage evidence itself is incomplete: a source warning
    // or a session limitation. Rate-card notices added below concern pricing
    // only; unpriced usage is already shown as such.
    let source_warnings = report.warnings.len();
    let limited = report
        .sessions
        .iter()
        .filter(|s| !s.limitations.is_empty())
        .count();
    let incomplete = source_warnings > 0 || limited > 0;
    if let Some(path) = cli.rates {
        let rates: Vec<analysis::Rate> = serde_json::from_reader(std::fs::File::open(path)?)?;
        analysis::apply_rates(&mut report, &rates)?;
    }
    for warning in &report.warnings {
        eprintln!("vessa: {warning}");
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
                    &serde_json::json!({"schema":"vessa-status.v1", "agent":s.agent,"session_id":s.id,"last_recorded_at":last_recorded,"entries":family.totals()?,"main_entries":model::aggregate(s)?,"subagents":subagents,"insights":work::summarize(s)?,"limitations":family.limitations(),"warnings":report.warnings}),
                )?;
            } else {
                let n = family.subagents.len();
                let scope = if s.agent != "claude" {
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
                    "{} {} · last recorded {}{scope}",
                    s.agent,
                    s.id,
                    last_recorded.as_deref().unwrap_or("unknown"),
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
                            sub.id,
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
                    &serde_json::json!({"schema":"vessa-statistics.v1","timezone":"UTC","buckets":buckets,"warnings":report.warnings,"limitations":report.sessions.iter().flat_map(|s| s.limitations.iter().map(move |l| format!("{}/{}: {l}",s.agent,s.id))).collect::<Vec<_>>() }),
                )?;
            } else {
                println!("Calendar usage · UTC · selected local records · estimates are not bills");
                for b in buckets {
                    println!(
                        "{} {} · {} sessions{} · known estimate subtotal={} · {} unpriced records",
                        b.period,
                        b.agent,
                        b.sessions,
                        if b.subagents > 0 {
                            format!(" · {} subagents", b.subagents)
                        } else {
                            String::new()
                        },
                        render::dollars(Some(b.known_estimated_usd_micros)),
                        b.unpriced_records
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
                "report: {} · {} sessions",
                output.display(),
                report.sessions.len()
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
                println!("Deduplicated agent totals · estimates are not bills");
                for (agent, entries) in totals {
                    println!("{agent}");
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
                        s.agent,
                        s.id,
                        s.started_at.as_deref().unwrap_or("unknown"),
                        s.cwd
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
                    s.agent,
                    s.id,
                    s.usage.len(),
                    match s.synthetic.len() {
                        0 => String::new(),
                        n => format!(" · {n} synthetic records (no usage)"),
                    },
                    s.cwd
                );
            }
        }
        Cmd::Usage { .. } => {
            println!("Estimates are not bills. Input excludes cached tokens. Unknown is not zero.");
            for s in &report.sessions {
                println!("{} {}", s.agent, s.id);
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
            eprintln!("vessa: {} {}: {limitation}", s.agent, s.id);
        }
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
/// given with `--input` that does not exist is a warning.
fn inputs(cli: &Cli) -> Result<Vec<PathBuf>> {
    if !cli.input.is_empty() {
        return Ok(cli.input.clone());
    }
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME unavailable; pass --input")?);
    Ok([
        home.join(".claude/projects"),
        home.join(".codex/sessions"),
        home.join(".pi/agent/sessions"),
    ]
    .into_iter()
    // Any other failure to look, such as a folder that cannot be read, is
    // still read and reported.
    .filter(|path| {
        std::fs::symlink_metadata(path).map_or_else(
            |error| error.kind() != std::io::ErrorKind::NotFound,
            |_| true,
        )
    })
    .collect())
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

    fn finish(&mut self, listing: &import::Listing) -> Result<()> {
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
        writeln!(self.out, "}}")?;
        self.out.flush()?;
        Ok(())
    }

    /// Ends a document the failure left open. Its messages are those read
    /// before it; without coverage, it says nothing of the rest.
    fn fail(&mut self, error: &anyhow::Error) {
        if self.written > 0 {
            let _ = write!(self.out, "\n],\n\"error\":");
            let _ = serde_json::to_writer(&mut self.out, &format!("{error:#}"));
            let _ = writeln!(self.out, "}}");
        }
        let _ = self.out.flush();
    }
}

/// One message in the plain form: where it was said, then its text.
fn plain(out: &mut impl Write, m: &model::Message) -> Result<()> {
    let mut notes = String::new();
    if m.kind != "message" {
        notes.push_str(" · ");
        notes.push_str(m.kind);
    }
    if m.reply.as_ref().is_some_and(|r| r.failed) {
        notes.push_str(" · after a failed reply");
    }
    writeln!(
        out,
        "{} {} · {} · {} · {}:{}{notes}",
        m.agent,
        m.session,
        m.at.as_deref().unwrap_or("undated"),
        m.cwd,
        m.source,
        m.line,
    )?;
    for line in m.text.lines() {
        // Imported text is untrusted terminal content.
        let line: String = line.chars().filter(|c| !c.is_control()).collect();
        writeln!(out, "  {line}")?;
    }
    Ok(())
}

/// `vessa messages`: each message is written as its transcript is read.
fn messages(cli: &Cli, from: Option<&str>) -> Result<i32> {
    if let Some(agent) = &cli.agent {
        supported(agent)?;
    }
    let select = import::Selection {
        agent: cli.agent.clone(),
        session: cli.session.clone(),
        since: from.map(since).transpose()?,
    };
    let paths = inputs(cli)?;
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
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    let listing = if cli.json {
        let mut doc = Document::new(&mut out);
        match import::messages(&paths, &select, &mut |m| doc.message(m)) {
            Ok(mut listing) => {
                listing.coverage.gaps.extend(gaps);
                doc.finish(&listing)?;
                listing
            }
            Err(error) => {
                doc.fail(&error);
                return Err(error);
            }
        }
    } else {
        let mut listing = import::messages(&paths, &select, &mut |m| plain(&mut out, m))?;
        listing.coverage.gaps.extend(gaps);
        let c = &listing.coverage;
        let rules = c
            .excluded
            .iter()
            .map(|(rule, n)| format!("{rule} {n}"))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            out,
            "{} messages from {} sessions ({} headless) · {} other user turns not listed{} · {} Codex subagent transcripts not read",
            c.human,
            listing.sessions.iter().filter(|s| s.messages > 0).count(),
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
    for warning in listing.warnings.iter().chain(&c.gaps) {
        eprintln!("vessa: {warning}");
    }
    for limitation in &c.limitations {
        eprintln!("vessa: {limitation}");
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
    match run(Cli::parse()) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("vessa: {e:#}");
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
        doc.fail(&anyhow::anyhow!("transcript vanished"));
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["schema"], "vessa-messages.v1");
        assert_eq!(v["messages"].as_array().unwrap().len(), 2);
        assert_eq!(v["error"], "transcript vanished");
        assert!(v.get("coverage").is_none());
    }

    #[test]
    fn a_failure_before_any_message_leaves_nothing() {
        let mut out = Vec::new();
        Document::new(&mut out).fail(&anyhow::anyhow!("no such session"));
        assert!(out.is_empty());
    }
}
