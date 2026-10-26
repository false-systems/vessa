# Vessa

Local evidence for what agent sessions consume.
One Rust binary: native usage imports, spending reports, JSON exports
and a standalone HTML report. No database, daemon, JavaScript or
network calls. Original files stay untouched.

```sh
cargo build --locked
cargo run -- sessions --input /path/to/session.jsonl
cargo run -- sessions --headers --json
cargo run -- usage --input /path/to/session.jsonl
cargo run -- usage --by-agent --input /path/to/sessions/
cargo run -- usage --input /path/to/session.jsonl --json
cargo run -- status --input /path/to/session.jsonl --watch 5
cargo run -- stats --period month
cargo run -- stats --period year
cargo run -- report --input /path/to/session.jsonl --output /tmp/vessa.html
cargo run -- messages --since 7d --json
```

`--input` is repeatable and accepts a file or directory. Without it, Vessa
reads `~/.claude/projects`, `~/.codex/sessions`, and `~/.pi/agent/sessions`.
One of these that does not exist is an agent not installed here, and no gap.
Directories discover native `.jsonl` transcripts; select ohjaa `.json` exports
as explicit files so agent metadata JSON is not mistaken for a usage source.
Pass explicit paths for relocated agent directories or a bounded selection.
Use `--agent claude|codex|pi` or `--session NATIVE-ID` to filter results.
Missing `--input` paths, malformed records and unsupported formats are
reported, not silently treated as zero. Exit 4 means the usage evidence is partial: a source
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

### Claude subagents

Claude Code writes each subagent to its own transcript, in
`<session>/subagents/agent-<id>.jsonl` beside the session's own. Vessa names a
subagent `<session>/<agentId>`, with the session as its `parent`. A subagent's
usage is its session's usage:

* `--session ID` selects the session and its subagents; a subagent's own id
  selects it alone. A session whose main transcript was not read is not
  found, even when its subagents were. A transcript given with `--input`
  brings its `subagents/` directory along.
* A subagent whose requests its session already holds counts nothing twice
  and is not unknown: status says how many it shares.
* `status` and `receipt` total the session with its subagents. Status says how
  many it included and shows each one; its JSON `entries` are that total, with
  `main_entries` and a `subagents` breakdown beside them. Command windows stay
  the main transcript's.
* The receipt says which subagents it includes in `limitations`, since
  `agent-usage.v1` has no field for them.
* A transcript that says it launched a subagent whose transcript was not read
  is partial (exit 4): that usage is unknown, not zero.
* `stats` counts sessions, not transcripts: a subagent's usage counts its
  session, and subagents are counted apart in `subagents`.

A request is still counted once, however many transcripts hold it.

### Session headers

`sessions --headers` lists each session's header alone: what its transcript
says about itself, without its usage. A caller that needs only these, such as
`ohjaa report`, need not wait for every usage record to be read. With `--json`
it emits `vessa-session-headers.v1`:

```json
{
  "schema": "vessa-session-headers.v1",
  "sessions": [
    {
      "agent": "claude",
      "cwd": "/demo/widget",
      "entrypoint": "cli",
      "id": "claude-demo/subagent-demo",
      "originator": null,
      "parent": "claude-demo",
      "source": "/path/to/claude-demo/subagents/agent-subagent-demo.jsonl",
      "started_at": "2026-09-30T09:10:00Z"
    }
  ],
  "warnings": []
}
```

* `agent` is `claude`, `codex` or `pi`; `id` the native session id, with a
  Claude subagent as `<session>/<agentId>`.
* `parent` is the session it descends from: a Claude subagent's session, a
  Codex fork's `forked_from_id`, a pi `parentSession`. Null for none.
* `started_at` (RFC 3339) and `cwd` come from the header, else from the first
  record that has them; unknown is null and `""`.
* `source` is the transcript read; of copies of one session, the first path.
* `entrypoint` (Claude: `cli`, or `sdk-cli` for `claude -p`) and `originator`
  (Codex: `codex-tui`, or `codex_exec` for `codex exec`) say how a session was
  started, and are null when unknown or for other agents.

Each field is the session's field of the same name in `vessa-usage.v1`, found
by the same code, and the list holds the same sessions in the same order, with
the same warnings. `--agent` and `--session` select as they do for `sessions`.

The header reader reads a transcript only until its header is known, and keeps
no records. Each fact is the first record's that states it, so a transcript
that never states one, such as a Claude session that never names its
`entrypoint`, is read to its end. Two rules look at every record; here the
header reader relies on how the agents write: each opens its own transcripts
with its header and none holds another agent's records, and Claude Code marks
every record of a transcript alike as a sidechain or not. On 928 local
transcripts (3.2 GB, 2026-10-26) the two listings were identical; headers took
under 0.5 s and 8 MB of memory, the full listing over 20 s and 1.2 GB on a
loaded machine.

