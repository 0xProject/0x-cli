//! Output types and assembly for gasless swaps.

use crate::api::types::{TokenAmount, TokenInfo};
use crate::chain;
use crate::output::trade::SideMeta;
use crate::output::HumanDisplay;
use serde::Serialize;
use std::io::{self, Write};

/// Gasless swap result.
#[derive(Debug, Serialize)]
pub struct GaslessSwapOutput {
    pub chain: String,
    pub sell_token: TokenInfo,
    pub buy_token: TokenInfo,
    pub sell_amount: TokenAmount,
    pub buy_amount: TokenAmount,
    pub min_buy_amount: TokenAmount,
    pub trade_hash: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explorer_url: Option<String>,
    pub terminal: bool,
    pub successful: bool,
    pub dry_run: bool,
}

impl HumanDisplay for GaslessSwapOutput {
    fn display_human(&self, writer: &mut dyn Write, color: bool) -> io::Result<()> {
        use colored::Colorize;

        if self.dry_run {
            if color {
                writeln!(writer, "\n  {}", "Gasless Dry Run Complete".bold().yellow())?;
            } else {
                writeln!(writer, "\n  Gasless Dry Run Complete")?;
            }
        } else if self.successful {
            if color {
                writeln!(writer, "\n  {}", "Gasless Swap Complete".bold().green())?;
            } else {
                writeln!(writer, "\n  Gasless Swap Complete")?;
            }
        } else if color {
            writeln!(
                writer,
                "\n  {}",
                format!("Gasless Swap: {}", self.status).bold()
            )?;
        } else {
            writeln!(writer, "\n  Gasless Swap: {}", self.status)?;
        }

        writeln!(writer, "  {}", "-".repeat(45))?;

        let sell_label = self
            .sell_token
            .symbol
            .as_deref()
            .unwrap_or(&self.sell_token.address);
        let buy_label = self
            .buy_token
            .symbol
            .as_deref()
            .unwrap_or(&self.buy_token.address);

        writeln!(
            writer,
            "  {:<14} {} {}",
            "Sell:",
            self.sell_amount.display(),
            sell_label
        )?;
        writeln!(
            writer,
            "  {:<14} {} {}",
            "Buy:",
            self.buy_amount.display(),
            buy_label
        )?;
        writeln!(writer, "  {:<14} {}", "Trade Hash:", self.trade_hash)?;
        writeln!(writer, "  {:<14} {}", "Status:", self.status)?;

        if let Some(ref hash) = self.tx_hash {
            writeln!(writer, "  {:<14} {}", "Tx Hash:", hash)?;
        }
        if let Some(ref url) = self.explorer_url {
            writeln!(writer, "  {:<14} {}", "Explorer:", url)?;
        }

        Ok(())
    }
}

/// Assemble a `GaslessSwapOutput` from a quote + outcome. Centralises the
/// `TokenInfo` / `TokenAmount` construction so the four call sites
/// (needs-confirmation preview, dry-run preview, final success/failure) can't
/// drift.
#[allow(clippy::too_many_arguments)]
pub(super) fn gasless_output(
    chain_info: &chain::ChainInfo,
    sell: &SideMeta,
    buy: &SideMeta,
    quote: &crate::api::gasless::GaslessQuoteResponse,
    trade_hash: String,
    status: &str,
    tx: Option<(String, String)>,
    terminal: bool,
    successful: bool,
    dry_run: bool,
) -> GaslessSwapOutput {
    let (tx_hash, explorer_url) = match tx {
        Some((hash, explorer)) => (Some(hash), Some(explorer)),
        None => (None, None),
    };

    GaslessSwapOutput {
        chain: chain_info.display_name.to_string(),
        sell_token: sell.token_info(),
        buy_token: buy.token_info(),
        sell_amount: sell.amount(&quote.sell_amount),
        buy_amount: buy.amount(&quote.buy_amount),
        min_buy_amount: buy.amount(&quote.min_buy_amount),
        trade_hash,
        status: status.to_string(),
        tx_hash,
        explorer_url,
        terminal,
        successful,
        dry_run,
    }
}
