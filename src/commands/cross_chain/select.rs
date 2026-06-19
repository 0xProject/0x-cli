//! Quote selection for cross-chain swaps: `--select-quote` flag handling,
//! `--yes` auto-select, and the interactive (Human-output) picker.

use super::output::{build_step_path, format_quote_amount, QuoteSummary, QuotesDisplay};
use crate::api::cross_chain::CrossChainQuotesResponse;
use crate::cli::CrossChainArgs;
use crate::error::{CliError, ErrorCode};
use crate::output::trade::SideMeta;
use crate::output::{HumanDisplay, OutputHandler};
use std::io;

pub(super) fn select_quote(
    args: &CrossChainArgs,
    output: &OutputHandler,
    resp: &CrossChainQuotesResponse,
    buy: &SideMeta,
    auto_confirm: bool,
) -> Result<usize, CliError> {
    // If user specified a selection via flag
    if let Some(ref sel) = args.select_quote {
        return match sel.as_str() {
            "best-price" | "0" => Ok(0), // Already sorted by API
            "fastest" => {
                let idx = resp
                    .quotes
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, q)| q.estimated_time_seconds.unwrap_or(u64::MAX))
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                Ok(idx)
            }
            n => {
                let idx: usize = n.parse().map_err(|_| CliError::Api {
                    code: ErrorCode::InputInvalid,
                    message: format!(
                        "Invalid quote selection: '{n}'. Use a number, 'best-price', or 'fastest'"
                    ),
                    status: None,
                    details: None,
                    suggestion: None,
                })?;
                if idx >= resp.quotes.len() {
                    return Err(CliError::Api {
                        code: ErrorCode::InputInvalid,
                        message: format!(
                            "Quote index {idx} out of range (0-{})",
                            resp.quotes.len() - 1
                        ),
                        status: None,
                        details: None,
                        suggestion: None,
                    });
                }
                Ok(idx)
            }
        };
    }

    // With --yes and no selection: auto-select first quote
    if auto_confirm {
        return Ok(0);
    }

    // Non-Human output formats can't drive an interactive prompt — fall back
    // to the first quote and surface a hint via the regular error/output path
    // instead of hanging on dialoguer.
    if !matches!(output.format, crate::cli::OutputFormat::Human) {
        if resp.quotes.len() > 1 {
            return Err(CliError::Api {
                code: ErrorCode::InputInvalid,
                message: format!(
                    "{} quotes returned but no --select-quote provided in non-interactive output mode",
                    resp.quotes.len()
                ),
                status: None,
                details: None,
                suggestion: Some(
                    "Re-run with --select-quote <index|best-price|fastest> or --yes to auto-select the first".into(),
                ),
            });
        }
        return Ok(0);
    }

    // Interactive selection (Human output only)
    let display = QuotesDisplay {
        quotes: resp
            .quotes
            .iter()
            .enumerate()
            .map(|(i, q)| QuoteSummary {
                index: i,
                bridge: q.bridge_provider(),
                buy_display: format_quote_amount(&q.buy_amount, buy),
                min_buy_display: format_quote_amount(&q.min_buy_amount, buy),
                estimated_time: q.estimated_time_display(),
                path: build_step_path(q),
            })
            .collect(),
    };

    // Display quotes table
    let stdout = io::stdout();
    let mut out = stdout.lock();
    display.display_human(&mut out, output.color).ok();
    drop(out);

    if resp.quotes.len() == 1 {
        return Ok(0);
    }

    let selection = dialoguer::Input::<usize>::new()
        .with_prompt(format!("Select quote [0-{}]", resp.quotes.len() - 1))
        .default(0)
        .interact()
        .map_err(|_| CliError::UserCancelled)?;

    if selection >= resp.quotes.len() {
        return Err(CliError::Api {
            code: ErrorCode::InputInvalid,
            message: format!("Invalid selection: {selection}"),
            status: None,
            details: None,
            suggestion: None,
        });
    }

    Ok(selection)
}
