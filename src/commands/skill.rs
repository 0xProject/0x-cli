//! `0x skill ...` — emit the bundled Claude agent skill.
//!
//! `CLAUDE_SKILL.md` is `include_str!`-ed at compile time, so the skill the
//! CLI prints is always exactly the one bundled with this binary. No I/O,
//! no chance of drift between published binary and shipped docs.

use crate::error::CliError;
use std::io::{self, Write};

const SKILL_MARKDOWN: &str = include_str!("../../CLAUDE_SKILL.md");

/// Print the bundled skill markdown to stdout verbatim. The global output
/// format flag is intentionally ignored — this command is documentation
/// emission, not a data response.
pub fn run_print() -> Result<i32, CliError> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    out.write_all(SKILL_MARKDOWN.as_bytes()).ok();
    // Ensure a trailing newline even if the source markdown lacks one.
    if !SKILL_MARKDOWN.ends_with('\n') {
        writeln!(out).ok();
    }
    Ok(0)
}
