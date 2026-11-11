# vessa reference

vessa is one Rust binary. It reads the session files coding agents leave on
disk and reports the token usage they recorded. It has no database, no
daemon and no JavaScript. Only `share` and `team` use the network, through
git (see [With a team](#with-a-team)). It never changes the files it reads.

Contents:

- [Commands](#commands)
- [Inputs and selection](#inputs-and-selection)
- [Exit codes](#exit-codes)
- [What each agent records](#what-each-agent-records)
- [Subagents](#subagents)
- [opencode](#opencode)
- [Session headers](#session-headers)
- [Status](#status)
- [Commands and changes](#commands-and-changes)
- [Statistics](#statistics)
- [Claude Code status line](#claude-code-status-line)
- [Usage export](#usage-export)
- [Receipts](#receipts)
- [Linked session records](#linked-session-records)
- [The HTML report](#the-html-report)
- [Messages](#messages)
- [Redaction](#redaction)
- [What counts as a person's message](#what-counts-as-a-persons-message)
- [Secrets scan](#secrets-scan)
- [With a team](#with-a-team)
- [Privacy](#privacy)
- [Limits](#limits)
- [Development](#development)

## Commands

| Command | What it does |
| --- | --- |
| `vessa sessions` | Lists sessions and what could not be read. `--headers` lists headers alone. |
| `vessa usage` | Token categories by session, provider and model. `--by-agent` sums across sessions for each agent and model. |
| `vessa status` | The latest recorded usage of exactly one session. |
| `vessa stats` | Usage by calendar period, in UTC. `--period day\|week\|month\|year`; the default is `month`. |
| `vessa report --output FILE` | Writes a standalone, private HTML report. Refuses to overwrite. |
| `vessa receipt` | Emits one session's usage as an `agent-usage.v1` receipt. |
| `vessa messages` | Lists the messages people typed, each with the agent reply it follows. `--since`, `--raw`. |
| `vessa secrets` | Scans for credentials this machine holds. Prints location, rule and count, never a value. `--since`. |
| `vessa claude-statusline` | Renders Claude Code's status-line JSON from stdin. |
| `vessa share` | Shares your usage in this repository with its team: one record of numbers, committed under `refs/vessa/usage/` and pushed. `--dry-run`, `--no-push`, `--remote`. |
| `vessa team` | What the team shared: each person's latest share, and a team total. `--by agent\|model\|week\|branch`, `--no-fetch`, `--remote`. |

Every command accepts these options:

| Option | Meaning |
| --- | --- |
| `--input PATH` | A file or directory to read. Repeatable. |
| `--agent NAME` | Only this agent: `claude`, `codex`, `pi` or `opencode`. |
| `--session ID` | Only this session, by the id the agent gave it. |
| `--json` | Print JSON, where the command has a JSON form. |
| `--watch SECONDS` | Refresh in the foreground, for `status`, `usage` and `stats`. Ctrl-C stops it. |
| `--samples N` | Stop after N refreshes. Requires `--watch`. |

With `--watch` and `--json`, each refresh is a separate JSON document.

## Inputs and selection

Without `--input`, vessa reads the default folders of the agents:

- `~/.claude/projects` (Claude Code)
- `~/.codex/sessions` (Codex)
- `~/.pi/agent/sessions` (pi)
- `$XDG_DATA_HOME/opencode/opencode.db`, else
  `~/.local/share/opencode/opencode.db` (opencode)

A default folder that does not exist means that agent is not installed. That
is not a gap. With `--agent`, a missing folder for the agent asked for is a
warning (`codex: not installed here`), because what was asked for cannot be
read.

`--input` accepts:

- a `.jsonl` transcript of Claude Code, Codex or pi;
- a directory, which is searched for `.jsonl` transcripts and for files
  named `opencode.db`;
- an opencode store: a SQLite database, or a file ending in `.db`;
- an `ohjaa-sessions.v1` JSON export, given as a file (see
  [Linked session records](#linked-session-records)). A directory search
  does not pick up `.json` files, so an agent's own metadata is not mistaken
  for a usage source.

Pass explicit paths when an agent's folder is somewhere else, or to read a
bounded selection. A transcript given with `--input` brings its `subagents/`
directory along. Each file is read once per run, by its canonical path. A
symbolic link is not followed: give its real path.

`--agent` and `--session` filter the result. `--session` takes the agent's
own session id.

Missing paths, malformed records and unsupported formats are reported. They
are never counted as zero.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | The command ran and its evidence is complete as far as vessa can tell. |
| 1 | The command failed. |
| 4 | The evidence is partial: a source warning or a session limitation. Each is printed to stderr, and the last line counts them. |

Use the printed limitations to judge coverage.

## What each agent records

Agent, model and provider are separate fields. Input excludes cached input.
Output includes reasoning tokens where the agent includes them, so vessa
never adds reasoning a second time. A token category the agent did not
record stays `null`.

vessa has no spending features. It does not read, calculate or show prices,
rate cards, currency estimates, credits or billing fields. Cost fields in
transcripts and in linked session records are ignored.

**Claude Code.** Claude Code writes one line per content block, so a request
appears on several lines. vessa counts each request id once. In subagent
transcripts the output count grows across the lines of one request; when
everything else agrees, vessa keeps the largest snapshot and cites its line.
Snapshots that disagree in any other way (input or cache counts, model) are
flagged, the first is kept, and the result is partial.

Claude Code transcripts never name a provider. vessa reads it from the
response ids: `msg_01…` or a `req_…` request id is `anthropic`,
`msg_bdrk_…` is `bedrock`, `msg_vrtx_…` is `vertex`, and anything else is
`unknown`. A model name proves nothing about who served the request.

Claude Code also writes `<synthetic>` assistant messages with zero usage
where a request failed or was refused. They are not requests. Each session
counts them in `synthetic_records`, and they are neither usage nor a
conflict. A session whose only answers were synthetic recorded no usage, so
it is not unknown. A synthetic message that reports usage is read as usage.

**Codex.** Codex writes cumulative counters. vessa takes the difference
between them, ignores repeated totals, and assigns each difference to the
model in use at that point. A counter that resets is flagged and becomes a
new baseline; vessa never invents negative usage. The provider is the
session's `model_provider`, else `openai`. Cache writes come from
`cache_write_input_tokens`. vessa assumes they are part of the inclusive
input count, as cached reads are. A non-zero value is reported as a session
limitation (exit 4), because that assumption is not confirmed. A counter
without the field records zero cache writes.

**pi.** vessa reads assistant, compaction, branch-summary and standalone
usage entries. An entry id copied across forks is counted once, with parent
sessions processed first when they are available.

**opencode.** vessa reads assistant and compaction responses from opencode's
store (see [opencode](#opencode)). opencode's output count leaves reasoning
out, so vessa adds it once.

## Subagents

A subagent's usage belongs to its session. The rules are the same for every
agent that has subagents:

- `--session ID` selects the session and its subagents. A subagent's own id
  selects it alone.
- A session whose main transcript was not read is not found, even when its
  subagents were.
- `status` and `receipt` total the session with its subagents.
- `stats` counts sessions, not transcripts. A subagent's usage counts toward
  its session, and subagents are counted apart.
- A request is counted once, however many transcripts hold it.

### Claude Code

Claude Code writes each subagent to its own transcript, in
`<session>/subagents/agent-<id>.jsonl` beside the session's own. vessa names
a subagent `<session>/<agentId>`, with the session as its `parent`.

- A main transcript in which some records say they are a sidechain and
  others say they are not is read as a subagent's, and the error says so.
- A subagent whose requests its session already holds adds nothing, and is
  not unknown. `status` says how many requests it shares.
- A subagent with requests that an unrelated session also holds is partial.
  Those requests are counted once, with that other session, and the
  subagent's own session leaves them out.
- A transcript that says it launched a subagent whose transcript was not
  read is partial (exit 4). That usage is unknown, not zero.
- A launch record can be a copy of another session's. Only the record proves
  it: when another session's transcripts hold the same record (its `uuid`,
  or the tool call it answers) and the subagent it names was read as that
  session's, the subagent ran there and nothing is missing here. The
  transcript then holds that session's history, so the session it copies is
  counted first and the shared requests belong to the original.
- A subagent with the same id in another session proves nothing. Nor does
  the same record held by sessions none of which, or several of which, read
  the subagent. Those launches stay partial, and the limitation names the
  other sessions that hold the record.

### Codex

Codex writes each subagent, and each other thread a session spawns (such as
a review), to its own transcript under its own thread id. Its `session_meta`
says it is a subagent (`source.subagent`) and names the session at its root
(`session_id`). Where only the thread it came from is named
(`parent_thread_id` or `forked_from_id`), vessa follows the threads it read
up to the root. A lineage that loops has no root: each thread in it stands
alone, with a limitation. A thread whose `session_meta` names no other
session stands alone.

### opencode

A session with a `parent_id` is a subagent. It counts toward the session at
the top of its chain. A chain that loops has no top: each session in it
stands alone, with a limitation.

## opencode

opencode keeps every session in one SQLite database. vessa has no SQLite
dependency. It reads the store with the `sqlite3` command (3.33 or newer,
for `-json`), read-only and with `-init /dev/null`, so a personal
`~/.sqliterc` cannot change what it prints. vessa keeps nothing from it.

A store is one of:

- a file given with `--input` that is a SQLite database or ends in `.db`;
- a file named `opencode.db` in a folder given with `--input`;
- the default store, which is read unless `--agent` asks for another agent.

An `XDG_DATA_HOME` that is empty or relative is ignored, as the XDG
specification says.

vessa reads the `session_v2` and `session_message` tables. A store in any
other layout (a missing table or column, or a file that is not a SQLite
database) is a source warning, never an empty store. So is a store when
`sqlite3` is not on `PATH` or cannot print JSON. Sessions held only in a
`session` table are a warning and are not read.

- **Sessions.** A session is a row of `session_v2`: its `id`, its
  `directory` as `cwd`, and its creation time as `started_at`. `source` is
  the store. A message's `seq` stands for its line. Warnings about one
  session's messages name the session, since every session counts from 0.
- **Usage.** Each `assistant` message, and each `compaction` (the summary
  opencode asks a model for), that records tokens is one request, under its
  message id. opencode counts reasoning apart from output; vessa counts it
  as output. Input is uncached, as opencode keeps it. A response that
  records no tokens at all, such as an error before any, is not a request.
  It is counted in `synthetic_records`. opencode's `cost` is not read.
- **Forks.** A session with a `fork_session_id` is a fork, and that session
  is its `parent`. opencode copies the messages of the original into the
  fork, each under a new id but with its `seq`, creation time and data. A
  message of the fork whose original holds one at the same `seq`, created at
  the same time, is a copy. A copied response counts under the id of the one
  it copies, so it is counted once, with the original. A copied message of a
  person's is `inherited`. A fork whose original is not in the store says
  that the messages it copied are listed nowhere.
- **Messages.** Only `user` messages can be a person's. opencode keeps what
  its harness adds (instruction files, background shell results, restarts,
  instruction updates) as `synthetic` and `system` messages, which are not
  turns. An agent reply is the text blocks of an `assistant` message. One
  that ended in an error is `failed`, and its error message stands for its
  text when it has none.
- **Messages in subagent sessions.** A person can type into a subagent
  session, so it is read. There, a `user` message is the prompt an agent
  wrote (`subagent`) when it opens with "You are a subagent spawned by
  another session.", or when it has neither `files` nor `agents`: opencode's
  prompt box sends both with whatever a person types, and its subagent tool
  sends neither. A migrated message keeps those fields only when they are
  not empty, so in a migrated subagent session what a person typed may count
  as a prompt.
- **`--since`.** A session whose newest message is not after the given time
  is counted in `unmodified_transcripts` and not read.
- **Secrets scan.** A `user` message is `person`. An assistant's text and
  reasoning blocks are `agent`. A tool block's `state.input` is
  `tool_input`, and every other field of its `state` is `tool_output`.
  Everything else is `other`. A message whose data is not JSON is a gap, and
  is still scanned as text.

## Session headers

`vessa sessions --headers` lists each session's header alone: what its
transcript says about itself, without its usage. It reads a transcript only
until its header is known, so it is fast and uses little memory on a large
archive. With `--json` it emits `vessa-session-headers.v1`:

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

- `agent` is `claude`, `codex`, `pi` or `opencode`. `id` is the agent's own
  session id; a Claude Code subagent is `<session>/<agentId>`.
- `parent` is the session it descends from: a Claude Code subagent's
  session, a Codex fork's `forked_from_id`, a pi `parentSession`, an
  opencode fork's origin. Null for none.
- `started_at` (RFC 3339) and `cwd` come from the header, else from the
  first record that has them. Unknown is null and `""`.
- `source` is the transcript read. Of copies of one session, it is the first
  path.
- `entrypoint` (Claude Code: `cli`, or `sdk-cli` for `claude -p`) and
  `originator` (Codex: `codex-tui`, or `codex_exec` for `codex exec`) say
  how a session was started. They are null when unknown and for other
  agents.

Each field equals the session's field of the same name in `vessa-usage.v2`.
The list holds the same sessions in the same order, with the same warnings.
`--agent` and `--session` select as they do for `sessions`.

Each fact is taken from the first record that states it. A transcript that
never states one, such as a Claude Code session that never names its
`entrypoint`, is read to its end.

The header reader relies on how the agents write. Each opens its transcripts
with its header, none holds another agent's records, and Claude Code marks
every record of a transcript alike as a sidechain or not. Where a file
breaks those habits, or is damaged past its header, the two listings differ:

- A Claude Code transcript that says it is not a sidechain, then that it is
  one: the full listing makes it a subagent of itself (`<id>/<agentId>`,
  with `<id>` as `parent`). Headers list the session itself, with no parent.
- A file that holds records of two agents: the full listing takes the higher
  ranked header (Codex, then pi, then Claude Code). Headers take the first
  in the file that names its session.
- Malformed records after the header, including invalid UTF-8: the full
  listing keeps the valid records and reports the damaged lines as
  limitations. Headers list the session without limitations, since they
  never read that far.

Both listings keep the first copy's header when copies are merged. No usage
is read, so headers carry no limitations, and exit 4 means only a source
warning: a transcript that could not be read.

## Status

`vessa status` shows the latest recorded usage of exactly one session. It
requires `--input` or `--session` and never picks the most recently modified
file. It prints the time of the last usage record, so an old transcript is
not mistaken for a live one.

Status totals the session with its subagents, says how many it included,
and shows each one. `status --json` emits `vessa-status.v2`:

- `agent`, `session_id`, and `last_recorded_at`: the time of the latest
  usage record of the session or its subagents.
- `entries`: the session's deduplicated usage with its subagents', by
  provider and model.
- `main_entries`: the main transcript's own usage.
- `subagents`: each subagent's `id`, `source`, `last_recorded_at` and own
  `entries`. Empty for none, and then `entries` equals `main_entries`.
- `insights`: the main transcript's command windows (see below).
- `limitations`: the session's, then each subagent's, named.
- `warnings`.

## Commands and changes

Status, the HTML report and usage JSON show usage by command and by change.

A command is a recorded user message and the usage up to the next user
message. Message bodies, tool arguments and tool output are not kept.
Context that a harness injects can also appear as a user message. Each
command shows its token usage, tool counts and the time elapsed through its
latest usage record. Where that time is unknown, vessa says why: no usage
was recorded in the command, the command or its usage records have no
timestamp, or its last usage record is timestamped before it.

The first request's usage is shown separately and is still included in
totals. Logs do not separate the tokens used by starting context from that
request's generation.

A change is an explicitly successful `apply_patch`, `Edit`, `Write`,
`MultiEdit`, `edit` or `write` call. Its window starts after the previous
successful edit result, or at the start of the command, and ends at its own
result. These windows are part of the command totals. They are not extra
usage, and they do not say what caused the usage. Edits made inside shell
commands or orchestration tools are not measured. A change is a recorded
operation, not a verified diff or commit.

```sh
vessa status --input examples/work.jsonl
```

## Statistics

`vessa stats --period day|week|month|year` groups usage records by their own
timestamp, agent and model. Calendar periods are UTC, and weeks use ISO week
years. A session that crosses a period boundary appears in both periods,
with only that period's usage. Records with a missing or invalid timestamp
go in an `undated` group. Each group shows token totals and the number of
usage records. Unknown token categories stay null. With `--json` it emits
`vessa-statistics.v2`. The HTML report includes all four periods.

A default scan covers the transcript files on this machine, not every
account or device.

## Claude Code status line

Claude Code can call vessa for its status line. In
`~/.claude/settings.json`:

```json
{"statusLine":{"type":"command","command":"vessa claude-statusline"}}
```

vessa reads Claude Code's session identity and the last API call's input,
output, cache-read and cache-write token counts from stdin. These are not
cumulative session counts. A count that is absent, negative or of the wrong
type stays unknown. Invalid JSON shows every field as unknown. The status
line always prints and exits 0. It makes no model calls and shows no
spending, budgets, quotas or limits. Keep any existing status-line
customization when you add this. See the
[Claude Code status-line contract](https://code.claude.com/docs/en/statusline).

## Usage export

```sh
vessa usage --input /path/to/session.jsonl --json > usage.private.json
```

`usage --json` and `sessions --json` emit `vessa-usage.v2`: `sessions` and
`warnings`. Consumers must check the schema.

Each session has:

- `agent` and the agent's own `id`, its source, lineage and limitations;
- `usage`: one record per request, with its source, line, time and token
  `entry`;
- `totals`: calculated by vessa from the deduplicated records, grouped by
  provider and model. Unknown categories stay null;
- `synthetic_records`: a Claude Code session's `<synthetic>` messages, and
  an opencode session's responses that recorded no tokens. It is 0 for
  other agents;
- `subagents`: the ids of its subagents;
- `totals_with_subagents`: the session summed with its subagents. This is
  the total to show for one session;
- `ohjaa`: linked session records, when an export was given (see
  [Linked session records](#linked-session-records)).

`totals` is each transcript's own, so summing it across sessions counts
every request once. Without subagents, `subagents` is empty and the two
totals are equal. Subagents are also sessions of their own in the export.

`usage --by-agent --json` is an aggregate view, not this format.

A consumer joins by agent and session id only, never by directory or time.
Whole-session usage does not show what a particular conversation pattern
used. Incomplete source coverage stays visible in the export and in the exit
code.

Usage exports, receipts and reports carry no message text. What they print
of other words is scrubbed of secrets by the rules under
[Redaction](#redaction): warnings, limitations, linked session records, and
the provider and model names in usage records. stderr says how many tokens
were redacted, when any were. An error printed on stderr is scrubbed too, by
every command except `messages --raw`.

## Receipts

```sh
vessa receipt --input /path/to/session.jsonl > receipt.json
```

`vessa receipt` requires exactly one session and emits an `agent-usage.v1`
document: `agent`, `session_id`, `source`, `observed_at_ms`, `complete`,
`limitations` and `entries` by provider and model. It carries token fields
only.

A receipt totals the session with its subagents. `agent-usage.v1` has no
field for subagents, so the receipt names them in `limitations`: how many,
and each by id up to ten. Past ten it names the first ten in id order and
counts the rest, which `vessa status --session ID` lists.

## Linked session records

`--input` also accepts an `ohjaa-sessions.v1` JSON export: a list of session
records, each of which can carry `agent-usage.v1` receipts. vessa attaches a
record to a session only when one of the record's receipts names that
session's agent and id. Directory and time never establish a link.

- A linked record appears in the session's `ohjaa` field in usage JSON, and
  beside the session in the HTML report.
- A record that names no session vessa read is reported as a warning.
- A session the export says it could not read is reported as a warning.
- Receipt totals and transcript totals are never added together.
- An export holds no conversation content. `messages` does not read one,
  and `secrets` reports it as unreadable.

```sh
vessa report --input /path/to/session.jsonl --input /path/to/export.json --output work.html
```

## The HTML report

`vessa report --output FILE` writes one standalone HTML file. It needs no
server and has no frontend dependencies. Links, disclosure controls and the
browser's Find provide navigation.

- The header, and the `report:` line the command prints, count sessions as
  `stats` does, with subagents apart.
- The report shows usage-record counts and groups sessions with their
  subagents. Expand a command to see its token categories, its tool, failure
  and edit counts, and its source location.
- It includes statistics for all four calendar periods.
- No prompt text is kept to label commands.
- It ends with general advice on reducing repeated work. It does not
  measure savings: a linked session record proves association, not that
  anything saved tokens.
- All source content shown is escaped, and a Content Security Policy blocks
  executable content.
- A new report is written with mode 0600 on Unix. An existing path is never
  overwritten.

A report holds private usage data. `reports/` and `*.private.json` are
ignored by Git in this repository.

## Messages

`vessa messages` lists what people wrote in their agent sessions, each
message with the agent reply it follows. It reads Claude Code, Codex, pi and
opencode sessions with the same discovery as the other commands. It keeps
nothing: no cache, no database. Text goes only to stdout.

```sh
vessa messages --since 7d
vessa messages --since 2026-10-01 --agent codex --json
vessa messages --session ID --json
```

`--since` takes RFC 3339, a UTC date (`YYYY-MM-DD`) or an age (`7d`, `12h`).
The boundary is exclusive: only messages after that time are listed. A
reader can pass the time of the last message it handled and not get that
message again. `--agent` and `--session` select as elsewhere; a Claude Code
subagent's own id selects it alone.

With `--json` it emits `vessa-messages.v1`: one JSON document with one
message per line. Each message is written as soon as its transcript is read,
so memory holds one transcript at a time.

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

**A message** has:

- `agent` and `session` (a Claude Code subagent's is `<session>/<agentId>`);
- `parent`: the session it descends from;
- `id`: the id of its record. Claude Code's `uuid`, pi's `id`, Codex's
  `payload.id` where Codex writes one;
- `at`, and `cwd` (the record's, or the turn's for Codex, else the
  session's);
- `text`, without the `<system-reminder>` blocks a harness writes into it;
- `source` and `line`: where it was read from;
- `kind`: what the words are.

| `kind` | Meaning |
| --- | --- |
| `message` | Typed into the conversation. |
| `queued` | Typed while the agent worked, and handed to it mid-turn. |
| `answer` | A person's own answer to the agent's question tool, with the `question` it answers. |
| `rejection` | The reason a person gave for rejecting a tool call. |
| `task` | The task a person typed into a launch prompt. |

**`reply`** is the agent turn the message follows in its transcript: at most
the last 600 characters of its text, starting at a word, with `truncated`
and `line`. `failed` says the turn was a Claude Code API error, a Codex
error event or a pi error stop. A message after a failed turn answers a
failure, so "try again" there is a retry, not a correction. `reply` is null
when no agent turn came before.

**`sessions`** lists each transcript read, with copies of one session as one
entry, and how many messages it gave. `headless` marks a session whose every
prompt a program sent. `subagent` marks a Claude Code subagent.

**`coverage`** says what was read and why each other user turn is not
listed. `user_turns` equals `human` plus every count in `excluded` plus
`earlier` and `undated` (with `--since`: a message at or before it, or with
no time). Codex subagent transcripts are counted in `subagent_transcripts`
and not read. With `--since`, transcripts last written before it are counted
in `unmodified_transcripts` and not read; an opencode store holds every
session, so its sessions are placed by their newest message instead. `gaps`
names sources vessa knows of and does not read. It is empty, because vessa
reads every source it knows of.

**Order.** Transcripts are read in path order, a Codex fork after the
sessions it was forked from. Messages come in file order.

**Damaged records.** Malformed JSON records, including invalid UTF-8, are
skipped without losing valid records elsewhere in the transcript. The usage
and message readers report these gaps with their line numbers and exit 4.

**Exit codes.** Exit 4 means the list is partial: a source warning, a gap or
a session limitation, each printed to stderr. A run that fails writes no
partial document. One that fails before its first message leaves stdout
empty. One that fails later ends the document with `"error"` in place of
`sessions`, `coverage` and `warnings`, so its messages are only those read
before the failure. Either way it exits 1.

No length limit and no prefix skip list are applied. Deciding which messages
matter is left to the reader of the output.

## Redaction

What people paste to agents, and what tools print back, includes
credentials. Every secret-looking token in what `messages` prints becomes
`[redacted]`: in a message's `text`, the `question` it answers and its
`reply`, and in the warnings, gaps and limitations about its sources.

- A reply is scrubbed whole before it is cut to its last 600 characters, and
  the part kept starts at a word. A cut never leaves a piece of a key, or a
  secret without the word that named it.
- The rules judge a text as a terminal shows it, without the control and
  bidirectional formatting characters a terminal drops. A key split by one
  of those is redacted whole.
- The original is never printed, logged or kept.
- `redacted` counts the tokens redacted, and `redacted_by_rule` counts them
  by rule. The plain form says the same on stderr when any were
  (`vessa: 2 secret-looking tokens redacted (github 1, id_and_secret 1)`).
- A message is scrubbed as it is printed, after it is read, so the rules
  that tell a person's messages apart see the words as written.

`--raw` prints the words as written and says so on stderr. `redacted` and
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
| `id_and_secret` | `<hex, 24 or more>.<letters and digits, 12 or more>`: an id and a secret joined by a dot, alone or as one name of a path or URL |
| `mixed_token` | 32 or more characters with two or more each of capitals, lower case letters and digits, and not hex; a path is judged by the pieces of its names |
| `named_secret` | 16 or more mixed letters and digits right after a word that names a secret: `key`, `token`, `secret`, `password`, `passwd` or `bearer`, alone or ending a name (`api_key`, `GITHUB_TOKEN`, `apiKey`) |

After a key's known prefix, the rest of its run, joined by `/` or `.`, is
judged as a token of its own. A path is a run that opens with `/`, `~` or
`.` and names a file with an extension, or has two or more names made of
words. `mixed_token` judges a path by its pieces, and `named_secret` leaves
a path alone.

Two things shaped like secrets are not redacted:

- AWS's documented example keys (`AKIAIOSFODNN7EXAMPLE` and its secret),
  each alone. One that opens a longer run is judged with that run.
- A signed evidence export: hex-encoded JSON, a dot, and a 64-byte signature
  in hex, where the JSON is an object that declares itself an export
  (`version` 1, `evidence`, `exported_at` and `operation_id`) and holds
  nothing the rules redact. Redacting one would destroy evidence. Anything
  else in that shape is redacted.

A hash, a UUID and a path made of words are left alone. `mixed_token` also
takes a long name that mixes capitals and digits, and a path with a name
that reads as no words. The scrub accepts that loss.

## What counts as a person's message

Each rule names the user turns it excludes in `coverage.excluded`. A turn is
counted under the first rule that applies. The rules are structural: what
the record says about itself, or a harness text in an exact form (a whole
text, or the fixed sentence a harness always opens it with). They never
match a mere prefix of a person's words, so a person who starts a message
with "Caveat:", a path or a tag name is still a person.

| Rule | Excludes | Why |
| --- | --- | --- |
| `request_record` | A Claude Code record of another type that carries a user message (`api-request-blob`) | It copies a request's content, without uuid or time |
| `other_session` | A Claude Code record naming another session; also a limitation | Not this session's |
| `tool_result` | A Claude Code user turn holding a tool result, even with text beside it, unless a person wrote inside it (below) | Claude Code returns tool output as user turns |
| `subagent` | In a Claude Code subagent's transcript, or as a sidechain record inside a main one, a turn whose origin is not a person | The prompts the parent agent wrote |
| `meta` | Claude Code `isMeta`, unless its origin is a person | Skill expansions, peer messages, safety and system notices |
| `compaction` | Claude Code `isCompactSummary`, or a text opening with the fixed first sentence of its continuation summary | A summary of earlier turns |
| `harness_origin` | Claude Code `origin.kind` other than `human`, or `promptSource: system`; a queued command whose origin is not a person | Claude Code says the harness wrote it: task notifications, peers, coordinators |
| `empty` | No text, as an image alone | Nothing to read |
| `envelope` | A message that is wholly known harness elements (startup context, notifications, slash-command records, interruptions, Codex's review, skill and plugin elements), Codex's `# AGENTS.md instructions` heading over them, the verbatim interruption notices and local-command caveat, pi's bare `/command` | The harness wrote it into a user turn |
| `harness_prompt` | A text opening with the fixed first sentence of Codex's `/init`, or of a launch prompt that carries no task (below) | A program wrote it in a person's place |
| `question_reply` | A Codex reply to its question tool whose every answer is one of the options the question offered | Picking an offered option says nothing in the person's own words; Claude Code's picks are in its `tool_result` |
| `fork_copy` | In a Codex fork, the longest run of turns after the copied parent header that matches the parent's transcript turn for turn; all of them, with a limitation, when the parent was not read | A fork copies its parent's transcript stamped with the fork time, so time cannot separate them |
| `inherited` | In a fork (Codex, pi, opencode), a turn from before the fork started; one with no time, with a limitation | Its parent's history |
| `headless` | Claude Code `sdk-*` entrypoints and `promptSource: sdk`, decided per turn; a `codex exec` file in which no turn was submitted interactively (a `user_message` event) | A program sent it: `claude -p`, an Agent SDK, `codex exec`. A session resumed by a script keeps its interactive turns |
| `duplicate` | A message another copy of the session already gave (the same id, else the same time and text), and a queued message that later arrives as an ordinary one of the same text | Read twice, said once |
| `retry` | The person's previous message sent again before the agent answered it, because the agent failed or never replied | Re-sent, not said anew |

What a person wrote is listed wherever the record keeps it apart from the
harness's words:

- **Writing to a subagent.** Claude Code lets a person write to a running
  subagent. It records the message in the subagent's transcript, marked
  meta, with a person's origin (`origin.kind: human`). That origin wins over
  `meta` and `subagent`. Claude Code subagent transcripts are read for these
  and listed under the subagent, with its session as `parent`. Nothing in a
  Codex subagent's transcript says a person wrote, so those are not read.
- **Typed while the agent works.** Claude Code records such a message as a
  `queued_command` attachment with a person's origin, and hands it to the
  agent mid-turn. It is listed as `queued`, once. When it also arrives later
  as an ordinary message, that one is listed instead.
- **Answers to the agent's question.** An answer that is one of the offered
  options is a pick, not the person's words. Claude Code's question tool
  returns its options with the answers (`toolUseResult.questions`, picks of
  several joined by ", "). Codex's reply names the `item_completed` that
  asked (`questionItemId`), whose `questions` carry the options. Any other
  answer is listed as `answer`, with its `question`, including an answer to
  a question vessa did not see asked.
- **Rejecting a tool call.** Claude Code writes the reason after a fixed
  sentence that ends "To tell you how to proceed, the user said:". The
  reason is listed as `rejection`. A rejection with no reason stays a
  `tool_result`.
- **A task in a launch prompt.** A toimija launch prompt opens with a fixed
  sentence, after the launcher's alerts when there are any
  (`## Before anything else (from the launcher)`). It holds the session
  packet, then `\nTask: <task>\n`, then a briefing. The task is the text
  between the last `Task:` line and the start of the briefing, and is listed
  as `task`. A launch prompt with no typed words left is a `harness_prompt`.
  The transcript does not say who wrote the task: a person who ran the
  launcher, or an agent that launched the session for one.

The briefing in a launch prompt starts in one of two ways:

- At the first line that, trimmed of whitespace, opens with
  `<!-- ohjaa-briefing ` and closes with `-->`. The version in the line does
  not matter. The same text inside a longer line does not count.
- Where there is no such line, at the first of these openings after a blank
  line: `FLAG: `, `Validation here (sykli's word, …`,
  `STILL RUNNING: session `, `ORPHANED: session `, `WARNING from `,
  `Knowledge (ohjaa, knowledge-node.v1):`, `Last session here (`,
  `How this session works (ohjaa):`,
  `Sessions in the other worktrees of this clone at launch`,
  `System perimeter (derived by ohjaa at launch; …`. In this case a task
  that contains a blank line followed by one of those openings ends early.

## Secrets scan

`vessa secrets` scans Claude Code, Codex, pi and opencode records for the
credentials this machine holds. It covers subagent and headless transcripts,
user and assistant text, tool inputs and tool outputs. A finding is an
alarm: one of your credentials reached an agent session, and so the provider
that ran it. Nothing is stored, and no network is used.

```sh
vessa secrets --input examples/codex.jsonl --json
vessa secrets --since 7d --agent claude
```

### What it compares with

The credentials are read at the start of each scan, kept in memory only,
compared by value, and never printed, logged or written. The sources are:

- environment variables with a secret-like name: one containing `TOKEN`,
  `SECRET`, `PASSWORD`, `PASSWD`, `KEY`, `CREDENTIAL` or `AUTH`, but not a
  socket, path, file, folder or session key;
- the GitHub CLI's login (`gh auth token`), when `gh` is on `PATH`;
- `~/.aws/credentials`, `~/.config/gh/hosts.yml`, `~/.git-credentials`,
  `~/.netrc`, `~/.npmrc`, `~/.docker/config.json`, `~/.codex/auth.json`,
  `~/.pi/agent/auth.json`, `~/.claude/.credentials.json` and
  `~/.ssh/id_{ed25519,rsa,ecdsa}`.

From a file, a credential is each secret-shaped string the detector
recognizes in it. From a variable or `gh`, it is also the whole value, when
that is 16 bytes or more, whatever its shape. Each credential is looked for
in every record as it stands, so it is found however it is labelled there.
macOS Keychain items are not read, since reading one can prompt. A source
that exists but cannot be read leaves the scan partial.

The report names each source in `compared_with`: its name, whether it was
`read`, `absent` or `unreadable`, and how many credentials it held. It never
shows a value.

### Other secret-shaped strings

Every other secret-shaped string the detector finds is counted by rule in
`candidates` and never listed: other people's keys, test fixtures, examples,
signatures, ids. The detector is the one [Redaction](#redaction) uses. Of
those, `placeholders` counts the plain ones: a few characters repeated
(`sk-a1a1a1…`), a straight run of the alphabet or digits, or an AWS
documentation example (`…EXAMPLE`).

Recognized key formats take precedence, then explicit secret labels
(`password`, `api_key`, `token` and similar words), then the generic long
mixed-string rule. A finding's `rule` is the one the detector gives the
credential where it stands, or `no_known_shape` where it gives none. Its
`held` names the source that holds it.

### `vessa-secrets.v1`

The document has `complete`, `since`, `transcripts`, `findings`,
`unreadable`, `compared_with`, `candidates` and `placeholders`.

Each finding has:

- `agent` and `session`, and optional `cwd`;
- `at`, with `at_inferred`;
- the source path and line. For opencode, the source is its store and the
  line is the message's `seq`;
- `where`: `person`, `agent`, `tool_input`, `tool_output` or `other`.
  `person` means a user-role input. It does not prove a human wrote it;
- `rule`, `held` and `count`;
- `opaque_count` and `opaque_native` (below).

`count` is the number of matched occurrences in the selected records, not
the number of unique credentials. Mirrored or copied records can repeat an
occurrence.

**Opaque fields.** `opaque_count` is the part of `count` found in Claude
Code thinking-signature fields or Codex encrypted-reasoning fields. These
fields are scanned like any other. Being in one is not an exemption, and
does not mean the content is safe. `opaque_native: true` says the opaque
occurrences were found at the agent's own positions for those fields: Claude
Code assistant thinking blocks or Codex response-item reasoning payloads.
User blocks, tool content, unknown records and fields that merely share a
name do not count. A report without `opaque_count` leaves it unknown, not
zero. A report without `opaque_native` is unverified and must not be used to
dismiss an alert.

**Time.** A record with no valid time of its own takes the time of the
record before it in its transcript, and says so (`at_inferred: true`). Known
file-history snapshots use their nested timestamp. Without a record before
it, a finding's time stays unknown (`at: null`). `undated_records` counts
finding records that produced a timestamp warning under `--since`.

**Damaged records.** Valid records around a damaged record are still
scanned. A match found in a damaged record has null time and location kind,
because its fields could not be decoded. Summaries keep known counts as
lower bounds. Unknown record shapes are scanned as `other`.

No value, message text or fingerprint is emitted. Metadata is scrubbed too,
including session ids and path components. There is no `--raw` mode.

### Selection and coverage

`--since` has the same syntax and exclusive boundary as for `messages`. An
inferred time places a finding only when it is inside the window. A finding
that stays undated makes a time-filtered scan partial; it does not silently
fall outside the window.

`--session` selects a session and its known subagents. A subagent alone does
not establish coverage of its unread parent.

Damaged JSON or UTF-8 records, unreadable paths, identity failures, missing
selected sessions, unsupported exports and opencode stores that could not be
read are reported in `unreadable`, with `complete: false` and exit 4.

A complete scan means the selected sources were read. It does not mean the
detector can recognize every possible secret. The plain output gives the
same metadata and coverage status as the JSON.

## With a team

A repository's team is whoever can push to it. `vessa share` keeps each
person's usage in that repository, under git refs of its own, and
`vessa team` reads what everyone shared. There is no server. Only numbers
are shared: nothing anyone wrote, no path and no machine name.

```sh
vessa share --dry-run   # the record that would be shared, and nothing else
vessa share             # share your sessions in this repository, and push
vessa team              # each person's latest share, with a team total
vessa team --by branch  # the same sessions, by branch
```

### Which sessions are shared

`vessa share` runs inside a clone, in any of its worktrees. A session
belongs to the repository when the folder its agent recorded (its `cwd`) is
inside the top folder of the clone's main worktree or of one of its linked
worktrees, as `git worktree list` names them. The two are compared as
paths, part by part, after each is resolved as the file system resolves it
(symbolic links, and Windows' short names and drive spellings). A folder
since deleted is placed by the part of it that still exists. A session that
recorded no folder, or a relative one, belongs to no repository. Time never
decides.

vessa reads every session, as `vessa usage` does, so each is counted as
`usage` counts it, and then shares those of this repository. A session's
subagents are counted with it. `--input` reads other places than the
agents' own folders, as elsewhere; `--agent` and `--session` do not apply.

### The record

A share is one `vessa-team-usage.v1` record for the person:

```json
{
  "schema": "vessa-team-usage.v1",
  "person": {"name": "Ada Lovelace", "email": "ada@example.org"},
  "shared_at": "2026-11-11T09:36:15Z",
  "entries_include_subagents": true,
  "source_warnings": 0,
  "sessions": [
    {
      "agent": "claude",
      "id": "ada-1",
      "started_at": "2026-11-09T09:00:00Z",
      "ended_at": "2026-11-09T09:20:00Z",
      "branch": "main",
      "subagents": [],
      "partial": false,
      "limitations": {},
      "entries": [
        {"provider": "anthropic", "model": "demo-model", "input_tokens": 1200,
         "output_tokens": 420, "cache_read_tokens": 9100, "cache_write_tokens": 640}
      ]
    }
  ]
}
```

- `person`: `git config user.name` and `user.email` in the clone.
- `shared_at`: when it was shared, in UTC.
- `entries_include_subagents`: always true.
- `source_warnings`: how many warnings leave the share incomplete: each
  source vessa could not read, since sessions of this repository may be
  among them, and one more when sessions of this repository were left out
  because their id reads as a secret or a path. A share keeps the sessions
  of the shares before it, so it carries the most any of them had: a
  session that a source held then may be gone since, and missing. A share
  with any is partial, and stays so.
- `sessions`, in agent and id order. Each has:
  - `agent`, and `id`: the agent's own session id;
  - `started_at`, and `ended_at`: the time of its latest usage record, its
    subagents' included. RFC 3339 in UTC; null when unknown;
  - `branch`: the first git branch its transcript records (Claude Code's
    `gitBranch`, Codex's `git.branch`). Null when it records none, as pi and
    opencode do not;
  - `subagents`: the ids of the subagents its entries include, as `vessa
    status` names them;
  - `entries`: its tokens by provider and model, as `vessa usage` counts
    them, its subagents' included. A category the agent did not record is
    null;
  - `partial`, and `limitations`: vessa's limitations of the session, counted
    by kind. A partial session's numbers are incomplete; on the machine that
    shared it, `vessa status --session ID` says why.

| Kind | The session |
| --- | --- |
| `malformed_record` | holds a record that is not JSON |
| `unreadable_record` | holds a record vessa could not read for usage |
| `subagent_not_read` | launched a subagent whose transcript was not read |
| `conflicting_usage` | holds two snapshots of one request that disagree |
| `no_usage_recorded` | recorded no usage |
| `counted_with_another_session` | holds requests counted with another session |
| `lineage_loop` | is a subagent whose lineage loops |
| `copied_launch_records` | and another hold copies of each other's launch records |
| `codex_cache_writes` | is a Codex session that recorded cache writes |
| `messages_unreadable` | is an opencode session whose messages could not be read |
| `other` | has another limitation |

**Never in the record:** what a person or an agent wrote (prompts, replies,
thinking, tool input and output, commit messages, summaries), any file path,
absolute or relative, the machine's name, and prices or costs. A
limitation's own words are not kept, since they quote record ids, other
sessions' ids and names: only its kind is. A branch, provider or model name
is scrubbed by the rules under [Redaction](#redaction), and one that reads
as a path (from the root, from home, from a dot, a Windows drive or share, a
file URL, or with a backslash) is written `[path]`. A session is known by
its id, so a session whose id the scrub would change, or that reads as a
path, is not shared at all: any stand-in made from it would be its
fingerprint. `share` says how many were left out, and counts it in
`source_warnings`.

`--dry-run` prints the record that would be written, and changes nothing.
`--json` prints the record that was written, byte for byte.

### Where it is kept

The record is the one file, `usage.json`, of a commit on
`refs/vessa/usage/<key>`. `<key>` is the first 16 hex digits of the git
hash of the lower-cased email, so the ref name does not spell the address:

```sh
printf %s ada@example.org | git hash-object --stdin | cut -c1-16
```

Each share is a new commit on the one before, with the person as its author
and `shared_at` as its time, so the ref's history shows each share. The
record's bytes are written as they are, with git's plumbing (`hash-object`,
`mktree`, `commit-tree`, `update-ref`).

A share holds the person's whole set for the repository, each session by
agent and id. A session read now is shared with its current numbers. A
session shared before and not found now keeps the record it was shared
with: agents delete old transcripts, and worktrees are removed. Sharing
again with nothing changed shares the same sessions, byte for byte. When no
session of the repository is found, nothing is written.

A subagent is shared once. Read without its session's transcript, it is
shared as a session of its own, under its own id. Once its session is read,
it is counted there, and its own record goes. A subagent whose session's
transcript is gone is not shared beside that session's record when the
record names it in `subagents`: its usage is there. One the record does not
name is shared alone.

### A push the remote refuses

`share` then pushes that one ref to `origin`, or to the remote `--remote
NAME` names. `--no-push` keeps it in the clone. The push is never forced.

When the remote refuses it because the ref moved, someone shared under the
same key: from another clone, another machine, or another person with the
same email. vessa fetches that share into
`refs/vessa/remotes/<remote>/usage/<key>`, pushes nothing, and says whose
share it is (exit 1). Nothing on the remote is overwritten, and the new
share stays in the clone. `vessa share` run again follows both shares,
keeps the sessions of both, and pushes.

### `vessa team`

`vessa team` fetches `refs/vessa/usage/*` from `origin`, or the remote
`--remote NAME` names, into `refs/vessa/remotes/<remote>/usage/*`, and drops
those the remote no longer has. It never fetches into `refs/vessa/usage/`,
so a share kept in the clone is never replaced. `--no-fetch` reads only what
the clone already has.

Of each person it reads the latest share: the clone's own when it holds the
remote's, else the remote's. It reads nothing else, never anyone's session
files: `--input`, `--agent` and `--session` do not apply.

It prints one block per person, with when they last shared, their sessions,
and their tokens by agent, provider and model, then the team total. `--by
agent|model|week|branch` groups the sessions by that instead. A week is the
ISO week of a session's start, in UTC, named as `stats` names it. It says
how many people have shared. A person who has not shared does not appear,
and is never counted as zero. A sum with an unknown category is unknown.
Partial sessions are counted, and listed with their limitations' kinds.

With `--json` it emits `vessa-team.v1`:

- `remote`; `fetched`, false with `--no-fetch` or when the fetch failed;
  `by`, `person` unless `--by` names another;
- `people`: each person's `key`, `name`, `email`, `shared_at`, `pushed`
  (false when the clone's own share is ahead of the remote's),
  `source_warnings`, and `sessions` as shared;
- `groups`: by person, with the person's key as `key`, or by what `--by`
  names. Each has its `key` (null when unknown: no branch recorded, no
  start, no usage recorded), how many `people`, `sessions` and
  `partial_sessions` it holds, and `entries` by agent, provider and model,
  each with how many `sessions` it sums;
- `total`: the same, for everyone, without a key;
- `warnings`.

### Network

`share` and `team` are the only commands that use the network: only through
git, only while they run, and only to push or fetch `refs/vessa/usage/*`.
Each push and fetch names its refspec, and takes no tags and no submodules,
whatever the clone's settings say. git uses the remote's own address and
credentials. Every other command works offline.

### Exit codes of `share` and `team`

| Code | Meaning |
| --- | --- |
| 0 | Every session shared or shown is complete, and every source was read. |
| 4 | Partial: a session with limitations, a share made with unreadable sources, or a warning (a fetch that failed, a share that cannot be read, a clone's share that differs from the remote's). |
| 1 | It failed: not in a clone, no git name or email, a remote name git would take for an option, a push the remote refused, or a git command that failed. |

## Privacy

- vessa reads local files and writes only where you tell it to: stdout, the
  `--output` file of `report`, and, for `share`, its record under this
  repository's `refs/vessa/usage/`.
- Only `messages` prints conversation text, and it redacts secret-looking
  tokens unless `--raw` is given.
- Text from transcripts that a command prints for a terminal (the status
  line, the plain form of every command, warnings and limitations) drops
  control characters and the bidirectional formatting characters that would
  reorder what is shown. JSON keeps the text as it was read.
- JSON printed to stdout gets the permissions of wherever you send it.

## Limits

vessa reads selected sessions into memory. For a very large archive, select
narrower input directories, or list headers alone with `sessions --headers`.

A local transcript cannot establish everything. These limit what vessa can
say:

- schema versions it does not support;
- fork history inherited without its original source;
- model rerouting that the logs do not record;
- requests that were never logged.

## Development

The Rust toolchain is pinned in `rust-toolchain.toml`. The tests that build
an opencode store need `sqlite3` on `PATH`.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run -- usage --input examples/claude.jsonl --input examples/codex.jsonl --input examples/pi.jsonl
```

The files in `examples/` are synthetic. Real transcripts, credentials and
generated reports do not belong in this repository.

The exports use these schemas: `vessa-usage.v2`, `vessa-status.v2`,
`vessa-statistics.v2`, `vessa-session-headers.v1`, `vessa-messages.v1`,
`vessa-secrets.v1`, `vessa-team-usage.v1`, `vessa-team.v1` and
`agent-usage.v1`. Consumers must check the schema.
