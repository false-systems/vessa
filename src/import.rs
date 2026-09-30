use crate::model::{Entry, Report, Session, Usage};
use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub fn read(paths: &[PathBuf]) -> Result<Report> {
    // ponytail: selected transcripts are held in memory; stream compact
    // summaries if archive size makes this one-shot reader too expensive.
    let mut report = Report {
        schema: "vessa-usage.v1".into(),
        ..Default::default()
    };
    let mut files = Vec::new();
    for path in paths {
        if let Err(error) = discover(path, &mut files) {
            report.warnings.push(format!("{}: {error}", path.display()));
        }
    }
    files.sort();
    files.dedup();
    let mut wrappers = Vec::new();
    for file in files {
        match parse_file(&file) {
            Ok(Parsed::Session(session)) => report.sessions.push(session),
            Ok(Parsed::Wrappers(records, warnings)) => {
                wrappers.extend(records);
                report.warnings.extend(warnings);
            }
            Err(error) => report
                .warnings
                .push(format!("{}: {error:#}", file.display())),
        }
    }
    // Native IDs survive resume. Merge transcript copies, then deduplicate real
    // request/message identities; never use equal text to deduplicate spending.
    let mut merged: BTreeMap<(String, String), Session> = BTreeMap::new();
    for session in report.sessions {
        let key = (session.agent.clone(), session.id.clone());
        if let Some(old) = merged.get_mut(&key) {
            old.usage.extend(session.usage);
            old.limitations.extend(session.limitations);
        } else {
            merged.insert(key, session);
        }
    }
    report.sessions = merged.into_values().collect();
    // Process parents before descendants, so copied history stays with its
    // original session when both sources are available.
    report.sessions.sort_by_key(|s| s.parent.is_some());
    // ponytail: lineage lookup scans the selected sessions; index IDs and
    // source paths if large archives make that scan measurable.
    let pi_lineages: HashMap<_, _> = report
        .sessions
        .iter()
        .filter(|s| s.agent == "pi")
        .map(|s| {
            let mut current = s;
            let mut visited = std::collections::HashSet::new();
            while visited.insert(current.id.as_str()) {
                let Some(parent) = &current.parent else {
                    break;
                };
                let Some(found) = report.sessions.iter().find(|other| {
                    other.agent == "pi" && (&other.id == parent || &other.source == parent)
                }) else {
                    break;
                };
                current = found;
            }
            (s.id.clone(), current.id.clone())
        })
        .collect();
    let mut seen: HashMap<(String, String), (Entry, String)> = HashMap::new();
    for session in &mut report.sessions {
        let mut usage = Vec::new();
        for item in session.usage.drain(..) {
            let started = session
                .started_at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
            let at = item
                .at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
            if session.parent.is_some() && started.zip(at).is_some_and(|(start, at)| at < start) {
                // Copied context is not new consumption, even when the original
                // session file is unavailable or a grandchild sorts first.
                continue;
            }
            let event_id = if session.agent == "pi" {
                format!("{}:{}", pi_lineages[&session.id], item.id)
            } else {
                item.id.clone()
            };
            let key = (session.agent.clone(), event_id);
            if let Some((old, owner)) = seen.get(&key) {
                if old != &item.entry {
                    session.limitations.push(format!(
                        "conflicting usage {} (first seen in {owner}); first record retained",
                        item.id
                    ));
                }
            } else {
                seen.insert(key, (item.entry.clone(), session.id.clone()));
                usage.push(item);
            }
        }
        session.usage = usage;
        for wrapper in &wrappers {
            if wrapper["usage"].as_array().is_some_and(|receipts| {
                receipts
                    .iter()
                    .any(|r| r["agent"] == session.agent && r["session_id"] == session.id)
            }) {
                session.ohjaa.push(wrapper.clone());
            }
        }
        if session.usage.is_empty() {
            session
                .limitations
                .push("No usage records found; consumption is unknown, not zero".into());
        }
        session.limitations.sort();
        session.limitations.dedup();
    }
    for wrapper in wrappers {
        if !report
            .sessions
            .iter()
            .any(|s| s.ohjaa.iter().any(|w| w["id"] == wrapper["id"]))
        {
            report.warnings.push(format!(
                "ohjaa session {} has no explicit matching native transcript",
                wrapper["id"].as_str().unwrap_or("unknown")
            ));
        }
    }
    if report.sessions.is_empty() {
        report
            .warnings
            .push("No supported native sessions found; usage is unknown".into());
    }
    Ok(report)
}

fn discover(path: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        bail!("symlink not followed; provide its real path");
    }
    if meta.is_dir() {
        for item in std::fs::read_dir(path)? {
            discover(&item?.path(), out)?;
        }
    } else if meta.is_file()
        && matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("jsonl" | "json")
        )
    {
        out.push(path.canonicalize()?);
    }
    Ok(())
}

enum Parsed {
    Session(Session),
    Wrappers(Vec<Value>, Vec<String>),
}

