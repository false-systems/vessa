# Vessa

Local evidence for what agent sessions consume.
One Rust binary: native usage imports, spending reports, JSON exports
and a standalone HTML report. No database, daemon, JavaScript or
network calls. Original files stay untouched.

```sh
cargo build --locked
cargo run -- sessions --input /path/to/session.jsonl
cargo run -- usage --input /path/to/session.jsonl
cargo run -- usage --by-agent --input /path/to/sessions/
cargo run -- usage --input /path/to/session.jsonl --json
cargo run -- status --input /path/to/session.jsonl --watch 5
cargo run -- stats --period month
cargo run -- stats --period year
cargo run -- report --input /path/to/session.jsonl --output /tmp/vessa.html
```

`--input` is repeatable and accepts a file or directory. Without it, Vessa
reads `~/.claude/projects`, `~/.codex/sessions`, and `~/.pi/agent/sessions`.
Directories discover native `.jsonl` transcripts; select ohjaa `.json` exports
as explicit files so agent metadata JSON is not mistaken for a usage source.
Pass explicit paths for relocated agent directories or a bounded selection.
Use `--agent claude|codex|pi` or `--session NATIVE-ID` to filter results.
Missing paths, malformed records and unsupported formats are reported, not
silently treated as zero. Exit 4 means the usage evidence is partial: a source
warning or a session limitation, each printed to stderr, and a last line that
counts them. Rate-card notices do not set it. Exit 1 means the requested
operation failed. Use the emitted limitations to judge coverage.

## Session display and statistics

`status` shows the latest recorded usage for exactly one explicit session.
It requires `--input` or `--session`, never selects the most recently modified
file, and includes the last usage timestamp so an old transcript is not
mistaken for a live balance. `--watch SECONDS` refreshes in the foreground;
Ctrl-C stops it. `--samples N` bounds a watch for checks. Watch also works with
`usage` and `stats`; each JSON refresh is a separate JSON document.

Status, HTML reports and usage JSON also show **command and change insights**.
A command is a recorded user message and the usage up to the next user message;
message bodies and tool arguments/output are never retained. Injected context
can also appear as user messages. Each command shows usage, estimates, tool
counts and elapsed time through its latest usage record. Initial request usage
is shown separately but remains included in totals; logs do not isolate the
cost of starting context from that request's generation.

A change is an explicitly successful `apply_patch`, `Edit`, `Write`, `MultiEdit`,
`edit` or `write` call. Its usage window starts after the previous successful
edit result, or at command start, and ends at its result. These windows are
included in command totals, not additional charges or causal allocation.
Edits hidden inside shell commands or orchestration tools remain unmeasured;
these are recorded operations, not verified surviving diffs or commits.

**Estimated waste** is a low-confidence range: zero to the priced spending
in a command with a recorded failed tool. A failure may be useful diagnosis,
so the upper bound is not a claim that the spending was wasted. Missing prices
or failure evidence leave waste unknown, not zero. No prompt classification
or sibling product is involved. Source lines remain available in the JSON
`insights` field and report. Try the synthetic example:

```sh
cargo run -- status --input examples/work.jsonl
```

`stats --period day|week|month|year` groups usage events by their own timestamp,
agent and model. Calendar periods are UTC; weeks use ISO week years. A session
crossing a period boundary appears in both periods, with only that period's
usage. Missing/invalid timestamps remain in an `undated` group. Unpriced usage
is counted explicitly alongside a **known estimate subtotal**, not priced at
zero. JSON emits `vessa-statistics.v1`; HTML reports include all four periods.
Default scans cover selected local transcript files, not every account/device.

Claude Code can call the Rust binary directly in `~/.claude/settings.json`:

```json
{"statusLine":{"type":"command","command":"~/.cargo/bin/vessa claude-statusline"}}
```

