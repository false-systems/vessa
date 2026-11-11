# Vessa

Run `toimija enter` first in this repository.

Rust only. Local usage accounting and reporting; no agent execution authority.
Preserve source references and unknown values. No spending features: do not import or calculate prices, currency estimates, credits or billing.
Vessa is the one reader of agent conversations: `messages` lists what people
typed, on demand, and keeps nothing. Everything Vessa prints of what was
said is scrubbed of secret-looking tokens first; only `messages --raw`
prints them as written. Vessa does not judge what a message means: that
belongs to whoever reads its output. Usage exports, receipts and reports
carry no message text.
`secrets` scans native records on demand for the credentials this machine
holds (secret-named environment variables, gh's login, the credential files
`docs/reference.md` lists), read at the start of the scan, compared in memory and never
printed, logged or written; it emits only scrubbed metadata, never matched
values or conversation text. A finding is one of those credentials in a
transcript; every other secret-shaped string is a count. It includes tool
traffic and subagents; unreadable sources or temporally unknown findings
cannot become an all-clear. Its optional JSON output creates no dependency
on any consumer.

`share` writes numbers only: never what anyone wrote, a path or the
machine's name. It shares, as one record for the person under the
repository's own `refs/vessa/usage/`, the sessions recorded inside the
clone's worktrees, with their token counts, times, agent, model and branch
names and the person's git name and email. `team` reads only what was
shared, never anyone's session files.

No runtime dependency on any other repository. No database or daemon. Every
command works offline, except `share` and `team`: they use the network only
through git, only while they run, and only to push or fetch
`refs/vessa/usage/*`. opencode's SQLite store is read with the `sqlite3` command,
read-only; Vessa takes no SQLite dependency. Tests that make such a store
need `sqlite3` on PATH, and CI installs it. Never commit real transcripts or reports; use synthetic fixtures.
Validate with cargo fmt --check, cargo clippy --all-targets -- -D warnings,
and cargo test. Run the binary on a fixture before claiming completion.
`README.md` is the short introduction; detail lives in `docs/reference.md`.
Check every command and flag written in either against `vessa --help`.
The gate is `sykli run`, by this repository's own `sykli.json`: list a new
file in its tasks' inputs and run `sykli lock sykli.json`, or sykli serves
an old cached pass.
