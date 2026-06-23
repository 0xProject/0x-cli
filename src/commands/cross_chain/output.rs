//! Output types and assembly for cross-chain swaps: the executed-swap shape,
//! the dry-run survey shape (all quotes), and the human quote-picker display.

use crate::api::cross_chain::{CrossChainQuote, CrossChainQuotesResponse};
use crate::api::types::{compute_rate, RouteSource, TokenAmount, TokenInfo};
use crate::chain;
use crate::output::trade::SideMeta;
use crate::output::HumanDisplay;
use serde::Serialize;
use std::io::{self, Write};

/// Cross-chain swap output.
#[derive(Debug, Serialize)]
pub struct CrossChainOutput {
    pub origin_chain: String,
    pub destination_chain: String,
    pub sell_token: TokenInfo,
    pub buy_token: TokenInfo,
    pub sell_amount: TokenAmount,
    pub buy_amount: TokenAmount,
    pub min_buy_amount: TokenAmount,
    pub rate: String,
    pub bridge: String,
    pub route: Vec<RouteSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_time_seconds: Option<u64>,
    pub status: String,
    pub terminal: bool,
    pub successful: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin_tx_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin_explorer_url: Option<String>,
    pub dry_run: bool,
}

impl HumanDisplay for CrossChainOutput {
    fn display_human(&self, writer: &mut dyn Write, color: bool) -> io::Result<()> {
        use colored::Colorize;

        let title = if self.dry_run {
            "Cross-Chain Swap (Dry Run)"
        } else if self.successful {
            "Cross-Chain Swap Complete"
        } else {
            "Cross-Chain Swap Status"
        };

        if color {
            writeln!(writer, "\n  {}", title.bold().green())?;
        } else {
            writeln!(writer, "\n  {title}")?;
        }
        writeln!(writer, "  {}", "-".repeat(45))?;

        writeln!(
            writer,
            "  {:<14} {} → {}",
            "Route:", self.origin_chain, self.destination_chain
        )?;
        writeln!(writer, "  {:<14} {}", "Bridge:", self.bridge)?;
        writeln!(writer, "  {:<14} {}", "Sell:", self.sell_amount.display())?;
        writeln!(writer, "  {:<14} {}", "Buy:", self.buy_amount.display())?;
        writeln!(
            writer,
            "  {:<14} {}",
            "Min Buy:",
            self.min_buy_amount.display()
        )?;
        writeln!(writer, "  {:<14} {}", "Rate:", self.rate)?;
        writeln!(writer, "  {:<14} {}", "Status:", self.status)?;

        if let Some(ref hash) = self.origin_tx_hash {
            writeln!(writer, "  {:<14} {}", "Origin Tx:", hash)?;
        }
        if let Some(ref url) = self.origin_explorer_url {
            writeln!(writer, "  {:<14} {}", "Explorer:", url)?;
        }

        Ok(())
    }
}

/// `--dry-run` output for `cross-chain`: route metadata + the full quotes
/// list, so JSON consumers can see every option the API returned without
/// having to first pick one. The execute path (`CrossChainOutput`) only
/// surfaces the selected quote; this is the survey shape.
#[derive(Debug, Serialize)]
pub struct CrossChainDryRunOutput {
    pub origin_chain: String,
    pub destination_chain: String,
    pub sell_token: TokenInfo,
    pub buy_token: TokenInfo,
    pub sell_amount: TokenAmount,
    pub quotes: Vec<DryRunQuote>,
    pub dry_run: bool,
}

/// Per-quote JSON payload for dry-run output. Raw + formatted amounts on
/// both sides, the API's estimated time, and the resolved step path so
/// agents can see what hops the bridge takes.
#[derive(Debug, Serialize)]
pub struct DryRunQuote {
    pub index: usize,
    pub bridge: String,
    pub buy_amount: TokenAmount,
    pub min_buy_amount: TokenAmount,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_time_seconds: Option<u64>,
    pub steps: Vec<DryRunStep>,
}

