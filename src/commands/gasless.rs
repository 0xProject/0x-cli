use crate::api::gasless::{GaslessSubmitRequest, GaslessSubmitSignable, SignatureSplit};
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
use alloy::primitives::{Address, U256};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::SignerSync;
use alloy_dyn_abi::eip712::TypedData;
use serde::Serialize;
use std::io::{self, Write};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

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

    let api_key = config::resolve_api_key(global, &config)?;

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
    let typed_data: TypedData =
        serde_json::from_value(eip712_json.clone()).map_err(|e| CliError::Transaction {
            code: ErrorCode::SigningFailed,
            message: format!("Failed to parse EIP-712 typed data: {e}"),
            tx_hash: None,
            suggestion: None,
        })?;

    // Compute EIP-712 signing hash and sign synchronously
    let hash = typed_data
        .eip712_signing_hash()
        .map_err(|e| CliError::Transaction {
            code: ErrorCode::SigningFailed,
            message: format!("Failed to compute EIP-712 hash: {e}"),
            tx_hash: None,
            suggestion: None,
        })?;

    let signature = signer
        .sign_hash_sync(&hash)
        .map_err(|e| CliError::Transaction {
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

/// Shared piece for both approval and trade domain checks: pull
/// `domain.{chainId, verifyingContract}` out of an EIP-712 payload, enforce
/// the chainId binding, and confirm the `verifyingContract` is present
/// and address-shaped. Returns the contract string verbatim so callers can
/// add their own type-specific equality checks.
fn extract_domain_verifying_contract(
    eip712: &serde_json::Value,
    expected_chain_id: u64,
    payload_kind: &str,
) -> Result<String, CliError> {
    let domain = eip712.get("domain").ok_or_else(|| CliError::Api {
        code: ErrorCode::InvalidSignature,
        message: format!("Gasless {payload_kind} EIP-712 is missing a `domain` field"),
        status: None,
        details: None,
        suggestion: Some(
            "This is an API contract violation — retry; if it persists, contact 0x support".into(),
        ),
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
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CliError::Api {
            code: ErrorCode::InvalidSignature,
            message: format!(
                "Gasless {payload_kind} EIP-712 domain.verifyingContract is missing or empty — refusing to sign"
            ),
            status: None,
            details: None,
            suggestion: Some(
                "Without a verifyingContract the signature isn't bound to a counterparty. Re-fetch the quote; if it persists, contact 0x support.".into(),
            ),
        })?;

    if !is_address_shaped(verifying_contract) {
        return Err(CliError::Api {
            code: ErrorCode::InvalidSignature,
            message: format!(
                "Gasless {payload_kind} verifyingContract '{verifying_contract}' isn't a 20-byte hex address — refusing to sign"
            ),
            status: None,
            details: None,
            suggestion: Some(
                "The API returned a malformed verifyingContract. Re-fetch the quote; if it persists, contact 0x support.".into(),
            ),
        });
    }
    Ok(verifying_contract.to_string())
}

/// Approval-side domain check. Unlike the trade payload, an approval's
/// `verifyingContract` is the **token contract itself** — pinning it to
/// the quote's sell_token is what stops a tampered API response from
/// having the user grant allowance on a different token.
fn validate_approval_domain(
    eip712: &serde_json::Value,
    sell_token: &str,
    chain_id: u64,
) -> Result<(), CliError> {
    let vc = extract_domain_verifying_contract(eip712, chain_id, "approval")?;
    if !vc.eq_ignore_ascii_case(sell_token) {
        return Err(CliError::Api {
            code: ErrorCode::InvalidSignature,
            message: format!(
                "Gasless approval verifyingContract={vc} doesn't match the quote's sell_token={sell_token} — refusing to sign"
            ),
            status: None,
            details: None,
            suggestion: Some(
                "An approval to a non-matching token would grant allowance for the wrong asset. Re-fetch the quote; if it persists, contact 0x support.".into(),
            ),
        });
    }
    Ok(())
}

/// Trade-side domain check. The 0x gasless trade is always a Permit2
/// `PermitTransferFrom`, so when the API tags the signable as `Permit2`
/// we additionally enforce the canonical Permit2 address. Any other tag
/// is allowed to pass as long as it's address-shaped — the surrounding
/// code doesn't yet support non-Permit2 trades but we don't want to fail
/// hard if the API evolves; the type-specific permitted check below
/// covers the actual binding.
fn validate_trade_domain(
    eip712: &serde_json::Value,
    signable_type: &str,
    chain_id: u64,
) -> Result<(), CliError> {
    let vc = extract_domain_verifying_contract(eip712, chain_id, "trade")?;
    if signable_type.eq_ignore_ascii_case("Permit2") && !vc.eq_ignore_ascii_case(PERMIT2_ADDRESS) {
        return Err(CliError::Api {
            code: ErrorCode::InvalidSignature,
            message: format!(
                "Gasless trade signable_type=Permit2 but verifyingContract={vc} is not canonical Permit2 ({PERMIT2_ADDRESS}) — refusing to sign"
            ),
            status: None,
            details: None,
            suggestion: Some(
                "A Permit2 signable bound to the wrong contract could redirect funds. Re-fetch the quote; if it persists, contact 0x support.".into(),
            ),
        });
    }
    Ok(())
}

/// Loose check: 0x-prefixed, 42 chars, hex body. Used as a sanity guard on
/// EIP-712 `verifyingContract` fields. Not a full EIP-55 checksum check
/// (those are case-sensitive and the 0x API doesn't always checksum its
/// addresses).
fn is_address_shaped(s: &str) -> bool {
    s.len() == 42 && s.starts_with("0x") && s[2..].chars().all(|c| c.is_ascii_hexdigit())
}

/// `approve(address,uint256)` selector — the only metatx wrapper we accept
/// for `executeMetaTransaction::approve`.
const ERC20_APPROVE_SELECTOR: [u8; 4] = [0x09, 0x5e, 0xa7, 0xb3];

/// Validate the trade-side Permit2 `PermitTransferFrom` message: the
/// `message.permitted.{token, amount}` fields must match the quote so the
/// trade can't be silently rebound to a different token/amount before
/// signature. Fails closed: missing fields → refuse.
fn validate_trade_permitted(
    eip712: &serde_json::Value,
    expected_token: &str,
    expected_amount: &str,
) -> Result<(), CliError> {
    let permitted = eip712
        .get("message")
        .and_then(|m| m.get("permitted"))
        .ok_or_else(|| CliError::Api {
            code: ErrorCode::InvalidSignature,
            message: "Gasless trade EIP-712 is missing `message.permitted` — refusing to sign"
                .into(),
            status: None,
            details: None,
            suggestion: Some(
                "Without the permit fields we can't verify what we're trading. Re-fetch the quote; if it persists, contact 0x support.".into(),
            ),
        })?;

    let msg_token = permitted
        .get("token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CliError::Api {
            code: ErrorCode::InvalidSignature,
            message: "Gasless trade EIP-712 `permitted.token` is missing or not a string — refusing to sign".into(),
            status: None,
            details: None,
            suggestion: Some("Re-fetch the quote; if it persists, contact 0x support.".into()),
        })?;

    let msg_amount = read_uint_string(permitted, "amount").ok_or_else(|| CliError::Api {
        code: ErrorCode::InvalidSignature,
        message: "Gasless trade EIP-712 `permitted.amount` is missing or not a string/number — refusing to sign".into(),
        status: None,
        details: None,
        suggestion: Some("Re-fetch the quote; if it persists, contact 0x support.".into()),
    })?;

    if !msg_token.eq_ignore_ascii_case(expected_token) {
        return Err(CliError::Api {
            code: ErrorCode::InvalidSignature,
            message: format!(
                "Gasless trade message permits token {msg_token} but the quote was for {expected_token} — refusing to sign"
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
                "Gasless trade message permits amount {msg_amount} but the quote was for {expected_amount} — refusing to sign"
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

/// Dispatcher for the approval payload. Validates domain (chainId +
/// verifyingContract == sell_token), then routes to the per-type message
/// validator based on `signable_type`. Unknown types refuse fail-closed
/// per the user's policy: a future API addition shouldn't be silently
/// signed by an older CLI.
fn validate_approval(
    signable_type: &str,
    eip712: &serde_json::Value,
    sell_token: &str,
    sell_amount: &str,
    signer_addr: Address,
    chain_id: u64,
) -> Result<(), CliError> {
    validate_approval_domain(eip712, sell_token, chain_id)?;
    let expected_amount = parse_uint(sell_amount).ok_or_else(|| CliError::Api {
        code: ErrorCode::InvalidSignature,
        message: format!("Quote sellAmount '{sell_amount}' is not a parseable uint — cannot cross-check the approval"),
        status: None,
        details: None,
        suggestion: None,
    })?;
    match signable_type {
        "permit" => validate_permit_message(eip712, signer_addr, expected_amount),
        "daiPermit" => validate_dai_permit_message(eip712, signer_addr),
        "executeMetaTransaction::approve" => {
            validate_metatx_approve_message(eip712, signer_addr, expected_amount)
        }
        other => Err(CliError::Api {
            code: ErrorCode::InvalidSignature,
            message: format!(
                "Unknown gasless approval signable_type '{other}' — refusing to sign"
            ),
            status: None,
            details: None,
            suggestion: Some(
                "This may be a newer approval mechanism the CLI hasn't been updated for. Update the CLI or contact 0x support.".into(),
            ),
        }),
    }
}

/// EIP-2612 `permit`: { owner, spender, value, nonce, deadline }.
fn validate_permit_message(
    eip712: &serde_json::Value,
    signer_addr: Address,
    expected_amount: U256,
) -> Result<(), CliError> {
    let msg = eip712
        .get("message")
        .ok_or_else(|| missing_message_err("permit"))?;
    require_address(msg, "owner", signer_addr, "permit owner")?;
    let value = read_uint(msg, "value").ok_or_else(|| missing_field_err("permit", "value"))?;
    if value < expected_amount {
        return Err(amount_too_small_err(
            "permit",
            "value",
            value,
            expected_amount,
        ));
    }
    let deadline =
        read_uint(msg, "deadline").ok_or_else(|| missing_field_err("permit", "deadline"))?;
    require_future_deadline("permit", "deadline", deadline)?;
    Ok(())
}

/// DAI's pre-EIP-2612 `daiPermit`:
/// { holder, spender, nonce, expiry, allowed }.
/// daiPermit is a boolean-allowance permit — it doesn't carry an explicit
/// amount; granting it gives `spender` unbounded DAI allowance until
/// `expiry`. We refuse the `allowed=false` form (revoke) and the
/// `expiry=0` form (no expiry — effectively unlimited).
fn validate_dai_permit_message(
    eip712: &serde_json::Value,
    signer_addr: Address,
) -> Result<(), CliError> {
    let msg = eip712
        .get("message")
        .ok_or_else(|| missing_message_err("daiPermit"))?;
    require_address(msg, "holder", signer_addr, "daiPermit holder")?;
    let allowed = msg
        .get("allowed")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| missing_field_err("daiPermit", "allowed"))?;
    if !allowed {
        return Err(CliError::Api {
            code: ErrorCode::InvalidSignature,
            message: "Gasless daiPermit message has allowed=false (revoke) — refusing to sign".into(),
            status: None,
            details: None,
            suggestion: Some(
                "Signing a revoke during a swap would be a no-op trade plus wasted signature. Re-fetch the quote; if it persists, contact 0x support.".into(),
            ),
        });
    }
    let expiry =
        read_uint(msg, "expiry").ok_or_else(|| missing_field_err("daiPermit", "expiry"))?;
    if expiry == U256::ZERO {
        return Err(CliError::Api {
            code: ErrorCode::InvalidSignature,
            message: "Gasless daiPermit has expiry=0 (no time limit on the unlimited allowance) — refusing to sign".into(),
            status: None,
            details: None,
            suggestion: Some(
                "0x's gasless flow should always issue time-bounded permits. Re-fetch the quote; if it persists, contact 0x support.".into(),
            ),
        });
    }
    require_future_deadline("daiPermit", "expiry", expiry)?;
    Ok(())
}

/// Polygon-style meta-transaction wrapping an ERC-20 `approve(spender, amount)`:
/// { nonce, from, functionSignature }. The encoded functionSignature must
/// be exactly 4 bytes of `0x095ea7b3` selector + 32 bytes spender +
/// 32 bytes amount (68 bytes total). Anything else is a different call
/// being smuggled through and we refuse it.
fn validate_metatx_approve_message(
    eip712: &serde_json::Value,
    signer_addr: Address,
    expected_amount: U256,
) -> Result<(), CliError> {
    let msg = eip712
        .get("message")
        .ok_or_else(|| missing_message_err("executeMetaTransaction::approve"))?;
    require_address(
        msg,
        "from",
        signer_addr,
        "executeMetaTransaction::approve from",
    )?;
    let sig_str = msg
        .get("functionSignature")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| missing_field_err("executeMetaTransaction::approve", "functionSignature"))?;
    let sig_hex = sig_str
        .strip_prefix("0x")
        .or_else(|| sig_str.strip_prefix("0X"))
        .unwrap_or(sig_str);
    let bytes = hex::decode(sig_hex).map_err(|_| CliError::Api {
        code: ErrorCode::InvalidSignature,
        message: format!(
            "executeMetaTransaction::approve functionSignature '{sig_str}' is not valid hex — refusing to sign"
        ),
        status: None,
        details: None,
        suggestion: None,
    })?;
    if bytes.len() != 4 + 32 + 32 {
        return Err(CliError::Api {
            code: ErrorCode::InvalidSignature,
            message: format!(
                "executeMetaTransaction::approve functionSignature is {} bytes; expected 68 (selector + address + uint256) — refusing to sign",
                bytes.len()
            ),
            status: None,
            details: None,
            suggestion: Some("The API returned a non-`approve` metatx — that would call a different function. Contact 0x support.".into()),
        });
    }
    if bytes[..4] != ERC20_APPROVE_SELECTOR {
        return Err(CliError::Api {
            code: ErrorCode::InvalidSignature,
            message: format!(
                "executeMetaTransaction::approve functionSignature selector is 0x{} (not approve 0x095ea7b3) — refusing to sign",
                hex::encode(&bytes[..4])
            ),
            status: None,
            details: None,
            suggestion: Some("Signing this metatx would call something other than ERC-20 approve. Contact 0x support.".into()),
        });
    }
    let amount = U256::from_be_slice(&bytes[4 + 32..]);
    if amount < expected_amount {
        return Err(amount_too_small_err(
            "executeMetaTransaction::approve",
            "encoded amount",
            amount,
            expected_amount,
        ));
    }
    Ok(())
}

// ── Shared helpers used by the per-type validators ──────────────────────

fn read_uint_string(msg: &serde_json::Value, field: &str) -> Option<String> {
    match msg.get(field)? {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn parse_uint(s: &str) -> Option<U256> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        U256::from_str_radix(hex, 16).ok()
    } else {
        U256::from_str_radix(s, 10).ok()
    }
}

fn read_uint(msg: &serde_json::Value, field: &str) -> Option<U256> {
    parse_uint(&read_uint_string(msg, field)?)
}

fn require_address(
    msg: &serde_json::Value,
    field: &str,
    expected: Address,
    label: &str,
) -> Result<(), CliError> {
    let s = msg
        .get(field)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| CliError::Api {
            code: ErrorCode::InvalidSignature,
            message: format!(
                "Gasless approval message missing `{field}` ({label}) — refusing to sign"
            ),
            status: None,
            details: None,
            suggestion: None,
        })?;
    let parsed = Address::from_str(s).map_err(|_| CliError::Api {
        code: ErrorCode::InvalidSignature,
        message: format!(
            "Gasless approval `{field}` '{s}' isn't a valid address — refusing to sign"
        ),
        status: None,
        details: None,
        suggestion: None,
    })?;
    if parsed != expected {
        return Err(CliError::Api {
            code: ErrorCode::InvalidSignature,
            message: format!(
                "Gasless approval {label}={parsed} doesn't match the signing wallet ({expected}) — refusing to sign"
            ),
            status: None,
            details: None,
            suggestion: Some(
                "A permit owned by a different address would not authorize this wallet's swap. Contact 0x support.".into(),
            ),
        });
    }
    Ok(())
}

fn require_future_deadline(
    payload_kind: &str,
    field: &str,
    deadline: U256,
) -> Result<(), CliError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let now_u256 = U256::from(now);
    if deadline <= now_u256 {
        return Err(CliError::Api {
            code: ErrorCode::InvalidSignature,
            message: format!(
                "Gasless {payload_kind} {field}={deadline} is in the past (now={now}) — refusing to sign"
            ),
            status: None,
            details: None,
            suggestion: Some(
                "An expired permit can't be relayed. Re-fetch the quote; if it persists, contact 0x support.".into(),
            ),
        });
    }
    Ok(())
}

fn missing_message_err(payload_kind: &str) -> CliError {
    CliError::Api {
        code: ErrorCode::InvalidSignature,
        message: format!(
            "Gasless {payload_kind} EIP-712 is missing the `message` block — refusing to sign"
        ),
        status: None,
        details: None,
        suggestion: Some("Re-fetch the quote; if it persists, contact 0x support.".into()),
    }
}

fn missing_field_err(payload_kind: &str, field: &str) -> CliError {
    CliError::Api {
        code: ErrorCode::InvalidSignature,
        message: format!("Gasless {payload_kind} message missing `{field}` — refusing to sign"),
        status: None,
        details: None,
        suggestion: Some("Re-fetch the quote; if it persists, contact 0x support.".into()),
    }
}

fn amount_too_small_err(payload_kind: &str, field: &str, got: U256, expected: U256) -> CliError {
    CliError::Api {
        code: ErrorCode::InvalidSignature,
        message: format!(
            "Gasless {payload_kind} {field}={got} is less than the quote's sellAmount {expected} — refusing to sign"
        ),
        status: None,
        details: None,
        suggestion: Some(
            "A short permit would let the relayer pull less than you intended to sell. Contact 0x support.".into(),
        ),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    const SIGNER: &str = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SELL_TOKEN: &str = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913";

    fn signer_addr() -> Address {
        Address::from_str(SIGNER).unwrap()
    }

    fn far_future_deadline() -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        (now + 3600).to_string()
    }

    // ── Trade-side: domain + Permit2 PermitTransferFrom binding ─────────

    fn trade_eip712(
        verifying_contract: &str,
        chain_id: u64,
        token: &str,
        amount: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "domain": { "chainId": chain_id, "verifyingContract": verifying_contract },
            "message": { "permitted": { "token": token, "amount": amount } }
        })
    }

    #[test]
    fn trade_domain_accepts_canonical_permit2_case_insensitively() {
        let eip = trade_eip712(&PERMIT2_ADDRESS.to_lowercase(), 1, SELL_TOKEN, "1000000");
        assert!(validate_trade_domain(&eip, "Permit2", 1).is_ok());
    }

    #[test]
    fn trade_domain_rejects_non_permit2_when_type_is_permit2() {
        let eip = trade_eip712(
            "0x1111111111111111111111111111111111111111",
            1,
            SELL_TOKEN,
            "1000000",
        );
        let err = validate_trade_domain(&eip, "Permit2", 1).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidSignature);
        assert!(format!("{err}").contains("Permit2"));
    }

    #[test]
    fn trade_domain_rejects_wrong_chain_id() {
        let eip = trade_eip712(PERMIT2_ADDRESS, 137, SELL_TOKEN, "1000000");
        let err = validate_trade_domain(&eip, "Permit2", 1).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidSignature);
    }

    #[test]
    fn trade_permitted_matches_quote() {
        let eip = trade_eip712(PERMIT2_ADDRESS, 1, SELL_TOKEN, "1000000");
        assert!(validate_trade_permitted(&eip, SELL_TOKEN, "1000000").is_ok());
    }

    #[test]
    fn trade_permitted_rejects_wrong_token() {
        let eip = trade_eip712(
            PERMIT2_ADDRESS,
            1,
            "0xdeadbeef0000000000000000000000000000beef",
            "1000000",
        );
        let err = validate_trade_permitted(&eip, SELL_TOKEN, "1000000").unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidSignature);
    }

    #[test]
    fn trade_permitted_rejects_wrong_amount() {
        let eip = trade_eip712(PERMIT2_ADDRESS, 1, SELL_TOKEN, "999");
        let err = validate_trade_permitted(&eip, SELL_TOKEN, "1000000").unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidSignature);
    }

    #[test]
    fn trade_permitted_rejects_missing_permitted() {
        let eip = serde_json::json!({
            "domain": { "chainId": 1, "verifyingContract": PERMIT2_ADDRESS },
            "message": {}
        });
        assert!(validate_trade_permitted(&eip, SELL_TOKEN, "1000000").is_err());
    }

    // ── Approval-side: domain bound to sell_token ───────────────────────

    fn approval_eip712_with(message: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "domain": { "chainId": 1, "verifyingContract": SELL_TOKEN },
            "message": message,
        })
    }

    #[test]
    fn approval_domain_rejects_when_verifying_contract_is_not_sell_token() {
        let eip = serde_json::json!({
            "domain": { "chainId": 1, "verifyingContract": "0x1111111111111111111111111111111111111111" },
            "message": {}
        });
        let err = validate_approval_domain(&eip, SELL_TOKEN, 1).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidSignature);
        assert!(format!("{err}").contains("sell_token"));
    }

    #[test]
    fn approval_domain_accepts_sell_token_case_insensitively() {
        // Uppercase the body but keep the `0x` prefix lowercase — that's
        // the EIP-55 mixed-case checksum shape the 0x API actually emits.
        // `is_address_shaped` rejects `0X` so testing pure to_uppercase()
        // would assert the wrong invariant.
        let mixed = format!("0x{}", SELL_TOKEN[2..].to_uppercase());
        let eip = serde_json::json!({
            "domain": { "chainId": 1, "verifyingContract": mixed },
            "message": {}
        });
        assert!(validate_approval_domain(&eip, SELL_TOKEN, 1).is_ok());
    }

    // ── EIP-2612 permit ─────────────────────────────────────────────────

    fn permit_eip712(owner: &str, value: &str, deadline: &str) -> serde_json::Value {
        approval_eip712_with(serde_json::json!({
            "owner": owner,
            "spender": "0x1111111111111111111111111111111111111111",
            "value": value,
            "nonce": "0",
            "deadline": deadline,
        }))
    }

    #[test]
    fn permit_happy_path() {
        let eip = permit_eip712(SIGNER, "1000000", &far_future_deadline());
        assert!(validate_approval("permit", &eip, SELL_TOKEN, "1000000", signer_addr(), 1).is_ok());
    }

    #[test]
    fn permit_rejects_owner_not_signer() {
        let eip = permit_eip712(
            "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "1000000",
            &far_future_deadline(),
        );
        let err =
            validate_approval("permit", &eip, SELL_TOKEN, "1000000", signer_addr(), 1).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidSignature);
    }

    #[test]
    fn permit_rejects_insufficient_value() {
        let eip = permit_eip712(SIGNER, "999", &far_future_deadline());
        let err =
            validate_approval("permit", &eip, SELL_TOKEN, "1000000", signer_addr(), 1).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidSignature);
        assert!(format!("{err}").contains("less than"));
    }

    #[test]
    fn permit_accepts_value_greater_than_sell_amount() {
        // The API can issue a larger permit than the sell amount (e.g. round
        // up). Only "smaller than" is unsafe.
        let eip = permit_eip712(SIGNER, "2000000", &far_future_deadline());
        assert!(validate_approval("permit", &eip, SELL_TOKEN, "1000000", signer_addr(), 1).is_ok());
    }

    #[test]
    fn permit_rejects_expired_deadline() {
        let eip = permit_eip712(SIGNER, "1000000", "1");
        let err =
            validate_approval("permit", &eip, SELL_TOKEN, "1000000", signer_addr(), 1).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidSignature);
        assert!(format!("{err}").contains("past"));
    }

    #[test]
    fn permit_accepts_hex_encoded_value() {
        // Some EIP-712 producers emit uint256 fields as 0x-prefixed hex.
        let eip = permit_eip712(SIGNER, "0xf4240", &far_future_deadline()); // 0xf4240 = 1_000_000
        assert!(validate_approval("permit", &eip, SELL_TOKEN, "1000000", signer_addr(), 1).is_ok());
    }

    // ── daiPermit ──────────────────────────────────────────────────────

    fn dai_permit_eip712(holder: &str, allowed: bool, expiry: &str) -> serde_json::Value {
        approval_eip712_with(serde_json::json!({
            "holder": holder,
            "spender": "0x1111111111111111111111111111111111111111",
            "nonce": "0",
            "expiry": expiry,
            "allowed": allowed,
        }))
    }

    #[test]
    fn dai_permit_happy_path() {
        let eip = dai_permit_eip712(SIGNER, true, &far_future_deadline());
        assert!(
            validate_approval("daiPermit", &eip, SELL_TOKEN, "1000000", signer_addr(), 1).is_ok()
        );
    }

    #[test]
    fn dai_permit_rejects_allowed_false() {
        let eip = dai_permit_eip712(SIGNER, false, &far_future_deadline());
        let err = validate_approval("daiPermit", &eip, SELL_TOKEN, "1000000", signer_addr(), 1)
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidSignature);
        assert!(format!("{err}").contains("revoke"));
    }

    #[test]
    fn dai_permit_rejects_zero_expiry() {
        let eip = dai_permit_eip712(SIGNER, true, "0");
        let err = validate_approval("daiPermit", &eip, SELL_TOKEN, "1000000", signer_addr(), 1)
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidSignature);
        assert!(format!("{err}").contains("expiry=0"));
    }

    // ── executeMetaTransaction::approve ────────────────────────────────

    fn metatx_function_signature(spender: &str, amount: U256) -> String {
        // selector(4) + spender(32) + amount(32) = 68 bytes
        let mut bytes = Vec::with_capacity(68);
        bytes.extend_from_slice(&ERC20_APPROVE_SELECTOR);
        // spender padded to 32 bytes
        let spender_bytes = hex::decode(spender.trim_start_matches("0x")).unwrap();
        bytes.extend_from_slice(&[0u8; 12]);
        bytes.extend_from_slice(&spender_bytes);
        // amount as 32-byte BE
        let amount_bytes: [u8; 32] = amount.to_be_bytes();
        bytes.extend_from_slice(&amount_bytes);
        format!("0x{}", hex::encode(bytes))
    }

    fn metatx_eip712(from: &str, function_signature: &str) -> serde_json::Value {
        approval_eip712_with(serde_json::json!({
            "nonce": "0",
            "from": from,
            "functionSignature": function_signature,
        }))
    }

    #[test]
    fn metatx_approve_happy_path() {
        let sig = metatx_function_signature(
            "0x1111111111111111111111111111111111111111",
            U256::from(1_000_000u64),
        );
        let eip = metatx_eip712(SIGNER, &sig);
        assert!(validate_approval(
            "executeMetaTransaction::approve",
            &eip,
            SELL_TOKEN,
            "1000000",
            signer_addr(),
            1,
        )
        .is_ok());
    }

    #[test]
    fn metatx_approve_rejects_wrong_selector() {
        // transfer(address,uint256) selector = 0xa9059cbb — same shape, different call.
        let mut bytes = Vec::with_capacity(68);
        bytes.extend_from_slice(&[0xa9, 0x05, 0x9c, 0xbb]);
        bytes.extend_from_slice(&[0u8; 32]);
        bytes.extend_from_slice(&[0u8; 32]);
        let sig = format!("0x{}", hex::encode(bytes));
        let eip = metatx_eip712(SIGNER, &sig);
        let err = validate_approval(
            "executeMetaTransaction::approve",
            &eip,
            SELL_TOKEN,
            "1000000",
            signer_addr(),
            1,
        )
        .unwrap_err();
        assert!(format!("{err}").contains("selector"));
    }

    #[test]
    fn metatx_approve_rejects_insufficient_amount() {
        let sig = metatx_function_signature(
            "0x1111111111111111111111111111111111111111",
            U256::from(999u64),
        );
        let eip = metatx_eip712(SIGNER, &sig);
        let err = validate_approval(
            "executeMetaTransaction::approve",
            &eip,
            SELL_TOKEN,
            "1000000",
            signer_addr(),
            1,
        )
        .unwrap_err();
        assert!(format!("{err}").contains("less than"));
    }

    #[test]
    fn metatx_approve_rejects_wrong_length() {
        // Truncated functionSignature — selector + only 16 bytes of args.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&ERC20_APPROVE_SELECTOR);
        bytes.extend_from_slice(&[0u8; 16]);
        let sig = format!("0x{}", hex::encode(bytes));
        let eip = metatx_eip712(SIGNER, &sig);
        let err = validate_approval(
            "executeMetaTransaction::approve",
            &eip,
            SELL_TOKEN,
            "1000000",
            signer_addr(),
            1,
        )
        .unwrap_err();
        assert!(format!("{err}").contains("bytes"));
    }

    // ── Unknown signable_type ───────────────────────────────────────────

    #[test]
    fn unknown_signable_type_rejected_fail_closed() {
        let eip = approval_eip712_with(serde_json::json!({}));
        let err = validate_approval("hyperPermit", &eip, SELL_TOKEN, "1000000", signer_addr(), 1)
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidSignature);
        assert!(format!("{err}").contains("Unknown"));
    }
}
