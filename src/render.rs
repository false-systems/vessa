use crate::model::{aggregate, Entry, Report};
use anyhow::Result;
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
pub fn html(report: &Report) -> Result<String> {
    let mut out = String::from(
        r##"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'"><title>Vessa · Usage and spending</title><style>body{margin:0;background:#f4f1e8;color:#172c28;font:16px/1.6 system-ui,sans-serif}main{max-width:1100px;margin:auto;padding:40px 24px}h1{font-size:48px;margin:0}a{color:#075c52}a:focus,summary:focus{outline:3px solid #bd5b2f}details{background:#fffdf8;border:1px solid #d7ded5;border-radius:12px;padding:18px;margin:12px 0}table{border-collapse:collapse;width:100%}th,td{text-align:left;padding:12px;border-bottom:1px solid #d7ded5;vertical-align:top}.scroll{overflow:auto}pre,code{white-space:pre-wrap;overflow-wrap:anywhere}summary{cursor:pointer;font-weight:650}.warning{border-left:4px solid #bd5b2f;padding-left:12px}small{color:#53645e}footer{margin-top:40px}</style><main><header><small>LOCAL · USAGE EVIDENCE</small><h1>Vessa</h1><p>What your agent sessions consumed.</p><nav><a href="#usage">Usage</a> · <a href="#sources">Sources</a></nav></header>"##,
    );
    let _ = write!(out, "<p>{} sessions · {} source warnings</p><p>Estimates are not bills. Unknown is not zero. Input excludes cached tokens.</p>", report.sessions.len(), report.warnings.len());
    for warning in &report.warnings {
        let _ = write!(out, "<p class=\"warning\">{}</p>", escape(warning));
    }
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
        let _ = write!(out, "<details><summary>{label} statistics</summary><div class=\"scroll\"><table><thead><tr><th>Period / agent</th><th>Sessions</th><th>Known estimate subtotal</th><th>Unpriced records</th></tr></thead><tbody>");
        for b in crate::metrics::history(report, period)? {
            let _ = write!(
                out,
                "<tr><td>{} / {}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                escape(&b.period),
                escape(&b.agent),
                b.sessions,
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
        let _ = write!(
            out,
            "<h4>Commands, changes and estimated waste</h4><pre>{}</pre><details><summary>How these estimates work</summary><p>{}</p></details>",
            escape(&crate::work::text(&insights)), escape(insights.basis)
        );
        if !s.ohjaa.is_empty() {
            out.push_str(
                "<details><summary>Explicitly linked ohjaa session records</summary><pre>",
            );
            out.push_str(&escape(&serde_json::to_string_pretty(&s.ohjaa)?));
            out.push_str("</pre></details>");
        }
        out.push_str("<details><summary>Usage source records</summary><pre>");
        out.push_str(&escape(&serde_json::to_string_pretty(&s.usage)?));
        out.push_str("</pre></details></section>");
    }
    out.push_str("<footer>Vessa · source adapters v1 · Conversation analysis belongs to Seula.</footer></main></html>");
    Ok(out)
}
