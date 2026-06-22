//! `0x cross-chain` — bridge tokens between chains via the 0x Cross-Chain
//! API. Orchestration and wallet handling live here; output assembly in
//! [`output`]; quote selection in [`select`].

mod output;
mod select;

pub use output::{CrossChainDryRunOutput, CrossChainOutput, DryRunQuote, DryRunStep};

use crate::chain;
use crate::cli::{CrossChainArgs, QuoteSort};
use crate::config;
use crate::confirm::{confirm_or_preview, ConfirmFlow, TradeSummary};
use crate::error::{CliError, ErrorCode};
use crate::output::envelope::{Metadata, Warning};
use crate::output::trade::SideMeta;
use crate::output::OutputHandler;
use output::{build_dry_run_output, cross_chain_output};
use select::select_quote;
use solana_sdk::signer::Signer as _;

pub async fn run(
    args: &CrossChainArgs,
    output: &OutputHandler,
    global: &crate::GlobalOpts,
) -> Result<i32, CliError> {
    let config = config::load_config()?;

    let origin = chain::resolve_chain(&args.from)?;
    let destination = chain::resolve_chain(&args.to)?;

    // `sell` lives on the origin chain, `buy` on the destination chain —
    // validate each against its own chain's address format.
    chain::validate_token_address(&args.sell, origin)?;
    chain::validate_token_address(&args.buy, destination)?;
    chain::validate_base_unit_amount(&args.amount)?;

    // Load the origin wallet once. We need its address up-front for the
    // quote request and the same wallet later for approval / signing —
    // re-loading on every call would re-hit the OS keyring (and prompt the
    // user) several times per cross-chain swap.
    let origin_wallet = OriginWallet::load(origin, &config, global.wallet.as_deref())?;
    let origin_address = origin_wallet.address();
    // The 0x cross-chain API requires `destinationAddress` (mandatory for
    // Solana-side bridges; some versions also require it for EVM-EVM). We
    // resolve it from the user's wallet on the destination chain so the
    // default behaviour is "bridge to myself" — same wallet for same-VM,
    // the user's other wallet for cross-VM.
    let destination_address =
        resolve_destination_address(&origin_wallet, origin, destination, &config)?;

    let mut metadata = Metadata::for_chain(origin);
    let client = crate::api::client_for(global, &config, output)?;

    let sort_by = match args.sort {
        QuoteSort::Price => "price",
        QuoteSort::Speed => "speed",
    };

    // Step 1: Get quotes
    // Some Solana-origin routes (e.g. Circle CCTP) need a one-shot extra
    // signer whose keypair the caller holds; generating one per request and
    // sending its pubkey unlocks those routes. The keypair only ever lives
    // in memory and co-signs at submission.
    let ephemeral_signer = origin.is_solana().then(solana_sdk::signature::Keypair::new);
    let ephemeral_signer_pubkey = ephemeral_signer.as_ref().map(|kp| kp.pubkey().to_string());
    let spinner = output.spinner_guard("Fetching cross-chain quotes...");
    let quotes_resp = client
        .get_cross_chain_quotes(
            &origin.api_chain_id(),
            &destination.api_chain_id(),
            &args.sell,
            &args.buy,
            &args.amount,
            &origin_address,
            &destination_address,
            Some(args.slippage),
            Some(sort_by),
            Some(args.max_quotes),
            ephemeral_signer_pubkey.as_deref(),
        )
        .await?;
    drop(spinner);

    if quotes_resp.quotes.is_empty() || !quotes_resp.liquidity_available {
        return Err(CliError::Api {
            code: ErrorCode::NoLiquidity,
            message: "No cross-chain quotes available for this route".into(),
            status: None,
            details: None,
            suggestion: Some("Try a different token pair, amount, or chain combination".into()),
        });
    }

    metadata.zid = quotes_resp.zid.clone();

    // Resolve sell/buy decimals for the origin/destination chains so amounts
    // render correctly. EVM-only; Solana origin/destination falls back to
    // unknown decimals (raw amount in output).
    let mut token_cache = crate::token_cache::TokenCache::new();
    let mut metadata_warnings: Vec<Warning> = Vec::new();
    let sell_meta = resolve_one_evm(
        &mut token_cache,
        origin,
        &args.sell,
        global.rpc_url.as_deref(),
        &config,
        "sell",
        &mut metadata_warnings,
    )
    .await;
    let buy_meta = resolve_one_evm(
        &mut token_cache,
        destination,
        &args.buy,
        global.rpc_url.as_deref(),
        &config,
        "buy",
        &mut metadata_warnings,
    )
    .await;
    let sell = SideMeta::from_meta(args.sell.clone(), sell_meta);
    let buy = SideMeta::from_meta(args.buy.clone(), buy_meta);

    // Dry-run short-circuit: surface every quote in the envelope and exit
    // before the (interactive) quote picker. Without this, piped
    // non-TTY `--dry-run` runs hit `select_quote`'s "multi-quote needs
    // --select-quote or --yes" error and never reach the dry-run path.
    if global.dry_run {
        let result =
            build_dry_run_output(origin, destination, &sell, &buy, &args.amount, &quotes_resp);
        return Ok(output.emit_success("cross-chain", &result, metadata, metadata_warnings, 30));
    }

    // Step 2: Select quote
    let selected_idx = select_quote(args, output, &quotes_resp, &buy, global.yes)?;
    let selected = &quotes_resp.quotes[selected_idx];

    // Balance shortfalls arrive inside the 200 quotes response
    // (`issues.balance`), not as an API error — fail with
    // INSUFFICIENT_BALANCE before asking the user to confirm a doomed trade.
    if let Some(balance) = selected.issues.as_ref().and_then(|i| i.balance.as_ref()) {
        return Err(balance.to_error());
    }

    // Step 3: Confirm
    let summary = TradeSummary::new(format!(
        "Cross-Chain Swap: {} → {}",
        origin.display_name, destination.display_name
    ))
    .row("Bridge", selected.bridge_provider())
    .row("Sell", format!("{} {}", args.amount, args.sell))
    .row("Buy", format!("~{} {}", selected.buy_amount, args.buy))
    .row("Est Time", selected.estimated_time_display())
    .row("Slippage", format!("{:.2}%", args.slippage as f64 / 100.0));

    let preview = cross_chain_output(
        origin,
        destination,
        &sell,
        &buy,
        selected,
        "needs_confirmation",
        false,
        false,
        None,
        false,
    );
    // Dry-run bypasses the confirmation gate (read-only path, nothing to sign).
    let auto_confirm = global.yes || global.dry_run;
    match confirm_or_preview(
        output,
        auto_confirm,
        &summary,
        "cross-chain",
        &preview,
        metadata.clone(),
        metadata_warnings.clone(),
    )? {
        ConfirmFlow::Confirmed => {}
        ConfirmFlow::PreviewEmitted => return Ok(25),
    }

    // Step 4: Handle allowance if needed (EVM origin)
    if let Some(ref issues) = selected.issues {
        if let Some(ref allowance) = issues.allowance {
            if let Some(signer) = origin_wallet.evm_signer() {
                let origin_rpc = config::resolve_rpc(global.rpc_url.as_deref(), &config, origin)?;

                output.info(&format!(
                    "Approving token for cross-chain swap (spender: {})...",
                    allowance.spender
                ));

                // `evm_signer()` returned Some, so the origin chain is EVM and
                // this can't fail; if that invariant ever breaks we get a
                // structured error instead of silently submitting chain_id=0
                // (a real, attestable network).
                let origin_chain_id = origin.evm_chain_id()?;
                crate::chain::evm::EvmExecutor::ensure_allowance(
                    &origin_rpc.url,
                    origin_chain_id,
                    signer.clone(),
                    &args.sell,
                    &allowance.spender,
                    &selected.sell_amount,
                    crate::cli::ApprovalStrategy::Exact,
                    global.dry_run,
                    &|status| {
                        output.info(status);
                    },
                )
                .await
                .map_err(|e| origin_rpc.enrich_rpc_error(e, origin))?;
            }
        }
    }

    // Step 5: Execute origin transaction
    let spinner = output.spinner_guard("Sending origin transaction...");

    let origin_tx_hash = if selected.transaction.chain_type == "evm" {
        // EVM origin: use alloy provider
        let details = &selected.transaction.details;
        let signer = origin_wallet.evm_signer().ok_or_else(|| CliError::Api {
            code: ErrorCode::ApiError,
            message: "Quote returned an EVM transaction but the origin chain isn't EVM".into(),
            status: None,
            details: None,
            suggestion: None,
        })?;
        let rpc = config::resolve_rpc(global.rpc_url.as_deref(), &config, origin)?;

        // Same reasoning as the ensure_allowance call above — EVM signer
        // implies EVM chain implies Some numeric id.
        let origin_chain_id = origin.evm_chain_id()?;
        let result = crate::chain::evm::EvmExecutor::execute_swap(
            &rpc.url,
            origin_chain_id,
            signer.clone(),
            &args.sell,
            None, // Already handled allowance
            &selected.sell_amount,
            crate::cli::ApprovalStrategy::Exact,
            details.to.as_deref().unwrap_or_default(),
            details.data.as_deref().unwrap_or_default(),
            details.value.as_deref().unwrap_or("0"),
            details.gas.as_deref(),
            details.gas_price.as_deref(),
            // Cross-chain: the buy_token lives on the destination chain, so
            // there's nothing useful in the origin receipt's logs. Skip
            // settled-amount decoding here.
            None,
            false,
            &|status| {
                spinner.set_message(status.to_string());
            },
        )
        .await
        .map_err(|e| rpc.enrich_rpc_error(e, origin))?;

        match result {
            crate::chain::evm::SwapResult::Success(receipt) => receipt.tx_hash,
            // dry_run was hard-coded false above, so DryRun shouldn't happen
            // here. If it does, the executor was refactored to return DryRun
            // for a non-dry-run input — surface as an internal error rather
            // than panicking the binary.
            crate::chain::evm::SwapResult::DryRun => {
                return Err(CliError::Api {
                    code: ErrorCode::ApiError,
                    message:
                        "Internal error: cross-chain origin executor returned DryRun for a non-dry-run send"
                            .into(),
                    status: None,
                    details: None,
                    suggestion: Some(
                        "This is a bug. Re-run with --verbose and report the trace.".into(),
                    ),
                });
            }
        }
    } else if selected.transaction.chain_type == "svm" {
        // Solana origin: deserialize and sign pre-built transaction
        let keypair = origin_wallet
            .solana_keypair()
            .ok_or_else(|| CliError::Api {
                code: ErrorCode::ApiError,
                message: "Quote returned a Solana transaction but the origin chain isn't Solana"
                    .into(),
                status: None,
                details: None,
                suggestion: None,
            })?;
        let serialized_tx = selected
            .transaction
            .details
            .serialized_transaction
            .as_deref()
            .ok_or_else(|| CliError::Api {
                code: ErrorCode::ApiError,
                message: "Cross-chain quote missing serialized Solana transaction".into(),
                status: None,
                details: None,
                suggestion: None,
            })?;

        let mut signers = vec![keypair];
        if let Some(eph) = ephemeral_signer.as_ref() {
            signers.push(eph);
        }
        let signed_tx =
            crate::chain::solana::sign_preserialized_transaction(serialized_tx, &signers)?;

        let solana_chain = chain::resolve_chain("solana")?;
        let resolved = config::resolve_rpc(global.rpc_url.as_deref(), &config, solana_chain)?;

        let rpc = solana_client::nonblocking::rpc_client::RpcClient::new(resolved.url.clone());
        let sig = rpc.send_transaction(&signed_tx).await.map_err(|e| {
            resolved.enrich_rpc_error(
                CliError::Transaction {
                    code: ErrorCode::RpcError,
                    message: format!("Failed to send Solana transaction: {e}"),
                    tx_hash: None,
                    suggestion: None,
                },
                solana_chain,
            )
        })?;

        sig.to_string()
    } else if selected.transaction.chain_type == "tvm" {
        // Tron origin: build, sign, and broadcast a TriggerSmartContract tx.
        let signer = origin_wallet.tron_signer().ok_or_else(|| CliError::Api {
            code: ErrorCode::ApiError,
            message: "Quote returned a Tron transaction but the origin chain isn't Tron".into(),
            status: None,
            details: None,
            suggestion: None,
        })?;
        let details = &selected.transaction.details;
        let to = details.to.as_deref().ok_or_else(|| CliError::Api {
            code: ErrorCode::ApiError,
            message: "Tron quote missing 'to' address".into(),
            status: None,
            details: None,
            suggestion: None,
        })?;
        let data = details.data.as_deref().unwrap_or_default();
        let owner = details.owner_address.as_deref().unwrap_or(&origin_address);
        let value_sun: u64 = details.value.as_deref().unwrap_or("0").parse().unwrap_or(0);

        let rpc = config::resolve_rpc(global.rpc_url.as_deref(), &config, origin)?;
        spinner.set_message("Building and broadcasting Tron transaction...".to_string());
        crate::chain::tron::build_sign_broadcast(
            &rpc.url,
            signer,
            to,
            owner,
            data,
            value_sun,
            crate::chain::tron::DEFAULT_FEE_LIMIT_SUN,
        )
        .await?
    } else {
        return Err(CliError::Api {
            code: ErrorCode::ApiError,
            message: format!(
                "Unknown transaction chain type: {}",
                selected.transaction.chain_type
            ),
            status: None,
            details: None,
            suggestion: None,
        });
    };

    drop(spinner);

    output.info(&format!("Origin tx: {origin_tx_hash}"));

    // Step 6: Poll bridge status
    let spinner = output.spinner_guard("Tracking bridge status...");
    let origin_chain_id = origin.api_chain_id();
    let final_status = crate::api::poll::poll_until_terminal(
        // 60-minute total budget — long-tail bridges (cross-VM or
        // congested L1 finality windows) can take 15-40 minutes; 10
        // minutes was way too aggressive.
        crate::api::poll::PollConfig::new(5, 3600, ErrorCode::BridgeTimeout),
        |elapsed, status: &crate::api::cross_chain::CrossChainStatusResponse| {
            spinner.set_message(format!("Status: {} ({}s)", status.status, elapsed));
        },
        || client.get_cross_chain_status(&origin_chain_id, &origin_tx_hash),
        |s| s.is_terminal(),
        || {
            format!(
                "Bridge not complete after 10 min. Track with: 0x status {origin_tx_hash} --type cross-chain --chain {} --poll",
                args.from
            )
        },
    )
    .await?;

    drop(spinner);

    let origin_explorer_url = origin.explorer_tx_url(&origin_tx_hash);
    let result = cross_chain_output(
        origin,
        destination,
        &sell,
        &buy,
        selected,
        &final_status.status,
        final_status.is_terminal(),
        final_status.is_successful(),
        Some((origin_tx_hash, origin_explorer_url)),
        false,
    );

    let mut warnings = metadata_warnings;
    if !final_status.is_successful() {
        warnings.push(Warning {
            code: "BRIDGE_FAILED".into(),
            message: final_status
                .failure_reason
                .clone()
                .unwrap_or_else(|| format!("Bridge ended with status: {}", final_status.status)),
        });
    }

    let exit_code = if final_status.is_successful() { 0 } else { 11 };
    Ok(output.emit_success("cross-chain", &result, metadata, warnings, exit_code))
}

