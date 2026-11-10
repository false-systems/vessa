//! The credentials this machine holds, for the secrets scan to compare with.
//!
//! A secret-shaped string in a transcript is an alarm only when it is one of
//! these: a credential of the person's that reached an agent session. They
//! are read on demand from the sources below, kept in memory for one scan,
//! compared by value, and never printed, logged or written anywhere: the
//! report names each source and how many credentials it held, nothing more.
//!
//! What a source holds is each secret-shaped string the scan's own detector
//! recognizes in it ([`crate::scrub::found`]), and, for a variable or gh's
//! login, its whole value, whatever its shape. Each is then looked for in
//! every transcript as it is, standing alone, so a credential is found
//! however it is labelled or formatted there. macOS Keychain items are not
//! read: reading one can prompt.

use crate::scrub;
use serde::Serialize;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Stdio};

/// Credential files under the home folder.
const FILES: [&str; 12] = [
    ".aws/credentials",
    ".config/gh/hosts.yml",
    ".git-credentials",
    ".netrc",
    ".npmrc",
    ".docker/config.json",
    ".codex/auth.json",
    ".pi/agent/auth.json",
    ".claude/.credentials.json",
    ".ssh/id_ed25519",
    ".ssh/id_rsa",
    ".ssh/id_ecdsa",
];

/// A whole value shorter than this is not held as a credential on its own:
/// short values are words and numbers that turn up anywhere.
const SHORTEST: usize = 16;

/// One place credentials were looked for, as the report says it.
#[derive(Debug, Serialize)]
pub struct Source {
    /// What it is, by name: never a value.
    pub source: String,
    /// `read`, `absent` or `unreadable`.
    pub status: &'static str,
    /// How many distinct credentials the scan compares from it.
    pub credentials: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// The credentials this machine holds, each with the source it came from.
#[derive(Debug, Default)]
pub struct Held {
    values: HashMap<String, String>,
    pub sources: Vec<Source>,
}

impl Held {
    /// Which source holds `value`, when one does.
    pub fn which(&self, value: &str) -> Option<&str> {
        self.values.get(value).map(String::as_str)
    }

    /// Every credential held, with its source's name.
    pub fn each(&self) -> impl Iterator<Item = (&str, &str)> {
        self.values.iter().map(|(v, s)| (v.as_str(), s.as_str()))
    }

    /// Read every source: the secret-named variables of `env`, the GitHub
    /// CLI's login (`gh auth token`, found on `path`; none without one), and
    /// the credential files under `home`.
    pub fn read(
        home: Option<&Path>,
        env: impl IntoIterator<Item = (String, String)>,
        path: Option<&OsStr>,
    ) -> Self {
        let mut held = Held::default();
        let mut variables: Vec<(String, String)> = env
            .into_iter()
            .filter(|(name, _)| secret_named(name))
            .collect();
        variables.sort();
        for (name, value) in variables {
            let label = format!("environment variable {name}");
            let credentials = held.add(&label, &value, true);
            held.sources.push(Source {
                source: label,
                status: "read",
                credentials,
                why: None,
            });
        }
        held.gh(path);
        match home {
            Some(home) => {
                for file in FILES {
                    held.file(&home.join(file), &format!("~/{file}"));
                }
            }
            None => held.sources.push(Source {
                source: "credential files".into(),
                status: "unreadable",
                credentials: 0,
                why: Some("HOME is not set".into()),
            }),
        }
        held
    }

    /// The detector's tokens in `text`, and with `whole` the trimmed text
    /// itself when it is one token of [`SHORTEST`] bytes or more and no
    /// path, held under `label`: how many were new.
    fn add(&mut self, label: &str, text: &str, whole: bool) -> usize {
        let mut values: Vec<String> = scrub::found(text).into_iter().map(|(_, v)| v).collect();
        let trimmed = text.trim();
        if whole
            && trimmed.len() >= SHORTEST
            && !trimmed.starts_with('/')
            && trimmed.bytes().all(scrub::token_byte)
        {
            values.push(trimmed.to_owned());
        }
        let mut added = 0;
        for value in values {
            if let std::collections::hash_map::Entry::Vacant(slot) = self.values.entry(value) {
                slot.insert(label.to_owned());
                added += 1;
            }
        }
        added
    }

    /// The GitHub CLI's active login. gh reads it from its own store; it is
    /// asked not to prompt, and no network is used.
    fn gh(&mut self, path: Option<&OsStr>) {
        let label = "GitHub CLI login (gh auth token)";
        let output = match path {
            None => Err(std::io::ErrorKind::NotFound.into()),
            // With PATH set on the command, the program is looked up there.
            Some(path) => Command::new("gh")
                .args(["auth", "token"])
                .env("PATH", path)
                .env("GH_PROMPT_DISABLED", "1")
                .env("GH_NO_UPDATE_NOTIFIER", "1")
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output(),
        };
        let (status, why, credentials) = match output {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ("absent", Some("gh is not installed".to_owned()), 0)
            }
            Err(error) => ("unreadable", Some(format!("gh could not run: {error}")), 0),
            Ok(out) if !out.status.success() => ("absent", Some("gh has no login".to_owned()), 0),
            Ok(out) => {
                let text = String::from_utf8_lossy(&out.stdout).into_owned();
                ("read", None, self.add(label, &text, true))
            }
        };
        self.sources.push(Source {
            source: label.into(),
            status,
            credentials,
            why,
        });
    }

