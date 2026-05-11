use crate::api::evm_swap::QuoteResponse;
use crate::api::types::{compute_rate, RouteSource, TokenAmount, TokenInfo};
use crate::api::ApiClient;
use crate::chain;
use crate::chain::evm::{EvmExecutor, SwapResult};
use crate::cli::SwapArgs;
use crate::config;
use crate::confirm::{confirm_trade, ConfirmResult, TradeSummary};
use crate::error::{CliError, ErrorCode};
use crate::output::envelope::{Metadata, Warning};
use crate::output::{HumanDisplay, OutputHandler};
use crate::token_cache::TokenCache;
use serde::Serialize;
use std::io::{self, Write};

/// Full swap result for JSON output. Shared by EVM and Solana swap commands.
#[derive(Debug, Serialize)]
pub struct SwapOutput {
    pub chain: String,
    pub sell_token: TokenInfo,
    pub buy_token: TokenInfo,
    pub sell_amount: TokenAmount,
    pub buy_amount: TokenAmount,
    pub min_buy_amount: TokenAmount,
    pub rate: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gas_used: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_gas_price: Option<String>,
    pub route: Vec<RouteSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explorer_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_number: Option<u64>,
    pub dry_run: bool,
}

impl HumanDisplay for SwapOutput {
    fn display_human(&self, writer: &mut dyn Write, color: bool) -> io::Result<()> {
        use colored::Colorize;

        if self.dry_run {
            if color {
                writeln!(writer, "\n  {}", "Dry Run Complete".bold().yellow())?;
            } else {
                writeln!(writer, "\n  Dry Run Complete")?;
            }
        } else if color {
            writeln!(writer, "\n  {}", "Swap Executed Successfully".bold().green())?;
        } else {
            writeln!(writer, "\n  Swap Executed Successfully")?;
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

        writeln!(writer, "  {:<12} {} {}", "Sell:", self.sell_amount.formatted, sell_label)?;

        let buy_usd = self
            .buy_amount
            .usd_value
            .as_ref()
            .map(|v| format!(" (~${v})"))
            .unwrap_or_default();
        writeln!(
            writer,
            "  {:<12} {} {}{}",
            "Buy:", self.buy_amount.formatted, buy_label, buy_usd
        )?;

        writeln!(writer, "  {:<12} {}", "Rate:", self.rate)?;
        writeln!(
            writer,
            "  {:<12} {} {}",
            "Min Buy:", self.min_buy_amount.formatted, buy_label
        )?;

        if !self.route.is_empty() {
            let route_str = self
                .route
                .iter()
                .map(|s| {
                    if s.proportion.is_empty() {
                        s.name.clone()
                    } else {
                        format!("{} {}", s.proportion, s.name)
                    }
                })
                .collect::<Vec<_>>()
                .join(" → ");
            writeln!(writer, "  {:<12} {}", "Route:", route_str)?;
        }

        if let Some(ref gas) = self.gas_used {
            writeln!(writer, "  {:<12} {}", "Gas Used:", gas)?;
        }

        if let Some(ref hash) = self.tx_hash {
            writeln!(writer, "  {:<12} {}", "Tx Hash:", hash)?;
        }
        if let Some(ref url) = self.explorer_url {
            writeln!(writer, "  {:<12} {}", "Explorer:", url)?;
        }

        Ok(())
    }
}

pub async fn run(
    args: &SwapArgs,
    output: &OutputHandler,
    global: &crate::GlobalOpts,
) -> Result<i32, CliError> {
    let config = config::load_config()?;
    let chain_info = chain::resolve_chain(&args.chain)?;

    chain::validate_token_address(&args.sell, chain_info)?;
    chain::validate_token_address(&args.buy, chain_info)?;

    if chain_info.is_solana() {
        return super::solana_swap::run(args, output, &config, global).await;
    }

    if args.gasless {
        return super::gasless::run_gasless(args, output, global).await;
    }

    run_evm_swap(args, output, global, &config, chain_info).await
}

async fn run_evm_swap(
    args: &SwapArgs,
    output: &OutputHandler,
    global: &crate::GlobalOpts,
    config: &config::types::AppConfig,
    chain_info: &chain::ChainInfo,
) -> Result<i32, CliError> {
    let chain_id = chain_info.numeric_id().unwrap();

    let api_key = global
        .api_key
        .as_deref()
        .or(config.api.api_key.as_deref())
        .ok_or_else(CliError::api_key_missing)?
        .to_string();

    let signer = crate::wallet::evm::load_evm_signer(config, global.wallet.as_deref())?;
    let taker_address = format!("{:?}", signer.address());

    let mut metadata = Metadata {
        chain_id: Some(chain_id),
        chain_name: Some(chain_info.display_name.to_string()),
        ..Default::default()
    };

    let client = ApiClient::new(api_key, global.timeout)?;

    // Step 1: Get quote
    let spinner = output.spinner("Fetching Allowance Holder quote...");
    let quote = client
        .get_evm_quote(
            chain_id,
            &args.sell,
            &args.buy,
            &args.amount,
            &taker_address,
            Some(args.slippage),
            args.recipient.as_deref(),
        )
        .await?;

    metadata.zid = quote.zid.clone();

    // Resolve token metadata for correct decimal display
    let rpc_url_for_meta = config::try_resolve_rpc_url(config, chain_info);
    let mut token_cache = TokenCache::new();
    let (sell_dec, sell_sym, buy_dec, buy_sym) = if let Some(ref rpc) = rpc_url_for_meta {
        if let Some(s) = &spinner {
            s.set_message("Resolving token metadata...");
        }
        let sm = token_cache.resolve_evm(rpc, &quote.sell_token).await;
        let bm = token_cache.resolve_evm(rpc, &quote.buy_token).await;
        (sm.decimals, Some(sm.symbol), bm.decimals, Some(bm.symbol))
    } else {
        (18, None, 18, None)
    };

    if let Some(s) = &spinner {
        s.finish_and_clear();
    }

    if quote.liquidity_available == Some(false) {
        return Err(CliError::Api {
            code: ErrorCode::NoLiquidity,
            message: "No liquidity available for this token pair".into(),
            status: None,
            details: None,
            suggestion: Some("Try a different token pair or amount".into()),
        });
    }

    let route = quote
        .route
        .as_ref()
        .map(|r| r.sources())
        .unwrap_or_default();

    let route_str = if route.is_empty() {
        "Direct".to_string()
    } else {
        route
            .iter()
            .map(|s| {
                if s.proportion.is_empty() {
                    s.name.clone()
                } else {
                    format!("{} {}", s.proportion, s.name)
                }
            })
            .collect::<Vec<_>>()
            .join(" → ")
    };

    let sell_display = crate::api::types::format_amount(&quote.sell_amount, sell_dec);
    let buy_display = crate::api::types::format_amount(&quote.buy_amount, buy_dec);
    let min_buy_display = crate::api::types::format_amount(&quote.min_buy_amount, buy_dec);

    let sell_label = sell_sym.as_deref().unwrap_or(&args.sell);
    let buy_label = buy_sym.as_deref().unwrap_or(&args.buy);

    let mut summary = TradeSummary::new(format!("Swap on {}", chain_info.display_name))
        .row("Sell", format!("{sell_display} {sell_label}"))
        .row("Buy", format!("{buy_display} {buy_label}"))
        .row("Min Buy", format!("{min_buy_display} {buy_label}"))
        .row("Slippage", format!("{:.2}%", args.slippage as f64 / 100.0))
        .row("Route", route_str);

    let needs_approval = quote
        .issues
        .as_ref()
        .and_then(|i| i.allowance.as_ref())
        .is_some();

    if needs_approval {
        let spender = quote
            .issues
            .as_ref()
            .unwrap()
            .allowance
            .as_ref()
            .unwrap()
            .spender
            .clone();
        summary = summary.warning(format!(
            "Approval needed: {} → {}",
            args.sell,
            truncate_address(&spender)
        ));
    }

    match confirm_trade(output.format, global.yes, output.color, &summary)? {
        ConfirmResult::Confirmed => {}
        ConfirmResult::NeedsConfirmation => {
            let (preview, preview_warnings) = build_swap_output(
                chain_info,
                &quote,
                &route,
                SwapResult::DryRun,
                sell_dec,
                &sell_sym,
                buy_dec,
                &buy_sym,
            );
            let _ = output.success("swap", &preview, metadata.clone(), preview_warnings);
            return Ok(20);
        }
    }

    let rpc_url = if let Some(ref url) = global.rpc_url {
        url.clone()
    } else {
        config::resolve_rpc_url(config, chain_info)?
    };

    let spender = quote
        .issues
        .as_ref()
        .and_then(|i| i.allowance.as_ref())
        .map(|a| a.spender.as_str());

    let tx = quote.transaction.as_ref().ok_or_else(|| CliError::Api {
        code: ErrorCode::ApiError,
        message: "Quote response missing transaction data".into(),
        status: None,
        details: None,
        suggestion: Some("Try fetching a new quote".into()),
    })?;

    let spinner = output.spinner("Executing swap...");

    let result = EvmExecutor::execute_swap(
        &rpc_url,
        signer,
        &args.sell,
        spender,
        &quote.sell_amount,
        args.approval,
        &tx.to,
        &tx.data,
        &tx.value,
        tx.gas.as_deref(),
        tx.gas_price.as_deref(),
        global.dry_run,
        &|status| {
            if let Some(s) = &spinner {
                s.set_message(status.to_string());
            }
        },
    )
    .await?;

    if let Some(s) = spinner {
        s.finish_and_clear();
    }

    let (swap_output, warnings) = build_swap_output(
        chain_info,
        &quote,
        &route,
        result,
        sell_dec,
        &sell_sym,
        buy_dec,
        &buy_sym,
    );

    let exit_code = if swap_output.dry_run { 30 } else { 0 };
    output
        .success("swap", &swap_output, metadata, warnings)
        .map(|_| exit_code)
        .map_err(|e| CliError::config(ErrorCode::Unknown, e.to_string()))
}

#[allow(clippy::too_many_arguments)]
fn build_swap_output(
    chain_info: &chain::ChainInfo,
    quote: &QuoteResponse,
    route: &[RouteSource],
    result: SwapResult,
    sell_decimals: u8,
    sell_symbol: &Option<String>,
    buy_decimals: u8,
    buy_symbol: &Option<String>,
) -> (SwapOutput, Vec<Warning>) {
    let mut warnings = Vec::new();

    if quote
        .issues
        .as_ref()
        .map(|i| i.simulation_incomplete)
        .unwrap_or(false)
    {
        warnings.push(Warning {
            code: "SIMULATION_INCOMPLETE".into(),
            message: "Quote simulation was incomplete — actual amounts may differ".into(),
        });
    }

    let rate = compute_rate(&quote.sell_amount, &quote.buy_amount);

    let sell_info = TokenInfo {
        address: quote.sell_token.clone(),
        symbol: sell_symbol.clone(),
        decimals: Some(sell_decimals),
    };
    let buy_info = TokenInfo {
        address: quote.buy_token.clone(),
        symbol: buy_symbol.clone(),
        decimals: Some(buy_decimals),
    };

    let (gas_used, effective_gas_price, tx_hash, explorer_url, block_number, dry_run) = match result
    {
        SwapResult::Success(receipt) => (
            Some(receipt.gas_used.to_string()),
            Some(receipt.effective_gas_price.to_string()),
            Some(receipt.tx_hash.clone()),
            Some(chain_info.explorer_tx_url(&receipt.tx_hash)),
            receipt.block_number,
            false,
        ),
        SwapResult::DryRun => (None, None, None, None, None, true),
    };

    let output = SwapOutput {
        chain: chain_info.display_name.to_string(),
        sell_token: sell_info,
        buy_token: buy_info,
        sell_amount: TokenAmount::new(&quote.sell_amount, sell_decimals),
        buy_amount: TokenAmount::new(&quote.buy_amount, buy_decimals),
        min_buy_amount: TokenAmount::new(&quote.min_buy_amount, buy_decimals),
        rate,
        gas_used,
        effective_gas_price,
        route: route.to_vec(),
        tx_hash,
        explorer_url,
        block_number,
        dry_run,
    };

    (output, warnings)
}

pub fn truncate_address(addr: &str) -> String {
    if addr.len() > 12 {
        format!("{}...{}", &addr[..6], &addr[addr.len() - 4..])
    } else {
        addr.to_string()
    }
}
