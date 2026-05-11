use crate::api::gasless::{
    GaslessSubmitRequest, GaslessSubmitSignable, SignatureSplit,
};
use crate::api::types::{format_amount, TokenAmount, TokenInfo};
use crate::api::ApiClient;
use crate::chain::{self};
use crate::cli::SwapArgs;
use crate::config;
use crate::confirm::{confirm_trade, ConfirmResult, TradeSummary};
use crate::error::{CliError, ErrorCode};
use crate::output::envelope::{Metadata, Warning};
use crate::output::{HumanDisplay, OutputHandler};
use crate::token_cache::TokenCache;
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::SignerSync;
use alloy_dyn_abi::eip712::TypedData;
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
}

impl HumanDisplay for GaslessSwapOutput {
    fn display_human(&self, writer: &mut dyn Write, color: bool) -> io::Result<()> {
        use colored::Colorize;

        if self.successful {
            if color {
                writeln!(writer, "\n  {}", "Gasless Swap Complete".bold().green())?;
            } else {
                writeln!(writer, "\n  Gasless Swap Complete")?;
            }
        } else if color {
            writeln!(writer, "\n  {}", format!("Gasless Swap: {}", self.status).bold())?;
        } else {
            writeln!(writer, "\n  Gasless Swap: {}", self.status)?;
        }

        writeln!(writer, "  {}", "-".repeat(45))?;

        let sell_label = self.sell_token.symbol.as_deref().unwrap_or(&self.sell_token.address);
        let buy_label = self.buy_token.symbol.as_deref().unwrap_or(&self.buy_token.address);

        writeln!(writer, "  {:<14} {} {}", "Sell:", self.sell_amount.formatted, sell_label)?;
        writeln!(writer, "  {:<14} {} {}", "Buy:", self.buy_amount.formatted, buy_label)?;
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

/// Execute a gasless swap.
pub async fn run_gasless(
    args: &SwapArgs,
    output: &OutputHandler,
    global: &crate::GlobalOpts,
) -> Result<i32, CliError> {
    let config = config::load_config()?;
    let chain_info = chain::resolve_chain(&args.chain)?;
    let chain_id = chain_info.numeric_id().ok_or_else(|| CliError::Api {
        code: ErrorCode::InputInvalid,
        message: "Gasless swaps are only supported on EVM chains".into(),
        status: None,
        details: None,
        suggestion: Some("Use --chain with an EVM chain like 'base' or 'ethereum'".into()),
    })?;

    let api_key = global
        .api_key
        .as_deref()
        .or(config.api.api_key.as_deref())
        .ok_or_else(CliError::api_key_missing)?
        .to_string();

    let signer = crate::wallet::evm::load_evm_signer(&config, global.wallet.as_deref())?;
    let taker = format!("{:?}", signer.address());

    let mut metadata = Metadata {
        chain_id: Some(chain_id),
        chain_name: Some(chain_info.display_name.to_string()),
        ..Default::default()
    };

    let client = ApiClient::new(api_key, global.timeout)?;

    // Step 1: Get gasless quote
    let spinner = output.spinner("Fetching gasless quote...");
    let quote = client
        .get_gasless_quote(chain_id, &args.sell, &args.buy, &args.amount, &taker)
        .await?;

    if let Some(s) = &spinner {
        s.finish_and_clear();
    }

    // Populate zid
    metadata.zid = quote.zid.clone();

    if quote.liquidity_available == Some(false) {
        return Err(CliError::Api {
            code: ErrorCode::NoLiquidity,
            message: "No liquidity available for this gasless swap".into(),
            status: None,
            details: None,
            suggestion: Some("Try a different token pair or amount, or try without --gasless".into()),
        });
    }

    // Resolve token metadata for correct decimals
    let rpc_url = config::try_resolve_rpc_url(&config, chain_info);
    let mut cache = TokenCache::new();
    let (sell_dec, sell_sym, buy_dec, buy_sym) = if let Some(ref rpc) = rpc_url {
        let sm = cache.resolve_evm(rpc, &quote.sell_token).await;
        let bm = cache.resolve_evm(rpc, &quote.buy_token).await;
        (sm.decimals, Some(sm.symbol), bm.decimals, Some(bm.symbol))
    } else {
        (18, None, 18, None)
    };

    // Show confirmation
    let sell_display = format_amount(&quote.sell_amount, sell_dec);
    let buy_display = format_amount(&quote.buy_amount, buy_dec);

    let sell_label = sell_sym.as_deref().unwrap_or(&args.sell);
    let buy_label = buy_sym.as_deref().unwrap_or(&args.buy);

    let summary = TradeSummary::new(format!("Gasless Swap on {}", chain_info.display_name))
        .row("Sell", format!("{sell_display} {sell_label}"))
        .row("Buy", format!("{buy_display} {buy_label}"))
        .row("Gas", "None (gasless)")
        .row("Slippage", format!("{:.2}%", args.slippage as f64 / 100.0));

    match confirm_trade(output.format, global.yes, output.color, &summary)? {
        ConfirmResult::Confirmed => {}
        ConfirmResult::NeedsConfirmation => {
            // Output quote preview for agent review
            let preview = GaslessSwapOutput {
                chain: chain_info.display_name.to_string(),
                sell_token: TokenInfo { address: quote.sell_token.clone(), symbol: sell_sym.clone(), decimals: Some(sell_dec) },
                buy_token: TokenInfo { address: quote.buy_token.clone(), symbol: buy_sym.clone(), decimals: Some(buy_dec) },
                sell_amount: TokenAmount::new(&quote.sell_amount, sell_dec),
                buy_amount: TokenAmount::new(&quote.buy_amount, buy_dec),
                min_buy_amount: TokenAmount::new(&quote.min_buy_amount, buy_dec),
                trade_hash: String::new(),
                status: "needs_confirmation".into(),
                tx_hash: None,
                explorer_url: None,
                terminal: false,
                successful: false,
            };
            let _ = output.success("swap", &preview, metadata, Vec::new());
            return Ok(20);
        }
    }

    // Step 2: Sign approval EIP-712 (if present)
    let spinner = output.spinner("Signing trade...");

    let approval_signable = if let Some(ref approval) = quote.approval {
        let sig = sign_eip712(&signer, &approval.eip712).await?;
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

    let trade_sig = sign_eip712(&signer, &trade.eip712).await?;
    let trade_signable = GaslessSubmitSignable {
        signable_type: trade.signable_type.clone(),
        eip712: trade.eip712.clone(),
        signature: trade_sig,
    };

    if let Some(s) = &spinner {
        s.finish_and_clear();
    }

    // Step 4: Submit
    let spinner = output.spinner("Submitting gasless swap...");
    let submit_req = GaslessSubmitRequest {
        chain_id,
        trade: trade_signable,
        approval: approval_signable,
    };

    let submit_resp = client.submit_gasless(&submit_req).await?;

    if let Some(s) = &spinner {
        s.finish_and_clear();
    }

    output.info(&format!("Trade hash: {}", submit_resp.trade_hash));

    // Step 5: Poll status
    let spinner = output.spinner("Waiting for confirmation...");
    let final_status =
        poll_gasless_status(&client, &submit_resp.trade_hash, chain_id, spinner.as_ref()).await?;

    if let Some(s) = spinner {
        s.finish_and_clear();
    }

    let tx_hash = final_status
        .transactions
        .first()
        .and_then(|t| t.hash.clone());

    let explorer_url = tx_hash
        .as_ref()
        .map(|h| chain_info.explorer_tx_url(h));

    // `poll_gasless_status` only returns Ok on a terminal state, so we know
    // `final_status.is_terminal()` is true here. Non-terminal states surface
    // as `CliError::Timeout` (exit code 12) and are handled by the `?` above.
    let result = GaslessSwapOutput {
        chain: chain_info.display_name.to_string(),
        sell_token: TokenInfo {
            address: quote.sell_token,
            symbol: None,
            decimals: None,
        },
        buy_token: TokenInfo {
            address: quote.buy_token,
            symbol: None,
            decimals: None,
        },
        sell_amount: TokenAmount::new(&quote.sell_amount, sell_dec),
        buy_amount: TokenAmount::new(&quote.buy_amount, buy_dec),
        min_buy_amount: TokenAmount::new(&quote.min_buy_amount, buy_dec),
        trade_hash: submit_resp.trade_hash,
        status: final_status.status.clone(),
        tx_hash,
        explorer_url,
        terminal: true,
        successful: final_status.is_successful(),
    };

    let mut warnings = Vec::new();
    if !final_status.is_successful() {
        warnings.push(Warning {
            code: "TRADE_FAILED".into(),
            message: format!("Trade ended with status: {}", final_status.status),
        });
    }

    let exit_code = if final_status.is_successful() { 0 } else { 11 };

    output
        .success("swap", &result, metadata, warnings)
        .map(|_| exit_code)
        .map_err(|e| CliError::config(ErrorCode::Unknown, e.to_string()))
}

/// Sign EIP-712 typed data and split the signature.
async fn sign_eip712(
    signer: &PrivateKeySigner,
    eip712_json: &serde_json::Value,
) -> Result<SignatureSplit, CliError> {
    let typed_data: TypedData = serde_json::from_value(eip712_json.clone()).map_err(|e| {
        CliError::Transaction {
            code: ErrorCode::SigningFailed,
            message: format!("Failed to parse EIP-712 typed data: {e}"),
            tx_hash: None,
            suggestion: None,
        }
    })?;

    // Compute EIP-712 signing hash and sign synchronously
    let hash = typed_data.eip712_signing_hash().map_err(|e| CliError::Transaction {
        code: ErrorCode::SigningFailed,
        message: format!("Failed to compute EIP-712 hash: {e}"),
        tx_hash: None,
        suggestion: None,
    })?;

    let signature = signer.sign_hash_sync(&hash).map_err(|e| CliError::Transaction {
        code: ErrorCode::SigningFailed,
        message: format!("Failed to sign EIP-712 data: {e}"),
        tx_hash: None,
        suggestion: None,
    })?;

    // Split signature into v, r, s components
    let v = if signature.v() { 28 } else { 27 };
    let r = format!("0x{:064x}", signature.r());
    let s = format!("0x{:064x}", signature.s());

    Ok(SignatureSplit {
        v,
        r,
        s,
        signature_type: 2, // EIP712
    })
}

/// Poll gasless status until terminal state. ~5 min total, 5 s interval.
async fn poll_gasless_status(
    client: &ApiClient,
    trade_hash: &str,
    chain_id: u64,
    spinner: Option<&indicatif::ProgressBar>,
) -> Result<crate::api::gasless::GaslessStatusResponse, CliError> {
    crate::api::poll::poll_until_terminal(
        crate::api::poll::PollConfig::new(5, 300, ErrorCode::TransactionTimeout),
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
