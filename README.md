# Vessa

Local evidence for what agent sessions consume and how conversations unfold.
One Rust binary: native transcript imports, usage reports, conversation pattern
candidates and a standalone HTML report. No database, daemon, JavaScript or
network calls. Original files stay untouched.

```sh
cargo build --locked
cargo run -- sessions --input /path/to/session.jsonl
cargo run -- usage --input /path/to/session.jsonl
cargo run -- usage --by-agent --input /path/to/sessions/
cargo run -- patterns --input /path/to/session.jsonl --json
cargo run -- report --input /path/to/session.jsonl --output /tmp/vessa.html
```

`--input` is repeatable and accepts a file or directory. Without it, Vessa
reads `~/.claude/projects`, `~/.codex/sessions`, and `~/.pi/agent/sessions`.
Pass explicit paths for relocated agent directories or a bounded selection.
Use `--agent claude|codex|pi` or `--session NATIVE-ID` to filter results.
Missing paths, malformed records and unsupported formats are reported, not
silently treated as zero. Exit 4 means the output is partial; exit 1 means
the requested operation failed. Use the emitted limitations to judge coverage.

## Usage evidence

Agent, model and provider are separate fields. Input excludes cached input;
output includes reasoning tokens where the producer includes them, so Vessa
never adds a reasoning subcategory a second time.

* Claude Code: deduplicates request IDs across repeated assistant content blocks.
  Provider is `unknown` when the transcript does not identify it; a model name
  does not prove which service billed the request. Conflicting usage snapshots
  are flagged and the first is retained; the result is partial.
* Codex: differences cumulative input/output/cache counters, ignores repeated
  totals, and associates deltas with the current model context. Counter resets
  are flagged, with a new baseline, rather than inventing negative usage.
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
Keep plan multipliers, discounts and service tiers out of this simple API-rate
estimate unless the supplied rate actually represents them.

## Conversation patterns

`patterns` identifies repeated normalized user text and likely corrections
using explicit English phrases. Every candidate links to native message IDs
and source lines. Subagent conversations are excluded from human-pattern
analysis. A repeated message is not proof of waste, and an agent saying
"done" is not verified completion. Read the surrounding conversation.

This first implementation is deliberately deterministic. Jev is not connected;
no conversations are sent to TypeSafe or another model. Semantic classification
needs evaluation on labelled examples before its findings should guide work.
Kide's source attribution, versioning and uncertainty principles inspired the
design; Vessa neither links Kide nor changes its existing contract.

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
knowledge and outcome evidence appears alongside the conversation only when
that explicit native-session association exists. Directory and time-window
guesses never establish identity. Unmatched wrapper records are reported.
Receipt totals and native totals are not added together.

With `ohjaa run --usage-file /fresh/receipt.json`, a launched wrapper can write
the receipt to `OHJAA_USAGE_FILE` before exiting and ohjaa imports it at leaving.
Native agents do not emit this normalized receipt by themselves.

## UI and privacy

The HTML is a local, standalone report. Native links, disclosure controls and
browser Find provide navigation without a server or frontend dependencies.
All transcript content is escaped, executable content is blocked by CSP, and
new reports use mode 0600 on Unix. Existing report paths are never overwritten.
Reports contain private conversations: generated files under `reports/` are
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
