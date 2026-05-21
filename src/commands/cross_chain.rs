use crate::api::cross_chain::CrossChainQuotesResponse;
use crate::api::types::{TokenAmount, TokenInfo};
use crate::api::ApiClient;
use crate::chain;
use crate::cli::{CrossChainArgs, QuoteSort};
use crate::config;
use crate::confirm::{confirm_trade, ConfirmResult, TradeSummary};
use crate::error::{CliError, ErrorCode};
use crate::output::envelope::{Metadata, Warning};
use crate::output::human::DataTable;
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
    pub bridge: String,
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

        writeln!(writer, "  {:<14} {} → {}", "Route:", self.origin_chain, self.destination_chain)?;
        writeln!(writer, "  {:<14} {}", "Bridge:", self.bridge)?;
        writeln!(writer, "  {:<14} {}", "Sell:", self.sell_amount.formatted)?;
        writeln!(writer, "  {:<14} {}", "Buy:", self.buy_amount.formatted)?;
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

/// Quotes display for human output.
#[derive(Debug, Serialize)]
struct QuotesDisplay {
    quotes: Vec<QuoteSummary>,
}

#[derive(Debug, Serialize)]
struct QuoteSummary {
    index: usize,
    bridge: String,
    buy_amount: String,
    estimated_time: String,
}

impl HumanDisplay for QuotesDisplay {
    fn display_human(&self, writer: &mut dyn Write, color: bool) -> io::Result<()> {
        let table = DataTable {
            title: Some("Cross-Chain Quotes".to_string()),
            headers: vec![
                "#".into(),
                "Bridge".into(),
                "You Receive".into(),
                "Est Time".into(),
            ],
            rows: self
                .quotes
                .iter()
                .map(|q| {
                    vec![
                        q.index.to_string(),
                        q.bridge.clone(),
                        q.buy_amount.clone(),
                        q.estimated_time.clone(),
                    ]
                })
                .collect(),
        };
        table.display_human(writer, color)
    }
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

    let metadata = Metadata {
        chain_id: origin.numeric_id(),
        chain_name: Some(origin.display_name.to_string()),
        ..Default::default()
    };

    let client = ApiClient::new(api_key, global.timeout)?;

    let sort_by = match args.sort {
        QuoteSort::Price => "price",
        QuoteSort::Speed => "speed",
    };

    // Step 1: Get quotes
    let spinner = output.spinner("Fetching cross-chain quotes...");
    let quotes_resp = client
        .get_cross_chain_quotes(
            &origin.api_chain_id(),
            &destination.api_chain_id(),
            &args.sell,
            &args.buy,
            &args.amount,
            &origin_address,
            Some(args.slippage),
            Some(sort_by),
            Some(args.max_quotes),
        )
        .await?;

    if let Some(s) = &spinner {
        s.finish_and_clear();
    }

    if quotes_resp.quotes.is_empty() || !quotes_resp.liquidity_available {
        return Err(CliError::Api {
            code: ErrorCode::NoLiquidity,
            message: "No cross-chain quotes available for this route".into(),
            status: None,
            details: None,
            suggestion: Some("Try a different token pair, amount, or chain combination".into()),
        });
    }

    // Step 2: Select quote
    let selected_idx = select_quote(args, output, &quotes_resp, global.yes)?;
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

    match confirm_trade(output.format, global.yes, output.color, &summary)? {
        ConfirmResult::Confirmed => {}
        ConfirmResult::NeedsConfirmation => {
            // Output selected quote for agent review
            let preview = CrossChainOutput {
                origin_chain: origin.display_name.to_string(),
                destination_chain: destination.display_name.to_string(),
                sell_token: TokenInfo { address: args.sell.clone(), symbol: None, decimals: None },
                buy_token: TokenInfo { address: args.buy.clone(), symbol: None, decimals: None },
                sell_amount: TokenAmount { raw: selected.sell_amount.clone(), formatted: selected.sell_amount.clone(), usd_value: None },
                buy_amount: TokenAmount { raw: selected.buy_amount.clone(), formatted: selected.buy_amount.clone(), usd_value: None },
                bridge: selected.bridge_provider(),
                estimated_time_seconds: selected.estimated_time_seconds,
                status: "needs_confirmation".into(),
                terminal: false,
                successful: false,
                origin_tx_hash: None,
                origin_explorer_url: None,
                dry_run: false,
            };
            let _ = output.success("cross-chain", &preview, metadata, Vec::new());
            return Ok(20);
        }
    }

    if global.dry_run {
        let result = CrossChainOutput {
            origin_chain: origin.display_name.to_string(),
            destination_chain: destination.display_name.to_string(),
            sell_token: TokenInfo { address: args.sell.clone(), symbol: None, decimals: None },
            buy_token: TokenInfo { address: args.buy.clone(), symbol: None, decimals: None },
            sell_amount: TokenAmount { raw: selected.sell_amount.clone(), formatted: selected.sell_amount.clone(), usd_value: None },
            buy_amount: TokenAmount { raw: selected.buy_amount.clone(), formatted: selected.buy_amount.clone(), usd_value: None },
            bridge: selected.bridge_provider(),
            estimated_time_seconds: selected.estimated_time_seconds,
            status: "dry_run".into(),
            terminal: true,
            successful: true,
            origin_tx_hash: None,
            origin_explorer_url: None,
            dry_run: true,
        };
        return output
            .success("cross-chain", &result, metadata, Vec::new())
            .map(|_| 30)
            .map_err(|e| CliError::config(ErrorCode::Unknown, e.to_string()));
    }

