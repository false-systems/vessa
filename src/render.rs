use crate::model::{aggregate, Entry, Report};
use anyhow::{Context, Result};
use std::fmt::Write;

pub fn number(n: Option<u64>) -> String {
    n.map_or_else(|| "unknown".into(), |n| n.to_string())
}
pub fn dollars(n: Option<u64>) -> String {
    n.map_or_else(
        || "unknown".into(),
        |n| format!("${}.{:06}", n / 1_000_000, n % 1_000_000),
    )
}
pub fn usage_line(e: &Entry) -> String {
    format!(
        "{}/{} in={} out={} cache-read={} cache-write={} estimated={} billed={} credits={}",
        e.provider,
        e.model,
        number(e.input_tokens),
        number(e.output_tokens),
        number(e.cache_read_tokens),
        number(e.cache_write_tokens),
        dollars(e.estimated_usd_micros),
        e.billed.as_ref().map_or("unknown".into(), |a| format!(
            "{} {} micros",
            a.unit, a.micros
        )),
        e.credits.as_ref().map_or("unknown".into(), |a| format!(
            "{} {} micros",
            a.unit, a.micros
        ))
    )
}
pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn spending<'a>(mut entries: impl Iterator<Item = &'a Entry>) -> Result<(u64, usize)> {
    entries.try_fold((0u64, 0), |(known, unpriced), e| {
        Ok((
            known
                .checked_add(e.estimated_usd_micros.unwrap_or(0))
                .context("estimate overflow")?,
            unpriced + usize::from(e.estimated_usd_micros.is_none()),
        ))
    })
}

fn entry_table(out: &mut String, entries: &[Entry]) {
    if entries.is_empty() {
        out.push_str("<p>Usage unknown: no records in this window.</p>");
        return;
    }
    out.push_str("<div class=\"scroll\"><table><thead><tr><th scope=\"col\">Provider / model</th><th scope=\"col\">Uncached input</th><th scope=\"col\">Cached input read</th><th scope=\"col\">Cache write</th><th scope=\"col\">Output</th><th scope=\"col\">Estimate</th></tr></thead><tbody>");
    for e in entries {
        let _ = write!(out, "<tr><th scope=\"row\">{} / {}</th><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>", escape(&e.provider), escape(&e.model), number(e.input_tokens), number(e.cache_read_tokens), number(e.cache_write_tokens), number(e.output_tokens), dollars(e.estimated_usd_micros));
    }
    out.push_str("</tbody></table></div>");
}