/// A loaded origin wallet — exactly one of EVM, Solana, or Tron, depending on
/// the origin chain. Held for the lifetime of a cross-chain swap so we don't
/// re-load (and re-prompt the OS keyring) on every step.
enum OriginWallet {
    Evm(alloy::signers::local::PrivateKeySigner),
    Solana(solana_sdk::signer::keypair::Keypair),
    Tron(crate::wallet::tron::TronSigner),
}

impl OriginWallet {
    fn load(
        origin: &chain::ChainInfo,
        config: &config::types::AppConfig,
        cli_wallet: Option<&str>,
    ) -> Result<Self, CliError> {
        if origin.is_tron() {
            let s = crate::wallet::tron::load_tron_signer(config, cli_wallet)?;
            Ok(OriginWallet::Tron(s))
        } else if origin.is_solana() {
            let kp = crate::wallet::solana::load_solana_keypair(config, cli_wallet)?;
            Ok(OriginWallet::Solana(kp))
        } else {
            let s = crate::wallet::evm::load_evm_signer(config, cli_wallet)?;
            Ok(OriginWallet::Evm(s))
        }
    }

    fn address(&self) -> String {
        match self {
            OriginWallet::Evm(s) => format!("{:?}", s.address()),
            OriginWallet::Solana(kp) => crate::wallet::solana::pubkey_string(kp),
            OriginWallet::Tron(s) => s.address().to_string(),
        }
    }

