use crate::analysis::patterns;
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
    let findings = patterns(&report.sessions);
    let mut out=String::from("<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'\"><title>Vessa · Session evidence</title><style>body{margin:0;background:#f4f1e8;color:#172c28;font:16px/1.6 system-ui,sans-serif}main{max-width:1100px;margin:auto;padding:40px 24px}h1{font-size:48px;margin:0}h2{margin-top:40px}a{color:#075c52}a:focus,summary:focus{outline:3px solid #bd5b2f}nav{display:flex;gap:24px}.note{color:#53645e}.cards{display:flex;gap:16px;flex-wrap:wrap}.card,details{background:#fffdf8;border:1px solid #d7ded5;border-radius:12px;padding:18px;margin:12px 0}.card strong{font-size:28px;display:block}table{border-collapse:collapse;width:100%}th,td{text-align:left;padding:12px;border-bottom:1px solid #d7ded5;vertical-align:top}.scroll{overflow:auto}pre{white-space:pre-wrap;overflow-wrap:anywhere;font:inherit}code{overflow-wrap:anywhere}summary{cursor:pointer;font-weight:650}.warning{border-left:4px solid #bd5b2f;padding-left:12px}article:target{background:#fff0b8}small{color:#53645e}.tag{background:#dcebe3;padding:3px 9px;border-radius:15px}footer{margin-top:40px}</style><main><header><span class=\"tag\">LOCAL · NO MODEL CALLS</span><h1>Vessa</h1><p>Where the tokens went. Where the conversation turned.</p><nav><a href=\"#usage\">Usage</a><a href=\"#patterns\">Patterns</a><a href=\"#sessions\">Conversations</a></nav></header>");
    let _=write!(out,"<div class=\"cards\"><div class=\"card\"><strong>{}</strong>sessions</div><div class=\"card\"><strong>{}</strong>pattern candidates</div><div class=\"card\"><strong>{}</strong>source warnings</div></div><p class=\"note\">Estimates are not bills. Unknown is not zero. Pattern labels are hypotheses; open their supporting messages. Use your browser’s Find to search this report.</p>",report.sessions.len(),findings.len(),report.warnings.len());
    for warning in &report.warnings {
        let _ = write!(out, "<p class=\"warning\">{}</p>", escape(warning));
    }
    out.push_str("<h2 id=\"usage\">Usage by session and model</h2><div class=\"scroll\"><table><thead><tr><th>Session</th><th>Model / provider</th><th>Input / output</th><th>Cache read / write</th><th>Estimated USD</th></tr></thead><tbody>");
    for (i, s) in report.sessions.iter().enumerate() {
        let rows = aggregate(s)?;
        if rows.is_empty() {
            let _=write!(out,"<tr><td><a href=\"#s{i}\">{} · {}</a></td><td colspan=\"4\">Usage unknown</td></tr>",escape(&s.agent),escape(&s.id));
        }
        for e in rows {
            let _=write!(out,"<tr><td><a href=\"#s{i}\">{} · {}</a></td><td>{}<br><small>{}</small></td><td>{} / {}</td><td>{} / {}</td><td>{}<br><small>{}</small></td></tr>",escape(&s.agent),escape(&s.id),escape(&e.model),escape(&e.provider),number(e.input_tokens),number(e.output_tokens),number(e.cache_read_tokens),number(e.cache_write_tokens),dollars(e.estimated_usd_micros),escape(e.estimate_basis.as_deref().unwrap_or("No rate evidence")));
        }
    }
    out.push_str("</tbody></table></div><h2 id=\"patterns\">Conversation patterns</h2>");
    if findings.is_empty() {
        out.push_str("<p>No matches for the current rules. This is not proof that conversations had no friction.</p>");
    }
    for f in findings {
        let _ = write!(
            out,
            "<div class=\"card\"><strong>{}</strong><p>{}</p>",
            escape(&f.kind),
            escape(&f.method)
        );
        for e in f.evidence {
            if let Some((si, s)) = report
                .sessions
                .iter()
                .enumerate()
                .find(|(_, s)| s.agent == e.agent && s.id == e.session)
            {
                if let Some((mi, m)) = s
                    .messages
                    .iter()
                    .enumerate()
                    .find(|(_, m)| m.id == e.message)
                {
                    let excerpt: String = m.text.chars().take(220).collect();
                    let _ = write!(
                        out,
                        "<p><a href=\"#m{si}-{mi}\">{} · line {}</a><br>{}</p>",
                        escape(&s.agent),
                        e.line,
                        escape(&excerpt)
                    );
                }
            }
        }
        out.push_str("</div>");
    }
    out.push_str("<h2 id=\"sessions\">Conversations and evidence</h2>");
    for (si, s) in report.sessions.iter().enumerate() {
        let _ = write!(
            out,
            "<section id=\"s{si}\"><h3>{} · {}</h3><p><code>{}</code></p>",
            escape(&s.agent),
            escape(&s.id),
            escape(&s.cwd)
        );
        for limitation in &s.limitations {
            let _ = write!(out, "<p class=\"warning\">{}</p>", escape(limitation));
        }
        if !s.ohjaa.is_empty() {
            out.push_str(
                "<details><summary>Explicitly linked ohjaa session records</summary><pre>",
            );
            out.push_str(&escape(&serde_json::to_string_pretty(&s.ohjaa)?));
            out.push_str("</pre></details>");
        }
        out.push_str("<details><summary>Usage source records</summary><pre>");
        out.push_str(&escape(&serde_json::to_string_pretty(&s.usage)?));
        out.push_str("</pre></details>");
        // Message anchors remain visible so browser fragment links work without JS.
        for (mi, m) in s.messages.iter().enumerate() {
            let _=write!(out,"<article id=\"m{si}-{mi}\"><details><summary>{} · {} · line {}</summary><small>{}</small><pre>{}</pre></details></article>",escape(&m.role),escape(m.at.as_deref().unwrap_or("time unknown")),m.line,escape(&m.source),escape(&m.text));
        }
        out.push_str("</section>");
    }
    out.push_str("<footer>Vessa · source adapters v1 · phrase-rules.v1 · Original conversations stay unchanged. No knowledge was made current.</footer></main></html>");
    Ok(out)
}
