//! `0x swap --gasless` — gasless (meta-transaction) swaps via the 0x Gasless
//! API. Orchestration lives here; EIP-712 signing and signature-safety
//! validation in [`eip712`]; output assembly in [`output`].

mod eip712;
mod output;

pub use output::GaslessSwapOutput;

use crate::api::gasless::{GaslessSubmitRequest, GaslessSubmitSignable};
use crate::api::types::display_amount;
use crate::api::ApiClient;
use crate::chain::{self};
use crate::cli::SwapArgs;
use crate::config;
use crate::confirm::{confirm_or_preview, ConfirmFlow, TradeSummary};
use crate::error::{CliError, ErrorCode};
use crate::output::envelope::{Metadata, Warning};
use crate::output::trade::SideMeta;
use crate::output::OutputHandler;
use crate::token_cache::{resolve_pair_evm, TokenCache};
use eip712::{sign_eip712, validate_approval, validate_trade_domain, validate_trade_permitted};
use output::gasless_output;

/// Execute a gasless swap.
pub async fn run_gasless(
    args: &SwapArgs,
    output: &OutputHandler,
    global: &crate::GlobalOpts,
) -> Result<i32, CliError> {
    let config = config::load_config()?;
    let chain_info = chain::resolve_chain(&args.chain)?;
    chain_info.reject_if_tron("gasless")?;
    let chain_id = chain_info.numeric_id().ok_or_else(|| CliError::Api {
        code: ErrorCode::InputInvalid,
        message: "Gasless swaps are only supported on EVM chains".into(),
        status: None,
        details: None,
        suggestion: Some("Use --chain with an EVM chain like 'base' or 'ethereum'".into()),
    })?;

    let signer = crate::wallet::evm::load_evm_signer(&config, global.wallet.as_deref())?;
    let taker = format!("{:?}", signer.address());

    let mut metadata = Metadata::for_chain(chain_info);
    let client = crate::api::client_for(global, &config, output)?;

    // Step 1: Get gasless quote. Gasless is exact-in only; the swap dispatcher
    // rejects --buy-amount before we get here, so this is the sell amount.
    let amount_spec = args.amount_spec();
    let spinner = output.spinner_guard("Fetching gasless quote...");
    let quote = client
        .get_gasless_quote(chain_id, &args.sell, &args.buy, amount_spec.value(), &taker)
        .await?;
    drop(spinner);

    // Populate zid
    metadata.zid = quote.zid.clone();

    // Balance shortfalls arrive inside the 200 quote response
    // (`issues.balance`), not as an API error — fail with
    // INSUFFICIENT_BALANCE before asking the user to confirm a doomed trade.
    if let Some(balance) = quote.issues.as_ref().and_then(|i| i.balance.as_ref()) {
        return Err(balance.to_error());
    }

    // Resolve token metadata for correct decimals
    let rpc_url =
        config::try_resolve_rpc_url_with_override(global.rpc_url.as_deref(), &config, chain_info);
    let mut cache = TokenCache::new();
    let mut metadata_warnings: Vec<Warning> = Vec::new();
    let (sell_meta, buy_meta) = resolve_pair_evm(
        &mut cache,
        rpc_url.as_deref(),
        chain_id,
        &quote.sell_token,
        &quote.buy_token,
        &mut metadata_warnings,
    )
    .await;
    let sell = SideMeta::from_meta(quote.sell_token.clone(), sell_meta);
    let buy = SideMeta::from_meta(quote.buy_token.clone(), buy_meta);

    // Show confirmation
    let sell_display = display_amount(&quote.sell_amount, sell.decimals);
    let buy_display = display_amount(&quote.buy_amount, buy.decimals);

    let summary = TradeSummary::new(format!("Gasless Swap on {}", chain_info.display_name))
        .row("Sell", format!("{sell_display} {}", sell.label()))
        .row("Buy", format!("{buy_display} {}", buy.label()))
        .row("Gas", "None (gasless)")
        .row("Slippage", format!("{:.2}%", args.slippage as f64 / 100.0));

    let preview = gasless_output(
        chain_info,
        &sell,
        &buy,
        &quote,
        String::new(),
        "needs_confirmation",
        None,
        false,
        false,
        false,
    );
    // Dry-run bypasses the confirmation gate (read-only path, nothing to sign).
    let auto_confirm = global.yes || global.dry_run;
    match confirm_or_preview(
        output,
        auto_confirm,
        &summary,
        "swap",
        &preview,
        metadata.clone(),
        metadata_warnings.clone(),
    )? {
        ConfirmFlow::Confirmed => {}
        ConfirmFlow::PreviewEmitted => return Ok(25),
    }

    // Dry-run: stop before signing and submitting. The gasless relayer would
    // execute the trade on submit, so there is no on-chain simulation step the
    // way there is for a regular EVM swap — the safe behavior is to exit with
    // the quoted amounts and skip the rest.
    if global.dry_run {
        let preview = gasless_output(
            chain_info,
            &sell,
            &buy,
            &quote,
            String::new(),
            "dry_run",
            None,
            true,
            true,
            true,
        );
        return Ok(output.emit_success("swap", &preview, metadata, metadata_warnings, 30));
    }

    // Step 2: Sign approval EIP-712 (if present)
    let spinner = output.spinner_guard("Signing trade...");

    let approval_signable = if let Some(ref approval) = quote.approval {
        validate_approval(
            &approval.signable_type,
            &approval.eip712,
            &quote.sell_token,
            &quote.sell_amount,
            signer.address(),
            chain_id,
        )?;
        let sig = sign_eip712(&signer, &approval.eip712)?;
        Some(GaslessSubmitSignable {
            signable_type: approval.signable_type.clone(),
            eip712: approval.eip712.clone(),
            signature: sig,
        })
    } else {
        None
    };

    // Step 3: Sign trade EIP-712
    let trade = quote.trade.as_ref().ok_or_else(|| CliError::Api {
        code: ErrorCode::ApiError,
        message: "Gasless quote missing trade data".into(),
        status: None,
        details: None,
        suggestion: None,
    })?;

    validate_trade_domain(&trade.eip712, &trade.signable_type, chain_id)?;
    // Bind the Permit2 trade signable to the quote's sellToken/sellAmount
    // so a tampered API response can't reroute the trade to a different
    // asset between quote and signature.
    validate_trade_permitted(&trade.eip712, &quote.sell_token, &quote.sell_amount)?;

    let trade_sig = sign_eip712(&signer, &trade.eip712)?;
    let trade_signable = GaslessSubmitSignable {
        signable_type: trade.signable_type.clone(),
        eip712: trade.eip712.clone(),
        signature: trade_sig,
    };

    drop(spinner);

    // Step 4: Submit
    let spinner = output.spinner_guard("Submitting gasless swap...");
    let submit_req = GaslessSubmitRequest {
        chain_id,
        trade: trade_signable,
        approval: approval_signable,
    };

    let submit_resp = client.submit_gasless(&submit_req).await?;

    drop(spinner);

    output.info(&format!("Trade hash: {}", submit_resp.trade_hash));

    // Step 5: Poll status
    let spinner = output.spinner_guard("Waiting for confirmation...");
    let final_status = poll_gasless_status(
        &client,
        &submit_resp.trade_hash,
        chain_id,
        spinner.progress_bar(),
    )
    .await?;
    drop(spinner);

    let tx_hash = final_status
        .transactions
        .first()
        .and_then(|t| t.hash.clone());

    let explorer_url = tx_hash.as_ref().map(|h| chain_info.explorer_tx_url(h));

    // `poll_gasless_status` only returns Ok on a terminal state, so we know
    // `final_status.is_terminal()` is true here. Non-terminal states surface
    // as `CliError::Timeout` (exit code 12) and are handled by the `?` above.
    let tx = tx_hash.zip(explorer_url);
    let result = gasless_output(
        chain_info,
        &sell,
        &buy,
        &quote,
        submit_resp.trade_hash,
        &final_status.status,
        tx,
        true,
        final_status.is_successful(),
        false,
    );

    let mut warnings = metadata_warnings;
    if !final_status.is_successful() {
        warnings.push(Warning {
            code: "TRADE_FAILED".into(),
            message: format!("Trade ended with status: {}", final_status.status),
        });
    }

    let exit_code = if final_status.is_successful() { 0 } else { 11 };

    Ok(output.emit_success("swap", &result, metadata, warnings, exit_code))
}

/// Poll gasless status until terminal state. ~5 min total, 5 s interval.
async fn poll_gasless_status(
    client: &ApiClient,
    trade_hash: &str,
    chain_id: u64,
    spinner: Option<&indicatif::ProgressBar>,
) -> Result<crate::api::gasless::GaslessStatusResponse, CliError> {
    crate::api::poll::poll_until_terminal(
        // 10-minute total budget — gasless relayers normally land within
        // 30s but congested L2 windows or paused operator queues can take
        // several minutes; 5 minutes was too tight in practice.
        crate::api::poll::PollConfig::new(5, 600, ErrorCode::TransactionTimeout),
        |elapsed, s: &crate::api::gasless::GaslessStatusResponse| {
            if let Some(sp) = spinner {
                sp.set_message(format!("Status: {} ({}s elapsed)", s.status, elapsed));
            }
        },
        || client.get_gasless_status(trade_hash, chain_id),
        |s| s.is_terminal(),
        || {
            format!(
                "Gasless trade not confirmed after 300s. Check with: 0x status {trade_hash} --type gasless --chain {chain_id}"
            )
        },
    )
    .await
}