Where a file breaks those habits, or is damaged past its header, the two
listings differ, and tests pin how:

* A Claude transcript that says it is no sidechain, then that it is one: the
  full listing makes it a subagent of itself (`<id>/<agentId>`, with `<id>`
  as `parent`); headers list the session itself, with no parent.
* A file that holds records of two agents: the full listing takes the higher
  ranked header (Codex, then pi, then Claude), headers the first in the file.
* Damage after the header, such as invalid UTF-8: the full listing drops the
  transcript with a warning; headers list it, warning of nothing, since they
  never read that far. So of two copies of one session, when the first by
  path is damaged, headers give that copy's `started_at`, `cwd` and `source`,
  and the full listing the second copy's.

No usage is read, so headers carry no limitations, and exit 4 means only a
source warning: a transcript that could not be read. `--rates` is refused,
before or after `sessions`.

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
not a universal live billing API. Correction patterns belong to ohjaa intake,
which reads `messages`; Vessa does not call a whole session's spending waste.

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
  Claude Code also writes `<synthetic>` assistant messages of its own, with
  zero usage, where a request failed or was refused, some under that
  request's id. They are no request: each session counts them once in
  `synthetic_records`, and they are neither usage nor a conflict. One that
  reports usage is read as usage.
* Codex: differences cumulative input/output/cache counters, ignores repeated
  totals, and associates deltas with the current model context. Counter resets
  are flagged, with a new baseline, rather than inventing negative usage. The
  provider is the session's `model_provider`, else `openai`. Cache writes come
  from `cache_write_input_tokens`. That they are part of the inclusive input,
  like cached reads, is an **assumption**: every value seen so far (32,548
  counters, all OpenAI) is zero, so nothing confirms it. A non-zero value is
  reported as a session limitation (exit 4) until a provider that reports writes
  settles it. Counters that predate the field record zero cache writes.
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

## Messages

`messages` lists what people wrote in their agent sessions, each message with
the agent reply it follows: the evidence a lesson drafter such as `ohjaa
intake` reads corrections from. Vessa is the one reader of agent
conversations. It reads Claude Code, Codex and pi transcripts with the same
discovery and headers as its other commands, and keeps nothing: no cache, no
database. Text goes only to stdout, which inherits the permissions you give it.

```sh
cargo run -- messages --since 7d
cargo run -- messages --since 2026-10-01 --agent codex --json
cargo run -- messages --session NATIVE-ID --json
```

`--since` takes RFC 3339, a UTC date (`YYYY-MM-DD`) or an age (`7d`, `12h`),
and lists the messages after it: exclusive, as ohjaa intake windows
(`at_ms > since`), so a reader can pass the time of the last message it
handled and not get that message again. `--agent` and `--session` select as
they do elsewhere; a Claude subagent's own id selects it alone. With `--json`
it emits `vessa-messages.v1`, one message a line inside one JSON document,
each written as soon as its transcript is read, so memory holds one transcript
at a time:

```json
{"schema":"vessa-messages.v1","messages":[
{"agent":"claude","session":"claude-demo","parent":null,"kind":"message","id":"u2","at":"2026-09-30T09:02:00Z","cwd":"/demo/widget","text":"I already told you to run the local checks.","question":null,"reply":{"text":"The implementation is ready.","truncated":false,"failed":false,"line":3},"source":"/path/to/claude-demo.jsonl","line":4}
],
"sessions":[{"agent":"claude","id":"claude-demo","parent":null,"subagent":false,"started_at":"2026-09-30T09:00:00Z","cwd":"/demo/widget","source":"/path/to/claude-demo.jsonl","headless":false,"messages":3}],
"coverage":{"transcripts":1,"subagent_transcripts":0,"unmodified_transcripts":0,"user_turns":3,"human":3,"excluded":{},"since":null,"earlier":0,"undated":0,"gaps":[],"limitations":[]},
"warnings":[]}
```

* A message names its `agent` and native `session` (a Claude subagent's is
  `<session>/<agentId>`), the `parent` it descends from (a Claude subagent's
  session, a Codex `forked_from_id`, a pi `parentSession`), and the native
  `id` of its record: Claude's `uuid`, pi's `id`, and Codex's `payload.id`,
  which newer Codex versions write. Then `at`, `cwd` (the record's, or the
  turn's for Codex, else the session's), its `text` without the
  `<system-reminder>` blocks a harness writes into it, and the `source` and
  `line` it was read from.
* `kind` says what the words are. `message`: typed into the conversation.
  `queued`: typed while the agent worked, and handed to it mid-turn.
  `answer`: a person's own answer to the agent's question tool, with the
  `question` it answers. `rejection`: the reason a person gave for rejecting
  a tool call. `task`: the task a person typed into a launch prompt.
