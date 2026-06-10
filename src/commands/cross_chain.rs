use crate::api::cross_chain::{CrossChainQuote, CrossChainQuotesResponse};
use crate::api::types::{compute_rate, RouteSource, TokenAmount, TokenInfo};
use crate::api::ApiClient;
use crate::chain;
use crate::cli::{CrossChainArgs, QuoteSort};
use crate::config;
use crate::confirm::{confirm_or_preview, ConfirmFlow, TradeSummary};
use crate::error::{CliError, ErrorCode};
use crate::output::envelope::{Metadata, Warning};
use crate::output::trade::SideMeta;
use crate::output::{HumanDisplay, OutputHandler};
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
fn build_dry_run_output(
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
struct QuotesDisplay {
    quotes: Vec<QuoteSummary>,
}

#[derive(Debug, Serialize)]
struct QuoteSummary {
    index: usize,
    bridge: String,
    /// Pretty-printed receive amount, with symbol when known
    /// (e.g. `"0.977102 USDC"`; falls back to raw integer + label).
    buy_display: String,
    /// Same shape for the post-slippage floor.
    min_buy_display: String,
    estimated_time: String,
    /// Step path summary like `"swap (Base) → bridge (relay) → swap (Arbitrum)"`.
    /// Falls back to the bridge provider name when steps are empty.
    path: String,
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
fn build_step_path(quote: &crate::api::cross_chain::CrossChainQuote) -> String {
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
fn format_quote_amount(raw: &str, buy: &SideMeta) -> String {
    let formatted = crate::api::types::display_amount(raw, buy.decimals);
    format!("{formatted} {}", buy.label())
}

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

    let api_key = global
        .api_key
        .as_deref()
        .or(config.api.api_key.as_deref())
        .ok_or_else(CliError::api_key_missing)?
        .to_string();

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
    let client = ApiClient::new(api_key, global.timeout)?;

    let sort_by = match args.sort {
        QuoteSort::Price => "price",
        QuoteSort::Speed => "speed",
    };

    // Step 1: Get quotes
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
                // `numeric_id()` is Some — express that with `.expect` so a
                // future regression panics instead of silently submitting
                // chain_id=0 (a real, attestable network).
                let origin_chain_id = origin
                    .numeric_id()
                    .expect("EVM origin chain has a numeric id");
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
        let origin_chain_id = origin
            .numeric_id()
            .expect("EVM origin chain has a numeric id");
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

        let signed_tx =
            crate::chain::solana::sign_preserialized_transaction(serialized_tx, keypair)?;

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

/// A loaded origin wallet — exactly one of EVM or Solana, depending on the
/// origin chain. Held for the lifetime of a cross-chain swap so we don't
/// re-load (and re-prompt the OS keyring) on every step.
enum OriginWallet {
    Evm(alloy::signers::local::PrivateKeySigner),
    Solana(solana_sdk::signer::keypair::Keypair),
}

impl OriginWallet {
    fn load(
        origin: &chain::ChainInfo,
        config: &config::types::AppConfig,
        cli_wallet: Option<&str>,
    ) -> Result<Self, CliError> {
        if origin.is_solana() {
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
    if origin.is_evm() == destination.is_evm() {
        return Ok(origin_wallet.address());
    }
    if destination.is_solana() {
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

fn select_quote(
    args: &CrossChainArgs,
    output: &OutputHandler,
    resp: &CrossChainQuotesResponse,
    buy: &SideMeta,
    auto_confirm: bool,
) -> Result<usize, CliError> {
    // If user specified a selection via flag
    if let Some(ref sel) = args.select_quote {
        return match sel.as_str() {
            "best-price" | "0" => Ok(0), // Already sorted by API
            "fastest" => {
                let idx = resp
                    .quotes
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, q)| q.estimated_time_seconds.unwrap_or(u64::MAX))
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                Ok(idx)
            }
            n => {
                let idx: usize = n.parse().map_err(|_| CliError::Api {
                    code: ErrorCode::InputInvalid,
                    message: format!(
                        "Invalid quote selection: '{n}'. Use a number, 'best-price', or 'fastest'"
                    ),
                    status: None,
                    details: None,
                    suggestion: None,
                })?;
                if idx >= resp.quotes.len() {
                    return Err(CliError::Api {
                        code: ErrorCode::InputInvalid,
                        message: format!(
                            "Quote index {idx} out of range (0-{})",
                            resp.quotes.len() - 1
                        ),
                        status: None,
                        details: None,
                        suggestion: None,
                    });
                }
                Ok(idx)
            }
        };
    }

    // With --yes and no selection: auto-select first quote
    if auto_confirm {
        return Ok(0);
    }

    // Non-Human output formats can't drive an interactive prompt — fall back
    // to the first quote and surface a hint via the regular error/output path
    // instead of hanging on dialoguer.
    if !matches!(output.format, crate::cli::OutputFormat::Human) {
        if resp.quotes.len() > 1 {
            return Err(CliError::Api {
                code: ErrorCode::InputInvalid,
                message: format!(
                    "{} quotes returned but no --select-quote provided in non-interactive output mode",
                    resp.quotes.len()
                ),
                status: None,
                details: None,
                suggestion: Some(
                    "Re-run with --select-quote <index|best-price|fastest> or --yes to auto-select the first".into(),
                ),
            });
        }
        return Ok(0);
    }

    // Interactive selection (Human output only)
    let display = QuotesDisplay {
        quotes: resp
            .quotes
            .iter()
            .enumerate()
            .map(|(i, q)| QuoteSummary {
                index: i,
                bridge: q.bridge_provider(),
                buy_display: format_quote_amount(&q.buy_amount, buy),
                min_buy_display: format_quote_amount(&q.min_buy_amount, buy),
                estimated_time: q.estimated_time_display(),
                path: build_step_path(q),
            })
            .collect(),
    };

    // Display quotes table
    let stdout = io::stdout();
    let mut out = stdout.lock();
    display.display_human(&mut out, output.color).ok();
    drop(out);

    if resp.quotes.len() == 1 {
        return Ok(0);
    }

    let selection = dialoguer::Input::<usize>::new()
        .with_prompt(format!("Select quote [0-{}]", resp.quotes.len() - 1))
        .default(0)
        .interact()
        .map_err(|_| CliError::UserCancelled)?;

    if selection >= resp.quotes.len() {
        return Err(CliError::Api {
            code: ErrorCode::InputInvalid,
            message: format!("Invalid selection: {selection}"),
            status: None,
            details: None,
            suggestion: None,
        });
    }

    Ok(selection)
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
    // is_evm() above implies numeric_id is Some — express that explicitly.
    let chain_id = chain_info.numeric_id().expect("EVM chain has a numeric id");
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

/// Assemble a `CrossChainOutput` from the selected quote + outcome. Used by
/// the needs-confirmation, dry-run, and final-status paths.
#[allow(clippy::too_many_arguments)]
fn cross_chain_output(
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
