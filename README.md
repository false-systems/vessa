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
$ vessa usage --by-agent --input examples
Deduplicated agent totals
claude
  anthropic/demo-model in=1206 out=1675 cache-read=3300 cache-write=180
codex
  openai/demo-codex in=800 out=200 cache-read=700 cache-write=0
pi
  test/demo in=700 out=250 cache-read=800 cache-write=0
  zai/demo-glm in=220 out=110 cache-read=400 cache-write=0
```

This runs on the sample sessions in `examples/`, so you can repeat it in a
clone. Each line is what that agent recorded for one model: input, output and
cache tokens, kept apart. When a session cannot be read in full, vessa says
the result is partial, names each limitation and exits with code 4.

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
- [Licence](LICENSE): MIT.