* `reply` is the agent turn it follows in its transcript: the last 600
  characters of its text, as ohjaa intake has taken them, with `truncated`
  and `line`. `failed` says the turn was a Claude API error, a Codex error
  event or a pi error stop: the message answers a failure, so "try again"
  there is a retry, not a correction. Null when no agent turn came before.
* `sessions` lists each transcript read, its copies as one session, with how
  many messages it gave. `headless` marks one whose every prompt a program
  sent; `subagent` a Claude subagent.
* `coverage` says what was read and why each other user turn is not listed:
  `user_turns` is `human` plus every count in `excluded` plus `earlier` and
  `undated` (with `--since`, a message at or before it, or with no time).
  Codex subagent transcripts and, with `--since`, transcripts last written
  before it are counted and not read. `gaps` names sources Vessa knows of and
  does not read. opencode keeps its sessions in SQLite
  (`~/.local/share/opencode/opencode.db`), and Vessa takes no SQLite
  dependency, so a default run where that store was written in the window is
  partial. SQLite writes through the store's `-wal` file, so the window looks
  at the newer of the two; one written before `--since` holds nothing after
  it, and is no gap.
* Exit 4 means the list is partial: a source warning, a gap, or a session
  limitation, each printed to stderr. Transcripts are read in path order, a
  Codex fork after the sessions it was forked from; messages in file order.
* A run that fails writes no partial document. One that fails before its
  first message leaves stdout empty; one that fails after ends the document
  with `"error"` in place of `sessions`, `coverage` and `warnings`, so its
  messages are only those read before the failure. Either way it exits 1.

### What counts as a person's message

Each rule names the user turns it excludes in `coverage.excluded`; a turn is
counted under the first that applies. The rules are structural: what the
record says about itself, or a harness text in an exact form, either a whole
text or the fixed sentence a harness always opens it with. They never match a
mere prefix of a person's words, so a person who starts a message with
"Caveat:", a path or a tag name is still a person. They come from Seula's
reader, which was the most careful, with ohjaa intake's where they are
stricter, and a few the archive called for:

