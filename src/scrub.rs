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
//! word that names a secret. Vessa names each rule, and adds two
//! exceptions that are no secret: AWS's documented example keys, and signed
//! evidence tokens, which are evidence and would be destroyed.

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
        let text = self.private_keys(text);
        self.tokens(&text)
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
            reply.text = self.text(&reply.text);
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

    /// PEM private key blocks, whole: to their end line, else to the end.
    fn private_keys(&mut self, text: &str) -> String {
        const BEGIN: &str = "-----BEGIN ";
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(begin) = rest.find(BEGIN) {
            let Some(label) = rest[begin + BEGIN.len()..].find("-----") else {
                break;
            };
            let header_end = begin + BEGIN.len() + label + "-----".len();
            if !rest[begin..header_end].contains("PRIVATE KEY") {
                out.push_str(&rest[..header_end]);
                rest = &rest[header_end..];
                continue;
            }
            out.push_str(&rest[..begin]);
            out.push_str(REDACTED);
            self.add("private_key");
            let after = &rest[header_end..];
            rest = after
                .find("-----END ")
                .and_then(|end| {
                    let label = end + "-----END ".len();
                    after[label..]
                        .find("-----")
                        .map(|close| &after[label + close + "-----".len()..])
                })
                .unwrap_or("");
        }
        out.push_str(rest);
        out
    }

    /// Every run of token bytes that looks like a secret, on its own or
    /// after the word before it.
    fn tokens(&mut self, text: &str) -> String {
        let bytes = text.as_bytes();
        let mut out = String::with_capacity(text.len());
        let (mut copied, mut i) = (0, 0);
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
            // Base64 padding is the token's, unless more of a token follows
            // it (`key=value`).
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
            if let Some((len, rule)) = secret(run, after) {
                out.push_str(&text[copied..start]);
                out.push_str(REDACTED);
                copied = start + len;
                self.add(rule);
            }
            previous = Some((start, stop));
        }
        out.push_str(&text[copied..]);
        out
    }
}

/// What a usage command prints of transcripts' and a wrapper's words,
/// scrubbed in place: the report's warnings, each session's limitations,
/// and the wrapper session records linked to it, which hold what a session
/// was asked and handed over. Usage records carry no words.
pub fn report(report: &mut Report) -> Redactions {
    let mut found = Redactions::default();
    found.lines(&mut report.warnings);
    for session in &mut report.sessions {
        found.lines(&mut session.limitations);
        for record in &mut session.ohjaa {
            found.value(record);
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
    if AWS_EXAMPLES.contains(&run) || AWS_EXAMPLES.contains(&head) || signed_evidence(run) {
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
    let long_and_mixed =
        run.len() >= 16 && mixed && distinct >= 8 && !run.starts_with(['/', '~', '.']);
    (separated && names_a_secret(word) && long_and_mixed).then_some((run.len(), "named_secret"))
}

/// The rule of a key that `head` (a run's leading letters, digits, `_` and
/// `-`) opens with a vendor's prefix: OpenAI's `sk-` (`sk-proj-` among
/// them) and Anthropic's `sk-ant-`; GitHub's `ghp_`, `gho_`, `ghu_`, `ghs_`,
/// `ghr_` and `github_pat_`; Slack's `xoxb-`, `xoxp-`, `xoxa-` and
/// `xoxs-`; AWS's `AKIA` and `ASIA` key ids; GitLab's `glpat-`; Google's
/// `AIza`. Each needs a rest long enough to be a key.
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
    if prefixed("AKIA", 16, &aws) || prefixed("ASIA", 16, &aws) {
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
/// letters and digits>`).
fn dotted_key(run: &str) -> bool {
    let parts: Vec<&str> = run.split('.').collect();
    parts.windows(2).any(|pair| {
        pair[0].len() >= 24
            && pair[0].bytes().all(|b| b.is_ascii_hexdigit())
            && pair[1].len() >= 12
            && pair[1].bytes().all(|b| b.is_ascii_alphanumeric())
    })
}

/// A signed evidence token: JSON in hex (`7b22` is `{"`), a dot, and a
/// 64-byte Ed25519 signature in hex, as habitat writes its runtime evidence
/// and proofs (`<payload hex>.<signature hex>`). It is shaped like an id and
/// a secret, but it is evidence, and its signature is public: redacting it
/// would destroy it.
fn signed_evidence(run: &str) -> bool {
    let hex = |s: &str| s.bytes().all(|b| b.is_ascii_hexdigit());
    run.split_once('.').is_some_and(|(payload, signature)| {
        payload.len() % 2 == 0
            && payload
                .get(..4)
                .is_some_and(|open| open.eq_ignore_ascii_case("7b22"))
            && hex(payload)
            && signature.len() == 128
            && hex(signature)
    })
}

/// 32 or more characters with no space, two or more each of capitals, lower
/// case letters and digits: what a key with no known prefix looks like. A
/// hash or a UUID, in either case, is hex and is left alone. A path is
/// judged by the pieces of its names (between `/`, `.`, `-` and `_`), so a
/// long path, or a versioned or dated file name, made of short words is no
/// key; a Codex rollout name has one capital besides. A long name with
/// digits and capitals (`ProjectSettingsV2Controller3`) is redacted too.
fn mixes_cases_and_digits(run: &str) -> bool {
    let long_and_mixed = |part: &str| {
        let count = |kind: fn(&u8) -> bool| part.bytes().filter(kind).count();
        part.len() >= 32
            && count(u8::is_ascii_uppercase) >= 2
            && count(u8::is_ascii_lowercase) >= 2
            && count(u8::is_ascii_digit) >= 2
            && !part.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
    };
    if run.starts_with(['/', '~', '.']) {
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

    /// A signed evidence token in the shape habitat writes one: JSON in hex
    /// (`7b22` is `{"`), a dot, and a 64-byte signature in hex.
    fn evidence() -> String {
        format!(
            "{}.{}",
            hex(br#"{"schema":"demo-evidence","claim":"ran"}"#),
            "5e0f".repeat(32)
        )
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
    }
}
