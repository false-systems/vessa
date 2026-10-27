//! Secret-looking tokens in what Vessa prints, redacted. Not yet written:
//! every function returns its input unchanged.

use crate::model::{Message, Report};
use serde_json::Value;
use std::collections::BTreeMap;

pub const REDACTED: &str = "[redacted]";

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

    pub fn text(&mut self, text: &str) -> String {
        text.to_owned()
    }

    pub fn lines<'a>(&mut self, _lines: impl IntoIterator<Item = &'a mut String>) {}

    pub fn value(&mut self, _value: &mut Value) {}

    pub fn message(&mut self, m: &Message) -> Message {
        m.clone()
    }

    pub fn said(&self) -> Option<String> {
        None
    }
}

pub fn report(_report: &mut Report) -> Redactions {
    Redactions::default()
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
