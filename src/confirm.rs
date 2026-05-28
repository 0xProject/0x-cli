use crate::cli::OutputFormat;
use crate::error::CliError;
use crate::output::envelope::{Metadata, Warning};
use crate::output::{HumanDisplay, OutputHandler};
use colored::Colorize;
use serde::Serialize;
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

/// Outcome of [`confirm_or_preview`]. Replaces the
/// `match confirm_trade { Confirmed => {}, NeedsConfirmation => { … emit
/// preview; return Ok(25) } }` block that used to live in every signing
/// command.
pub enum ConfirmFlow {
    /// The user (or `--yes`) confirmed; the caller should proceed.
    Confirmed,
    /// JSON mode without `--yes`: a preview envelope was emitted on stdout
    /// and the caller should return exit code 25 immediately.
    PreviewEmitted,
}

/// Combined "ask the user / emit a preview" flow used by every signing
/// command (swap, gasless, cross-chain). Behaves like [`confirm_trade`] but
/// also takes ownership of the JSON-mode preview emission:
///
/// - `--yes`, or interactive confirmation → returns [`ConfirmFlow::Confirmed`].
/// - Non-human output without `--yes` → emits `preview` as a success
///   envelope on stdout and returns [`ConfirmFlow::PreviewEmitted`]; the
///   caller maps this to exit code 25.
///
/// `preview` is taken by reference, so callers still construct it eagerly
/// (cheap — every command needs a structurally-equivalent value for the
/// final output anyway). `metadata` and `warnings` are moved into the emit,
/// matching the signature of [`OutputHandler::success`].
pub fn confirm_or_preview<T: Serialize + HumanDisplay>(
    output: &OutputHandler,
    auto_confirm: bool,
    summary: &TradeSummary,
    command: &str,
    preview: &T,
    metadata: Metadata,
    warnings: Vec<Warning>,
) -> Result<ConfirmFlow, CliError> {
    match confirm_trade(output.format, auto_confirm, output.color, summary)? {
        ConfirmResult::Confirmed => Ok(ConfirmFlow::Confirmed),
        ConfirmResult::NeedsConfirmation => {
            let _ = output.success(command, preview, metadata, warnings);
            Ok(ConfirmFlow::PreviewEmitted)
        }
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