| Rule | Excludes | Why | From |
| --- | --- | --- | --- |
| `request_record` | A Claude record of another type that carries a user message (`api-request-blob`) | It copies a request's content, without uuid or time | the archive |
| `other_session` | A Claude record naming another session; also a limitation | Not this session's | Seula |
| `tool_result` | A Claude user turn holding a tool result, even with text beside it, unless a person wrote inside it (below) | Claude Code returns tool output as user turns | intake; Seula took the text beside a result |
| `subagent` | In a Claude subagent's transcript, or as a sidechain record inside a main one, a turn whose origin is not a person | The prompts the parent agent wrote | intake |
| `meta` | Claude `isMeta`, unless its origin is a person | Skill expansions, peer messages, safety and system notices | Seula |
| `compaction` | Claude `isCompactSummary`, or a text opening with the fixed first sentence of its continuation summary | A summary of earlier turns | Seula, intake |
| `harness_origin` | Claude `origin.kind` other than `human`, or `promptSource: system`; a queued command whose origin is not a person | Claude Code says the harness wrote it: task notifications, peers, coordinators | the archive; Seula's envelopes caught the same turns by their text |
| `empty` | No text, as an image alone | Nothing to read | Seula |
| `envelope` | A message that is wholly known harness elements (startup context, notifications, slash-command records, interruptions, Codex's review, skill and plugin elements), Codex's `# AGENTS.md instructions` heading over them, the verbatim interruption notices and local-command caveat, pi's bare `/command` | The harness wrote it into a user turn | Seula, whose element set followed intake's |
| `harness_prompt` | A text opening with the fixed first sentence of Codex's `/init`, or of a toimija launch prompt that carries no task | A program wrote it in a person's place | the archive |
| `question_reply` | A Codex reply to its question tool whose every answer is one of the options the question offered | Picking an offered option says nothing in the person's own words; Claude's picks are in its `tool_result` | the archive |
| `fork_copy` | In a Codex fork, the longest run of turns after the copied parent header that matches the parent's transcript turn for turn; all of them, with a limitation, when the parent was not read | A fork copies its parent's transcript stamped with the fork time, so time cannot separate them | Seula |
| `inherited` | In a fork (Codex, pi), a turn from before the fork started; one with no time, with a limitation | Its parent's history | Seula |
| `headless` | Claude `sdk-*` entrypoints and `promptSource: sdk`, decided per turn; a `codex exec` file in which no turn was submitted interactively (a `user_message` event) | A program sent it: `claude -p`, an Agent SDK, `codex exec`. A session resumed by a script keeps its interactive turns | Seula |
| `duplicate` | A message another copy of the session already gave (the same native id, else the same time and text), and a queued message that later arrives as an ordinary one of the same text | Read twice, said once | Seula, by id |
| `retry` | The person's previous message sent again before the agent answered it, because the agent failed or never replied | Re-sent, not said anew | the archive |

What a person wrote is listed wherever the record keeps it apart from the
harness's words:

* **Writing to a subagent.** Claude Code lets a person write to a running
  subagent. It records the message in the subagent's transcript, marked meta,
  with a person's origin (`origin.kind: human`); that origin wins over `meta`
  and `subagent`. Claude subagent transcripts are read for these and listed
  under the subagent, with its session as `parent`. Nothing in a Codex
  subagent's transcript says a person wrote, so those are not read.
* **Typed while the agent works.** Claude Code records such a message as a
  `queued_command` attachment with a person's origin, and hands it to the
  agent mid-turn. It is listed as `queued`, once: when it also arrives later
  as an ordinary message, that one is listed instead.
* **Answers to the agent's question.** An answer that is one of the offered
  options is a pick, not the person's words. Claude Code's question tool
  returns its options with the answers (`toolUseResult.questions`, picks of
  several joined by ", "). Codex's reply names the `item_completed` that
  asked (`questionItemId`), whose `questions` carry the options. Any other
  answer is listed as `answer`, with its `question`, including an answer to
  a question Vessa did not see asked.
* **Rejecting a tool call.** Claude Code writes the reason after a fixed
  sentence that ends "To tell you how to proceed, the user said:". The reason
  is listed as `rejection`; a rejection with no reason stays a `tool_result`.
* **A task in a launch prompt.** toimija composes the launch prompt as the
  launcher's alerts, if any (`## Before anything else (from the launcher)`),
  the fixed framing and the session packet, then `\nTask: <task>\n`, then
  ohjaa's briefing as `\n<briefing>\n`. The briefing is parts joined by a
  newline, each opening with a sentence ohjaa always writes: `FLAG: `,
  `Validation here (sykli's word, …`, `STILL RUNNING: session `, `ORPHANED:
  session `, `WARNING from `, `Knowledge (ohjaa, knowledge-node.v1):`, `Last
  session here (`, `How this session works (ohjaa):`, `Sessions in the other
  worktrees of this clone at launch`, `System perimeter (derived by ohjaa at
  launch; …`. The task is the text between the last `Task:` line and the
  first of these after a blank line, listed as `task`; a launch prompt with
  no typed words left is a `harness_prompt`. A task containing a blank line
  followed by one of those sentences would end early. The transcript does
  not say who wrote the task: a person who ran `ohjaa run`, or an agent that
  launched the session for one.

Intake's 1,500-character limit is not applied: it is a correction heuristic,
and intake keeps it. Its prefix skip list is replaced by the exact forms above.

On the local archive (2026-10-26) `messages` listed 5,583 messages: claude
630, codex 4,649, pi 304, from 628 transcripts, 258 of them Claude
subagents', in 5.3 s and 129 MB; with `--since 1d`, in 0.6 s. Seula's reader
counted 5,797 typed human messages in the same folders earlier that day. Of
those, Vessa excludes the `request_record` copies (72), the harness's own
`/init` and launch prompts (150; from 4 of them it lists the typed task, and
24 hold only ohjaa's briefing), the question replies that only pick options
(43 of 63; the other 20 give 20 answers in the person's words) and one
`retry`. It also lists what Seula did
not read: 28 messages written to Claude subagents, 15 queued while the agent
worked, 4 own answers to Claude's question tool and 1 rejection reason. 2,701
of the Codex messages carry an id.

## Usage export

Seula, which read this export beside the conversations, was archived on
2026-10-26, and `messages` took over its reader. Usage exports, receipts and
reports carry no message text. No sibling crate or binary is needed.

```sh
vessa usage --input /path/to/session.jsonl --json > /tmp/usage.private.json
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
`synthetic_records` counts a Claude session's `<synthetic>` messages, read and
known to hold no usage; it is 0 for other agents.

`totals` stays each transcript's own, so summing it across sessions counts
every request once. Each session also names its Claude subagents in
`subagents` and sums itself with them in `totals_with_subagents`: the total to
show for one session. Without subagents the list is empty and the two totals
are equal. Subagents remain sessions of their own in the export.

A consumer joins by agent and native session ID only, never directory or time.
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
A Claude session's receipt includes its subagents and names them in
`limitations`. Ohjaa attaches it to an explicit wrapper session. Exported work, handover,
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
select narrower input directories, or list headers alone with
`sessions --headers`; streaming summaries can replace it when archive size
warrants that work. Unsupported schema versions, inherited fork
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
