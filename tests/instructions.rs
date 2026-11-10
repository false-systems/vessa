//! What every agent session here loads before its first task: this
//! repository's rules, and no more than it needs.
//!
//! Claude Code reads AGENTS.md by itself only where no CLAUDE.md sits in or
//! above the folder it starts in, so any CLAUDE.md above a checkout would
//! hide it: CLAUDE.md imports AGENTS.md instead, outside code, where Claude
//! Code's documentation says it does not follow an import. Codex reads
//! AGENTS.md directly.

/// CLAUDE.md and AGENTS.md together, in bytes: about 1,500 tokens. Detail
/// an agent needs only for some tasks belongs in the README or `docs/`.
const BUDGET: usize = 6_000;

fn read(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Whether `text` imports AGENTS.md on a line of its own, outside a fenced
/// code block.
fn imports_agents_md(text: &str) -> bool {
    let mut fenced = false;
    text.lines().any(|line| {
        let line = line.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            fenced = !fenced;
            return false;
        }
        !fenced && line == "@AGENTS.md"
    })
}

#[test]
fn claude_code_loads_agents_md_and_both_stay_small() {
    let claude = read("CLAUDE.md");
    assert!(
        imports_agents_md(&claude),
        "CLAUDE.md must import AGENTS.md on a line of its own (`@AGENTS.md`), outside \
         code, or Claude Code never loads this repository's rules"
    );
    let size = claude.len() + read("AGENTS.md").len();
    assert!(
        size <= BUDGET,
        "CLAUDE.md and AGENTS.md hold {size} bytes, over the {BUDGET} every session \
         loads before its first task: move detail to the README or docs/"
    );
}

#[test]
fn an_import_inside_code_does_not_count() {
    assert!(imports_agents_md("@AGENTS.md\n"));
    assert!(imports_agents_md("# Notes\n\n@AGENTS.md\n"));
    assert!(!imports_agents_md("See [`AGENTS.md`](AGENTS.md).\n"));
    assert!(!imports_agents_md("```\n@AGENTS.md\n```\n"));
    assert!(!imports_agents_md("~~~md\n@AGENTS.md\n~~~\n"));
}