#[derive(Debug, Serialize)]
pub struct DryRunStep {
    #[serde(rename = "type")]
    pub step_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

impl HumanDisplay for CrossChainDryRunOutput {
    fn display_human(&self, writer: &mut dyn Write, color: bool) -> io::Result<()> {
        use colored::Colorize;

        let title = "Cross-Chain Quotes (Dry Run)";
        if color {
            writeln!(writer, "\n  {}", title.bold().yellow())?;
        } else {
            writeln!(writer, "\n  {title}")?;
        }
        writeln!(writer, "  {}", "─".repeat(45))?;
        writeln!(
            writer,
            "  {:<14} {} → {}",
            "Route:", self.origin_chain, self.destination_chain
        )?;
        writeln!(writer, "  {:<14} {}", "Sell:", self.sell_amount.display())?;

        for q in &self.quotes {
            let header = if color {
                format!(
                    "\n  [{}] {}",
                    q.index.to_string().bold().cyan(),
                    q.bridge.bold()
                )
            } else {
                format!("\n  [{}] {}", q.index, q.bridge)
            };
            writeln!(writer, "{header}")?;
            writeln!(
                writer,
                "      {:<14} {}",
                "You receive:",
                q.buy_amount.display()
            )?;
            writeln!(
                writer,
                "      {:<14} {}",
                "Min receive:",
                q.min_buy_amount.display()
            )?;
            let time = match q.estimated_time_seconds {
                Some(s) if s < 60 => format!("~{s}s"),
                Some(s) if s < 3600 => format!("~{} min", s / 60),
                Some(s) => format!("~{} hr", s / 3600),
                None => "unknown".to_string(),
            };
            writeln!(writer, "      {:<14} {}", "Bridge time:", time)?;
            let path = q
                .steps
                .iter()
                .map(|s| {
                    let chain = s.chain_name.as_deref().or(s.chain_id.as_deref());
                    match (chain, &s.provider) {
                        (Some(c), Some(p)) => format!("{} ({c} / {p})", s.step_type),
                        (Some(c), None) => format!("{} ({c})", s.step_type),
                        (None, Some(p)) => format!("{} ({p})", s.step_type),
                        (None, None) => s.step_type.clone(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" → ");
            let path = if path.is_empty() {
                q.bridge.clone()
            } else {
                path
            };
            writeln!(writer, "      {:<14} {}", "Path:", path)?;
        }
        writeln!(writer)?;
        Ok(())
    }
}

/// Build the dry-run output (route metadata + every quote) from the API
/// response. Skipped on execute paths.
pub(super) fn build_dry_run_output(
    origin: &chain::ChainInfo,
    destination: &chain::ChainInfo,
    sell: &SideMeta,
    buy: &SideMeta,
    sell_amount_raw: &str,
    quotes_resp: &CrossChainQuotesResponse,
) -> CrossChainDryRunOutput {
    let quotes = quotes_resp
        .quotes
        .iter()
        .enumerate()
        .map(|(i, q)| DryRunQuote {
            index: i,
            bridge: q.bridge_provider(),
            buy_amount: buy.amount(&q.buy_amount),
            min_buy_amount: buy.amount(&q.min_buy_amount),
            estimated_time_seconds: q.estimated_time_seconds,
            steps: q
                .steps
                .iter()
                .map(|s| {
                    let chain_id_str = s.chain_id.as_ref().and_then(|v| {
                        v.as_u64()
                            .map(|n| n.to_string())
                            .or_else(|| v.as_str().map(String::from))
                    });
                    let chain_name = chain_id_str.as_deref().and_then(|id| {
                        chain::resolve_chain(id)
                            .ok()
                            .map(|c| c.display_name.to_string())
                    });
                    DryRunStep {
                        step_type: s.step_type.clone(),
                        chain_id: chain_id_str,
                        chain_name,
                        provider: s.provider.clone(),
                    }
                })
                .collect(),
        })
        .collect();

    CrossChainDryRunOutput {
        origin_chain: origin.display_name.to_string(),
        destination_chain: destination.display_name.to_string(),
        sell_token: sell.token_info(),
        buy_token: buy.token_info(),
        sell_amount: sell.amount(sell_amount_raw),
        quotes,
        dry_run: true,
    }
}

/// Quotes display for human output. Rendered as a multi-line block per
/// quote (instead of a wide table) so each quote shows enough detail for
/// a user to actually choose: formatted receive amount with symbol, the
/// min after slippage with its percentage cost, total time, and the step
/// path across chains.
#[derive(Debug, Serialize)]
pub(super) struct QuotesDisplay {
    pub(super) quotes: Vec<QuoteSummary>,
}

#[derive(Debug, Serialize)]
pub(super) struct QuoteSummary {
    pub(super) index: usize,
    pub(super) bridge: String,
    /// Pretty-printed receive amount, with symbol when known
    /// (e.g. `"0.977102 USDC"`; falls back to raw integer + label).
    pub(super) buy_display: String,
    /// Same shape for the post-slippage floor.
    pub(super) min_buy_display: String,
    pub(super) estimated_time: String,
    /// Step path summary like `"swap (Base) → bridge (relay) → swap (Arbitrum)"`.
    /// Falls back to the bridge provider name when steps are empty.
    pub(super) path: String,
}

impl HumanDisplay for QuotesDisplay {
    fn display_human(&self, writer: &mut dyn Write, color: bool) -> io::Result<()> {
        use colored::Colorize;

        let title = "Cross-Chain Quotes";
        if color {
            writeln!(writer, "\n  {}", title.bold())?;
        } else {
            writeln!(writer, "\n  {title}")?;
        }
        writeln!(writer, "  {}", "─".repeat(45))?;

        for q in &self.quotes {
            let header = if color {
                format!(
                    "  [{}] {}",
                    q.index.to_string().bold().cyan(),
                    q.bridge.bold()
                )
            } else {
                format!("  [{}] {}", q.index, q.bridge)
            };
            writeln!(writer, "\n{header}")?;
            writeln!(writer, "      {:<14} {}", "You receive:", q.buy_display)?;
            writeln!(writer, "      {:<14} {}", "Min receive:", q.min_buy_display)?;
            writeln!(writer, "      {:<14} {}", "Bridge time:", q.estimated_time)?;
            writeln!(writer, "      {:<14} {}", "Path:", q.path)?;
        }
        writeln!(writer)?;
        Ok(())
    }
}

/// Build the human "swap (Base) → bridge (relay) → swap (Arbitrum)" path
/// from the API's step list. Each step's chain is resolved through the
/// chain registry (so the user sees "Base" not "8453"); unknown chains
/// fall back to the numeric id. Providers are appended in parens when
/// the API supplies them.
pub(super) fn build_step_path(quote: &crate::api::cross_chain::CrossChainQuote) -> String {
    if quote.steps.is_empty() {
        return quote.bridge_provider();
    }
    quote
        .steps
        .iter()
        .map(|s| {
            let chain_label = s.chain_id.as_ref().and_then(|v| {
                let id_str = v
                    .as_u64()
                    .map(|n| n.to_string())
                    .or_else(|| v.as_str().map(String::from))?;
                Some(
                    chain::resolve_chain(&id_str)
                        .map(|c| c.display_name.to_string())
                        .unwrap_or(id_str),
                )
            });
            let mut parts = vec![s.step_type.clone()];
            match (&chain_label, &s.provider) {
                (Some(c), Some(p)) => parts.push(format!("({c} / {p})")),
                (Some(c), None) => parts.push(format!("({c})")),
                (None, Some(p)) => parts.push(format!("({p})")),
                (None, None) => {}
            }
            parts.join(" ")
        })
        .collect::<Vec<_>>()
        .join(" → ")
}

/// Render a base-unit amount as a `"<formatted> <symbol>"` pair when
/// decimals are known, falling back to the raw integer when they aren't.
pub(super) fn format_quote_amount(raw: &str, buy: &SideMeta) -> String {
    let formatted = crate::api::types::display_amount(raw, buy.decimals);
    format!("{formatted} {}", buy.label())
}

/// Assemble a `CrossChainOutput` from the selected quote + outcome. Used by
/// the needs-confirmation, dry-run, and final-status paths.
#[allow(clippy::too_many_arguments)]
pub(super) fn cross_chain_output(
    origin: &chain::ChainInfo,
    destination: &chain::ChainInfo,
    sell: &SideMeta,
    buy: &SideMeta,
    selected: &CrossChainQuote,
    status: &str,
    terminal: bool,
    successful: bool,
    origin_tx: Option<(String, String)>,
    dry_run: bool,
) -> CrossChainOutput {
    let (origin_tx_hash, origin_explorer_url) = match origin_tx {
        Some((hash, explorer)) => (Some(hash), Some(explorer)),
        None => (None, None),
    };

    CrossChainOutput {
        origin_chain: origin.display_name.to_string(),
        destination_chain: destination.display_name.to_string(),
        sell_token: sell.token_info(),
        buy_token: buy.token_info(),
        sell_amount: sell.amount(&selected.sell_amount),
        buy_amount: buy.amount(&selected.buy_amount),
        min_buy_amount: buy.amount(&selected.min_buy_amount),
        rate: compute_rate(&selected.sell_amount, &selected.buy_amount),
        bridge: selected.bridge_provider(),
        route: Vec::new(),
        estimated_time_seconds: selected.estimated_time_seconds,
        status: status.to_string(),
        terminal,
        successful,
        origin_tx_hash,
        origin_explorer_url,
        dry_run,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::cross_chain::{
        CrossChainQuote, CrossChainStep, CrossChainTransaction, CrossChainTxDetails,
    };
    use crate::token_cache::TokenMeta;

    fn step(step_type: &str, chain_id_num: Option<u64>, provider: Option<&str>) -> CrossChainStep {
        CrossChainStep {
            step_type: step_type.to_string(),
            chain_id: chain_id_num.map(|n| serde_json::json!(n)),
            sell_token: None,
            buy_token: None,
            sell_amount: None,
            buy_amount: None,
            provider: provider.map(String::from),
            estimated_time_seconds: None,
        }
    }

    fn quote_with_steps(steps: Vec<CrossChainStep>) -> CrossChainQuote {
        CrossChainQuote {
            sell_amount: "1000000".into(),
            buy_amount: "977102".into(),
            min_buy_amount: "974129".into(),
            steps,
            transaction: CrossChainTransaction {
                chain_type: "evm".into(),
                details: CrossChainTxDetails {
                    to: None,
                    data: None,
                    gas: None,
                    gas_price: None,
                    value: None,
                    serialized_transaction: None,
                    owner_address: None,
                },
            },
            gas_costs: None,
            issues: None,
            estimated_time_seconds: Some(1),
            quote_id: None,
        }
    }

    #[test]
    fn step_path_with_bridge_only() {
        let q = quote_with_steps(vec![step("bridge", None, Some("relay"))]);
        assert_eq!(build_step_path(&q), "bridge (relay)");
    }

    #[test]
    fn step_path_swap_bridge_swap() {
        let q = quote_with_steps(vec![
            step("swap", Some(8453), Some("uniswap_v3")),
            step("bridge", None, Some("across")),
            step("swap", Some(42161), Some("uniswap_v3")),
        ]);
        // Chain ids resolve to display names via the registry.
        assert_eq!(
            build_step_path(&q),
            "swap (Base / uniswap_v3) → bridge (across) → swap (Arbitrum / uniswap_v3)"
        );
    }

    #[test]
    fn step_path_unknown_chain_falls_back_to_numeric_id() {
        let q = quote_with_steps(vec![step("swap", Some(99999), Some("dex"))]);
        assert_eq!(build_step_path(&q), "swap (99999 / dex)");
    }

    #[test]
    fn step_path_empty_falls_back_to_bridge_provider() {
        // No steps at all → use the existing bridge_provider() helper,
        // which scans steps for a "bridge" type and ends up returning
        // "unknown".
        let q = quote_with_steps(vec![]);
        assert_eq!(build_step_path(&q), "unknown");
    }

    #[test]
    fn format_quote_amount_with_known_decimals() {
        let buy = SideMeta::from_meta(
            "0x4200000000000000000000000000000000000006".into(),
            Some(TokenMeta {
                symbol: "WETH".into(),
                decimals: 18,
            }),
        );
        assert_eq!(
            format_quote_amount("1000000000000000000", &buy),
            "1.000000000000000000 WETH"
        );
    }

    #[test]
    fn format_quote_amount_unknown_decimals_falls_back_to_raw() {
        let buy = SideMeta::from_meta("0xaaaa".into(), None);
        let out = format_quote_amount("1000000", &buy);
        // Raw integer kept verbatim; label is the address fallback.
        assert!(out.contains("1000000"));
    }
}