fn parse_file(path: &Path) -> Result<Parsed> {
    let source = path.display().to_string();
    if path.extension().is_some_and(|s| s == "json") {
        let value: Value = serde_json::from_reader(std::fs::File::open(path)?)?;
        ensure!(
            value["schema"] == "ohjaa-sessions.v1",
            "unsupported JSON export (expected ohjaa-sessions.v1)"
        );
        let records = value["sessions"]
            .as_array()
            .context("missing sessions")?
            .clone();
        let warnings = value["unreadable"]
            .as_array()
            .context("missing unreadable coverage")?
            .iter()
            .map(|id| format!("ohjaa could not read session {id}"))
            .collect();
        return Ok(Parsed::Wrappers(records, warnings));
    }
    let file = BufReader::new(std::fs::File::open(path)?);
    let mut records = Vec::new();
    let mut limitations = Vec::new();
    for (index, line) in file.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(&line) {
            Ok(value) => records.push((index + 1, value)),
            Err(error) => limitations.push(format!("line {}: malformed JSON: {error}", index + 1)),
        }
    }
    let agent = if records.iter().any(|(_, v)| v["type"] == "session_meta") {
        "codex"
    } else if records
        .iter()
        .any(|(_, v)| v["type"] == "session" && v["id"].is_string())
    {
        "pi"
    } else if records.iter().any(|(_, v)| v["sessionId"].is_string()) {
        "claude"
    } else {
        bail!("unsupported or missing native session header");
    };
    let header = records
        .iter()
        .find(|(_, v)| match agent {
            "codex" => v["type"] == "session_meta",
            "pi" => v["type"] == "session",
            _ => v["sessionId"].is_string(),
        })
        .unwrap();
    let meta = if agent == "codex" {
        &header.1["payload"]
    } else {
        &header.1
    };
    let mut id = string(meta, if agent == "claude" { "sessionId" } else { "id" });
    ensure!(!id.is_empty(), "missing session identity");
    let mut parent = meta["parentSession"]
        .as_str()
        .or_else(|| meta["forked_from_id"].as_str())
        .map(str::to_owned);
    if agent == "claude" && records.iter().any(|(_, v)| v["isSidechain"] == true) {
        parent = Some(id.clone());
        let child = records
            .iter()
            .find_map(|(_, v)| v["agentId"].as_str())
            .unwrap_or_else(|| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
            });
        id = format!("{id}/{child}");
    }
    if agent == "codex" && !meta["source"]["subagent"].is_null() && parent.is_none() {
        parent = Some("parent identity unavailable (subagent source)".into());
    }
    let mut session = Session {
        agent: agent.into(),
        id,
        parent,
        started_at: timestamp(meta).or_else(|| timestamp(&header.1)),
        cwd: string(meta, "cwd"),
        source: source.clone(),
        usage: Vec::new(),
        limitations,
        ohjaa: Vec::new(),
    };
    let mut model = "unknown".to_owned();
    let mut provider = "unknown".to_owned();
    let mut previous: Option<[u64; 3]> = None;
    let mut pi_model_changed = false;
    for (line, value) in records {
        let result = match agent {
            "claude" => claude(&value, line, &mut session),
            "codex" => codex(
                &value,
                line,
                &mut session,
                &mut model,
                &mut provider,
                &mut previous,
            ),
            _ => pi(
                &value,
                line,
                &mut session,
                &mut model,
                &mut provider,
                &mut pi_model_changed,
            ),
        };
        if let Err(error) = result {
            session.limitations.push(format!("line {line}: {error:#}"));
        }
    }
    Ok(Parsed::Session(session))
}

fn string(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}
fn timestamp(v: &Value) -> Option<String> {
    if let Some(text) = v["timestamp"].as_str() {
        Some(text.to_owned())
    } else {
        v["timestamp"]
            .as_i64()
            .and_then(chrono::DateTime::from_timestamp_millis)
            .map(|t| t.to_rfc3339())
    }
}
fn count(v: &Value, key: &str) -> Result<Option<u64>> {
    if v[key].is_null() {
        return Ok(None);
    }
    Ok(Some(v[key].as_u64().with_context(|| {
        format!("invalid nonnegative integer {key}")
    })?))
}
fn base(provider: String, model: String) -> Entry {
    Entry {
        provider,
        model,
        input_tokens: None,
        output_tokens: None,
        cache_read_tokens: None,
        cache_write_tokens: None,
        estimated_usd_micros: None,
        estimate_basis: None,
        credits: None,
        billed: None,
    }
}
fn add(s: &mut Session, v: &Value, line: usize, id: String, entry: Entry) {
    s.usage.push(Usage {
        id,
        entry,
        at: timestamp(v),
        source: s.source.clone(),
        line,
    });
}
fn fallback(s: &Session, line: usize) -> String {
    format!("{}:{}:{line}", s.id, s.source)
}

