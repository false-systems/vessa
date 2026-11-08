# Vessa

Rust only. Local usage accounting and reporting; no agent execution authority.
Preserve source references and unknown values. No spending features: do not import or calculate prices, currency estimates, credits or billing.
Vessa is the one reader of agent conversations: `messages` lists what people
typed, on demand, and keeps nothing. Everything Vessa prints of what was
said is scrubbed of secret-looking tokens first (rules copied from ohjaa
intake); only `messages --raw` prints them as written. Correction detection
belongs to ohjaa intake. Usage exports, receipts and reports carry no message
text.
`secrets` scans native records on demand for the credentials this machine
holds (secret-named environment variables, gh's login, the credential files
README lists), read at the start of the scan, compared in memory and never
printed, logged or written; it emits only scrubbed metadata, never matched
values or conversation text. A finding is one of those credentials in a
transcript; every other secret-shaped string is a count. It includes tool
traffic and subagents; unreadable sources or temporally unknown findings
cannot become an all-clear. Its optional JSON output creates no dependency
on any consumer.

No runtime dependency on sibling repositories. No database, daemon or implicit
network calls. Never commit real transcripts or reports; use synthetic fixtures.
Validate with cargo fmt --check, cargo clippy --all-targets -- -D warnings,
and cargo test. Run the binary on a fixture before claiming completion.
