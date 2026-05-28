use crate::api::ApiClient;
use crate::chain;
use crate::cli::{ApprovalStrategy, SwapArgs};
use crate::config;
use crate::confirm::{confirm_or_preview, ConfirmFlow, TradeSummary};
use crate::error::{CliError, ErrorCode};
use crate::output::envelope::{Metadata, Warning};
use crate::output::trade::SideMeta;
use crate::output::OutputHandler;
use crate::token_cache::TokenMeta;

use super::swap::{truncate_address, SwapOutput};

/// Best-effort metadata lookup for a Solana mint. We don't fetch on-chain
/// metadata for Solana mints today, so this only covers a few well-known
/// mints. Returns `None` to mean "unknown — fall back to raw integers".
fn known_solana_meta(mint: &str) -> Option<TokenMeta> {
    let (symbol, decimals) = match mint {
        "So11111111111111111111111111111111111111112" => ("SOL", 9),
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v" => ("USDC", 6),
        "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB" => ("USDT", 6),
        _ => return None,
    };
    Some(TokenMeta {
        symbol: symbol.to_string(),
        decimals,
    })
}

/// Execute a Solana swap. Reached from `commands::swap::run` when the chain
/// resolves to Solana.
pub async fn run(
    args: &SwapArgs,
    output: &OutputHandler,
    config: &config::types::AppConfig,
    global: &crate::GlobalOpts,
) -> Result<i32, CliError> {
    let chain_info = chain::resolve_chain("solana")?;

    let api_key = global
        .api_key
        .as_deref()
        .or(config.api.api_key.as_deref())
        .ok_or_else(CliError::api_key_missing)?
        .to_string();

    let keypair = crate::wallet::solana::load_solana_keypair(config, global.wallet.as_deref())?;
    let taker = crate::wallet::solana::pubkey_string(&keypair);

    let mut metadata = Metadata::for_chain(chain_info);

    let sell = SideMeta::from_meta(args.sell.clone(), known_solana_meta(&args.sell));
    let buy = SideMeta::from_meta(args.buy.clone(), known_solana_meta(&args.buy));

    // EVM-only flags on a Solana swap are silently ignored except for an
    // explicit non-default approval, which we surface so the user knows.
    // Pushed as a structured warning so JSON consumers see it; the OutputHandler
    // renders these to stderr in Human mode too.
    let mut ignored_flag_warnings: Vec<Warning> = Vec::new();
    if !matches!(args.approval, ApprovalStrategy::Exact) {
        ignored_flag_warnings.push(Warning {
            code: "FLAG_IGNORED".into(),
            message: "--approval is EVM-only and was ignored for this Solana swap".into(),
        });
    }
    if args.recipient.is_some() {
        ignored_flag_warnings.push(Warning {
            code: "FLAG_IGNORED".into(),
            message: "--recipient is EVM-only and was ignored for this Solana swap".into(),
        });
    }

    let client = ApiClient::new(api_key, global.timeout)?;

    chain::validate_base_unit_amount(&args.amount)?;
    let amount_in: u64 = args.amount.parse().map_err(|_| CliError::Api {
        code: ErrorCode::InputInvalid,
        message: format!("Amount '{}' overflows u64 (max ~1.8e19 base units)", args.amount),
        status: None,
        details: None,
        suggestion: None,
    })?;

    let spinner = output.spinner_guard("Fetching Solana swap instructions...");
    let swap_req = crate::api::solana_swap::SolanaSwapRequest {
        token_in: args.sell.clone(),
        token_out: args.buy.clone(),
        amount_in,
        slippage_bps: args.slippage,
        taker: taker.clone(),
    };

    let swap_resp = client.get_solana_swap(&swap_req).await?;
    metadata.zid = swap_resp.zid.clone();
    drop(spinner);

    let summary = TradeSummary::new("Solana Swap")
        .row("Sell", format!("{} ({})", args.amount, args.sell))
        .row("Buy", format!("~{} ({})", swap_resp.amount_out, args.buy))
        .row("Slippage", format!("{:.2}%", args.slippage as f64 / 100.0))
        .row("Taker", truncate_address(&taker));

    let preview = solana_swap_output(
        chain_info,
        &sell,
        &buy,
        &args.amount,
        swap_resp.amount_out,
        amount_in,
        None,
        false,
        true,
    );
    match confirm_or_preview(
        output,
        global.yes,
        &summary,
        "swap",
        &preview,
        metadata.clone(),
        ignored_flag_warnings.clone(),
    )? {
        ConfirmFlow::Confirmed => {}
        ConfirmFlow::PreviewEmitted => return Ok(25),
    }

    let rpc_url =
        config::resolve_rpc_url_with_override(global.rpc_url.as_deref(), config, chain_info)?;

    let spinner = output.spinner_guard("Executing Solana swap...");

    let result = crate::chain::solana::execute_solana_swap(
        &rpc_url,
        &keypair,
        &swap_resp,
        global.dry_run,
        &|status| {
            spinner.set_message(status.to_string());
        },
    )
    .await?;

    drop(spinner);

    let (tx_hash, explorer_url, dry_run, exit_code) = match result {
        crate::chain::solana::SolanaSwapResult::Success { signature } => {
            let explorer = chain_info.explorer_tx_url(&signature);
            (Some(signature), Some(explorer), false, 0)
        }
        crate::chain::solana::SolanaSwapResult::DryRun => (None, None, true, 30),
    };

    let out = solana_swap_output(
        chain_info,
        &sell,
        &buy,
        &args.amount,
        swap_resp.amount_out,
        amount_in,
        tx_hash.map(|h| (h, explorer_url.unwrap_or_default())),
        dry_run,
        false,
    );

    Ok(output.emit_success("swap", &out, metadata, ignored_flag_warnings, exit_code))
}

/// Assemble a `SwapOutput` for a Solana swap. Centralises the rate
/// computation + raw-amount-to-`TokenAmount` conversion so the confirmation
/// preview and the final-result paths can't drift.
#[allow(clippy::too_many_arguments)]
fn solana_swap_output(
    chain_info: &chain::ChainInfo,
    sell: &SideMeta,
    buy: &SideMeta,
    sell_amount_raw: &str,
    buy_amount_raw: u64,
    sell_amount_in: u64,
    tx: Option<(String, String)>,
    dry_run: bool,
    needs_confirmation: bool,
) -> SwapOutput {
    let buy_amount_raw_s = buy_amount_raw.to_string();
    let rate = if sell_amount_in > 0 {
        format!("{:.10}", buy_amount_raw as f64 / sell_amount_in as f64)
    } else {
        "N/A".to_string()
    };

    let (tx_hash, explorer_url) = match tx {
        Some((hash, explorer)) => (Some(hash), Some(explorer)),
        None => (None, None),
    };

    SwapOutput {
        chain: chain_info.display_name.to_string(),
        sell_token: sell.token_info(),
        buy_token: buy.token_info(),
        sell_amount: sell.amount(sell_amount_raw),
        buy_amount: buy.amount(&buy_amount_raw_s),
        min_buy_amount: buy.amount(&buy_amount_raw_s),
        rate,
        gas_used: None,
        effective_gas_price: None,
        route: Vec::new(),
        tx_hash,
        explorer_url,
        block_number: None,
        dry_run,
        needs_confirmation,
    }
}
