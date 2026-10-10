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
    Sessions,
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
    if matches!(cli.command, Cmd::Status) {
        ensure!(
            !cli.input.is_empty() || cli.session.is_some(),
            "status requires --input or --session; never guesses the latest session"
        );
    }
    let paths = if cli.input.is_empty() {
        let home =
            PathBuf::from(std::env::var_os("HOME").context("HOME unavailable; pass --input")?);
        vec![
            home.join(".claude/projects"),
            home.join(".codex/sessions"),
            home.join(".pi/agent/sessions"),
        ]
    } else {
        cli.input
    };
    let mut report = import::read(&paths)?;
    if let Some(agent) = cli.agent {
        ensure!(
            ["claude", "codex", "pi"].contains(&agent.as_str()),
            "unsupported agent {agent}"
        );
        report.sessions.retain(|s| s.agent == agent);
    }
    if let Some(id) = cli.session {
        report.sessions.retain(|s| s.id == id);
        ensure!(!report.sessions.is_empty(), "native session {id} not found");
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
        Cmd::ClaudeStatusline => unreachable!(),
        Cmd::Status => {
            ensure!(
                report.sessions.len() == 1,
                "status needs exactly one native session; select --session"
            );
            let s = &report.sessions[0];
            let last = s
                .usage
                .iter()
                .filter_map(|u| u.at.as_deref())
                .filter_map(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .max()
                .map(|t| t.to_rfc3339());
            if cli.json {
                json(
                    &serde_json::json!({"schema":"vessa-status.v1", "agent":s.agent,"session_id":s.id,"last_recorded_at":last,"entries":model::aggregate(s)?,"insights":work::summarize(s)?,"limitations":s.limitations,"warnings":report.warnings}),
                )?;
            } else {
                println!(
                    "{} {} · last recorded {}",
                    s.agent,
                    s.id,
                    last.as_deref().unwrap_or("unknown")
                );
                for e in model::aggregate(s)? {
                    println!("  {}", render::usage_line(&e));
                }
                println!("{}", work::text(&work::summarize(s)?));
                if s.usage.is_empty() {
                    println!("  usage unknown");
                }
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
                        "{} {} · {} sessions · known estimate subtotal={} · {} unpriced records",
                        b.period,
                        b.agent,
                        b.sessions,
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
            ensure!(
                report.sessions.len() == 1,
                "receipt needs exactly one session; select with --session"
            );
            json(&model::receipt(&report.sessions[0])?)?;
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
        Cmd::Sessions | Cmd::Usage { .. } if cli.json => {
            let mut export = serde_json::to_value(&report)?;
            for (value, session) in export["sessions"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .zip(&report.sessions)
            {
                value["totals"] = serde_json::to_value(model::aggregate(session)?)?;
                value["insights"] = serde_json::to_value(work::summarize(session)?)?;
            }
            json(&export)?;
        }
        Cmd::Sessions => {
            for s in &report.sessions {
                println!(
                    "{} {} · {} usage records · {}",
                    s.agent,
                    s.id,
                    s.usage.len(),
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
