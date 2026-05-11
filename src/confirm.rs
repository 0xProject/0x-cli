use crate::cli::OutputFormat;
use crate::error::CliError;
use colored::Colorize;
use std::io::{self, Write};

/// Result of a confirmation check.
pub enum ConfirmResult {
    /// User confirmed (or --yes was passed).
    Confirmed,
    /// Non-interactive mode (JSON output) without --yes.
    /// Caller should output the quote data and exit with code 20.
    NeedsConfirmation,
}

/// Check trade confirmation.
///
/// - With `--yes`: returns `Confirmed` immediately.
/// - In human mode without `--yes`: shows summary and prompts user.
/// - In JSON mode without `--yes`: returns `NeedsConfirmation` so caller
///   can output the quote before exiting with code 20.
pub fn confirm_trade(
    output_format: OutputFormat,
    auto_confirm: bool,
    color: bool,
    summary: &TradeSummary,
) -> Result<ConfirmResult, CliError> {
    if auto_confirm {
        return Ok(ConfirmResult::Confirmed);
    }

    // In JSON mode, return NeedsConfirmation so caller can output the quote
    if !matches!(output_format, OutputFormat::Human) {
        return Ok(ConfirmResult::NeedsConfirmation);
    }

    // Display summary in human mode
    let stdout = io::stdout();
    let mut out = stdout.lock();

    if color {
        writeln!(out, "\n  {}", summary.title.bold()).ok();
        writeln!(out, "  {}", "─".repeat(45)).ok();
    } else {
        writeln!(out, "\n  {}", summary.title).ok();
        writeln!(out, "  {}", "-".repeat(45)).ok();
    }

    for (key, value) in &summary.rows {
        if color {
            writeln!(out, "  {:<12} {}", key.cyan(), value).ok();
        } else {
            writeln!(out, "  {:<12} {}", key, value).ok();
        }
    }

    if let Some(ref warning) = summary.warning {
        if color {
            writeln!(out, "\n  {} {}", "⚠".yellow(), warning.yellow()).ok();
        } else {
            writeln!(out, "\n  WARNING: {}", warning).ok();
        }
    }

    writeln!(out).ok();
    drop(out); // Release stdout lock before dialoguer takes over

    let confirmed = dialoguer::Confirm::new()
        .with_prompt("  Confirm trade?")
        .default(false)
        .interact()
        .map_err(|_| CliError::UserCancelled)?;

    if confirmed {
        Ok(ConfirmResult::Confirmed)
    } else {
        Err(CliError::UserCancelled)
    }
}

/// Trade summary for the confirmation prompt.
pub struct TradeSummary {
    pub title: String,
    pub rows: Vec<(String, String)>,
    pub warning: Option<String>,
}

impl TradeSummary {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            rows: Vec::new(),
            warning: None,
        }
    }

    pub fn row(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.rows.push((key.into(), value.into()));
        self
    }

    pub fn warning(mut self, warning: impl Into<String>) -> Self {
        self.warning = Some(warning.into());
        self
    }
}
