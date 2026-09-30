mod analysis;
mod import;
mod model;
mod render;

use anyhow::{ensure, Context, Result};
use clap::{Parser, Subcommand};
use std::io::Write;
use std::path::PathBuf;

#[derive(Parser)]
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
}

#[derive(Subcommand)]
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
}

fn run(cli: Cli) -> Result<i32> {
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
        report.sessions.retain(|s| s.agent == agent);
    }
    if let Some(id) = cli.session {
        report.sessions.retain(|s| s.id == id);
        ensure!(!report.sessions.is_empty(), "native session {id} not found");
    }
    if let Some(path) = cli.rates {
        let rates: Vec<analysis::Rate> = serde_json::from_reader(std::fs::File::open(path)?)?;
        analysis::apply_rates(&mut report, &rates)?;
    }
    for warning in &report.warnings {
        eprintln!("vessa: {warning}");
    }
    let incomplete =
        !report.warnings.is_empty() || report.sessions.iter().any(|s| !s.limitations.is_empty());
    match cli.command {
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
        Cmd::Sessions | Cmd::Usage { .. } if cli.json => json(&report)?,
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
    Ok(if incomplete { 4 } else { 0 })
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