    fn evm_signer(&self) -> Option<&alloy::signers::local::PrivateKeySigner> {
        match self {
            OriginWallet::Evm(s) => Some(s),
            _ => None,
        }
    }

    fn solana_keypair(&self) -> Option<&solana_sdk::signer::keypair::Keypair> {
        match self {
            OriginWallet::Solana(kp) => Some(kp),
            _ => None,
        }
    }

    fn tron_signer(&self) -> Option<&crate::wallet::tron::TronSigner> {
        match self {
            OriginWallet::Tron(s) => Some(s),
            _ => None,
        }
    }
}

/// Resolve the address that will receive the bridged tokens on
/// `destination`. Same chain type as origin (EVM↔EVM, Solana↔Solana) → the
/// origin wallet's address is correct. Cross-VM → load the destination
/// chain's wallet to read its address (we don't need to sign with it; the
/// API just needs to know where to send funds). `--wallet` is deliberately
/// not threaded through for the destination case because the path/secret
/// stored there is for the *origin* wallet — the destination wallet falls
/// through to env / config.
fn resolve_destination_address(
    origin_wallet: &OriginWallet,
    origin: &chain::ChainInfo,
    destination: &chain::ChainInfo,
    config: &config::types::AppConfig,
) -> Result<String, CliError> {
    // Same VM → the origin wallet's own address receives. Compare chain TYPE,
    // not is_evm(): Solana and Tron are both non-EVM but are NOT the same VM.
    if origin.chain_type == destination.chain_type {
        return Ok(origin_wallet.address());
    }
    if destination.is_tron() {
        let s = crate::wallet::tron::load_tron_signer(config, None).map_err(|e| match e {
            CliError::Wallet { code, message } => CliError::Wallet {
                code,
                message: format!(
                    "Cross-chain into Tron needs a Tron wallet to receive into. {message}"
                ),
            },
            other => other,
        })?;
        Ok(s.address().to_string())
    } else if destination.is_solana() {
        let kp = crate::wallet::solana::load_solana_keypair(config, None).map_err(|e| {
            // Surface a clearer message: the user wants to bridge INTO Solana
            // but has no Solana wallet configured. We need at least its
            // pubkey to tell the bridge where to deliver.
            match e {
                CliError::Wallet { code, message } => CliError::Wallet {
                    code,
                    message: format!(
                        "Cross-chain into Solana needs a Solana wallet to receive into. {message}"
                    ),
                },
                other => other,
            }
        })?;
        Ok(crate::wallet::solana::pubkey_string(&kp))
    } else {
        let s = crate::wallet::evm::load_evm_signer(config, None).map_err(|e| match e {
            CliError::Wallet { code, message } => CliError::Wallet {
                code,
                message: format!(
                    "Cross-chain into an EVM chain needs an EVM wallet to receive into. {message}"
                ),
            },
            other => other,
        })?;
        Ok(format!("{:?}", s.address()))
    }
}

