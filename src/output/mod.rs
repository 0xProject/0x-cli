pub mod envelope;
pub mod human;

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

    /// Render a successful result.
    pub fn success<T: Serialize + HumanDisplay>(
        &self,
        command: &str,
        data: &T,
        metadata: Metadata,
        warnings: Vec<Warning>,
    ) -> io::Result<i32> {
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

        Ok(0)
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
}
