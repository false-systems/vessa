//! Secret-looking tokens in what Vessa prints of conversations, redacted.
//!
//! What people paste to agents, and what tools print back, includes
//! credentials, and Vessa is the one reader of agent conversations: what it
//! prints is what every agent that reads them through it sees. So a token
//! that looks like a secret becomes `[redacted]` before it is printed. The
//! original is never printed, logged or kept; only how many were redacted,
//! by rule, is said. `vessa messages --raw` alone prints the words as
//! written.
//!
//! The rules are ohjaa intake's, which scrubs the prompt it sends to its
//! drafter (false-systems/ohjaa, `src/intake.rs`, the `// ---- secrets ----`
//! section, main at 9cad209, 2026-10-28). They are copied, not shared,
//! since repositories stand alone: known key prefixes, JWTs, PEM private
//! key blocks, an id and a secret joined by a dot, a long token mixing
//! capitals, lower case and digits, and a long mixed token right after a
//! word that names a secret. Vessa names each rule, judges a text as a
//! terminal shows it, and adds two exceptions that are no secret: AWS's
//! documented example keys, and signed evidence exports that hold none,
//! which are evidence and would be destroyed.

use crate::model::{Message, Report};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const REDACTED: &str = "[redacted]";

/// The access key pair AWS's documentation uses in its examples ("Managing
/// access keys for IAM users"), and code and tests copy: no secret.
const AWS_EXAMPLES: [&str; 2] = [
    "AKIAIOSFODNN7EXAMPLE",
    "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
];

/// How many tokens a scrub redacted, by rule.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Redactions(BTreeMap<&'static str, usize>);

impl Redactions {
    pub fn total(&self) -> usize {
        self.0.values().sum()
    }

    pub fn by_rule(&self) -> &BTreeMap<&'static str, usize> {
        &self.0
    }

    /// `text` with every secret-looking token replaced by `[redacted]`.
    pub fn text(&mut self, text: &str) -> String {
        let (out, marks) = marked(text);
        for (_, rule) in marks {
            self.add(rule);
        }
        out
    }

    /// Each of `lines`, scrubbed in place.
    pub fn lines<'a>(&mut self, lines: impl IntoIterator<Item = &'a mut String>) {
        for line in lines {
            *line = self.text(line);
        }
    }

    /// Every string in `value`, at any depth, scrubbed in place. Its keys
    /// are names, and kept.
    pub fn value(&mut self, value: &mut Value) {
        match value {
            Value::String(text) => *text = self.text(text),
            Value::Array(items) => {
                for item in items {
                    self.value(item);
                }
            }
            Value::Object(fields) => {
                for field in fields.values_mut() {
                    self.value(field);
                }
            }
            _ => {}
        }
    }

    /// `m` as it is printed: its words, the question it answers and the
    /// reply it follows scrubbed. Where it was said is not words, and kept.
    pub fn message(&mut self, m: &Message) -> Message {
        let mut m = m.clone();
        m.text = self.text(&m.text);
        if let Some(question) = &mut m.question {
            *question = self.text(question);
        }
        if let Some(reply) = &mut m.reply {
            // A reply was scrubbed whole before it was cut (see [`tail`]):
            // what that redacted in the part kept is counted here, where it
            // is printed, each time it is.
            reply.text = self.text(&reply.text);
            for (rule, n) in &reply.redacted.0 {
                *self.0.entry(rule).or_default() += n;
            }
        }
        m
    }

    /// What is said on stderr when any token was redacted: how many, and
    /// by which rule.
    pub fn said(&self) -> Option<String> {
        let rules = self
            .0
            .iter()
            .map(|(rule, n)| format!("{rule} {n}"))
            .collect::<Vec<_>>()
            .join(", ");
        (self.total() > 0).then(|| {
            format!(
                "{} redacted ({rules})",
                crate::render::many(self.total(), "secret-looking token")
            )
        })
    }

    fn add(&mut self, rule: &'static str) {
        *self.0.entry(rule).or_default() += 1;
    }
}

/// The end of a text, as a message carries the reply it follows.
#[derive(Debug, Default)]
pub struct Tail {
    pub text: String,
    /// The text was longer.
    pub truncated: bool,
    /// What was redacted in the part kept.
    pub redacted: Redactions,
}

/// The last `chars` characters of `text`, as a message carries the reply
/// it follows. The text is scrubbed whole before it is cut, so a cut never
/// leaves a piece of a key no rule knows, or a secret without the word that
/// named it. The part kept never starts inside a `[redacted]` or inside a
/// token: a cut through a word starts at the next one.
pub fn tail(text: &str, chars: usize) -> Tail {
    let (scrubbed, marks) = marked(text);
    let count = scrubbed.chars().count();
    let mut start = scrubbed
        .char_indices()
        .nth(count.saturating_sub(chars))
        .map_or(scrubbed.len(), |(i, _)| i);
    if let Some((at, _)) = marks
        .iter()
        .find(|(at, _)| *at < start && start < at + REDACTED.len())
    {
        start = at + REDACTED.len();
    }
    let bytes = scrubbed.as_bytes();
    if start > 0 && start < bytes.len() && token_byte(bytes[start - 1]) && token_byte(bytes[start])
    {
        while start < bytes.len() && token_byte(bytes[start]) {
            start += 1;
        }
    }
    let mut redacted = Redactions::default();
    for (_, rule) in marks.iter().filter(|(at, _)| *at >= start) {
        redacted.add(rule);
    }
    Tail {
        truncated: start > 0,
        text: scrubbed[start..].to_owned(),
        redacted,
    }
}

