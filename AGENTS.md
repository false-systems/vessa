# Vessa

Rust only. Local usage accounting and reporting; no agent execution authority.
Preserve source references and unknown values. Estimates are not bills.
Vessa is the one reader of agent conversations: `messages` lists what people
typed, on demand, and keeps nothing. Everything Vessa prints of what was
said is scrubbed of secret-looking tokens first (rules copied from ohjaa
intake); only `messages --raw` prints them as written. Correction detection
belongs to ohjaa intake. Usage exports, receipts and reports carry no message
text.
No runtime dependency on sibling repositories. No database, daemon or implicit
network calls. Never commit real transcripts or reports; use synthetic fixtures.
Validate with cargo fmt --check, cargo clippy --all-targets -- -D warnings,
and cargo test. Run the binary on a fixture before claiming completion.
