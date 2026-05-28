use crate::api::gasless::{
    GaslessSubmitRequest, GaslessSubmitSignable, SignatureSplit,
};
use crate::api::types::{display_amount, TokenAmount, TokenInfo};
use crate::api::ApiClient;
use crate::chain::{self};
use crate::cli::SwapArgs;
use crate::config;
use crate::confirm::{confirm_or_preview, ConfirmFlow, TradeSummary};
use crate::error::{CliError, ErrorCode};
use crate::output::envelope::{Metadata, Warning};
use crate::output::trade::SideMeta;
use crate::output::{HumanDisplay, OutputHandler};
use crate::token_cache::{resolve_pair_evm, TokenCache};
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
            writeln!(writer, "\n  {}", format!("Gasless Swap: {}", self.status).bold())?;
        } else {
            writeln!(writer, "\n  Gasless Swap: {}", self.status)?;
        }

        writeln!(writer, "  {}", "-".repeat(45))?;

        let sell_label = self.sell_token.symbol.as_deref().unwrap_or(&self.sell_token.address);
        let buy_label = self.buy_token.symbol.as_deref().unwrap_or(&self.buy_token.address);

        writeln!(writer, "  {:<14} {} {}", "Sell:", self.sell_amount.display(), sell_label)?;
        writeln!(writer, "  {:<14} {} {}", "Buy:", self.buy_amount.display(), buy_label)?;
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

    let mut metadata = Metadata::for_chain(chain_info);
    let client = ApiClient::new(api_key, global.timeout)?;

    // Step 1: Get gasless quote
    let spinner = output.spinner_guard("Fetching gasless quote...");
    let quote = client
        .get_gasless_quote(chain_id, &args.sell, &args.buy, &args.amount, &taker)
        .await?;
    drop(spinner);

    // Populate zid
    metadata.zid = quote.zid.clone();

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
    match confirm_or_preview(
        output,
        global.yes,
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
        validate_signable_domain(&approval.eip712, chain_id, "approval", &mut metadata_warnings)?;
        validate_approval_message(
            &approval.eip712,
            &quote.sell_token,
            &quote.sell_amount,
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

    validate_signable_domain(&trade.eip712, chain_id, "trade", &mut metadata_warnings)?;

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
    let final_status =
        poll_gasless_status(&client, &submit_resp.trade_hash, chain_id, spinner.progress_bar()).await?;
    drop(spinner);

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

/// Assemble a `GaslessSwapOutput` from a quote + outcome. Centralises the
/// `TokenInfo` / `TokenAmount` construction so the four call sites
/// (needs-confirmation preview, dry-run preview, final success/failure) can't
/// drift.
#[allow(clippy::too_many_arguments)]
fn gasless_output(
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

/// Sign EIP-712 typed data and split the signature.
fn sign_eip712(
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

    // Take v from the canonical 65-byte secp256k1 encoding — alloy's
    // `Signature::as_bytes()` writes `[r(32) || s(32) || v(1)]` with v already
    // normalized to the 27/28 form the 0x submit endpoint expects.
    let sig_bytes = signature.as_bytes();
    let v = sig_bytes[64];
    let r = format!("0x{:064x}", signature.r());
    let s = format!("0x{:064x}", signature.s());

    Ok(SignatureSplit {
        v,
        r,
        s,
        signature_type: EIP712_SIGNATURE_TYPE,
    })
}

/// The 0x Settler `Signature` enum uses tag `2` for EIP-712 typed-data
/// signatures (vs. `0` for `Pre-Signed` and `3` for `ETH_SIGN`).
const EIP712_SIGNATURE_TYPE: u8 = 2;

/// Canonical Uniswap Permit2 address — deployed at the same address on every
/// EVM chain 0x's gasless flow supports. The approval EIP-712 message must
/// be verified by this contract; anything else means the API is telling us
/// to grant allowance to an unknown contract.
const PERMIT2_ADDRESS: &str = "0x000000000022D473030F116dDEE9F6B43aC78BA3";

/// Reject an EIP-712 payload whose `domain.chainId` doesn't match the chain
/// we expected to sign on, or that we cannot parse a chainId out of at all.
/// Warns (but doesn't reject) when `verifyingContract` is not Permit2 — the
/// trade-side payload is signed against the 0x Settler, which is chain-specific
/// and we don't carry an exhaustive allowlist.
fn validate_signable_domain(
    eip712: &serde_json::Value,
    expected_chain_id: u64,
    payload_kind: &str,
    warnings: &mut Vec<Warning>,
) -> Result<(), CliError> {
    let domain = eip712.get("domain").ok_or_else(|| CliError::Api {
        code: ErrorCode::InvalidSignature,
        message: format!("Gasless {payload_kind} EIP-712 is missing a `domain` field"),
        status: None,
        details: None,
        suggestion: Some("This is an API contract violation — retry; if it persists, contact 0x support".into()),
    })?;

    let domain_chain_id = match domain.get("chainId") {
        Some(serde_json::Value::Number(n)) => n.as_u64(),
        Some(serde_json::Value::String(s)) => s.parse::<u64>().ok(),
        _ => None,
    }
    .ok_or_else(|| CliError::Api {
        code: ErrorCode::InvalidSignature,
        message: format!("Gasless {payload_kind} EIP-712 domain missing or unparseable chainId"),
        status: None,
        details: None,
        suggestion: None,
    })?;

    if domain_chain_id != expected_chain_id {
        return Err(CliError::Api {
            code: ErrorCode::InvalidSignature,
            message: format!(
                "Gasless {payload_kind} EIP-712 domain.chainId={domain_chain_id} but --chain selected {expected_chain_id} — refusing to sign"
            ),
            status: None,
            details: None,
            suggestion: Some(
                "The API returned a payload for the wrong chain. Re-fetch the quote; if it persists, contact 0x support.".into(),
            ),
        });
    }

    let verifying_contract = domain
        .get("verifyingContract")
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    let is_approval = payload_kind == "approval";
    if is_approval && !verifying_contract.eq_ignore_ascii_case(PERMIT2_ADDRESS) {
        return Err(CliError::Api {
            code: ErrorCode::InvalidSignature,
            message: format!(
                "Gasless approval verifyingContract={verifying_contract} is not canonical Permit2 ({PERMIT2_ADDRESS}) — refusing to sign"
            ),
            status: None,
            details: None,
            suggestion: Some(
                "An approval to a non-Permit2 contract could grant allowance to an arbitrary spender. Contact 0x support.".into(),
            ),
        });
    }
    if !is_approval && !verifying_contract.starts_with("0x") {
        warnings.push(Warning {
            code: "EIP712_DOMAIN_UNRECOGNIZED".into(),
            message: format!(
                "Trade verifyingContract '{verifying_contract}' is not a recognized address; proceeding because chainId matched."
            ),
        });
    }
    Ok(())
}

/// Validate that an approval EIP-712 actually authorizes the token and amount
/// we asked the API to quote — guards against a tampered response that tries
/// to permit a different token (or unlimited amount). Fails closed: if we
/// can't extract the `permitted.{token,amount}` fields the approval cannot be
/// cross-checked and we refuse to sign.
fn validate_approval_message(
    eip712: &serde_json::Value,
    expected_token: &str,
    expected_amount: &str,
) -> Result<(), CliError> {
    // Permit2 PermitTransferFrom shape: { permitted: { token, amount }, ... }.
    let permitted = eip712
        .get("message")
        .and_then(|m| m.get("permitted"))
        .ok_or_else(|| CliError::Api {
            code: ErrorCode::InvalidSignature,
            message: "Gasless approval EIP-712 is missing `message.permitted` — refusing to sign"
                .into(),
            status: None,
            details: None,
            suggestion: Some(
                "Without the permit fields we can't verify what we're approving. Re-fetch the quote; if it persists, contact 0x support.".into(),
            ),
        })?;

    let msg_token = permitted
        .get("token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CliError::Api {
            code: ErrorCode::InvalidSignature,
            message: "Gasless approval EIP-712 `permitted.token` is missing or not a string — refusing to sign".into(),
            status: None,
            details: None,
            suggestion: Some("Re-fetch the quote; if it persists, contact 0x support.".into()),
        })?;

    let msg_amount = match permitted.get("amount") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        _ => {
            return Err(CliError::Api {
                code: ErrorCode::InvalidSignature,
                message: "Gasless approval EIP-712 `permitted.amount` is missing or not a string/number — refusing to sign".into(),
                status: None,
                details: None,
                suggestion: Some("Re-fetch the quote; if it persists, contact 0x support.".into()),
            });
        }
    };

    if !msg_token.eq_ignore_ascii_case(expected_token) {
        return Err(CliError::Api {
            code: ErrorCode::InvalidSignature,
            message: format!(
                "Gasless approval message permits token {msg_token} but the quote was for {expected_token} — refusing to sign"
            ),
            status: None,
            details: None,
            suggestion: Some(
                "The API returned a Permit2 message for a different token than requested. Contact 0x support.".into(),
            ),
        });
    }
    if msg_amount != expected_amount {
        return Err(CliError::Api {
            code: ErrorCode::InvalidSignature,
            message: format!(
                "Gasless approval message permits amount {msg_amount} but the quote was for {expected_amount} — refusing to sign"
            ),
            status: None,
            details: None,
            suggestion: Some(
                "The API returned a Permit2 message for a different amount than requested. Contact 0x support.".into(),
            ),
        });
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn permit_message(token: &str, amount: &str) -> serde_json::Value {
        serde_json::json!({
            "message": { "permitted": { "token": token, "amount": amount } }
        })
    }

    #[test]
    fn approval_validation_passes_when_token_and_amount_match() {
        let eip = permit_message("0xAAAAaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "1000000");
        assert!(validate_approval_message(
            &eip,
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "1000000",
        )
        .is_ok());
    }

    #[test]
    fn approval_validation_rejects_wrong_token() {
        let eip = permit_message("0xdeadbeef0000000000000000000000000000beef", "1000000");
        let err = validate_approval_message(
            &eip,
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "1000000",
        )
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidSignature);
    }

    #[test]
    fn approval_validation_rejects_wrong_amount() {
        let eip = permit_message("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "999");
        let err = validate_approval_message(
            &eip,
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "1000000",
        )
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidSignature);
    }

    #[test]
    fn approval_validation_fails_closed_when_permitted_missing() {
        // Empty message: must fail — used to pass with the old "if let (Some, Some)" guard.
        let eip = serde_json::json!({ "message": {} });
        let err = validate_approval_message(
            &eip,
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "1000000",
        )
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidSignature);
    }

    #[test]
    fn approval_validation_accepts_numeric_amount() {
        let eip = serde_json::json!({
            "message": {
                "permitted": {
                    "token": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "amount": 1000000u64
                }
            }
        });
        assert!(validate_approval_message(
            &eip,
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "1000000",
        )
        .is_ok());
    }
}