/// What a usage command prints of transcripts' and a wrapper's words,
/// scrubbed in place: the report's warnings, each session's limitations,
/// the wrapper session records linked to it, which hold what a session was
/// asked and handed over, and the names its usage records give: the
/// provider and model a transcript names.
pub fn report(report: &mut Report) -> Redactions {
    let mut found = Redactions::default();
    found.lines(&mut report.warnings);
    for session in &mut report.sessions {
        found.lines(&mut session.limitations);
        for record in &mut session.ohjaa {
            found.value(record);
        }
        for usage in &mut session.usage {
            let entry = &mut usage.entry;
            found.lines([&mut entry.provider, &mut entry.model]);
        }
    }
    found
}

/// `text` with each secret-looking token replaced by `[redacted]`, and
/// where each marker stands in what is returned, with its rule. The rules
/// judge the text as a terminal shows it: without the control and
/// bidirectional formatting characters [`crate::render::for_terminal`]
/// drops, line breaks aside. So a key split by one is judged whole, and
/// what hides inside it is replaced with it.
fn marked(text: &str) -> (String, Vec<(usize, &'static str)>) {
    let hidden = |c: char| c != '\n' && !crate::render::shown(c);
    // Where each byte of what is shown stands in `text`, when anything is
    // hidden.
    let (shown, at): (std::borrow::Cow<str>, Option<Vec<usize>>) = if text.chars().any(hidden) {
        let mut shown = String::with_capacity(text.len());
        let mut at = Vec::with_capacity(text.len());
        for (i, c) in text.char_indices().filter(|(_, c)| !hidden(*c)) {
            shown.push(c);
            at.extend(i..i + c.len_utf8());
        }
        (shown.into(), Some(at))
    } else {
        (text.into(), None)
    };
    let original = |offset: usize| at.as_ref().map_or(offset, |at| at[offset]);
    let mut out = String::with_capacity(text.len());
    let mut marks = Vec::new();
    let mut copied = 0;
    for (start, end, rule) in spans(&shown) {
        out.push_str(&text[copied..original(start)]);
        marks.push((out.len(), rule));
        out.push_str(REDACTED);
        copied = original(end - 1) + 1;
    }
    out.push_str(&text[copied..]);
    (out, marks)
}

/// What looks secret in `text`, in order, as byte ranges with their rule:
/// its PEM private key blocks, and the tokens around them.
fn spans(text: &str) -> Vec<(usize, usize, &'static str)> {
    let mut found = Vec::new();
    let mut from = 0;
    // The tokens before each block, and, after an empty last one, the rest.
    let end = (text.len(), text.len());
    for (begin, end) in private_keys(text).into_iter().chain([end]) {
        found.extend(
            tokens(&text[from..begin])
                .into_iter()
                .map(|(start, stop, rule)| (from + start, from + stop, rule)),
        );
        if begin < end {
            found.push((begin, end, "private_key"));
        }
        from = end;
    }
    found
}

/// PEM private key blocks, whole: to their end line, else to the end.
fn private_keys(text: &str) -> Vec<(usize, usize)> {
    const BEGIN: &str = "-----BEGIN ";
    let mut blocks = Vec::new();
    let mut from = 0;
    while let Some(found) = text[from..].find(BEGIN) {
        let begin = from + found;
        let Some(label) = text[begin + BEGIN.len()..].find("-----") else {
            break;
        };
        let header_end = begin + BEGIN.len() + label + "-----".len();
        if !text[begin..header_end].contains("PRIVATE KEY") {
            from = header_end;
            continue;
        }
        let end = text[header_end..]
            .find("-----END ")
            .and_then(|end| {
                let label = header_end + end + "-----END ".len();
                text[label..]
                    .find("-----")
                    .map(|close| label + close + "-----".len())
            })
            .unwrap_or(text.len());
        blocks.push((begin, end));
        from = end;
    }
    blocks
}

/// Every run of token bytes that looks like a secret, on its own or after
/// the word before it, as byte ranges with their rule.
fn tokens(text: &str) -> Vec<(usize, usize, &'static str)> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    let mut previous: Option<(usize, usize)> = None;
    while i < bytes.len() {
        if !token_byte(bytes[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && token_byte(bytes[i]) {
            i += 1;
        }
        // Base64 padding is the token's, unless more of a token follows it
        // (`key=value`).
        let mut end = i;
        while end < bytes.len() && bytes[end] == b'=' {
            end += 1;
        }
        if end < bytes.len() && token_byte(bytes[end]) {
            end = i;
        }
        i = end;
        // A sentence's full stop is not the token's.
        let mut stop = end;
        while stop > start && bytes[stop - 1] == b'.' {
            stop -= 1;
        }
        let run = &text[start..stop];
        let after = previous.map(|(s, e)| (&text[s..e], &text[e..start]));
        previous = Some((start, stop));
        let Some((len, rule)) = secret(run, after) else {
            continue;
        };
        found.push((start, start + len, rule));
        if len < run.len() {
            // A key's known prefix opened the run: the rest of it, past what
            // joins them (`/`, `.`), is judged as a token of its own.
            previous = Some((start, start + len));
            i = start + len;
            while i < stop && !bytes[i].is_ascii_alphanumeric() {
                i += 1;
            }
        }
    }
    found
}

/// The bytes a token is made of (all ASCII, so a run of them always starts
/// and ends on a character boundary).
fn token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'+' | b'/' | b'~')
}

/// How much of `run` is a secret, if any, and the rule that says so: a
/// known key prefix, a JWT, a key of a vendor not named by its shape, or a
/// long mixed token right after a word that names a secret (`after`: that
/// word, and what stands between).
fn secret(run: &str, after: Option<(&str, &str)>) -> Option<(usize, &'static str)> {
    let head = &run[..run
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(run.len())];
    if AWS_EXAMPLES.contains(&run) || signed_evidence(run) {
        return None;
    }
    let base64url = |part: &str| {
        part.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    };
    let parts: Vec<&str> = run.split('.').collect();
    if run.starts_with("eyJ")
        && parts.len() == 3
        && parts.iter().all(|p| p.len() >= 10 && base64url(p))
    {
        return Some((run.len(), "jwt"));
    }
    if let Some(rule) = known_prefix(head) {
        return Some((head.len(), rule));
    }
    // A key of a vendor not named here, alone: an id and a secret joined by
    // a dot, or a long token mixing capitals, lower case and digits.
    if dotted_key(run) {
        return Some((run.len(), "id_and_secret"));
    }
    if mixes_cases_and_digits(run) {
        return Some((run.len(), "mixed_token"));
    }
    let (word, between) = after?;
    let separated = (1..=4).contains(&between.len())
        && between
            .bytes()
            .all(|b| matches!(b, b' ' | b'\t' | b':' | b'=' | b'"' | b'\''));
    let mixed =
        run.bytes().any(|b| b.is_ascii_digit()) && run.bytes().any(|b| b.is_ascii_alphabetic());
    let distinct = run.bytes().collect::<BTreeSet<u8>>().len();
    let long_and_mixed = run.len() >= 16 && mixed && distinct >= 8 && !path(run);
    (separated && names_a_secret(word) && long_and_mixed).then_some((run.len(), "named_secret"))
}

/// The rule of a key that `head` (a run's leading letters, digits, `_` and
/// `-`) opens with a vendor's prefix: OpenAI's `sk-` (`sk-proj-` among
/// them) and Anthropic's `sk-ant-`; GitHub's `ghp_`, `gho_`, `ghu_`, `ghs_`,
/// `ghr_` and `github_pat_`; Slack's `xoxb-`, `xoxp-`, `xoxa-` and
/// `xoxs-`; AWS's `AKIA` and `ASIA` key ids; GitLab's `glpat-`; Google's
/// `AIza`. Each needs a rest long enough to be a key. AWS's example key id
/// is none, and a run it opens is judged whole.
fn known_prefix(head: &str) -> Option<&'static str> {
    let mixed = |s: &str| {
        s.bytes().any(|b| b.is_ascii_digit()) && s.bytes().any(|b| b.is_ascii_alphabetic())
    };
    let alnum = |s: &str| s.bytes().all(|b| b.is_ascii_alphanumeric());
    let aws = |s: &str| {
        s.len() == 16
            && s.bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    };
    let prefixed = |prefix: &str, least: usize, ok: &dyn Fn(&str) -> bool| {
        head.strip_prefix(prefix)
            .is_some_and(|rest| rest.len() >= least && ok(rest))
    };
    if prefixed("sk-", 20, &mixed) {
        return Some(if head.starts_with("sk-ant-") {
            "anthropic"
        } else {
            "openai"
        });
    }
    if ["ghp_", "gho_", "ghu_", "ghs_", "ghr_"]
        .iter()
        .any(|p| prefixed(p, 30, &alnum))
        || prefixed("github_pat_", 20, &mixed)
    {
        return Some("github");
    }
    if ["xoxb-", "xoxp-", "xoxa-", "xoxs-"]
        .iter()
        .any(|p| prefixed(p, 10, &mixed))
    {
        return Some("slack");
    }
    if (prefixed("AKIA", 16, &aws) || prefixed("ASIA", 16, &aws)) && !AWS_EXAMPLES.contains(&head) {
        return Some("aws");
    }
    if prefixed("glpat-", 20, &mixed) {
        return Some("gitlab");
    }
    if prefixed("AIza", 30, &|_| true) {
        return Some("google");
    }
    None
}

