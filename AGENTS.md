# Vessa

Rust only. Local usage accounting and reporting; no agent execution authority.
Preserve source references and unknown values. Estimates are not bills.
Conversation classification belongs to Seula; do not retain native chat bodies.
No runtime dependency on sibling repositories. No database, daemon or implicit
network calls. Never commit real transcripts or reports; use synthetic fixtures.
Validate with cargo fmt --check, cargo clippy --all-targets -- -D warnings,
and cargo test. Run the binary on a fixture before claiming completion.
The gate is `sykli run sykli.json --json > .sykli/receipt.json && sykli verify
.sykli/receipt.json --contract sykli.json`. Every input is listed by path:
after adding a source, test or fixture file, add it to the task inputs,
stage it and run `sykli lock sykli.json`, or sykli serves a stale pass.
