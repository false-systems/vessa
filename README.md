<img src="docs/assets/vessa.svg" alt="vessa: Tokens counted, not guessed." width="100%">

# vessa

vessa is a local command-line tool that reads the session files coding
agents leave on disk and reports the token usage they recorded.

Claude Code, Codex, pi and opencode each write their sessions in a different
format, and none of them gives you one view across all of them. vessa reads
those files where they are and counts what they recorded, by session,
provider and model. It keeps measured numbers apart from anything inferred.
A count that is missing stays unknown; it never becomes zero. Nothing leaves
your machine, and the files it reads are never changed.

## See it

```console
$ vessa stats --period year
Calendar usage · UTC · selected local records
2026 claude · 48 sessions · 253 subagents · 36981 usage records
  anthropic/claude-opus-5-5 in=72986 out=22914244 cache-read=10809505589 cache-write=316461626
  anthropic/claude-sonnet-5-5 in=178 out=121297 cache-read=6526679 cache-write=717275
  ...
2026 codex · 335 sessions · 404 subagents · 104759 usage records
  openai/gpt-5.5 in=124952943 out=7211705 cache-read=3134139264 cache-write=0
  ...
2026 pi · 32 sessions · 5889 usage records
  zai/glm-5.3 in=26009337 out=3694531 cache-read=1132067712 cache-write=0
vessa: partial usage evidence (exit 4): 47 sessions with limitations, 0 source warnings
```

This is a real run, trimmed where `...` stands. The last line is the point:
some sessions could not be read in full, so vessa says the result is partial
and exits with code 4, and lists each limitation above that line.

## Install

vessa builds from source with Rust. The toolchain version is pinned in
`rust-toolchain.toml` (1.98), and `rustup` installs it on the first build.

```sh
git clone https://github.com/false-systems/vessa
cd vessa
cargo install --path .
```

Reading opencode sessions also needs the `sqlite3` command (3.33 or newer)
on your `PATH`. The other agents need nothing extra.

## Use

Without `--input`, vessa reads the agents' default folders under your home
folder. Pass `--input PATH` to read a specific file or directory.

```sh
vessa sessions                     # which sessions it found, and what it could not read
vessa usage --by-agent             # token totals for each agent and model
vessa stats --period week          # usage by day, week, month or year, in UTC
vessa status --session ID          # the latest recorded usage of one session
vessa report --output report.html  # a private, standalone HTML report
vessa secrets --since 7d           # whether a credential this machine holds reached a session
```

`vessa messages` lists the messages people typed, with secret-looking tokens
redacted. Add `--json` to most commands for machine-readable output, and
`--agent claude|codex|pi|opencode` to select one agent.

Exit code 0 means the evidence is complete, 4 means it is partial, and 1
means the command failed.

## How it fits

vessa is one of a family of small tools for working with coding agents. Each
one works alone.

- **toimija**: facts about a repository, for agents.
- **sykli**: declared checks, and proof that they ran.
- **ohjaa**: what agents are told, decided by a person.
- **vessa**: what agents used.
- **faija**: the words they share.

## What it does not do

- It has no prices, costs, credits, budgets or limits. It counts tokens.
- It does not estimate. A number the agent did not record stays unknown.
- It sees only the session files on this machine, not other devices or a
  provider's account.
- It does not run, launch or control agents, and it does not run in the
  background.
- `vessa secrets` finds credentials this machine holds. It cannot recognize
  every possible secret, and it never prints a value.

## More

- [Reference](docs/reference.md): every command, the JSON formats, and how
  each agent's records are counted.
- [Licence](LICENSE): Apache-2.0.