/// `<hex, 24 or more>.<letters and digits, 12 or more>`: an id and a secret
/// joined by a dot, as some providers issue keys (Z.ai's are `<32 hex>.<16
/// letters and digits>`), alone or as one name of a path or URL.
fn dotted_key(run: &str) -> bool {
    run.split('/').any(|name| {
        let parts: Vec<&str> = name.split('.').collect();
        parts.windows(2).any(|pair| {
            pair[0].len() >= 24
                && pair[0].bytes().all(|b| b.is_ascii_hexdigit())
                && pair[1].len() >= 12
                && pair[1].bytes().all(|b| b.is_ascii_alphanumeric())
        })
    })
}

/// A signed evidence export, as habitat signs its runtime evidence
/// (`<payload hex>.<signature hex>`, its `sign_evidence`): a 64-byte Ed25519
/// signature in hex, over JSON in hex that declares itself an export, with
/// the `version` 1, `evidence`, `exported_at` and `operation_id` vartio's
/// verifier reads, and holds nothing a rule redacts. It is shaped like an
/// id and a secret, but it is evidence, and redacting it would destroy it.
/// Anything else on that wire is judged by the rules, and redacted: habitat
/// signs its grant proofs on it too, which authorize whoever holds one for
/// up to 30 s.
fn signed_evidence(run: &str) -> bool {
    let Some((payload, signature)) = run.split_once('.') else {
        return false;
    };
    if signature.len() != 128 || !signature.bytes().all(|b| b.is_ascii_hexdigit()) {
        return false;
    }
    let Some(json) = unhex(payload).and_then(|bytes| String::from_utf8(bytes).ok()) else {
        return false;
    };
    let Ok(export) = serde_json::from_str::<Value>(&json) else {
        return false;
    };
    export.is_object()
        && export["version"] == 1
        && export["evidence"].is_array()
        && export["exported_at"].is_u64()
        && export["operation_id"].is_string()
        && marked(&json).1.is_empty()
}

