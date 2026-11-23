//! `vessa line`: what happened, for the person at a terminal's opening.
//!
//! The human view ohjaa once kept moved here (ohjaa's 0.9.0 cut removed it;
//! its step 5 names vessa as the home), built only from what vessa owns:
//! the sessions the agents left on disk and the usage they recorded. A line
//! reads nothing else — no git, no network, no other tool's state — and it
//! starts nothing.
//!
//! A line is for a person. It prints only when stdout is a terminal and no
//! agent session's marker is set (an agent's shell reads `~/.zshrc` too),
//! once a day or when what it says changes, and it always exits 0: a
//! terminal that cannot say what happened still opens. What could not be
//! read is part of the line, said as that, never an all-clear over a gap.
//!
//! `--always` prints now, whatever the cursor and stdout say — still never
//! under an agent marker — and counts as shown. `--json` prints the
//! `vessa-line.v1` record and changes nothing: inspection moves no cursor.
//!
//! What was shown is kept: the cursor in `$XDG_STATE_HOME/vessa/line.json`
//! (or `~/.local/state/vessa/line.json`), and one record per shown line in
//! `vessa/line-log.jsonl`, the log the human view once wrote, owned here.

use std::ffi::OsString;
use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::model;

/// The shells an agent runs its tools in carry these, each set by the agent
/// (or by `toimija run` around it), never by a person's own startup files.
/// A variable a person may set (such as `CODEX_HOME`) is not one. The list
/// ohjaa's human view kept, carried over as it was.
pub const AGENT_MARKERS: [&str; 16] = [
    "TOIMIJA_SESSION",
    "TOIMIJA_RUN",
    // Claude Code.
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    // Codex: its thread and session, its CI mode and sandbox, and the
    // launcher npm installs.
    "CODEX_THREAD_ID",
    "CODEX_SESSION_ID",
    "CODEX_CI",
    "CODEX_SANDBOX",
    "CODEX_SANDBOX_NETWORK_DISABLED",
    "CODEX_MANAGED_BY_NPM",
    // pi.
    "PI_SESSION_ID",
    "PI_CODING_AGENT",
    // opencode.
    "OPENCODE",
    "OPENCODE_SESSION_ID",
    // What Claude Code, pi and opencode set for any tool to read.
    "AI_AGENT",
];

/// The marker an agent's shell carries, when one does.
fn agent_marker(var: impl Fn(&str) -> Option<OsString>) -> Option<&'static str> {
    AGENT_MARKERS
        .iter()
        .find(|marker| var(marker).is_some())
        .copied()
}

/// The day's line, as `--json` prints it.
#[derive(Serialize)]
struct Line {
    schema: &'static str,
    /// The UTC day the line speaks for.
    day: String,
    /// Sessions of the day, as `vessa stats` counts them: roots, and
    /// subagents with them.
    sessions: usize,
    subagents: usize,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_tokens: Option<u64>,
    /// Sources that could not be read, of every session, not only today's.
    warnings: usize,
    /// Sessions of the day read with limitations.
    limited: usize,
    /// What a terminal shows.
    text: String,
}

/// The cursor: what the person was last shown, and when.
#[derive(Serialize, Deserialize)]
struct Cursor {
    schema: String,
    day: String,
    line: String,
    shown_at: String,
}

const CURSOR_SCHEMA: &str = "vessa-line-cursor.v1";
const LINE_SCHEMA: &str = "vessa-line.v1";

/// Now, as an RFC 3339 UTC time: the crate's clock-less chrono builds
/// `now` from the system clock, as `--since` does.
fn now() -> chrono::DateTime<chrono::Utc> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the system clock is before the epoch")
        .as_secs();
    chrono::DateTime::from_timestamp(i64::try_from(seconds).expect("the clock is in range"), 0)
        .expect("the clock is in range")
}