    // Step 4: Handle allowance if needed (EVM origin)
    if let Some(ref issues) = selected.issues {
        if let Some(ref allowance) = issues.allowance {
            if let Some(signer) = origin_wallet.evm_signer() {
                let origin_rpc = config::resolve_rpc_url_with_override(
                    global.rpc_url.as_deref(),
                    &config,
                    origin,
                )?;

                output.info(&format!(
                    "Approving token for cross-chain swap (spender: {})...",
                    allowance.spender
                ));

                crate::chain::evm::EvmExecutor::ensure_allowance(
                    &origin_rpc,
                    signer.clone(),
                    &args.sell,
                    &allowance.spender,
                    &selected.sell_amount,
                    crate::cli::ApprovalStrategy::Exact,
                    false,
                    &|status| { output.info(status); },
                )
                .await?;
            }
        }
    }

    // Step 5: Execute origin transaction
    let spinner = output.spinner("Sending origin transaction...");

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
        let rpc_url =
            config::resolve_rpc_url_with_override(global.rpc_url.as_deref(), &config, origin)?;

        let result = crate::chain::evm::EvmExecutor::execute_swap(
            &rpc_url,
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
            false,
            &|status| {
                if let Some(s) = &spinner {
                    s.set_message(status.to_string());
                }
            },
        )
        .await?;

        match result {
            crate::chain::evm::SwapResult::Success(receipt) => receipt.tx_hash,
            _ => unreachable!(),
        }
    } else if selected.transaction.chain_type == "svm" {
        // Solana origin: deserialize and sign pre-built transaction
        let keypair = origin_wallet.solana_keypair().ok_or_else(|| CliError::Api {
            code: ErrorCode::ApiError,
            message: "Quote returned a Solana transaction but the origin chain isn't Solana".into(),
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

        let signed_tx = crate::chain::solana::sign_preserialized_transaction(serialized_tx, keypair)?;

        let solana_chain = chain::resolve_chain("solana")?;
        let rpc_url = config::resolve_rpc_url_with_override(
            global.rpc_url.as_deref(),
            &config,
            solana_chain,
        )?;

        let rpc = solana_client::nonblocking::rpc_client::RpcClient::new(rpc_url);
        let sig = rpc.send_transaction(&signed_tx).await.map_err(|e| CliError::Transaction {
            code: ErrorCode::SigningFailed,
            message: format!("Failed to send Solana transaction: {e}"),
            tx_hash: None,
            suggestion: None,
        })?;

        sig.to_string()
    } else {
        return Err(CliError::Api {
            code: ErrorCode::ApiError,
            message: format!("Unknown transaction chain type: {}", selected.transaction.chain_type),
            status: None,
            details: None,
            suggestion: None,
        });
    };

    if let Some(s) = &spinner {
        s.finish_and_clear();
    }

    output.info(&format!("Origin tx: {origin_tx_hash}"));

    // Step 6: Poll bridge status
    let spinner = output.spinner("Tracking bridge status...");
    let origin_chain_id = origin.api_chain_id();
    let final_status = crate::api::poll::poll_until_terminal(
        crate::api::poll::PollConfig::new(5, 600, ErrorCode::BridgeTimeout),
        |elapsed, status: &crate::api::cross_chain::CrossChainStatusResponse| {
            if let Some(s) = &spinner {
                s.set_message(format!("Status: {} ({}s)", status.status, elapsed));
            }
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

    if let Some(s) = spinner {
        s.finish_and_clear();
    }

    let origin_explorer_url = Some(origin.explorer_tx_url(&origin_tx_hash));
    let result = CrossChainOutput {
        origin_chain: origin.display_name.to_string(),
        destination_chain: destination.display_name.to_string(),
        sell_token: TokenInfo { address: args.sell.clone(), symbol: None, decimals: None },
        buy_token: TokenInfo { address: args.buy.clone(), symbol: None, decimals: None },
        sell_amount: TokenAmount { raw: selected.sell_amount.clone(), formatted: selected.sell_amount.clone(), usd_value: None },
        buy_amount: TokenAmount { raw: selected.buy_amount.clone(), formatted: selected.buy_amount.clone(), usd_value: None },
        bridge: selected.bridge_provider(),
        estimated_time_seconds: selected.estimated_time_seconds,
        status: final_status.status.clone(),
        terminal: final_status.is_terminal(),
        successful: final_status.is_successful(),
        origin_tx_hash: Some(origin_tx_hash),
        origin_explorer_url,
        dry_run: false,
    };

    let mut warnings = Vec::new();
    if !final_status.is_successful() {
        warnings.push(Warning {
            code: "BRIDGE_FAILED".into(),
            message: final_status.failure_reason.unwrap_or_else(|| format!("Bridge ended with status: {}", final_status.status)),
        });
    }

    output
        .success("cross-chain", &result, metadata, warnings)
        .map_err(|e| CliError::config(ErrorCode::Unknown, e.to_string()))
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

fn select_quote(
    args: &CrossChainArgs,
    output: &OutputHandler,
    resp: &CrossChainQuotesResponse,
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
                    message: format!("Invalid quote selection: '{n}'. Use a number, 'best-price', or 'fastest'"),
                    status: None,
                    details: None,
                    suggestion: None,
                })?;
                if idx >= resp.quotes.len() {
                    return Err(CliError::Api {
                        code: ErrorCode::InputInvalid,
                        message: format!("Quote index {idx} out of range (0-{})", resp.quotes.len() - 1),
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

    // Interactive selection
    let display = QuotesDisplay {
        quotes: resp
            .quotes
            .iter()
            .enumerate()
            .map(|(i, q)| QuoteSummary {
                index: i,
                bridge: q.bridge_provider(),
                buy_amount: q.buy_amount.clone(),
                estimated_time: q.estimated_time_display(),
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