This reads Claude's native session identity and running estimate on stdin and
prints the session estimate first, separately from unknown actual billing.
A second line shows the last API call's input, output, cache-read and
cache-write tokens. These are not cumulative session counts; absent categories
remain unknown, including before the first response or after compaction. It makes
no model calls and displays no budgets, quotas or limits. Preserve any existing
status-line customization when installing. See the official
[Claude status-line contract](https://code.claude.com/docs/en/statusline).

Codex's native `tui.status_line` supports input/output token counters and,
where supplied by the account, estimated thread cost and thread credits. A
missing account cost is not fabricated from tokens. Use `vessa status` with
an explicit transcript and dated `--rates` for an external priced view. Pi
already shows accumulated usage/cost in its footer and `/session`; its cost is
still an estimate. Native display support differs by agent/account; this is
not a universal live billing API. Seula owns repeated-correction patterns and
their calendar counts; Vessa does not call a whole session's spending waste.

In Codex, use `/statusline` to move spending/token fields before long model and
session labels; `/status` opens the native usage details. The footer supports
native fields, not a Vessa shell-command renderer. Keep account-provided cost
distinct from Vessa's rate-card estimate. In Pi, use its existing footer and
`/session` rather than installing a second token counter. Vessa's full command
breakdown remains available in its reports; it is not injected into agent prompts.

## Usage evidence

Agent, model and provider are separate fields. Input excludes cached input;
output includes reasoning tokens where the producer includes them, so Vessa
never adds a reasoning subcategory a second time.

* Claude Code: deduplicates request IDs across repeated assistant content blocks.
  Claude Code writes one line per content block, and in subagent transcripts
  the output count grows across the lines of one request; when everything else
  agrees, the largest snapshot is kept and its line is the evidence. Snapshots
  that disagree in any other way (input or cache counts, model) are flagged and
  the first is retained; the result is partial. Claude Code transcripts never
  name a provider, and Anthropic's API, Amazon Bedrock, Google Vertex AI and
  Anthropic-compatible gateways write the same shape. The provider comes from
  the response ids: `msg_01…` or a `req_…` request id is `anthropic`,
  `msg_bdrk_…` is `bedrock`, `msg_vrtx_…` is `vertex`, and anything else is
  `unknown`. A model name proves nothing about who served the request.
* Codex: differences cumulative input/output/cache counters, ignores repeated
  totals, and associates deltas with the current model context. Counter resets
  are flagged, with a new baseline, rather than inventing negative usage. The
  provider is the session's `model_provider`, else `openai`. Cache writes come
  from `cache_write_input_tokens`, part of the inclusive input like cached
  reads; counters that predate the field record zero cache writes.
* Pi: reads assistant, compaction, branch-summary and standalone usage entries.
  Recorded costs are estimates. Copied entry IDs across forks are counted once,
  with parent sessions processed first when available.

Unknown counters and prices stay `null`. Agent-reported USD estimates,
provider-specific credits, and billed currency amounts are different fields.
No subscription bill is allocated to sessions implicitly. Native transcript
imports do not establish actual plan consumption or invoice charges.

`--rates FILE` accepts a JSON array of dated rate cards. See
[`examples/rates.json`](examples/rates.json) for **synthetic** examples; these
are not provider prices. Rates use integer USD millionths per million tokens,
and require provider/model, an RFC3339 validity interval, and a source.
Unknown token categories prevent repricing. Overlapping applicable rates fail.
A card that prices no selected usage record is named in a warning, since it
usually means a provider, model or date mismatch; pricing notices do not make
the output partial (exit 4).
Keep plan multipliers, discounts and service tiers out of this simple API-rate
estimate unless the supplied rate actually represents them.

## Seula handoff

Conversation analysis is now a separate product, Seula. Vessa no longer has a
`patterns` command, retains no native message bodies and does not render chats.
Each product builds and runs independently. No sibling crate or binary is needed.

```sh
vessa usage --input /path/to/session.jsonl --json > /tmp/usage.private.json
seula report --input /path/to/session.jsonl --usage /tmp/usage.private.json --output /tmp/conversations.html
```

`usage --json` and `sessions --json` emit `vessa-usage.v1`: `sessions` and
`warnings`. Each session includes explicit `agent` and native `id`, source,
lineage, limitations and request-level `usage`. Each usage record preserves
source, line, time and its token/money `entry`. Optional `ohjaa` records are
explicitly linked wrapper context. This replaces the initial local
`vessa-report.v1` export; consumers must check the schema. `--by-agent --json`
is an aggregate view, not this exchange format.

Each session also exports `totals`, calculated by Vessa from deduplicated
records and grouped by provider/model. Consumers can display these beside work
evidence without implementing accounting. Unknown categories remain null; older
exports without this additive field still contain the original usage records.

Seula joins by agent and native session ID only, never directory or time.
Whole-session spending does not establish spending caused by a conversation
pattern. Incomplete source coverage remains visible in the export and exit code.

## Ohjaa handoff

The programs exchange files; neither links or launches the other.

```sh
vessa receipt --input /path/to/session.jsonl > /tmp/usage.json
ohjaa usage import /tmp/usage.json --session oh-EXISTING-ID
ohjaa usage export > /tmp/ohjaa.json
vessa report --input /path/to/session.jsonl --input /tmp/ohjaa.json --output /tmp/work.html
```

`receipt` requires exactly one native session and emits `agent-usage.v1`.
Ohjaa attaches it to an explicit wrapper session. Exported work, handover,
knowledge and outcome evidence appears alongside the usage only when
that explicit native-session association exists. Directory and time-window
guesses never establish identity. Unmatched wrapper records are reported.
Receipt totals and native totals are not added together.

With `ohjaa run --usage-file /fresh/receipt.json`, a launched wrapper can write
the receipt to `OHJAA_USAGE_FILE` before exiting and ohjaa imports it at leaving.
Native agents do not emit this normalized receipt by themselves.

## UI and privacy

The report starts with the known spending subtotal, priced/unpriced record
coverage and sessions ranked by that subtotal. Rankings are not complete cost
comparisons when prices are missing. Expand a command to see separate token
categories, tool/failure/edit counts and the source location; full evidence is
still available below it. No prompt bodies are retained to label commands.

The report also explains the existing Ohjaa/Toimija workflow for reducing
repeated discovery: launch once through `ohjaa run` (which invokes Toimija),
use the delivered workset and handover, and refresh only when facts become
stale. Use the receipt/export commands above to associate usage with an
explicit Ohjaa session. These remain optional file exchanges, not runtime
dependencies. Wrapper association does not prove savings: compare similar
tasks, model/rates, token categories and verified outcomes before making that
claim. Do not remove safety instructions or validation to lower usage.

The HTML is a local, standalone report. Native links, disclosure controls and
browser Find provide navigation without a server or frontend dependencies.
All displayed source content is escaped, executable content is blocked by CSP, and
new reports use mode 0600 on Unix. Existing report paths are never overwritten.
Reports contain private usage and optional linked ohjaa context: files under `reports/` are
ignored by Git. JSON printed to stdout inherits the destination permissions
you choose. No knowledge becomes current automatically.

The in-memory reader is suitable for selected sessions. For very large archives,
select narrower input directories; streaming summaries can replace it when
archive size warrants that work. Unsupported schema versions, inherited fork
history without its original source, model rerouting absent from logs, and
unlogged requests limit what a local transcript can establish.

## Check it

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run -- report --input examples/claude.jsonl --input examples/codex.jsonl --input examples/pi.jsonl --rates examples/rates.json --output /tmp/vessa-demo.html
```

The fixtures are synthetic. No real transcripts, credentials, rate assumptions,
or generated reports belong in this repository.
