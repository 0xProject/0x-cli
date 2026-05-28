pub mod envelope;
pub mod human;
pub mod trade;

use crate::cli::OutputFormat;
use crate::error::CliError;
use colored::Colorize;
use envelope::{CliOutput, Metadata, Warning};
use serde::Serialize;
use std::io::{self, Write};
use std::time::Instant;

/// Trait for types that can render human-readable output.
pub trait HumanDisplay {
    fn display_human(&self, writer: &mut dyn Write, color: bool) -> io::Result<()>;
}

/// Central output handler for the CLI.
pub struct OutputHandler {
    pub format: OutputFormat,
    pub color: bool,
    pub quiet: bool,
    pub start_time: Instant,
}

impl OutputHandler {
    pub fn new(format: OutputFormat, color: bool, quiet: bool) -> Self {
        Self {
            format,
            color,
            quiet,
            start_time: Instant::now(),
        }
    }

    fn elapsed_ms(&self) -> u64 {
        self.start_time.elapsed().as_millis() as u64
    }

    /// Render a successful result envelope to stdout. Returns the raw
    /// `io::Result` for callers that want to handle write errors directly;
    /// most commands should use [`Self::emit_success`] instead, which
    /// converts BrokenPipe to a silent success and bubbles other write
    /// failures to stderr.
    pub fn success<T: Serialize + HumanDisplay>(
        &self,
        command: &str,
        data: &T,
        metadata: Metadata,
        warnings: Vec<Warning>,
    ) -> io::Result<()> {
        let stdout = io::stdout();
        let mut out = stdout.lock();

        match self.format {
            OutputFormat::Human => {
                data.display_human(&mut out, self.color)?;
                if !warnings.is_empty() {
                    writeln!(out)?;
                    for w in &warnings {
                        if self.color {
                            writeln!(out, "{} {}", "Warning:".yellow().bold(), w.message)?;
                        } else {
                            writeln!(out, "Warning: {}", w.message)?;
                        }
                    }
                }
            }
            OutputFormat::Json => {
                serde_json::to_writer_pretty(&mut out, data)
                    .map_err(io::Error::other)?;
                writeln!(out)?;
            }
            OutputFormat::JsonEnvelope => {
                let envelope =
                    CliOutput::success(command, data, self.elapsed_ms(), metadata)
                        .with_warnings(warnings);
                serde_json::to_writer_pretty(&mut out, &envelope)
                    .map_err(io::Error::other)?;
                writeln!(out)?;
            }
        }

        Ok(())
    }

    /// Render a successful result envelope and return the supplied
    /// `exit_code`. Used by every command to terminate the success path:
    ///
    /// - On a clean write: returns `exit_code` verbatim.
    /// - On `BrokenPipe` (e.g. `0x chains -o json | head`): returns
    ///   `exit_code` anyway — the downstream consumer closed the pipe, which
    ///   is not a failure of *our* work.
    /// - On any other write failure: prints a one-line diagnostic to stderr
    ///   and returns `1`. We can't do anything more useful at that point.
    ///
    /// Replaces the old `output.success(...).map_err(|e| CliError::config(
    /// ErrorCode::Unknown, ...))` boilerplate, which leaked a meaningless
    /// "unknown" error code through the envelope for IO failures that
    /// were not config errors.
    pub fn emit_success<T: Serialize + HumanDisplay>(
        &self,
        command: &str,
        data: &T,
        metadata: Metadata,
        warnings: Vec<Warning>,
        exit_code: i32,
    ) -> i32 {
        match self.success(command, data, metadata, warnings) {
            Ok(()) => exit_code,
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => exit_code,
            Err(e) => {
                let _ = writeln!(io::stderr(), "Error writing output: {e}");
                1
            }
        }
    }