/// Run `vessa line`. Always exits 0: a line that cannot be worked out says
/// so, and a terminal opens whatever happened.
pub fn run(paths: &[PathBuf], warnings: &[String], json: bool, always: bool) -> Result<i32> {
    if agent_marker(|name| std::env::var_os(name)).is_some() {
        // An agent's shell gets no line, whatever it asks for.
        return Ok(0);
    }
    let report = match crate::import::read(paths) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("vessa line: the sessions cannot be read: {error:#}");
            return Ok(0);
        }
    };
    let mut warnings = warnings.to_vec();
    warnings.extend(report.warnings);
    let today = now().date_naive();
    let of_the_day: Vec<model::Session> = report
        .sessions
        .iter()
        .filter(|session| started(session).is_some_and(|day| day == today))
        .cloned()
        .collect();
    let (sessions, subagents) = model::count(&of_the_day);
    // Tokens are a sum of what was recorded, by `model::add`'s rule: an
    // entry that lacks a category makes that category unknown, never a
    // complete-looking total. No entries at all is no usage recorded.
    let (mut entries, mut input, mut output, mut cache_read) =
        (0usize, Some(0u64), Some(0u64), Some(0u64));
    for session in &of_the_day {
        for usage in &session.usage {
            entries += 1;
            input = model::add(input, usage.entry.input_tokens)?;
            output = model::add(output, usage.entry.output_tokens)?;
            cache_read = model::add(cache_read, usage.entry.cache_read_tokens)?;
        }
    }
    let unknown = |sum: Option<u64>| if entries == 0 { None } else { sum };
    let (input, output, cache_read) = (unknown(input), unknown(output), unknown(cache_read));
    let limited = of_the_day
        .iter()
        .filter(|session| !session.limitations.is_empty())
        .count();
    let text = text(
        sessions,
        subagents,
        input,
        output,
        cache_read,
        warnings.len(),
        limited,
    );
    let line = Line {
        schema: LINE_SCHEMA,
        day: today.to_string(),
        sessions,
        subagents,
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        warnings: warnings.len(),
        limited,
        text: text.clone(),
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&line)?);
        return Ok(0);
    }
    if !always && !std::io::stdout().is_terminal() {
        return Ok(0);
    }
    let shown_at = now().to_rfc3339();
    if !always && !due(cursor().ok().flatten().as_ref(), &line.day, &text) {
        return Ok(0);
    }
    println!("{text}");
    keep(&Cursor {
        schema: CURSOR_SCHEMA.to_owned(),
        day: line.day,
        line: text,
        shown_at,
    });
    Ok(0)
}

/// What the day's line says.
fn text(
    sessions: usize,
    subagents: usize,
    input: Option<u64>,
    output: Option<u64>,
    cache_read: Option<u64>,
    warnings: usize,
    limited: usize,
) -> String {
    let mut said = String::new();
    if sessions == 0 {
        said.push_str("no agent sessions recorded today");
    } else {
        let counted = match subagents {
            0 => crate::render::many(sessions, "session"),
            _ => format!(
                "{} · {}",
                crate::render::many(sessions, "session"),
                crate::render::many(subagents, "subagent")
            ),
        };
        said.push_str(&counted);
        said.push_str(" today");
        match (input, output) {
            (Some(input), Some(output)) => {
                said.push_str(&format!(" · {} in", magnitude(input)));
                if let Some(cache_read) = cache_read.filter(|cache| *cache > 0) {
                    said.push_str(&format!(" · {} cached", magnitude(cache_read)));
                }
                said.push_str(&format!(" · {} out", magnitude(output)));
            }
            _ => said.push_str(" · no usage recorded"),
        }
    }
    if warnings > 0 || limited > 0 {
        said.push_str("; not all read: ");
        let mut parts = Vec::new();
        if warnings > 0 {
            parts.push(crate::render::many(warnings, "source warning"));
        }
        if limited > 0 {
            parts.push(format!(
                "{} with limitations",
                crate::render::many(limited, "session")
            ));
        }
        said.push_str(&parts.join(", "));
    }
    said
}

/// A token count for a person: plain below a thousand, then `k` and `M`,
/// one decimal while the count is small enough for it to say something
/// (1.2k, 9.9k) and a whole number from there (302k, 12M).
fn magnitude(tokens: u64) -> String {
    let count = tokens as f64;
    let (divided, suffix) = if count < 1_000.0 {
        return tokens.to_string();
    } else if count < 1_000_000.0 {
        (count / 1_000.0, "k")
    } else {
        (count / 1_000_000.0, "M")
    };
    if divided < 10.0 {
        format!("{divided:.1}{suffix}")
    } else {
        format!("{}{suffix}", divided.round() as u64)
    }
}

/// A session's start, as a UTC day, when its transcript says one.
fn started(session: &model::Session) -> Option<chrono::NaiveDate> {
    chrono::DateTime::parse_from_rfc3339(session.started_at.as_deref()?)
        .ok()
        .map(|at| at.date_naive())
}

/// The vessa state directory: `$XDG_STATE_HOME` when it names an absolute
/// path, else `$HOME/.local/state`; `None` when neither can be named.
fn state_dir() -> Option<PathBuf> {
    let absolute = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
    match absolute {
        Some(state) => Some(state.join("vessa")),
        None => std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join(".local")
                .join("state")
                .join("vessa")
        }),
    }
}

