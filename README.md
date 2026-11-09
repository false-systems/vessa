# Vessa

Local evidence for what agent sessions consume.
One Rust binary: native usage imports, token summaries, JSON exports
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
reads `~/.claude/projects`, `~/.codex/sessions`, `~/.pi/agent/sessions`, and
opencode's store (`$XDG_DATA_HOME/opencode/opencode.db`, else
`~/.local/share/opencode/opencode.db`; see [opencode](#opencode)).
One of these that does not exist is an agent not installed here, and no gap,
unless `--agent` asks for that agent: then it is a warning (`codex: not
installed here`), since what was asked for cannot be read.
Directories discover native `.jsonl` transcripts; select a wrapper's
`ohjaa-sessions.v1` `.json` export as an explicit file, so agent metadata JSON
is not mistaken for a usage source.
Pass explicit paths for relocated agent directories or a bounded selection.
Use `--agent claude|codex|pi|opencode` or `--session NATIVE-ID` to filter results.
Missing `--input` paths, malformed records and unsupported formats are
reported, not silently treated as zero. Exit 4 means the usage evidence is
partial: a source warning or a session limitation, each printed to stderr, and
a last line that counts them. Exit 1 means
the requested operation failed. Use the emitted limitations to judge coverage.

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
  found, even when its subagents were. A main transcript some of whose
  records say they are a sidechain, and others that they are not, is read as
  a subagent's, and the error says so. A transcript given with `--input` brings its `subagents/`
  directory along.
* A subagent whose requests its session already holds counts nothing twice
  and is not unknown: status says how many it shares. One with requests that
  an unrelated session holds too is partial, whether those are all of its
  requests or some: they were counted once, with that session, and its own
  session's total leaves them out.
* `status` and `receipt` total the session with its subagents. Status says how
  many it included and shows each one; its JSON `entries` are that total, with
  `main_entries` and a `subagents` breakdown beside them (see
  [`vessa-status.v2`](#vessa-statusv2)). Command windows stay the main
  transcript's.
* The receipt says which subagents it includes in `limitations`, since
  `agent-usage.v1` has no field for them: how many, each by id up to ten, and
  past ten the first ten in id order with a count of the rest, which
  `vessa status --session ID` lists.
* A transcript that says it launched a subagent whose transcript was not read
  is partial (exit 4): that usage is unknown, not zero. A launch record can
  also be a copy of another session's, carried in this one's transcript under
  its id. Only the record proves it: when another session's transcripts
  hold the very same record (its `uuid`, or the tool call it answers) and
  the subagent it names was read as that session's, it ran there, and
  nothing is missing here. The transcript then holds that session's
  history, so the session it copies is counted first and the requests both
  hold are the original's, whichever id sorts first. A subagent of the same
  id in another session proves nothing; nor does the same record held by
  sessions none of which, or several of which, read the subagent. Those
  launches stay partial, and the limitation names the other sessions that
  hold the record.
* `stats` counts sessions, not transcripts: a subagent's usage counts its
  session, and subagents are counted apart in `subagents`.

A request is still counted once, however many transcripts hold it.

### `vessa-status.v2`

`status --json` emits one session:

* `agent`, `session_id`, and `last_recorded_at`, the time of the latest
  usage record of the session or its subagents.
* `entries`: the session's deduplicated usage **with its subagents'**, by
  provider and model. Until vessa#4 (2026-10-26) it was the main transcript's
  alone; output that has `main_entries` has this meaning.
* `main_entries`: the main transcript's own usage, what `entries` was before.
* `subagents`: each subagent's `id`, `source`, `last_recorded_at` and own
  `entries`. Empty for none, and then `entries` equals `main_entries`.
* `insights` (the main transcript's command windows), `limitations` (the
  session's, then each subagent's, named) and `warnings`.

The token-only status format is v2; v1 also carried spending fields.

### Codex subagents

Codex writes each subagent, and each other thread a session spawns (such as a
review), to a transcript of its own, under its own thread id. Its
`session_meta` says it is a subagent (`source.subagent`) and names the session
at its root (`session_id`; older Codex writes only the thread it came from,
`parent_thread_id` or `forked_from_id`, which Vessa follows through the threads
read to the root). A lineage that loops has no root: each thread in it stands
alone, with a limitation. Vessa keeps the thread id and its
`parent`, and counts it to that root session by the rules above: `--session`
selects the session with its subagents, `status` and `receipt` total them
(the receipt names them in `limitations`), `stats` counts the session once and
its subagents apart, and the usage export names them in `subagents`. A thread
whose `session_meta` names no other session stands alone.

### opencode

opencode keeps every session in one SQLite database. Vessa takes no SQLite
dependency: it reads the store with the `sqlite3` command (3.33 or newer,
for `-json`), read-only and with `-init /dev/null`, so a person's
`~/.sqliterc` cannot change what it prints; it keeps nothing. A store is a
file given with `--input` that is a SQLite database or ends in `.db`, a file
named `opencode.db` in a folder given with `--input`, or the default one,
which is read unless `--agent` asks for another agent. An `XDG_DATA_HOME`
that is empty or relative is ignored, as the XDG specification says. Vessa
reads opencode's current layout, the `session_v2` and `session_message`
tables; a store in any other (a missing table or column, or a file that is
no SQLite database) is a source warning, never an empty store, and so is a
store when `sqlite3` is not on `PATH` or cannot print JSON. Older opencode
kept sessions in a `session` table, which newer opencode copies into
`session_v2`; one it has not copied is a warning, and not read.

* A session is a row of `session_v2`: its `id`, its `directory` for `cwd`,
  and its creation for `started_at`; `source` is the store. A message's
  `seq` stands for its line. Warnings and gaps about one session's messages
  name the session, since every session counts from 0.
* Usage: each `assistant` message, and each `compaction` (the summary
  opencode asks a model for), that records tokens is one request, under its
  message id, read for the whole store in one query. opencode counts
  reasoning apart from output; Vessa counts it as output, as Codex's
  `output_tokens` does. Input is uncached, as opencode keeps it. A response
  that records no tokens at all, such as an error before any, is no request:
  a known record with no usage, counted in `synthetic_records`. opencode's
  `cost` is not read.
* A session with a `parent_id` is a subagent: opencode keeps one as a session
  of its own. It counts to the session at the top of its chain, followed as
  [Codex subagents](#codex-subagents)' lineages are; a chain that loops has
  no top, and each session in it stands alone, with a limitation.
* A session with a `fork_session_id` is a fork, and that session is its
  `parent`, as a Codex fork's is. opencode copies the messages of the
  session it was forked from up to the fork into it, each under a new id but
  with its `seq`, creation time and data. A message of the fork whose origin
  holds one at its `seq` created at the same time is a copy: a copied
  response counts under the id of the one it copies, so it is counted once,
  with the origin, and the fork's own total leaves it out. A copied message
  of a person's was created before the fork, and is `inherited`. A fork
  whose origin is no longer in the store says that the messages it copied
  are listed nowhere.
* Messages: only `user` messages can be a person's. opencode keeps what its
  harness adds (instruction files it loaded, background shell results,
  restarts, instruction updates) as `synthetic` and `system` messages,
  which are no turns. An agent reply is the text blocks of an `assistant`
  message; one that ended in an error is `failed`, its error message
  standing for its text when it has none. A person can type into a
  subagent session, so it is read, as a Claude subagent's transcript is. In
  one, a `user` message is the prompt an agent wrote (`subagent`) when it
  opens with "You are a subagent spawned by another session.", as opencode
  opens each prompt its subagent tool or a command run as a subtask writes,
  or when it has neither `files` nor `agents`: opencode's prompt box sends
  both with whatever a person types, and its subagent tool neither.
  Messages migrated from older opencode keep those only when not empty, so
  in a migrated subagent session what a person typed may count as a prompt.
  With `--since`, a session whose newest message is not after it is counted
  in `unmodified_transcripts` and not read.
* Secrets: a `user` message is `person`; an assistant's text and reasoning
  blocks are `agent`; a tool block's `state.input` is `tool_input` and every
  other field of its `state` is `tool_output`; everything else is `other`.
  A message whose data is not JSON is a gap, and still scanned as text.

### Session headers

`sessions --headers` lists each session's header alone: what its transcript
says about itself, without its usage. A caller that needs only these need not
wait for every usage record to be read. With `--json` it emits
`vessa-session-headers.v1`:

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

Each field is the session's field of the same name in `vessa-usage.v2`, found
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
  ranked header (Codex, then pi, then Claude), headers the first in the file
  that names its session.
* Malformed records after the header, including invalid UTF-8: the full
  listing preserves valid records and reports the damaged lines as limitations;
  headers list the session without limitations, since they never read that
  far. Both modes keep the first copy's header when copies are merged.

No usage is read, so headers carry no limitations, and exit 4 means only a
source warning: a transcript that could not be read.

Status, HTML reports and usage JSON also show **command and change insights**.
A command is a recorded user message and the usage up to the next user message;
message bodies and tool arguments/output are never retained. Injected context
can also appear as user messages. Each command shows token usage, tool
counts and elapsed time through its latest usage record. Where that time is
unknown it says why: no usage was recorded in the command, the command or
its usage records have no timestamp, or its last usage record is
timestamped before it. Initial request usage
is shown separately but remains included in totals; logs do not isolate the
tokens used by starting context from that request's generation.

A change is an explicitly successful `apply_patch`, `Edit`, `Write`, `MultiEdit`,
`edit` or `write` call. Its usage window starts after the previous successful
edit result, or at command start, and ends at its result. These windows are
included in command totals, not additional usage or causal allocation.
Edits hidden inside shell commands or orchestration tools remain unmeasured;
these are recorded operations, not verified surviving diffs or commits.

```sh
cargo run -- status --input examples/work.jsonl
```

`stats --period day|week|month|year` groups usage events by their own timestamp,
agent and model. Calendar periods are UTC; weeks use ISO week years. A session
crossing a period boundary appears in both periods, with only that period's
usage. Missing/invalid timestamps remain in an `undated` group. Each group
shows token totals and the number of usage records. Unknown token categories
stay null. JSON emits `vessa-statistics.v2`; HTML includes all four periods.
Default scans cover selected local transcript files, not every account/device.

Claude Code can call the Rust binary directly in `~/.claude/settings.json`:

```json
{"statusLine":{"type":"command","command":"~/.cargo/bin/vessa claude-statusline"}}
```

This reads Claude's native session identity and the last API call's input,
output, cache-read and cache-write token counts from stdin. These are not
cumulative session counts. Absent, negative or mistyped counts remain unknown.
Invalid JSON shows every field as unknown; the status line always prints and
exits 0. It makes no model calls and displays no spending, budgets, quotas or
limits. Preserve existing status-line customization when installing. See the
[Claude status-line contract](https://code.claude.com/docs/en/statusline).

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
  `synthetic_records`, and they are neither usage nor a conflict. A session
  whose only answers were synthetic recorded no usage, so it is not unknown.
  One that reports usage is read as usage.
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
  Copied entry IDs across forks are counted once,
  with parent sessions processed first when available.
* opencode: reads assistant and compaction responses (see [opencode](#opencode)).
  opencode's output count leaves reasoning out, so Vessa adds it once.

Unknown token counters stay `null`. Spending is not imported, calculated or
displayed: no prices, rate cards, currency estimates, credits or billing fields.
Legacy cost fields in native transcripts and linked wrapper receipts are ignored.
`--rates` is no longer accepted.

## Secret-shaped findings

`vessa secrets` scans native Claude Code, Codex, pi and opencode records on demand,
including subagent and headless transcripts, user/assistant text, tool inputs
and tool outputs, for **the credentials this machine holds**. A finding is
an alarm: one of your credentials reached an agent session, and so the
provider that ran it. Nothing is stored; no network is used.

The credentials are read at the start of each scan, kept in memory only,
compared by value and never printed, logged or written: the report names
each source (`compared_with`: its name, `read`, `absent` or `unreadable`,
and how many credentials it held), never a value. The sources are the
environment's secret-named variables (a name with `TOKEN`, `SECRET`,
`PASSWORD`, `PASSWD`, `KEY`, `CREDENTIAL` or `AUTH`, but not a socket, path,
file, folder or session key), the GitHub CLI's login (`gh auth token`, when
gh is on PATH), and `~/.aws/credentials`, `~/.config/gh/hosts.yml`,
`~/.git-credentials`, `~/.netrc`, `~/.npmrc`, `~/.docker/config.json`,
`~/.codex/auth.json`, `~/.pi/agent/auth.json`, `~/.claude/.credentials.json`
and `~/.ssh/id_{ed25519,rsa,ecdsa}`. From a file, a credential is each
secret-shaped string the detector recognizes in it; from a variable or gh,
also the whole value (16 bytes or more), whatever its shape. Each is looked
for in every record as it stands, alone, so it is found however it is
labelled there. macOS Keychain items are not read, since reading one can
prompt. A source that exists but cannot be read leaves the scan partial.

Every other secret-shaped string the detector finds (the same detector as
message redaction) is counted by its rule in `candidates`, never listed:
other people's keys, test fixtures, examples, signatures, ids. Of those,
`placeholders` counts the plain ones: a few characters repeated
(`sk-a1a1a1…`), a straight run of the alphabet or digits, or an AWS
documentation example (`…EXAMPLE`). Rule names describe the evidence:
recognised key formats take precedence, then explicit secret labels
(`password`, `api_key`, `token`, and similar words), then the generic long
mixed-string heuristic. A finding's `rule` is the one the detector gives the
credential where it stands, `no_known_shape` where it gives none, and its
`held` names the source that holds it.

```sh
cargo run -- secrets --input examples/codex.jsonl --json
cargo run -- secrets --since 7d --agent claude
```

The `vessa-secrets.v1` document carries `complete`, `since`, `transcripts`,
`findings`, `unreadable`, `compared_with`, `candidates` and `placeholders`.
A report without `compared_with` is from an earlier vessa, whose findings
were every secret-shaped match, not held credentials. Each finding contains
the native `agent` and `session`, optional `cwd` and `at` (with
`at_inferred`), source path and line, `where` (`person`, `agent`,
`tool_input`, `tool_output`, or `other`), detector `rule`, `held` and `count`.
For opencode, the source is its store and the line is the message's `seq`.
`person` denotes a native user-role input, not independent verification that
a human authored it. Counts are matched occurrences in selected native
records, not unique credentials; mirrored or copied records can repeat an
occurrence. Canonical source paths are read only once per invocation.
Each finding also reports `opaque_count`: a measured subset of `count` in
native Claude thinking-signature or Codex encrypted-reasoning fields. These
fields are still scanned, including for provider-shaped keys; their context
is not an exemption or a claim that their contents are safe. Older producers
may omit this field, which means unknown, not zero. Retained matches from
damaged records carry null context because their native fields could not be
decoded; summaries keep known counts as lower bounds.
`opaque_native: true` explicitly verifies that positive opaque subsets were
classified at native transport positions: Claude assistant thinking blocks
or Codex response-item reasoning payloads. User blocks, tool content, unknown
records and merely matching field names do not gain that context. Older
reports without this marker are unverified; they must not be used to dismiss
credential alerts. Detection and redaction still include every occurrence.
`undated_records` counts readable finding records that produced a timestamp
coverage warning under `--since`; it is not a count of unreadable files.
Known file-history snapshots use their nested native timestamp. A record
with no valid time of its own takes the time of the record before it in its
transcript, which is appended to, and says so (`at_inferred: true`); under
`--since` that places it only when that time is inside the window. Without
a record before it, or after one at or before `--since`, its time stays
unknown and the scan partial.
No value, message text or fingerprint is emitted. Metadata is scrubbed too,
including native IDs and path components. There is no `--raw` mode.

`--since` has the same syntax and exclusive boundary as `messages`.
Findings that stay undated are visible as `at: null`; with a time filter
they make the scan partial rather than silently falling outside the window. Damaged
JSON/UTF-8 records, unreadable paths, identity failures, missing selected
sessions, unsupported exports and known unread OpenCode stores are reported
in `unreadable` with `complete: false` and exit 4. Valid records around a
damaged record are still scanned; detectable damaged-record occurrences
have unknown time and location kind. Unknown record shapes are scanned as
`other`, not silently ignored. A complete scan means its selected sources
were read, not that the detector can recognize every possible secret.

`--session` selects native sessions and their known subagents. A subagent
alone does not establish coverage for its unread parent. Normal input
discovery and symlink refusal apply. Plain output gives the same metadata
and coverage status; JSON is suitable for an optional downstream consumer.

## Messages

`messages` lists what people wrote in their agent sessions, each message with
the agent reply it follows: the evidence a lesson drafter reads corrections
from. Vessa is the one reader of agent conversations. It reads Claude Code,
Codex and pi transcripts with the same discovery and headers as its other
commands, and keeps nothing: no cache, no database. Text goes only to stdout,
which inherits the permissions you give it.

Malformed JSON records, including invalid UTF-8, are skipped without losing
valid records elsewhere in the transcript. Usage and message readers report
these gaps with their original line numbers and exit 4 (partial evidence);
the header-only reader skips them while looking for a valid session header.

```sh
cargo run -- messages --since 7d
cargo run -- messages --since 2026-10-01 --agent codex --json
cargo run -- messages --session NATIVE-ID --json
```

`--since` takes RFC 3339, a UTC date (`YYYY-MM-DD`) or an age (`7d`, `12h`),
and lists the messages after it: exclusive (`at_ms > since`), so a reader can
pass the time of the last message it handled and not get that message again.
`--agent` and `--session` select as they do elsewhere; a Claude subagent's own
id selects it alone. With `--json` it emits `vessa-messages.v1`, one message a
line inside one JSON document, each written as soon as its transcript is read,
so memory holds one transcript at a time:

```json
{"schema":"vessa-messages.v1","messages":[
{"agent":"claude","session":"claude-demo","parent":null,"kind":"message","id":"u2","at":"2026-09-30T09:02:00Z","cwd":"/demo/widget","text":"I already told you to run the local checks.","question":null,"reply":{"text":"The implementation is ready.","truncated":false,"failed":false,"line":3},"source":"/path/to/claude-demo.jsonl","line":4}
],
"sessions":[{"agent":"claude","id":"claude-demo","parent":null,"subagent":false,"started_at":"2026-09-30T09:00:00Z","cwd":"/demo/widget","source":"/path/to/claude-demo.jsonl","headless":false,"messages":3}],
"coverage":{"transcripts":1,"subagent_transcripts":0,"unmodified_transcripts":0,"user_turns":3,"human":3,"excluded":{},"since":null,"earlier":0,"undated":0,"gaps":[],"limitations":[]},
"warnings":[],
"redacted":0,
"redacted_by_rule":{}}
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
* `reply` is the agent turn it follows in its transcript: at most the last
  600 characters of its text, from the start of a word, with `truncated` and
  `line`. The turn is scrubbed of secrets whole before it is cut (see below). `failed` says the turn
  was a Claude API error, a Codex error event or a pi error stop: the message
  answers a failure, so "try again" there is a retry, not a correction. Null
  when no agent turn came before.
* `sessions` lists each transcript read, its copies as one session, with how
  many messages it gave. `headless` marks one whose every prompt a program
  sent; `subagent` a Claude subagent.
* `coverage` says what was read and why each other user turn is not listed:
  `user_turns` is `human` plus every count in `excluded` plus `earlier` and
  `undated` (with `--since`, a message at or before it, or with no time).
  Codex subagent transcripts (`subagent_transcripts`) and, with `--since`,
  transcripts last written before it are counted and not read; an opencode
  store holds every session, so its sessions are placed by their newest
  message instead. `gaps` names sources Vessa knows of and does not
  read; every source it knows of is now read, so it stays empty, and remains
  for consumers of the schema.
* Exit 4 means the list is partial: a source warning, a gap, or a session
  limitation, each printed to stderr. Transcripts are read in path order, a
  Codex fork after the sessions it was forked from; messages in file order.
* A run that fails writes no partial document. One that fails before its
  first message leaves stdout empty; one that fails after ends the document
  with `"error"` in place of `sessions`, `coverage` and `warnings` (and
  before `redacted`), so its messages are only those read before the
  failure. Either way it exits 1.

### Secrets are redacted

What people paste to agents, and what tools print back, includes
credentials. Every secret-looking token in what `messages` prints becomes
`[redacted]`: in a message's `text`, the `question` it answers and its
`reply`, and in the warnings, gaps and limitations said of its sources. A
reply is scrubbed whole before it is cut to its last 600 characters, and the
part kept starts at a word, never inside a token or a `[redacted]`: so a cut
never leaves a piece of a key, or a secret without the word that named it.
The rules judge a text as a terminal shows it, without the control and
bidirectional formatting characters it drops, so a key split by one is
redacted whole. The original is never printed, logged or kept. `redacted`
counts the tokens redacted, and `redacted_by_rule` counts them by rule. The
plain form says the same on stderr when any were (`vessa: 2 secret-looking
tokens redacted (github 1, id_and_secret 1)`), counting only the words it
prints. A message is scrubbed as it is printed, after it is read, so the
rules below that tell a person's messages apart see the words as written.

`--raw` prints the words as written, and says so on stderr; `redacted` and
`redacted_by_rule` are then null. Nothing else turns the scrub off.

| Rule | Redacts |
| --- | --- |
| `anthropic` | `sk-ant-` keys |
| `openai` | other `sk-` keys, `sk-proj-` among them |
| `github` | `ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_` and `github_pat_` tokens |
| `slack` | `xoxb-`, `xoxp-`, `xoxa-` and `xoxs-` tokens |
| `aws` | `AKIA` and `ASIA` key ids |
| `gitlab` | `glpat-` tokens |
| `google` | `AIza` API keys |
| `jwt` | Three base64url parts, the first opening `eyJ` |
| `private_key` | A PEM `PRIVATE KEY` block, to its end line, else to the end |
| `id_and_secret` | `<hex, 24 or more>.<letters and digits, 12 or more>`: an id and a secret joined by a dot, as Z.ai issues its keys (`<32 hex>.<16>`), alone or as one name of a path or URL |
| `mixed_token` | 32 or more characters with two or more each of capitals, lower case letters and digits, and not hex; a path is judged by the pieces of its names |
| `named_secret` | 16 or more mixed letters and digits right after a word that names a secret: `key`, `token`, `secret`, `password`, `passwd` or `bearer`, alone or ending a name (`api_key`, `GITHUB_TOKEN`, `apiKey`) |

The rules are ohjaa intake's, which scrubs the prompt it sends to its
drafter (`src/intake.rs` in false-systems/ohjaa), copied, since repositories
stand alone. After a key's known prefix, the rest of its run, joined by `/` or
`.`, is judged as a token of its own. A path, which `mixed_token` judges by
its pieces and `named_secret` leaves alone, is a run that opens with `/`, `~`
or `.` and names a file with an extension, or has two or more names made of
words (in one case, short, or capitalised words run together): a base64
secret can open with `/` too.

Two things shaped like secrets are not redacted. AWS's documented example
keys (`AKIAIOSFODNN7EXAMPLE` and its secret), each alone; one that opens a
longer run is judged with it. And a signed evidence export, as habitat signs
its runtime evidence: hex-encoded JSON (`7b22…` is `{"`), a dot and a 64-byte
signature in hex, where the JSON is an object that declares itself an export
(`version` 1, `evidence`, `exported_at` and `operation_id`, as vartio's
verifier reads it) and holds nothing the rules redact. Redacting one would
destroy evidence. Anything else on that wire is redacted, habitat's grant
proofs among them: a grant proof authorizes whoever holds it, for up to 30 s.

A hash, a UUID and a path made of words are left alone; `mixed_token` also
takes a long name that mixes capitals and digits, and a path with a name that
reads as no words, a loss the scrub accepts. On the local archive
(2026-10-28) a default run redacted 54 tokens in 5,733 messages and their
replies: 51 `mixed_token`, 2 `named_secret` and 1 `id_and_secret`.

### What counts as a person's message

Each rule names the user turns it excludes in `coverage.excluded`; a turn is
counted under the first that applies. The rules are structural: what the
record says about itself, or a harness text in an exact form, either a whole
text or the fixed sentence a harness always opens it with. They never match a
mere prefix of a person's words, so a person who starts a message with
"Caveat:", a path or a tag name is still a person. They come from the
earlier readers of these transcripts, the stricter where they differ, and a
few the local archive called for:

| Rule | Excludes | Why |
| --- | --- | --- |
| `request_record` | A Claude record of another type that carries a user message (`api-request-blob`) | It copies a request's content, without uuid or time |
| `other_session` | A Claude record naming another session; also a limitation | Not this session's |
| `tool_result` | A Claude user turn holding a tool result, even with text beside it, unless a person wrote inside it (below) | Claude Code returns tool output as user turns |
| `subagent` | In a Claude subagent's transcript, or as a sidechain record inside a main one, a turn whose origin is not a person | The prompts the parent agent wrote |
| `meta` | Claude `isMeta`, unless its origin is a person | Skill expansions, peer messages, safety and system notices |
| `compaction` | Claude `isCompactSummary`, or a text opening with the fixed first sentence of its continuation summary | A summary of earlier turns |
| `harness_origin` | Claude `origin.kind` other than `human`, or `promptSource: system`; a queued command whose origin is not a person | Claude Code says the harness wrote it: task notifications, peers, coordinators |
| `empty` | No text, as an image alone | Nothing to read |
| `envelope` | A message that is wholly known harness elements (startup context, notifications, slash-command records, interruptions, Codex's review, skill and plugin elements), Codex's `# AGENTS.md instructions` heading over them, the verbatim interruption notices and local-command caveat, pi's bare `/command` | The harness wrote it into a user turn |
| `harness_prompt` | A text opening with the fixed first sentence of Codex's `/init`, or of a launch prompt that carries no task (below) | A program wrote it in a person's place |
| `question_reply` | A Codex reply to its question tool whose every answer is one of the options the question offered | Picking an offered option says nothing in the person's own words; Claude's picks are in its `tool_result` |
| `fork_copy` | In a Codex fork, the longest run of turns after the copied parent header that matches the parent's transcript turn for turn; all of them, with a limitation, when the parent was not read | A fork copies its parent's transcript stamped with the fork time, so time cannot separate them |
| `inherited` | In a fork (Codex, pi), a turn from before the fork started; one with no time, with a limitation | Its parent's history |
| `headless` | Claude `sdk-*` entrypoints and `promptSource: sdk`, decided per turn; a `codex exec` file in which no turn was submitted interactively (a `user_message` event) | A program sent it: `claude -p`, an Agent SDK, `codex exec`. A session resumed by a script keeps its interactive turns |
| `duplicate` | A message another copy of the session already gave (the same native id, else the same time and text), and a queued message that later arrives as an ordinary one of the same text | Read twice, said once |
| `retry` | The person's previous message sent again before the agent answered it, because the agent failed or never replied | Re-sent, not said anew |

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
* **A task in a launch prompt.** A launching wrapper (`ohjaa run`, through
  toimija) composes the launch prompt as the launcher's alerts, if any
  (`## Before anything else (from the launcher)`), the fixed framing and the
  session packet, then `\nTask: <task>\n`, then its briefing as
  `\n<briefing>\n`. Since ohjaa#56 the briefing's first line is
  `<!-- ohjaa-briefing v1 -->`, and the task is the text between the last
  `Task:` line and the first line that, trimmed of whitespace, opens with
  `<!-- ohjaa-briefing ` and closes with `-->`, in any version: the version
  tells what follows, not where the briefing starts. A sentinel inside a line
  is not one. A briefing from an older wrapper has no such line. It is parts
  joined by a newline, each opening with a sentence the briefing always
  writes: `FLAG: `, `Validation here (sykli's word, …`, `STILL RUNNING:
  session `, `ORPHANED: session `, `WARNING from `, `Knowledge (ohjaa,
  knowledge-node.v1):`, `Last session here (`, `How this session works
  (ohjaa):`, `Sessions in the other worktrees of this clone at launch`,
  `System perimeter (derived by ohjaa at launch; …`. There the task ends at
  the first of these after a blank line, so a task containing a blank line
  followed by one of those sentences ends early. The task is listed as
  `task`; a launch prompt with no typed words left is a `harness_prompt`.
  The transcript does not say who wrote the task: a person who ran the
  launcher, or an agent that launched the session for one.

No length limit is applied: one is a correction heuristic, and belongs to the
reader that looks for corrections. No prefix skip list is used either; the
exact forms above replace one.

On the local archive (2026-10-26) `messages` listed 5,583 messages: claude
630, codex 4,649, pi 304, from 628 transcripts, 258 of them Claude
subagents', in 5.3 s and 129 MB; with `--since 1d`, in 0.6 s. The reader it
replaced counted 5,797 typed human messages in the same folders earlier that
day. Of those, Vessa excludes the `request_record` copies (72), the harness's
own `/init` and launch prompts (150; from 4 of them it lists the typed task,
and 24 hold only the launcher's briefing), the question replies that only pick
options (43 of 63; the other 20 give 20 answers in the person's words) and one
`retry`. It also lists what that reader did not read: 28 messages written to
Claude subagents, 15 queued while the agent worked, 4 own answers to Claude's
question tool and 1 rejection reason. 2,701 of the Codex messages carry an id.

## Usage export

Usage exports, receipts and reports carry no message text; only `messages`
prints any. What they do print of transcripts' and a wrapper's words is
scrubbed of secrets by the same rules, always: their warnings and
limitations, the wrapper session records in exports and reports, which hold
what a session was asked and handed over, and the provider and model names
usage records give. stderr says
how many tokens were redacted, when any were. An error said on stderr is
scrubbed too, by every command but `messages --raw`. No sibling crate or
binary is needed.

```sh
vessa usage --input /path/to/session.jsonl --json > /tmp/usage.private.json
```

`usage --json` and `sessions --json` emit `vessa-usage.v2`: `sessions` and
`warnings`. Each session includes explicit `agent` and native `id`, source,
lineage, limitations and request-level `usage`. Each usage record preserves
source, line, time and its token `entry`. Optional `ohjaa` records are
explicitly linked wrapper context. This replaces the initial local
`vessa-report.v1` export; consumers must check the schema. `--by-agent --json`
is an aggregate view, not this exchange format.

Each session also exports `totals`, calculated by Vessa from deduplicated
records and grouped by provider/model. Consumers can display these beside work
evidence without implementing accounting. Unknown categories remain null; older
exports without this additive field still contain the original usage records.
`synthetic_records` counts a Claude session's `<synthetic>` messages, and an
opencode session's responses that recorded no tokens, read and known to hold
no usage; it is 0 for other agents.

`totals` stays each transcript's own, so summing it across sessions counts
every request once. Each session also names its Claude, Codex and opencode subagents in
`subagents` and sums itself with them in `totals_with_subagents`: the total to
show for one session. Without subagents the list is empty and the two totals
are equal. Subagents remain sessions of their own in the export.

A consumer joins by agent and native session ID only, never directory or time.
Whole-session usage does not establish usage caused by a conversation
pattern. Incomplete source coverage remains visible in the export and exit code.

## Wrapper handoff

The programs exchange files; neither links or launches the other.

```sh
vessa receipt --input /path/to/session.jsonl > /tmp/usage.json
ohjaa usage import /tmp/usage.json --session oh-EXISTING-ID
ohjaa usage export > /tmp/ohjaa.json
vessa report --input /path/to/session.jsonl --input /tmp/ohjaa.json --output /tmp/work.html
```

`receipt` requires exactly one native session and emits `agent-usage.v1`.
A Claude or Codex session's receipt includes its subagents and names them in
`limitations`. The wrapper attaches it to an explicit wrapper session.
Exported work, handover, knowledge and outcome evidence appears alongside the
usage only when that explicit native-session association exists. Directory
and time-window guesses never establish identity. Unmatched wrapper records
are reported. Receipt totals and native totals are not added together.

With `ohjaa run --usage-file /fresh/receipt.json`, a launched wrapper can write
the receipt to `OHJAA_USAGE_FILE` before exiting, to be imported at leaving.
Native agents do not emit this normalized receipt by themselves.

## UI and privacy

The report's header and the `report:` line it prints count sessions as
`stats` does, with their subagents apart. It shows usage-record counts and
session/subagent groupings. Expand a command to see separate token
categories, tool/failure/edit counts and the source location; full evidence is
still available below it. No prompt bodies are retained to label commands.

The report also explains how to reduce repeated discovery: launch once through
a wrapper, use the working set and handover it delivers, and refresh only when
facts become stale. Use the receipt/export commands above to associate usage
with an explicit wrapper session. These remain optional file exchanges, not
runtime dependencies. Wrapper association does not prove savings: compare
similar tasks, model, token categories and verified outcomes before
making that claim. Do not remove safety instructions or validation to lower
usage.

The HTML is a local, standalone report. Native links, disclosure controls and
browser Find provide navigation without a server or frontend dependencies.
All displayed source content is escaped, executable content is blocked by CSP, and
new reports use mode 0600 on Unix. Text from transcripts that a command
prints for a terminal, on stdout or stderr (the status line, the text forms
of every command, warnings and limitations), drops control characters and
the bidirectional formatting characters that would reorder what is shown;
JSON keeps the text as it was read. Existing report paths are never overwritten.
Reports contain private usage and optional linked wrapper context: files under
`reports/` are ignored by Git. JSON printed to stdout inherits the destination
permissions you choose. No knowledge becomes current automatically.

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
cargo run -- report --input examples/claude.jsonl --input examples/codex.jsonl --input examples/pi.jsonl --output /tmp/vessa-demo.html
```

The fixtures are synthetic. No real transcripts, credentials or generated
reports belong in this repository.

GitHub Actions runs formatting, Clippy and tests on pull requests and pushes
to `main`, using the existing self-hosted `arc-arm64` runner pool and the
pinned Rust toolchain. A final smoke check runs the binary on the three
synthetic agent fixtures. The job has read-only repository permissions and
does not read native session directories or install a service.

The token-only exports use `vessa-usage.v2`, `vessa-status.v2` and
`vessa-statistics.v2` because spending fields were removed from v1. Consumers
must check the schema. `agent-usage.v1` receipts retain their external contract
with token fields only; optional money fields are omitted. Messages and session
headers retain their existing schemas.
