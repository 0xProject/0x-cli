use crate::api::types::{format_amount, TokenAmount, TokenInfo};
use crate::api::ApiClient;
use crate::chain;
use crate::cli::{ApprovalStrategy, SwapArgs};
use crate::config;
use crate::confirm::{confirm_trade, ConfirmResult, TradeSummary};
use crate::error::{CliError, ErrorCode};
use crate::output::envelope::Metadata;
use crate::output::OutputHandler;
use std::io::{self, Write};

use super::swap::{truncate_address, SwapOutput};

/// Best-effort decimals lookup for a Solana mint. We don't fetch on-chain
/// metadata for Solana mints today, so this only covers a few well-known
/// mints. Returns `None` to mean "unknown — fall back to the raw integer".
fn known_solana_decimals(mint: &str) -> Option<u8> {
    match mint {
        // Wrapped SOL
        "So11111111111111111111111111111111111111112" => Some(9),
        // USDC
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v" => Some(6),
        // USDT
        "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB" => Some(6),
        _ => None,
    }
}

fn format_solana_amount(raw: &str, mint: &str) -> String {
    match known_solana_decimals(mint) {
        Some(d) => format_amount(raw, d),
        None => raw.to_string(),
    }
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

    let mut metadata = Metadata {
        chain_id: None,
        chain_name: Some(chain_info.display_name.to_string()),
        ..Default::default()
    };

    // EVM-only flags on a Solana swap are silently ignored except for an
    // explicit non-default approval, which we surface so the user knows.
    if !matches!(args.approval, ApprovalStrategy::Exact) {
        output.info(
            "Note: --approval is EVM-only and will be ignored for Solana",
        );
    }
    if args.recipient.is_some() {
        output.info(
            "Note: --recipient is EVM-only and will be ignored for Solana",
        );
    }

    let client = ApiClient::new(api_key, global.timeout)?;

    let amount_in: u64 = args.amount.parse().map_err(|_| CliError::Api {
        code: ErrorCode::InputInvalid,
        message: format!(
            "Invalid amount '{}'. Use base units (lamports for SOL, raw units for tokens).",
            args.amount
        ),
        status: None,
        details: None,
        suggestion: Some("For SOL, 1 SOL = 1000000000 lamports".into()),
    })?;

    if amount_in == 0 {
        return Err(CliError::Api {
            code: ErrorCode::InputInvalid,
            message: "Amount must be greater than 0".into(),
            status: None,
            details: None,
            suggestion: None,
        });
    }

    let spinner = output.spinner("Fetching Solana swap instructions...");
    let swap_req = crate::api::solana_swap::SolanaSwapRequest {
        token_in: args.sell.clone(),
        token_out: args.buy.clone(),
        amount_in,
        slippage_bps: args.slippage,
        taker: taker.clone(),
    };

    let swap_resp = client.get_solana_swap(&swap_req).await?;
    metadata.zid = swap_resp.zid.clone();

    if let Some(s) = &spinner {
        s.finish_and_clear();
    }

    let summary = TradeSummary::new("Solana Swap")
        .row("Sell", format!("{} ({})", args.amount, args.sell))
        .row("Buy", format!("~{} ({})", swap_resp.amount_out, args.buy))
        .row("Slippage", format!("{:.2}%", args.slippage as f64 / 100.0))
        .row("Taker", truncate_address(&taker));

    match confirm_trade(output.format, global.yes, output.color, &summary)? {
        ConfirmResult::Confirmed => {}
        ConfirmResult::NeedsConfirmation => {
            let preview_data = serde_json::json!({
                "chain": chain_info.display_name,
                "sell": args.sell,
                "buy": args.buy,
                "amount_in": args.amount,
                "amount_out": swap_resp.amount_out,
                "needs_confirmation": true,
            });
            let stdout = io::stdout();
            let mut out = stdout.lock();
            let _ = serde_json::to_writer_pretty(&mut out, &preview_data);
            let _ = writeln!(out);
            return Ok(20);
        }
    }

    let rpc_url = config::resolve_rpc_url(config, chain_info)?;

    let spinner = output.spinner("Executing Solana swap...");

    let result = crate::chain::solana::execute_solana_swap(
        &rpc_url,
        &keypair,
        &swap_resp,
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

    let rate = format!("{:.10}", swap_resp.amount_out as f64 / amount_in as f64);
    let sell_dec = known_solana_decimals(&args.sell);
    let buy_dec = known_solana_decimals(&args.buy);
    let sell_token = TokenInfo {
        address: args.sell.clone(),
        symbol: None,
        decimals: sell_dec,
    };
    let buy_token = TokenInfo {
        address: args.buy.clone(),
        symbol: None,
        decimals: buy_dec,
    };
    let sell_amount = TokenAmount {
        raw: args.amount.clone(),
        formatted: format_solana_amount(&args.amount, &args.sell),
        usd_value: None,
    };
    let buy_amount_raw = swap_resp.amount_out.to_string();
    let buy_amount = TokenAmount {
        raw: buy_amount_raw.clone(),
        formatted: format_solana_amount(&buy_amount_raw, &args.buy),
        usd_value: None,
    };

    let (tx_hash, explorer_url, dry_run, exit_code) = match result {
        crate::chain::solana::SolanaSwapResult::Success { signature } => {
            let explorer = chain_info.explorer_tx_url(&signature);
            (Some(signature), Some(explorer), false, 0)
        }
        crate::chain::solana::SolanaSwapResult::DryRun => (None, None, true, 30),
    };

    let out = SwapOutput {
        chain: chain_info.display_name.to_string(),
        sell_token,
        buy_token,
        sell_amount,
        buy_amount: buy_amount.clone(),
        min_buy_amount: buy_amount,
        rate,
        gas_used: None,
        effective_gas_price: None,
        route: Vec::new(),
        tx_hash,
        explorer_url,
        block_number: None,
        dry_run,
    };

    output
        .success("swap", &out, metadata, Vec::new())
        .map(|_| exit_code)
        .map_err(|e| CliError::config(ErrorCode::Unknown, e.to_string()))
}