/// The cursor, when there is one that says what it is.
fn cursor() -> Result<Option<Cursor>> {
    let Some(dir) = state_dir() else {
        return Ok(None);
    };
    let read = std::fs::read_to_string(dir.join("line.json")).or_else(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            Ok(String::new())
        } else {
            Err(error)
        }
    })?;
    if read.is_empty() {
        return Ok(None);
    }
    let cursor: Cursor = serde_json::from_str(&read)?;
    if cursor.schema != CURSOR_SCHEMA {
        return Ok(None);
    }
    Ok(Some(cursor))
}

/// Whether the person should see this line: a new day, or a line that
/// changed. A cursor that is not there always says yes.
fn due(cursor: Option<&Cursor>, day: &str, text: &str) -> bool {
    match cursor {
        Some(cursor) => cursor.day != day || cursor.line != text,
        None => true,
    }
}

/// Keep the cursor and append the log. Both are said, never fatal: a
/// terminal opens with or without them.
fn keep(cursor: &Cursor) {
    let Some(dir) = state_dir() else {
        eprintln!("vessa line: neither XDG_STATE_HOME nor HOME names a state directory; the line is not kept");
        return;
    };
    if let Err(error) = std::fs::create_dir_all(&dir).and_then(|()| {
        std::fs::write(
            dir.join("line.json"),
            serde_json::to_string_pretty(cursor).expect("the cursor serialises"),
        )
    }) {
        eprintln!(
            "vessa line: the cursor is not kept ({}): {error}",
            dir.join("line.json").display()
        );
        return;
    }
    let log = serde_json::json!({
        "at": cursor.shown_at,
        "day": cursor.day,
        "line": cursor.line,
    });
    use std::io::Write;
    let append = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("line-log.jsonl"))
        .and_then(|mut file| writeln!(file, "{log}"));
    if let Err(error) = append {
        eprintln!(
            "vessa line: the line log is not kept ({}): {error}",
            dir.join("line-log.jsonl").display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magnitudes_read_as_a_person_reads_them() {
        assert_eq!(magnitude(0), "0");
        assert_eq!(magnitude(999), "999");
        assert_eq!(magnitude(1_200), "1.2k");
        assert_eq!(magnitude(9_940), "9.9k");
        assert_eq!(magnitude(302_000), "302k");
        assert_eq!(magnitude(1_400_000), "1.4M");
        assert_eq!(magnitude(12_300_000), "12M");
    }

    #[test]
    fn an_empty_day_says_so_and_gaps_are_never_an_all_clear() {
        let said = text(0, 0, None, None, None, 0, 0);
        assert_eq!(said, "no agent sessions recorded today");
        let said = text(0, 0, None, None, None, 1, 0);
        assert_eq!(
            said,
            "no agent sessions recorded today; not all read: 1 source warning"
        );
    }

    #[test]
    fn a_day_is_counted_with_its_usage_and_its_limits() {
        let said = text(3, 2, Some(1_400_000), Some(302_000), Some(12_300_000), 0, 0);
        assert_eq!(
            said,
            "3 sessions · 2 subagents today · 1.4M in · 12M cached · 302k out"
        );
        let said = text(1, 0, Some(1_200), Some(300), None, 2, 1);
        assert_eq!(
            said,
            "1 session today · 1.2k in · 300 out; not all read: 2 source warnings, 1 session with limitations"
        );
        let said = text(1, 0, None, None, None, 0, 0);
        assert_eq!(said, "1 session today · no usage recorded");
    }

    #[test]
    fn an_agent_marker_is_one_of_the_agents_own_variables() {
        assert_eq!(
            agent_marker(|name| (name == "CLAUDECODE").then(|| OsString::from("1"))),
            Some("CLAUDECODE")
        );
        assert_eq!(
            agent_marker(|name| (name == "CODEX_HOME").then(|| OsString::from("1"))),
            None
        );
        assert_eq!(agent_marker(|_| None), None);
    }

    #[test]
    fn a_line_is_due_on_a_new_day_or_when_it_changes() {
        let cursor = Cursor {
            schema: CURSOR_SCHEMA.to_owned(),
            day: "2026-11-23".to_owned(),
            line: "1 session today".to_owned(),
            shown_at: "2026-11-23T08:00:00+00:00".to_owned(),
        };
        // The same day, the same line: not due.
        assert!(!due(Some(&cursor), "2026-11-23", "1 session today"));
        // A new day, or a changed line: due.
        assert!(due(Some(&cursor), "2026-11-24", "1 session today"));
        assert!(due(Some(&cursor), "2026-11-23", "2 sessions today"));
        // No cursor at all: due.
        assert!(due(None, "2026-11-23", "1 session today"));
        // A cursor from another schema is no cursor: due.
        let mut other = cursor;
        other.schema = "something-else.v9".to_owned();
        // (read as no cursor by `cursor()`, which refuses it; `due` itself
        // only compares what it was given)
        assert!(due(None, "2026-11-23", "1 session today"));
    }
}
