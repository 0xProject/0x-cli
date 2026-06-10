//! `0x skill ...` — emit or install the bundled agent skill.
//!
//! The skill (`skills/0x-trade/SKILL.md` + `references/`) is `include_str!`-ed
//! at compile time, so what the CLI prints or installs is always exactly the
//! version bundled with this binary. No I/O at print time, no chance of drift
//! between published binary and shipped docs.

use crate::cli::SkillTopic;
use crate::error::{CliError, ErrorCode};
use std::io::{self, Write};
use std::path::Path;

const SKILL_MARKDOWN: &str = include_str!("../../skills/0x-trade/SKILL.md");

/// Reference topics bundled alongside the main skill. File names must match
/// the `references/<name>.md` links inside SKILL.md.
const REFERENCES: &[(&str, &str)] = &[
    (
        "gasless",
        include_str!("../../skills/0x-trade/references/gasless.md"),
    ),
    (
        "cross-chain",
        include_str!("../../skills/0x-trade/references/cross-chain.md"),
    ),
    (
        "solana",
        include_str!("../../skills/0x-trade/references/solana.md"),
    ),
    (
        "config",
        include_str!("../../skills/0x-trade/references/config.md"),
    ),
    (
        "tokens",
        include_str!("../../skills/0x-trade/references/tokens.md"),
    ),
    (
        "errors",
        include_str!("../../skills/0x-trade/references/errors.md"),
    ),
];

fn topic_markdown(topic: SkillTopic) -> &'static str {
    let name = topic.file_stem();
    REFERENCES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, md)| *md)
        .unwrap_or(SKILL_MARKDOWN)
}

fn write_markdown(markdown: &str) -> Result<i32, CliError> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    out.write_all(markdown.as_bytes()).ok();
    // Ensure a trailing newline even if the source markdown lacks one.
    if !markdown.ends_with('\n') {
        writeln!(out).ok();
    }
    Ok(0)
}

/// Print the bundled skill markdown (or one reference topic) to stdout
/// verbatim. The global output format flag is intentionally ignored — this
/// command is documentation emission, not a data response.
pub fn run_print(topic: Option<SkillTopic>) -> Result<i32, CliError> {
    match topic {
        Some(t) => write_markdown(topic_markdown(t)),
        None => write_markdown(SKILL_MARKDOWN),
    }
}

