use crate::api::evm_swap::QuoteResponse;
use crate::api::types::{compute_rate, RouteSource, TokenAmount, TokenInfo};
use crate::api::ApiClient;
use crate::chain;
use crate::chain::evm::{EvmExecutor, SwapResult};
use crate::cli::SwapArgs;
use crate::config;
use crate::confirm::{confirm_or_preview, ConfirmFlow, TradeSummary};
use crate::error::{CliError, ErrorCode};
use crate::output::envelope::{Metadata, Warning};
use crate::output::trade::SideMeta;
use crate::output::{HumanDisplay, OutputHandler};
use crate::token_cache::{resolve_pair_evm, TokenCache};
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
    /// Preview-only result that should NOT be interpreted as a completed
    /// simulation. Set when `confirm_trade` returned `NeedsConfirmation` and
    /// the command exited early with code 25.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub needs_confirmation: bool,
}

impl HumanDisplay for SwapOutput {
    fn display_human(&self, writer: &mut dyn Write, color: bool) -> io::Result<()> {
        use colored::Colorize;

        if self.needs_confirmation {
            if color {
                writeln!(writer, "\n  {}", "Quote Preview (needs confirmation)".bold().yellow())?;
            } else {
                writeln!(writer, "\n  Quote Preview (needs confirmation)")?;
            }
        } else if self.dry_run {
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

        writeln!(writer, "  {:<12} {} {}", "Sell:", self.sell_amount.display(), sell_label)?;

        let buy_usd = self
            .buy_amount
            .usd_value
            .as_ref()
            .map(|v| format!(" (~${v})"))
            .unwrap_or_default();
        writeln!(
            writer,
            "  {:<12} {} {}{}",
            "Buy:", self.buy_amount.display(), buy_label, buy_usd
        )?;

        writeln!(writer, "  {:<12} {}", "Rate:", self.rate)?;
        writeln!(
            writer,
            "  {:<12} {} {}",
            "Min Buy:", self.min_buy_amount.display(), buy_label
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
    chain::validate_base_unit_amount(&args.amount)?;
    if let Some(ref recipient) = args.recipient {
        chain::validate_token_address(recipient, chain_info)?;
    }

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

    let mut metadata = Metadata::for_chain(chain_info);
    let client = ApiClient::new(api_key, global.timeout)?;

    // Step 1: Get quote. Spinner is cleared automatically on Drop, so a `?`
    // early-return from the API call doesn't leak tick characters.
    let spinner = output.spinner_guard("Fetching Allowance Holder quote...");
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
    let rpc_url_for_meta =
        config::try_resolve_rpc_url_with_override(global.rpc_url.as_deref(), config, chain_info);
    spinner.set_message("Resolving token metadata...");
    let mut token_cache = TokenCache::new();
    let mut metadata_warnings: Vec<Warning> = Vec::new();
    let (sell_meta, buy_meta) = resolve_pair_evm(
        &mut token_cache,
        rpc_url_for_meta.as_deref(),
        chain_id,
        &quote.sell_token,
        &quote.buy_token,
        &mut metadata_warnings,
    )
    .await;
    let sell = SideMeta::from_meta(quote.sell_token.clone(), sell_meta);
    let buy = SideMeta::from_meta(quote.buy_token.clone(), buy_meta);

    drop(spinner);

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

    let sell_display = crate::api::types::display_amount(&quote.sell_amount, sell.decimals);
    let buy_display = crate::api::types::display_amount(&quote.buy_amount, buy.decimals);
    let min_buy_display = crate::api::types::display_amount(&quote.min_buy_amount, buy.decimals);

    let mut summary = TradeSummary::new(format!("Swap on {}", chain_info.display_name))
        .row("Sell", format!("{sell_display} {}", sell.label()))
        .row("Buy", format!("{buy_display} {}", buy.label()))
        .row("Min Buy", format!("{min_buy_display} {}", buy.label()))
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

    let (preview, mut preview_warnings) =
        build_swap_output(chain_info, &quote, &route, SwapResult::Preview, &sell, &buy);
    preview_warnings.extend(metadata_warnings.iter().cloned());
    match confirm_or_preview(
        output,
        global.yes,
        &summary,
        "swap",
        &preview,
        metadata.clone(),
        preview_warnings,
    )? {
        ConfirmFlow::Confirmed => {}
        ConfirmFlow::PreviewEmitted => return Ok(25),
    }

    let rpc_url =
        config::resolve_rpc_url_with_override(global.rpc_url.as_deref(), config, chain_info)?;

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

    let spinner = output.spinner_guard("Executing swap...");

    let result = EvmExecutor::execute_swap(
        &rpc_url,
        chain_id,
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
            spinner.set_message(status.to_string());
        },
    )
    .await?;

    drop(spinner);

    let (swap_output, mut warnings) =
        build_swap_output(chain_info, &quote, &route, result, &sell, &buy);
    warnings.extend(metadata_warnings);

    let exit_code = if swap_output.dry_run { 30 } else { 0 };
    Ok(output.emit_success("swap", &swap_output, metadata, warnings, exit_code))
}

fn build_swap_output(
    chain_info: &chain::ChainInfo,
    quote: &QuoteResponse,
    route: &[RouteSource],
    result: SwapResult,
    sell: &SideMeta,
    buy: &SideMeta,
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

    let (gas_used, effective_gas_price, tx_hash, explorer_url, block_number, dry_run, needs_confirmation) =
        match result {
            SwapResult::Success(receipt) => (
                Some(receipt.gas_used.to_string()),
                Some(receipt.effective_gas_price.to_string()),
                Some(receipt.tx_hash.clone()),
                Some(chain_info.explorer_tx_url(&receipt.tx_hash)),
                receipt.block_number,
                false,
                false,
            ),
            SwapResult::DryRun => (None, None, None, None, None, true, false),
            SwapResult::Preview => (None, None, None, None, None, false, true),
        };

    let output = SwapOutput {
        chain: chain_info.display_name.to_string(),
        sell_token: sell.token_info(),
        buy_token: buy.token_info(),
        sell_amount: sell.amount(&quote.sell_amount),
        buy_amount: buy.amount(&quote.buy_amount),
        min_buy_amount: buy.amount(&quote.min_buy_amount),
        rate,
        gas_used,
        effective_gas_price,
        route: route.to_vec(),
        tx_hash,
        explorer_url,
        block_number,
        dry_run,
        needs_confirmation,
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