/// Resolve token metadata for one side of a cross-chain swap. Solana
/// origins/destinations return None (no on-chain metadata lookup); EVM sides
/// hit the configured RPC and push a `TOKEN_METADATA_UNRESOLVED` warning when
/// the lookup fails. Replaces the near-duplicated blocks in `run`.
async fn resolve_one_evm(
    cache: &mut crate::token_cache::TokenCache,
    chain_info: &chain::ChainInfo,
    token: &str,
    rpc_override: Option<&str>,
    config: &config::types::AppConfig,
    side_label: &str,
    warnings: &mut Vec<Warning>,
) -> Option<crate::token_cache::TokenMeta> {
    if !chain_info.is_evm() {
        return None;
    }
    let rpc = config::try_resolve_rpc_url_with_override(rpc_override, config, chain_info);
    // is_evm() above implies numeric_id is Some; `?` keeps this panic-free.
    let chain_id = chain_info.numeric_id()?;
    let result = match rpc.as_deref() {
        Some(u) => cache.resolve_evm(u, chain_id, token).await,
        None => None,
    };
    if result.is_none() {
        warnings.push(Warning {
            code: crate::token_cache::WARN_METADATA_UNRESOLVED.into(),
            message: format!(
                "Could not resolve metadata for {side_label} token on {}. Showing raw amount.",
                chain_info.display_name
            ),
        });
    }
    result
}

#[cfg(test)]
mod tron_wiring_tests {
    use super::*;

    #[test]
    fn test_same_vm_check_uses_chain_type_not_is_evm() {
        // Solana and Tron are both non-EVM; they must NOT be treated as same-VM.
        let solana = chain::resolve_chain("solana").unwrap();
        let tron = chain::resolve_chain("tron").unwrap();
        assert_ne!(solana.chain_type, tron.chain_type);
    }
}
