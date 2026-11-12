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

The commit hook (`vessa hook prepare-commit-msg`) writes pointers only: one
`Agent-Session` trailer per session of the clone, placed by `share`'s rule,
active since the commit's parent, holding the agent, the agent's own
session id, how many messages people typed then (by `messages`' rules) and
when, to the minute. Never message text, a path, a user or machine name. It
never stops a commit: on any error it adds nothing, says so on stderr and
exits 0. `messages --commit` lists the words, on this machine only.

`share` writes numbers only: never what anyone wrote, a path or the
machine's name. It adds, under the repository's own `refs/vessa/usage/`,
the person's sessions of the repository that are new or changed: those
recorded inside the clone's worktrees, or whose structured tool calls name
a file in them (never free text; those paths stay on the machine), with
their token counts, times, agent, model and branch names and the person's
git name and email. A share never rewrites an earlier one; the ref's
history is the record. `team` reads only what was shared, never anyone's
session files.

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