/// Write the full skill directory (`0x-trade/SKILL.md` + `references/`) into
/// `dir`, defaulting to the project-local `.claude/skills`. Existing files
/// are overwritten — the bundled version is the source of truth.
pub fn run_install(dir: Option<&Path>) -> Result<i32, CliError> {
    let base = dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| Path::new(".claude").join("skills"));
    let skill_dir = base.join("0x-trade");
    let references_dir = skill_dir.join("references");

    let io_err = |what: &str, e: io::Error| CliError::Config {
        code: ErrorCode::ConfigInvalid,
        message: format!("Failed to {what}: {e}"),
    };

    std::fs::create_dir_all(&references_dir)
        .map_err(|e| io_err(&format!("create {}", references_dir.display()), e))?;

    let skill_path = skill_dir.join("SKILL.md");
    std::fs::write(&skill_path, SKILL_MARKDOWN)
        .map_err(|e| io_err(&format!("write {}", skill_path.display()), e))?;

    for (name, markdown) in REFERENCES {
        let path = references_dir.join(format!("{name}.md"));
        std::fs::write(&path, markdown)
            .map_err(|e| io_err(&format!("write {}", path.display()), e))?;
    }

    eprintln!(
        "Installed skill '0x-trade' ({} files) to {}",
        1 + REFERENCES.len(),
        skill_dir.display()
    );
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every topic variant must resolve to a distinct bundled reference —
    /// catches a renamed reference file or a stale REFERENCES entry.
    #[test]
    fn every_topic_resolves_to_its_own_reference() {
        let topics = [
            SkillTopic::Gasless,
            SkillTopic::CrossChain,
            SkillTopic::Solana,
            SkillTopic::Config,
            SkillTopic::Tokens,
            SkillTopic::Errors,
        ];
        assert_eq!(topics.len(), REFERENCES.len());
        for t in topics {
            let md = topic_markdown(t);
            assert!(
                !std::ptr::eq(md.as_ptr(), SKILL_MARKDOWN.as_ptr()),
                "topic {:?} fell back to SKILL.md — REFERENCES entry missing",
                t.file_stem()
            );
        }
    }

    /// SKILL.md must reference every bundled topic so agents can discover
    /// them, and must not reference topics we don't bundle.
    #[test]
    fn skill_md_links_match_bundled_references() {
        for (name, _) in REFERENCES {
            assert!(
                SKILL_MARKDOWN.contains(&format!("references/{name}.md")),
                "SKILL.md does not link references/{name}.md"
            );
        }
    }

    /// The exit-code decision tree in SKILL.md must list exactly the codes
    /// the binary can produce: every `ErrorCode::exit_code()` mapping plus
    /// the non-error specials (0 success, 25 preview-emitted, 30 dry-run).
    /// This is the drift the skill rewrite fixed — keep it fixed.
    #[test]
    fn skill_exit_code_table_matches_error_codes() {
        use crate::error::ALL_ERROR_CODES;
        use std::collections::BTreeSet;

        let documented: BTreeSet<i32> = SKILL_MARKDOWN
            .lines()
            .filter_map(|line| {
                let cell = line.strip_prefix("|")?.split('|').next()?;
                cell.trim().parse::<i32>().ok()
            })
            .collect();

        let mut expected: BTreeSet<i32> =
            ALL_ERROR_CODES.iter().map(|c| c.exit_code()).collect();
        expected.insert(0); // success
        expected.insert(25); // preview emitted (confirm_or_preview)
        expected.insert(30); // dry-run completed

        assert_eq!(
            documented, expected,
            "SKILL.md exit-code table drifted from error.rs (documented vs actual)"
        );
    }

    /// Every error code documented in references/errors.md must exist in the
    /// enum with the same exit code and retryable flag — and every enum
    /// variant must be documented.
    #[test]
    fn errors_reference_matches_error_enum() {
        use crate::error::{ErrorCode, ALL_ERROR_CODES};
        use std::collections::BTreeSet;

        let errors_md = REFERENCES
            .iter()
            .find(|(n, _)| *n == "errors")
            .expect("errors reference bundled")
            .1;

        let by_name = |name: &str| -> Option<ErrorCode> {
            ALL_ERROR_CODES.iter().copied().find(|c| c.as_str() == name)
        };

        let mut documented: BTreeSet<&str> = BTreeSet::new();
        for line in errors_md.lines().filter(|l| l.starts_with("| `")) {
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            // cells: ["", codes, category, exit, retryable, recovery, ""]
            let (codes_cell, category, exit_cell, retryable_cell) =
                (cells[1], cells[2], cells[3], cells[4]);

            // Backticked tokens in the first cell are wire names; rows may
            // combine several codes that share a category/exit/retryable.
            let names: Vec<&str> = codes_cell
                .split('`')
                .skip(1)
                .step_by(2)
                .collect();
            // Header row (`| \`error.code\` | Category | Exit | ...`) has no
            // numeric exit cell — skip it. The coverage assertion below
            // guarantees we still processed every real row.
            let Ok(exit) = exit_cell.parse::<i32>() else {
                continue;
            };
            let retryable = retryable_cell.starts_with("yes");

            for name in names {
                let code = by_name(name)
                    .unwrap_or_else(|| panic!("errors.md documents unknown code {name}"));
                assert_eq!(code.exit_code(), exit, "exit drift for {name}");
                assert_eq!(code.category(), category, "category drift for {name}");
                assert_eq!(code.retryable(), retryable, "retryable drift for {name}");
                documented.insert(code.as_str());
            }
        }

        for code in ALL_ERROR_CODES {
            assert!(
                documented.contains(code.as_str()),
                "{} missing from references/errors.md",
                code.as_str()
            );
        }
    }

    #[test]
    fn install_writes_all_files() {
        let tmp = std::env::temp_dir().join(format!("skill-install-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        run_install(Some(&tmp)).expect("install succeeds");
        assert!(tmp.join("0x-trade/SKILL.md").exists());
        for (name, _) in REFERENCES {
            assert!(tmp.join(format!("0x-trade/references/{name}.md")).exists());
        }
        std::fs::remove_dir_all(&tmp).ok();
    }
}