fn overview(out: &mut String, report: &Report) -> Result<()> {
    let (known, unpriced) = spending(
        report
            .sessions
            .iter()
            .flat_map(|s| s.usage.iter().map(|u| &u.entry)),
    )?;
    let records: usize = report.sessions.iter().map(|s| s.usage.len()).sum();
    let _ = write!(out, "<section aria-labelledby=\"overview\"><h2 id=\"overview\">Where the spending goes</h2><p><strong>{}</strong> known estimate subtotal · <strong>{}/{records}</strong> usage records priced · <strong>{unpriced}</strong> unpriced.</p><p>This is not a bill or a complete total when records are unpriced. No usage records means unknown consumption.</p>", dollars(Some(known)), records - unpriced);
    let mut ranked = report
        .sessions
        .iter()
        .enumerate()
        .map(|(i, s)| {
            spending(s.usage.iter().map(|u| &u.entry))
                .map(|(known, unpriced)| (i, s, known, unpriced))
        })
        .collect::<Result<Vec<_>>>()?;
    ranked.sort_by_key(|(_, _, known, _)| std::cmp::Reverse(*known));
    out.push_str("<h3>Sessions by known estimate subtotal</h3><p>Unpriced sessions can cost more than their position suggests.</p><div class=\"scroll\"><table><thead><tr><th scope=\"col\">Session</th><th scope=\"col\">Known estimate subtotal</th><th scope=\"col\">Unpriced records</th><th scope=\"col\">Linked wrapper records</th></tr></thead><tbody>");
    for (i, s, known, unpriced) in ranked {
        let _ = write!(out, "<tr><th scope=\"row\"><a href=\"#s{i}\">{} · {}</a></th><td>{}</td><td>{unpriced}</td><td>{}</td></tr>", escape(&s.agent), escape(&s.id), dollars(Some(known)), s.ohjaa.len());
    }
    out.push_str("</tbody></table></div><h3>What the token categories mean</h3><p>Uncached input is new input processing; cached input read is reused input processing, not free tokens. Cache writes and output have separate rates. Output already includes reasoning where the source counts it. These categories do not identify which file or instruction caused the usage.</p></section>");
    out.push_str("<section aria-labelledby=\"reduce\"><h2 id=\"reduce\">Reduce repeated work</h2><ol><li>Start each bounded task in its repository through one launching wrapper that hands the agent what is already known and the previous handover. Do not nest a second wrapper.</li><li>Use the working set and read order it delivers instead of scanning the repository again. Read additional files only for a concrete missing dependency; refresh when Git facts become stale.</li><li>Use recorded decisions and the handover instead of rediscovering accepted choices. Record remaining work at completion, not after every reply. Do not trim safety instructions or skip checks to save tokens.</li><li>Attach a Vessa receipt (<code>vessa receipt</code>) to the explicit wrapper session and include the wrapper's session export in this report. Compare similar tasks with the same model, rates and validation: uncached input, output, cache usage and completion evidence, not dollars alone.</li></ol><p><strong>Savings are not measured here.</strong> A linked wrapper record proves association, not that its briefing was used or that it saved tokens. A before/after comparison needs comparable task and outcome evidence; launch overhead alone is not net savings.</p></section>");
    Ok(())
}
pub fn html(report: &Report) -> Result<String> {
    let mut out = String::from(
        r##"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'"><title>Vessa · Usage and spending</title><style>body{margin:0;background:#f4f1e8;color:#172c28;font:16px/1.6 system-ui,sans-serif}main{max-width:1100px;margin:auto;padding:40px 24px}h1{font-size:48px;margin:0}a{color:#075c52}a:focus,summary:focus{outline:3px solid #bd5b2f}details{background:#fffdf8;border:1px solid #d7ded5;border-radius:12px;padding:18px;margin:12px 0}table{border-collapse:collapse;width:100%}th,td{text-align:left;padding:12px;border-bottom:1px solid #d7ded5;vertical-align:top}.scroll{overflow:auto}pre,code{white-space:pre-wrap;overflow-wrap:anywhere}summary{cursor:pointer;font-weight:650}.warning{border-left:4px solid #bd5b2f;padding-left:12px}small{color:#53645e}footer{margin-top:40px}</style><main><header><small>LOCAL · USAGE EVIDENCE</small><h1>Vessa</h1><p>What your agent sessions consumed.</p><nav><a href="#usage">Usage</a> · <a href="#sources">Sources</a></nav></header>"##,
    );
    let _ = write!(out, "<p>{} sessions · {} source warnings</p><p>Estimates are not bills. Unknown is not zero. Input excludes cached tokens.</p>", report.sessions.len(), report.warnings.len());
    for warning in &report.warnings {
        let _ = write!(out, "<p class=\"warning\">{}</p>", escape(warning));
    }
    overview(&mut out, report)?;
    out.push_str("<h2 id=\"usage\">Usage by session and model</h2><div class=\"scroll\"><table><thead><tr><th>Session</th><th>Usage and spending</th></tr></thead><tbody>");
    for (i, s) in report.sessions.iter().enumerate() {
        let entries = aggregate(s)?;
        let _ = write!(
            out,
            "<tr><td><a href=\"#s{i}\">{} · {}</a></td><td>",
            escape(&s.agent),
            escape(&s.id)
        );
        if entries.is_empty() {
            out.push_str("Usage unknown");
        }
        for e in entries {
            let _ = write!(
                out,
                "<p>{}<br><small>{}</small></p>",
                escape(&usage_line(&e)),
                escape(e.estimate_basis.as_deref().unwrap_or("No rate evidence"))
            );
        }
        out.push_str("</td></tr>");
    }
    out.push_str("</tbody></table></div><h2>Usage over time</h2><p>UTC calendar periods. Known estimates are subtotals; unpriced records are not free. Sessions spanning periods appear in each applicable period.</p>");
    for (label, period) in [
        ("Daily", crate::metrics::Period::Day),
        ("Weekly", crate::metrics::Period::Week),
        ("Monthly", crate::metrics::Period::Month),
        ("Yearly", crate::metrics::Period::Year),
    ] {
        let _ = write!(out, "<details><summary>{label} statistics</summary><div class=\"scroll\"><table><thead><tr><th>Period / agent</th><th>Sessions</th><th>Subagents</th><th>Known estimate subtotal</th><th>Unpriced records</th></tr></thead><tbody>");
        for b in crate::metrics::history(report, period)? {
            let _ = write!(
                out,
                "<tr><td>{} / {}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                escape(&b.period),
                escape(&b.agent),
                b.sessions,
                b.subagents,
                dollars(Some(b.known_estimated_usd_micros)),
                b.unpriced_records
            );
        }
        out.push_str("</tbody></table></div></details>");
    }
    out.push_str("<h2 id=\"sources\">Sources and coverage</h2>");
    for (i, s) in report.sessions.iter().enumerate() {
        let _ = write!(
            out,
            "<section id=\"s{i}\"><h3>{} · {}</h3><p><code>{}</code></p>",
            escape(&s.agent),
            escape(&s.id),
            escape(&s.source)
        );
        for limitation in &s.limitations {
            let _ = write!(out, "<p class=\"warning\">{}</p>", escape(limitation));
        }
        let insights = crate::work::summarize(s)?;
        out.push_str("<h4>Command breakdown</h4><p>Recorded user-message windows, not named tasks. Each window is already included in session totals. Open a command to see token categories and its source location.</p>");
        for (command, window) in insights.commands.iter().enumerate() {
            let _ = write!(out, "<details><summary>Command {} · {} tool calls · {} recorded failures · {} successful edit results</summary><p>Source: <code>{}</code>, line {} · {} usage records</p>", command + 1, window.tools, window.failed_tools, window.changes.len(), escape(&window.source), window.line, window.records);
            entry_table(&mut out, &window.entries);
            out.push_str("</details>");
        }
        if insights.commands.is_empty() {
            out.push_str("<p>No command boundaries recorded; command attribution is unknown.</p>");
        }
        let _ = write!(
            out,
            "<details><summary>All command, change and waste evidence</summary><pre>{}</pre></details><details><summary>How these estimates work</summary><p>{}</p></details>",
            escape(&crate::work::text(&insights)), escape(insights.basis)
        );
        if !s.ohjaa.is_empty() {
            out.push_str(
                "<details><summary>Explicitly linked wrapper session records</summary><pre>",
            );
            out.push_str(&escape(&serde_json::to_string_pretty(&s.ohjaa)?));
            out.push_str("</pre></details>");
        }
        out.push_str("<details><summary>Usage source records</summary><pre>");
        out.push_str(&escape(&serde_json::to_string_pretty(&s.usage)?));
        out.push_str("</pre></details></section>");
    }
    out.push_str("<footer>Vessa · source adapters v1 · Reports carry no message text.</footer></main></html>");
    Ok(out)
}