/// The bytes `hex` spells, two digits each; none for what is not hex.
fn unhex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    hex.as_bytes()
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect()
}

/// A path, not a key: a run that opens with `/`, `~` or `.` and names a file
/// with an extension (`/etc/vessa/deploy.pem`), or has two or more names,
/// each made of words (`~/.ssh/id_ed25519`). A base64 secret can open with
/// `/` too, and has no extension, and seldom two names made of words.
fn path(run: &str) -> bool {
    if !run.starts_with(['/', '~', '.']) {
        return false;
    }
    let names: Vec<&str> = run
        .trim_start_matches('~')
        .split('/')
        .filter(|name| !name.is_empty())
        .collect();
    let extension = names
        .last()
        .and_then(|name| name.rsplit_once('.'))
        .is_some_and(|(stem, extension)| {
            !stem.is_empty()
                && (1..=8).contains(&extension.len())
                && extension.bytes().all(|b| b.is_ascii_alphanumeric())
        });
    extension
        || names.len() >= 2
            && names
                .iter()
                .all(|name| name.split(['.', '-', '_']).all(words))
}

/// Letters and digits that read as words, as names are written: short
/// (`macOS`), in lower case, in capitals, or capitalised words run together
/// (`ScrubTokens2`), capitals alone only at the end. Random letters and
/// digits mix their cases in any order, and seldom pass.
fn words(part: &str) -> bool {
    let lower = |b: &u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    let upper = |b: &u8| b.is_ascii_uppercase() || b.is_ascii_digit();
    let bytes = part.as_bytes();
    if (bytes.len() <= 6 && bytes.iter().all(u8::is_ascii_alphanumeric))
        || bytes.iter().all(lower)
        || bytes.iter().all(upper)
    {
        return true;
    }
    // Words run together: a capital, then two or more lower case letters or
    // digits, again; capitals alone only at the end.
    let mut i = bytes.iter().take_while(|b| lower(b)).count();
    while i < bytes.len() {
        let capitals = bytes[i..]
            .iter()
            .take_while(|b| b.is_ascii_uppercase())
            .count();
        let rest = bytes[i + capitals..]
            .iter()
            .take_while(|b| lower(b))
            .count();
        let end = i + capitals == bytes.len();
        if capitals == 0 || !(end || (capitals == 1 && rest >= 2)) {
            return false;
        }
        i += capitals + rest;
    }
    true
}

/// 32 or more characters with no space, two or more each of capitals, lower
/// case letters and digits: what a key with no known prefix looks like. A
/// hash or a UUID, in either case, is hex and is left alone. A path is
/// judged by the pieces of its names (between `/`, `.`, `-` and `_`), so a
/// long path, or a versioned or dated file name, made of short words is no
/// key; a Codex rollout name has one capital besides. A long name with
/// digits and capitals (`ProjectSettingsV2Controller3`) is redacted too.
/// What is a path is [`path`]'s to say: opening with `/` is not enough.
fn mixes_cases_and_digits(run: &str) -> bool {
    let long_and_mixed = |part: &str| {
        let count = |kind: fn(&u8) -> bool| part.bytes().filter(kind).count();
        part.len() >= 32
            && count(u8::is_ascii_uppercase) >= 2
            && count(u8::is_ascii_lowercase) >= 2
            && count(u8::is_ascii_digit) >= 2
            && !part.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
    };
    if path(run) {
        run.split(['/', '.', '-', '_']).any(long_and_mixed)
    } else {
        long_and_mixed(run)
    }
}