fn claude(v: &Value, line: usize, s: &mut Session) -> Result<()> {
    if v["sessionId"].is_string() {
        ensure!(
            v["sessionId"] == s.id || s.parent.as_ref().is_some_and(|p| v["sessionId"] == *p),
            "mixed native session IDs"
        );
    }
    let m = &v["message"];
    if v["type"] == "assistant" && m["usage"].is_object() {
        let usage = &m["usage"];
        let mut entry = base(
            v["provider"].as_str().unwrap_or("unknown").into(),
            m["model"].as_str().unwrap_or("unknown").into(),
        );
        entry.input_tokens = count(usage, "input_tokens")?;
        entry.output_tokens = count(usage, "output_tokens")?;
        entry.cache_read_tokens = count(usage, "cache_read_input_tokens")?;
        entry.cache_write_tokens = count(usage, "cache_creation_input_tokens")?;
        let id = v["requestId"]
            .as_str()
            .or_else(|| m["id"].as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| fallback(s, line));
        // Claude may emit multiple content blocks for one request. Equal usage
        // deduplicates; conflicting snapshots are reported instead of summed.
        add(s, v, line, id, entry);
    }
    Ok(())
}

fn codex(
    v: &Value,
    line: usize,
    s: &mut Session,
    model: &mut String,
    provider: &mut String,
    previous: &mut Option<[u64; 3]>,
) -> Result<()> {
    let p = &v["payload"];
    if matches!(v["type"].as_str(), Some("session_meta" | "turn_context")) {
        if let Some(m) = p["model"].as_str() {
            *model = m.into();
        }
        if let Some(p) = p["model_provider"].as_str() {
            *provider = p.into();
        }
    }
    if v["type"] == "event_msg" && p["type"] == "token_count" && p["info"].is_object() {
        let total = &p["info"]["total_token_usage"];
        let values = [
            count(total, "input_tokens")?,
            count(total, "output_tokens")?,
            count(total, "cached_input_tokens")?,
        ];
        let [Some(input), Some(output), Some(cached)] = values else {
            bail!("missing cumulative token categories; cannot safely derive request usage");
        };
        ensure!(
            cached <= input,
            "cached tokens exceed inclusive input tokens"
        );
        let current = [input, output, cached];
        let mut prior = previous.unwrap_or([0; 3]);
        if previous.is_none() && s.parent.is_some() {
            let last = &p["info"]["last_token_usage"];
            let last = [
                count(last, "input_tokens")?,
                count(last, "output_tokens")?,
                count(last, "cached_input_tokens")?,
            ];
            *previous = Some(current);
            let [Some(li), Some(lo), Some(lc)] = last else {
                bail!("first child counter has no last-request breakdown; inherited usage cannot be attributed");
            };
            for (i, last) in [li, lo, lc].into_iter().enumerate() {
                prior[i] = current[i]
                    .checked_sub(last)
                    .context("last-request tokens exceed cumulative total")?;
            }
        }
        *previous = Some(current);
        let delta: Vec<_> = current
            .iter()
            .zip(prior)
            .map(|(a, b)| {
                a.checked_sub(b)
                    .context("cumulative counters reset; baseline re-established")
            })
            .collect::<Result<_>>()?;
        if delta.iter().all(|n| *n == 0) {
            return Ok(());
        }
        ensure!(delta[2] <= delta[0], "cache delta exceeds input delta");
        let mut entry = base(provider.clone(), model.clone());
        entry.input_tokens = Some(delta[0] - delta[2]);
        entry.output_tokens = Some(delta[1]);
        entry.cache_read_tokens = Some(delta[2]);
        entry.cache_write_tokens = Some(0);
        add(
            s,
            v,
            line,
            format!("{}:total:{input}:{output}:{cached}", s.id),
            entry,
        );
    }
    Ok(())
}

fn pi(
    v: &Value,
    line: usize,
    s: &mut Session,
    model: &mut String,
    provider: &mut String,
    changed: &mut bool,
) -> Result<()> {
    if v["type"] == "session" {
        ensure!(
            v["version"].as_u64().is_some_and(|n| (1..=3).contains(&n)),
            "unsupported pi session version"
        );
    }
    if v["type"] == "model_change" {
        *model = string(v, "modelId");
        *provider = string(v, "provider");
        *changed = true;
    }
    let m = if v["type"] == "message" {
        &v["message"]
    } else {
        v
    };
    let id = v["id"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| fallback(s, line));
    if m["usage"].is_object() {
        let u = &m["usage"];
        let mut entry = base(
            m["provider"]
                .as_str()
                .unwrap_or(if *changed { provider } else { "unknown" })
                .into(),
            m["model"]
                .as_str()
                .unwrap_or(if *changed { model } else { "unknown" })
                .into(),
        );
        entry.input_tokens = count(u, "input")?;
        entry.output_tokens = count(u, "output")?;
        entry.cache_read_tokens = count(u, "cacheRead")?;
        entry.cache_write_tokens = count(u, "cacheWrite")?;
        if let Some(cost) = u["cost"]["total"].as_f64() {
            ensure!(
                cost.is_finite() && cost >= 0.0 && cost * 1_000_000.0 < u64::MAX as f64,
                "invalid cost"
            );
            entry.estimated_usd_micros = Some((cost * 1_000_000.0).round() as u64);
            entry.estimate_basis = Some("pi recorded model rates; not provider billing".into());
        }
        add(s, v, line, id, entry);
    }
    Ok(())
}