    fn file(&mut self, path: &Path, label: &str) {
        let (status, why, credentials) = match std::fs::read(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => ("absent", None, 0),
            // Never the content: only why it could not be read.
            Err(error) => ("unreadable", Some(error.kind().to_string()), 0),
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                ("read", None, self.add(label, &text, false))
            }
        };
        self.sources.push(Source {
            source: label.into(),
            status,
            credentials,
            why,
        });
    }
}

/// How often `value` stands alone in `text`: not as part of a longer token.
pub fn occurrences(text: &str, value: &str) -> usize {
    let alone = |c: Option<u8>| c.is_none_or(|c| c == b'.' || !scrub::token_byte(c));
    let bytes = text.as_bytes();
    text.match_indices(value)
        .filter(|(at, _)| {
            alone(at.checked_sub(1).map(|i| bytes[i]))
                && alone(bytes.get(at + value.len()).copied())
        })
        .count()
}

/// A variable whose name says it holds a credential. Names of sockets,
/// paths, files and folders do not, whatever word they contain, and nor
/// does a session key, which names a shell or terminal session
/// (`STARSHIP_SESSION_KEY`), not an account.
fn secret_named(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    let says = [
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "KEY",
        "CREDENTIAL",
        "AUTH",
    ]
    .iter()
    .any(|word| name.contains(word));
    let names_no_credential = ["_SOCK", "_PATH", "_FILE", "_DIR", "_HOME", "SESSION_KEY"]
        .iter()
        .any(|end| name.ends_with(end));
    says && !names_no_credential
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> String {
        ["gh", "p_", "Q7hB4mR9vT2kN8pS6xA3dF5jL1cE0uWzY4tR"].concat()
    }

    #[test]
    fn a_secret_named_variable_is_held_and_others_are_not() {
        let held = Held::read(
            None,
            [
                ("MY_GITHUB_TOKEN".to_owned(), key()),
                (
                    "SESSION_LABEL".to_owned(),
                    ["Ab3dE5gH7jK9", "mN1pQ3sT5vW7"].concat(),
                ),
                (
                    "SSH_AUTH_SOCK".to_owned(),
                    "/tmp/launchd.Ab3dE5gH7jK9mN1/Listeners".to_owned(),
                ),
                (
                    "STARSHIP_SESSION_KEY".to_owned(),
                    "5839201746382910".to_owned(),
                ),
            ],
            None,
        );
        assert_eq!(held.which("5839201746382910"), None);
        assert_eq!(
            held.which(&key()),
            Some("environment variable MY_GITHUB_TOKEN")
        );
        assert_eq!(held.which("Ab3dE5gH7jK9mN1pQ3sT5vW7"), None);
        let named: Vec<&str> = held.sources.iter().map(|s| s.source.as_str()).collect();
        assert!(
            named.contains(&"environment variable MY_GITHUB_TOKEN"),
            "{named:?}"
        );
        assert!(
            !named.iter().any(|n| n.contains("SSH_AUTH_SOCK")),
            "{named:?}"
        );
    }

    #[test]
    fn a_variable_is_held_whole_whatever_its_shape_but_not_a_short_word() {
        let hex = "0123456789abcdef0123456789abcdef";
        let held = Held::read(
            None,
            [
                ("DATADOG_API_KEY".to_owned(), format!("{hex}\n")),
                ("SOME_TOKEN".to_owned(), "short".to_owned()),
            ],
            None,
        );
        assert_eq!(
            held.which(hex),
            Some("environment variable DATADOG_API_KEY")
        );
        assert_eq!(held.which("short"), None);
    }

    #[test]
    fn a_credential_counts_only_where_it_stands_alone() {
        let k = key();
        assert_eq!(occurrences(&format!("token: {k}. and \"{k}\""), &k), 2);
        assert_eq!(occurrences(&format!("x{k} {k}y {k}_"), &k), 0);
    }

    #[test]
    fn credential_files_are_read_and_a_missing_one_is_absent_never_unreadable() {
        let home = std::env::temp_dir().join(format!("vessa-held-{}", std::process::id()));
        std::fs::create_dir_all(home.join(".aws")).unwrap();
        std::fs::write(
            home.join(".aws/credentials"),
            format!("[default]\naws_secret_access_key = {}\n", key()),
        )
        .unwrap();
        let held = Held::read(Some(&home), [], None);
        assert_eq!(held.which(&key()), Some("~/.aws/credentials"));
        let status = |name: &str| {
            held.sources
                .iter()
                .find(|s| s.source == name)
                .map(|s| s.status)
        };
        assert_eq!(status("~/.aws/credentials"), Some("read"));
        assert_eq!(status("~/.netrc"), Some("absent"));
        let _ = std::fs::remove_dir_all(&home);
    }
}