    /// Render an error.
    pub fn error(&self, command: &str, err: &CliError, metadata: Metadata) -> i32 {
        let exit_code = err.exit_code();

        match self.format {
            OutputFormat::Human => {
                self.print_human_error(err);
            }
            OutputFormat::Json => {
                let detail = crate::error::ErrorDetail::from(err);
                let stdout = io::stdout();
                let mut out = stdout.lock();
                let _ = serde_json::to_writer_pretty(&mut out, &detail);
                let _ = writeln!(out);
            }
            OutputFormat::JsonEnvelope => {
                let envelope =
                    CliOutput::<serde_json::Value>::error(command, err, self.elapsed_ms(), metadata);
                let stdout = io::stdout();
                let mut out = stdout.lock();
                let _ = serde_json::to_writer_pretty(&mut out, &envelope);
                let _ = writeln!(out);
            }
        }

        exit_code
    }

    fn print_human_error(&self, err: &CliError) {
        let stderr = io::stderr();
        let mut out = stderr.lock();

        if self.color {
            let _ = writeln!(out, "{} {}", "Error:".red().bold(), err);
        } else {
            let _ = writeln!(out, "Error: {}", err);
        }

        if let Some(suggestion) = err.suggestion() {
            if self.color {
                let _ = writeln!(out, "{} {}", "Suggestion:".cyan().bold(), suggestion);
            } else {
                let _ = writeln!(out, "Suggestion: {}", suggestion);
            }
        }
    }

    /// Print an informational message to stderr (suppressed with --quiet).
    pub fn info(&self, msg: &str) {
        if !self.quiet {
            let stderr = io::stderr();
            let mut out = stderr.lock();
            if self.color {
                let _ = writeln!(out, "{}", msg.dimmed());
            } else {
                let _ = writeln!(out, "{}", msg);
            }
        }
    }

    /// Create a progress spinner on stderr (suppressed with --quiet).
    ///
    /// Prefer [`Self::spinner_guard`] in code paths that may error out — that
    /// version guarantees the spinner is cleared on Drop so a `?` early-return
    /// doesn't leave dangling tick characters on the terminal. This raw API
    /// remains for non-fallible call sites (status command, price command).
    pub fn spinner(&self, msg: &str) -> Option<indicatif::ProgressBar> {
        if self.quiet {
            return None;
        }
        let pb = indicatif::ProgressBar::new_spinner();
        pb.set_style(
            indicatif::ProgressStyle::with_template("{spinner:.cyan} {msg} {elapsed:.dim}")
                .unwrap()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"),
        );
        pb.set_message(msg.to_string());
        pb.enable_steady_tick(std::time::Duration::from_millis(80));
        Some(pb)
    }

    /// Spinner wrapped in an RAII guard: cleared automatically on Drop, so
    /// `?` early-returns don't leak tick characters to the user's terminal.
    pub fn spinner_guard(&self, msg: &str) -> SpinnerGuard {
        SpinnerGuard {
            inner: self.spinner(msg),
        }
    }
}

/// RAII wrapper that clears its spinner when dropped. Use `.set_message(...)`
/// while the work is in flight; on success call `.finish()` (which clears
/// the spinner) and on error/early-return the Drop impl does the same.
pub struct SpinnerGuard {
    inner: Option<indicatif::ProgressBar>,
}

impl SpinnerGuard {
    pub fn set_message(&self, msg: impl Into<std::borrow::Cow<'static, str>>) {
        if let Some(ref pb) = self.inner {
            pb.set_message(msg);
        }
    }

    /// Borrow the underlying spinner for APIs that take `Option<&ProgressBar>`
    /// directly (e.g. the polling helpers). The Drop guard still owns the
    /// spinner — don't outlive the guard with the returned reference.
    pub fn progress_bar(&self) -> Option<&indicatif::ProgressBar> {
        self.inner.as_ref()
    }

}

impl Drop for SpinnerGuard {
    fn drop(&mut self) {
        if let Some(pb) = self.inner.take() {
            pb.finish_and_clear();
        }
    }
}