/// `key`, `token`, `secret`, `password`, `passwd` or `bearer`, alone or
/// ending a name (`api_key`, `GITHUB_TOKEN`, `apiKey`), never inside a word
/// (`monkey`).
fn names_a_secret(word: &str) -> bool {
    let lower = word.to_ascii_lowercase();
    ["key", "token", "secret", "password", "passwd", "bearer"]
        .iter()
        .any(|name| {
            lower.ends_with(name) && {
                let cut = word.len() - name.len();
                cut == 0
                    || matches!(word.as_bytes()[cut - 1], b'_' | b'-' | b'.')
                    || word.as_bytes()[cut].is_ascii_uppercase()
                        && word.as_bytes()[cut - 1].is_ascii_lowercase()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Secret-shaped strings, put together at run time so that none sits in
    /// the source for a scanner to find. None is a real credential.
    fn fake(parts: &[&str]) -> String {
        parts.concat()
    }

    const BODY: &str = "Ab3dE5gH7jK9mN1pQ3sT5vW7yZ9bC1dF3hJ5kL7";

    /// An id and a secret joined by a dot, in the shape Z.ai issues its keys:
    /// `<32 hex>.<16 letters and digits>`.
    fn zai() -> String {
        fake(&[
            "3f9a1c0b7e2d4f6a",
            "8b1c3d5e7f9a0b2c",
            ".",
            "Xk9Qm2Lp7RtA4nWz",
        ])
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// The JSON of an evidence export, as habitat writes one (its
    /// `export_evidence`, keys in order), with `extra` among the claims of
    /// its one envelope.
    fn export(extra: &str) -> String {
        format!(
            r#"{{"anomalies":[],"effects":[],"evidence":[{{"claim_type":"effect_attempted","lease_generation":1,"observation_grade":"executor_observed","operation_id":"op-demo","payload":{{"effect_id":"eff-demo"{extra}}},"source":"habitat","source_identity":"executor-demo","timestamp":1793000000000,"version":1}}],"exported_at":1793000000500,"lease_generation":1,"operation_id":"op-demo","results":[],"status":"active","version":1}}"#
        )
    }

    /// A 64-byte signature in hex.
    fn signature() -> String {
        "5e0f".repeat(32)
    }

    /// A signed evidence token in the shape habitat writes one: the export's
    /// JSON in hex (`7b22` is `{"`), a dot, and its signature in hex.
    fn evidence() -> String {
        format!("{}.{}", hex(export("").as_bytes()), signature())
    }

    /// `n` hex digits that follow no pattern a rule knows.
    fn noise(n: usize) -> String {
        let mut state: u64 = 0x5eed_0f0f_a11c_e5e5;
        (0..n)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                char::from(b"0123456789abcdef"[(state % 16) as usize])
            })
            .collect()
    }

    fn scrub(text: &str) -> (String, Redactions) {
        let mut found = Redactions::default();
        (found.text(text), found)
    }

    fn one(rule: &'static str) -> BTreeMap<&'static str, usize> {
        BTreeMap::from([(rule, 1)])
    }

    #[test]
    fn each_known_format_is_redacted_counted_and_named_by_its_rule() {
        let cases = [
            ("openai", fake(&["s", "k-proj-", &BODY[..32]])),
            ("openai", fake(&["s", "k-", &BODY[..32]])),
            ("anthropic", fake(&["s", "k-ant-api03-", &BODY[..36]])),
            ("github", fake(&["gh", "p_", &BODY[..36]])),
            ("github", fake(&["gh", "o_", &BODY[..36]])),
            ("github", fake(&["github", "_pat_11", &BODY[..30]])),
            ("slack", fake(&["xo", "xb-", "1234567890-", &BODY[..24]])),
            ("slack", fake(&["xo", "xp-", "1234567890-", &BODY[..24]])),
            ("aws", fake(&["AK", "IA", "Q3SWZ7YT2NB5XK4M"])),
            ("aws", fake(&["AS", "IA", "Q3SWZ7YT2NB5XK4M"])),
            ("gitlab", fake(&["gl", "pat-", &BODY[..20]])),
            ("google", fake(&["AI", "za", "Sy", &BODY[..33]])),
            (
                "jwt",
                fake(&[
                    "ey",
                    "JhbGciOiJIUzI1NiJ9",
                    ".",
                    "eyJzdWIiOiIxMjM0NTY3ODkwIn0",
                    ".",
                    &BODY[..28],
                ]),
            ),
            ("id_and_secret", zai()),
        ];
        for (rule, secret) in &cases {
            for text in [
                secret.clone(),
                format!("use {secret} for it."),
                format!("({secret}), then retry"),
                format!("{{\"key\":\"{secret}\"}}"),
            ] {
                let (scrubbed, found) = scrub(&text);
                assert_eq!(found.by_rule(), &one(rule), "{rule}: {scrubbed}");
                assert!(!scrubbed.contains(secret.as_str()), "{rule}: {scrubbed}");
                assert!(scrubbed.contains(REDACTED), "{rule}: {scrubbed}");
            }
        }
        // Two in one text are two, each under its rule.
        let (scrubbed, found) = scrub(&format!("{} and {}", cases[3].1, zai()));
        assert_eq!(scrubbed, "[redacted] and [redacted]");
        assert_eq!(
            found.by_rule(),
            &BTreeMap::from([("github", 1), ("id_and_secret", 1)])
        );
        assert_eq!(found.total(), 2);
    }

    #[test]
    fn a_private_key_block_is_redacted_whole() {
        let pem = fake(&[
            "-----BEGIN RSA PRIV",
            "ATE KEY-----\n",
            BODY,
            "\n",
            BODY,
            "\n-----END RSA PRIV",
            "ATE KEY-----",
        ]);
        let (scrubbed, found) = scrub(&format!("here:\n{pem}\nthanks"));
        assert_eq!(scrubbed, "here:\n[redacted]\nthanks");
        assert_eq!(found.by_rule(), &one("private_key"));
        // Without its end line, to the end.
        let open = fake(&["-----BEGIN OPENSSH PRIV", "ATE KEY-----\n", BODY]);
        let (scrubbed, found) = scrub(&format!("x {open}"));
        assert_eq!(scrubbed, "x [redacted]");
        assert_eq!(found.total(), 1);
        // A certificate is no secret.
        let cert = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----";
        assert_eq!(scrub(cert), (cert.to_owned(), Redactions::default()));
    }

    #[test]
    fn an_explicit_secret_label_takes_precedence_over_generic_shape() {
        let value = fake(&["Ab3dE5gH7jK9mN1pQ3sT", "5vW7yZ9bC1dF3hJ5kL7mN9p"]);
        assert_eq!(scrub(&value).1.by_rule(), &one("mixed_token"));
        for label in ["password", "api_key", "access_token", "client_secret", "Bearer"] {
            let (hidden, found) = scrub(&format!("{label}: {value}"));
            assert_eq!(found.by_rule(), &one("named_secret"), "{label}");
            assert_eq!(found.total(), 1);
            assert!(!hidden.contains(&value));
        }
    }

    #[test]
    fn a_long_mixed_token_after_a_word_that_names_a_secret_is_redacted() {
        let value = &BODY[..24];
        for text in [
            format!("api_key={value}"),
            format!("API_KEY: {value}"),
            format!("apiKey = '{value}'"),
            format!("\"access_token\": \"{value}\""),
            format!("client_secret {value}"),
            format!("password: {value}"),
            format!("Authorization: Bearer {value}"),
            format!("export GITHUB_TOKEN={value}=="),
        ] {
            let (scrubbed, found) = scrub(&text);
            assert_eq!(found.by_rule(), &one("named_secret"), "{text}: {scrubbed}");
            assert!(!scrubbed.contains(value), "{scrubbed}");
        }
    }

    #[test]
    fn a_key_of_a_vendor_not_named_is_redacted_by_its_shape() {
        for (rule, key) in [
            (
                "mixed_token",
                fake(&["be3f", "Q9xT2mLk7RvA4nWp8ZcH1sYd6JgU3eBo5KfN0iMq2"]),
            ),
            (
                "mixed_token",
                fake(&["Ab3dE5gH7jK9mN1pQ3sT", "/", "5vW7yZ9bC1dF3hJ5kL7mN9p"]),
            ),
            (
                "id_and_secret",
                fake(&["3f9a1c0b7e2d4f6a8b1c3d5e", ".", "k9qm2lp7rtaz"]),
            ),
        ] {
            for text in [
                key.clone(),
                format!("use this one: {key}."),
                format!("({key})"),
            ] {
                let (scrubbed, found) = scrub(&text);
                assert_eq!(found.by_rule(), &one(rule), "{text}: {scrubbed}");
                assert!(!scrubbed.contains(key.as_str()), "{scrubbed}");
            }
        }
    }

    #[test]
    fn what_only_looks_long_is_not_redacted() {
        for text in [
            // A git sha, a UUID, alone or after ordinary words.
            "commit 1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b is on main",
            "1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b",
            "session 8c1f2e3a-4b5c-4d6e-8f70-123456789abc ended",
            "8c1f2e3a-4b5c-4d6e-8f70-123456789abc",
            // Words that name a secret, followed by no secret.
            "the key thing is to keep the token budget small",
            "password: hunter2",
            "the token count was 1234",
            "key /Users/demo/.ssh/id_ed25519_github_2026",
            // Prefixes inside ordinary words, or too short to be keys.
            "use sk-learn, or task-1234567890abcdefghij",
            "the AKIA prefix names an AWS key",
            "eyJ is how base64 JSON starts",
            "monkey Ab3dE5gH7jK9mN1pQ3sT",
            // Hashes and ids in either case, and long names and paths with no
            // capital, digit or long segment that mixes them.
            "1A2B3C4D5E6F7A8B9C0D1E2F3A4B5C6D7E8F9A0B and 8C1F2E3A-4B5C-4D6E-8F70-123456789ABC",
            "sha256 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
            "image sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
            "a 9f86d081884c7d659a2f.feaa0c55ad01 short id",
            "test_the_cursor_moves_only_past_what_was_handled_and_never_back_2",
            "/Users/demo/projects/_worktrees/vessa-scrub/src/ScrubTokens2/main.rs",
            "~/.codex/sessions/2026/10/26/rollout-2026-10-26T10-00-00-8c1f2e3a.jsonl",
            "ProjectManagementDashboardSettingsController is long but has no digit",
            // A path is judged by the pieces of its names: versioned and dated
            // file names are made of short words.
            "/Users/demo/Downloads/Vessa-1.2.3-macOS-arm64-applesilicon.zip",
            "~/Desktop/2026-10-26T10-20-33Z_Screen-Recording-8c1f2e3a-Final.mov",
            // What a scrub leaves is no secret either.
            "[redacted] and [redacted].",
        ] {
            assert_eq!(
                scrub(text),
                (text.to_owned(), Redactions::default()),
                "{text}"
            );
        }
    }

    #[test]
    fn awss_documented_example_keys_are_not_redacted() {
        // The access key pair AWS's own documentation uses, which is no
        // secret; a key of the same shape is.
        for text in [
            "AKIAIOSFODNN7EXAMPLE",
            "aws_access_key_id = AKIAIOSFODNN7EXAMPLE",
            "aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        ] {
            assert_eq!(
                scrub(text),
                (text.to_owned(), Redactions::default()),
                "{text}"
            );
        }
        let (_, found) = scrub(&fake(&["AK", "IA", "IOSFODNN7EXAMPL3"]));
        assert_eq!(found.by_rule(), &one("aws"));
    }

    #[test]
    fn signed_evidence_is_not_redacted() {
        let token = evidence();
        for text in [
            token.clone(),
            format!("verified {token}."),
            format!("evidence token: {token}"),
            format!("token={token}"),
            format!("{{\"evidence\":\"{token}\"}}"),
        ] {
            assert_eq!(
                scrub(&text),
                (text.clone(), Redactions::default()),
                "{text}"
            );
        }
        // Only that shape: hex JSON before a secret that is no signature is
        // an id and a secret.
        let (payload, _) = token.split_once('.').unwrap();
        for secret in ["Xk9Qm2Lp7RtA4nWz".to_owned(), "5e0f".repeat(31)] {
            let (scrubbed, found) = scrub(&format!("{payload}.{secret}"));
            assert_eq!(found.by_rule(), &one("id_and_secret"), "{scrubbed}");
        }
    }

    #[test]
    fn only_an_evidence_export_that_holds_no_secret_is_kept() {
        let github = fake(&["gh", "p_", &BODY[..36]]);
        // A grant proof is an authorization: whoever holds it may use one
        // right until it expires (habitat-proof's `ProofClaims`, signed on
        // the same wire).
        let grant = r#"{"version":2,"habitat_id":"hab-demo","key_id":"5e0f5e0f5e0f5e0f","grant_id":"grant-demo","operation_id":"op-demo","resource_id":"res-demo","right":"execute","issued_at_ms":1793000000000,"expires_at_ms":1793000030000,"nonce":"0a1b2c3d4e5f60718293a4b5c6d7e8f9"}"#;
        for (what, token) in [
            (
                "hex that is no JSON",
                format!("7b22{}.{}", noise(60), signature()),
            ),
            (
                "an export holding a key",
                format!(
                    "{}.{}",
                    hex(export(&format!(r#","note":"{github}""#)).as_bytes()),
                    signature()
                ),
            ),
            (
                "an id and a secret with 7b22 in front",
                fake(&["7b22", &noise(28), ".", "Xk9Qm2Lp7RtA4nWz"]),
            ),
            (
                "a grant proof",
                format!("{}.{}", hex(grant.as_bytes()), signature()),
            ),
            (
                "JSON that is no object",
                format!("{}.{}", hex(br#"["evidence",1]"#), signature()),
            ),
            (
                "an object that declares nothing",
                format!("{}.{}", hex(br#"{"claim":"ran"}"#), signature()),
            ),
        ] {
            let (scrubbed, found) = scrub(&format!("here: {token} done"));
            assert_eq!(scrubbed, "here: [redacted] done", "{what}");
            assert_eq!(found.by_rule(), &one("id_and_secret"), "{what}");
        }
    }

    #[test]
    fn a_base64_secret_that_opens_with_a_slash_is_no_path() {
        // About 1 in 64 AWS secret access keys (40 base64 characters) opens
        // with `/`; each of these does.
        const BASE64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut state: u64 = 0x0b5e_55ed_c0ff_ee01;
        for _ in 0..100 {
            let secret: String = std::iter::once('/')
                .chain((0..39).map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    char::from(BASE64[(state % 64) as usize])
                }))
                .collect();
            let (scrubbed, found) = scrub(&format!("aws_secret_access_key = {secret}"));
            assert_eq!(scrubbed, "aws_secret_access_key = [redacted]");
            assert_eq!(found.total(), 1);
        }
        // A path after a word that names a secret is still a path: it names
        // a file, or its names are words.
        for text in [
            "key ~/.ssh/id_ed25519_github_2026",
            "private_key: /etc/vessa/Keys2/deploy2.pem",
            "token: /Users/demo/Library/ScrubTokens2/main2",
        ] {
            assert_eq!(
                scrub(text),
                (text.to_owned(), Redactions::default()),
                "{text}"
            );
        }
    }

    #[test]
    fn a_key_joined_to_another_by_a_slash_or_a_dot_is_redacted_too() {
        let github = fake(&["gh", "p_", &BODY[..36]]);
        let zai = zai();
        for text in [
            format!("{github}/{zai}"),
            format!("{github}.{zai}"),
            format!("AKIAIOSFODNN7EXAMPLE/{BODY}"),
            format!("AKIAIOSFODNN7EXAMPLE.{zai}"),
            format!("https://api.example.com/v1/keys/{zai}"),
        ] {
            let (scrubbed, found) = scrub(&text);
            assert!(found.total() > 0, "{text}");
            for secret in [github.as_str(), &zai, &zai[33..], BODY] {
                assert!(!scrubbed.contains(secret), "{scrubbed}");
            }
        }
    }

    #[test]
    fn a_key_split_by_a_character_a_terminal_hides_is_judged_as_shown() {
        // A terminal is shown the text without control and bidirectional
        // formatting characters, so a key split by one shows whole.
        let key = fake(&["gh", "p_", &BODY[..36]]);
        let (a, b) = key.split_at(20);
        for hidden in [
            '\u{0}', '\u{7}', '\u{1b}', '\r', '\t', '\u{202E}', '\u{2066}',
        ] {
            let (scrubbed, found) = scrub(&format!("use {a}{hidden}{b} now"));
            assert_eq!(scrubbed, "use [redacted] now", "{hidden:?}");
            assert_eq!(found.by_rule(), &one("github"), "{hidden:?}");
        }
        // Outside a key, what a terminal hides is kept; a line break still
        // ends a token, as it ends a line.
        let text = "a\u{7}b and \u{202E}c\nAb3dE5gH7jK9mN1p\nQ3sT5vW7yZ9bC1dF3hJ5";
        assert_eq!(scrub(text), (text.to_owned(), Redactions::default()));
    }

    #[test]
    fn a_tail_is_cut_from_the_scrubbed_text_never_inside_a_marker_or_a_token() {
        // Short enough: whole, and scrubbed.
        let key = zai();
        let cut = tail(&format!("use {key} now"), 600);
        assert_eq!(
            (cut.text.as_str(), cut.truncated, cut.redacted.by_rule()),
            ("use [redacted] now", false, &one("id_and_secret"))
        );
        // Cut anywhere through a key: no piece of it is kept, and the tail
        // counts the markers it keeps.
        let text = format!("Set it to {key} and the rest of the reply follows.");
        let scrubbed = scrub(&text).0;
        let token = |b: u8| b.is_ascii_alphanumeric() || b"_-.+/~".contains(&b);
        for chars in 1..=text.chars().count() {
            let cut = tail(&text, chars);
            assert!(cut.text.chars().count() <= chars, "{chars}: {}", cut.text);
            assert!(scrubbed.ends_with(&cut.text), "{chars}: {}", cut.text);
            assert_eq!(cut.truncated, cut.text.len() < scrubbed.len());
            for piece in key.as_bytes().windows(4) {
                let piece = std::str::from_utf8(piece).unwrap();
                assert!(!cut.text.contains(piece), "{chars}: {}", cut.text);
            }
            let start = scrubbed.len() - cut.text.len();
            let marker = scrubbed.find(REDACTED).unwrap();
            assert!(
                !(marker < start && start < marker + REDACTED.len()),
                "{chars}: {}",
                cut.text
            );
            let bytes = scrubbed.as_bytes();
            assert!(
                start == 0
                    || start == bytes.len()
                    || !(token(bytes[start - 1]) && token(bytes[start])),
                "{chars}: {}",
                cut.text
            );
            assert_eq!(
                cut.redacted.total(),
                usize::from(cut.text.contains(REDACTED)),
                "{chars}: {}",
                cut.text
            );
        }
        // A cut inside a word starts at the next one; between words, the
        // tail is exactly the last characters.
        assert_eq!(tail("alpha beta gamma", 8).text, " gamma");
        assert_eq!(tail("alpha beta gamma", 6).text, " gamma");
        assert_eq!(tail("alpha beta gamma", 10).text, "beta gamma");
        // The word that names a secret may be cut off: its secret was
        // redacted before the cut.
        let value = &BODY[..24];
        let cut = tail(&format!("api_key={value} done"), 15);
        assert_eq!(cut.text, "[redacted] done");
        assert_eq!(cut.redacted.by_rule(), &one("named_secret"));
    }

    #[test]
    fn a_scrubbed_text_scrubs_to_itself() {
        let text = format!(
            "use {} and {}, then {}",
            fake(&["gh", "p_", &BODY[..36]]),
            zai(),
            fake(&["s", "k-ant-api03-", &BODY[..36]])
        );
        let (once, found) = scrub(&text);
        assert_eq!(found.total(), 3);
        assert_eq!(scrub(&once), (once.clone(), Redactions::default()));
    }

    #[test]
    fn every_string_in_a_record_is_scrubbed_and_its_names_kept() {
        let key = fake(&["gh", "p_", &BODY[..36]]);
        let mut record = serde_json::json!({
            "id": "oh-fixture",
            "task": format!("deploy with {key}"),
            "handover": {"next": [format!("rotate {key}"), "nothing else"]},
            "started_ms": 1,
        });
        let mut found = Redactions::default();
        found.value(&mut record);
        assert_eq!(
            record,
            serde_json::json!({
                "id": "oh-fixture",
                "task": "deploy with [redacted]",
                "handover": {"next": ["rotate [redacted]", "nothing else"]},
                "started_ms": 1,
            })
        );
        assert_eq!(found.by_rule(), &BTreeMap::from([("github", 2)]));
        let mut lines = vec![format!("a {key}"), "b".to_owned()];
        found.lines(&mut lines);
        assert_eq!(lines, ["a [redacted]", "b"]);
        assert_eq!(found.total(), 3);
        assert_eq!(
            found.said().as_deref(),
            Some("3 secret-looking tokens redacted (github 3)")
        );
        assert_eq!(Redactions::default().said(), None);
    }

    #[test]
    fn a_message_is_scrubbed_in_its_words_question_and_reply() {
        let key = zai();
        let m = Message {
            agent: "claude".into(),
            session: "claude-demo".into(),
            parent: None,
            kind: "answer",
            id: Some("u1".into()),
            at: Some("2026-09-30T09:00:00Z".into()),
            cwd: "/demo/widget".into(),
            text: format!("It is {key}."),
            question: Some(format!("Is it {key}?")),
            reply: Some(crate::model::Reply {
                text: format!("I set {key}."),
                truncated: false,
                failed: false,
                line: 2,
                redacted: Redactions::default(),
            }),
            source: "/demo/claude-demo.jsonl".into(),
            line: 3,
        };
        let mut found = Redactions::default();
        let shown = found.message(&m);
        assert_eq!(shown.text, "It is [redacted].");
        assert_eq!(shown.question.as_deref(), Some("Is it [redacted]?"));
        assert_eq!(shown.reply.as_ref().unwrap().text, "I set [redacted].");
        assert_eq!(
            (shown.session.as_str(), shown.cwd.as_str(), shown.line),
            ("claude-demo", "/demo/widget", 3)
        );
        assert_eq!(found.by_rule(), &BTreeMap::from([("id_and_secret", 3)]));
        // A reply scrubbed before it was cut is counted where it is printed,
        // once each time.
        let cut = tail(&format!("I set {key}."), 600);
        let m = Message {
            reply: Some(crate::model::Reply {
                text: cut.text,
                truncated: cut.truncated,
                failed: false,
                line: 2,
                redacted: cut.redacted,
            }),
            ..m
        };
        let mut found = Redactions::default();
        found.message(&m);
        found.message(&m);
        assert_eq!(found.by_rule(), &BTreeMap::from([("id_and_secret", 6)]));
    }
}
