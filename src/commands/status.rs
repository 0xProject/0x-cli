use crate::api::ApiClient;
use crate::chain;
use crate::cli::{StatusArgs, StatusType};
use crate::config;
use crate::error::{CliError, ErrorCode};
use crate::output::envelope::Metadata;
use crate::output::{HumanDisplay, OutputHandler};
use serde::Serialize;
use std::io::{self, Write};

/// Unified status result.
#[derive(Debug, Serialize)]
pub struct StatusOutput {
    pub status: String,
    pub status_detail: String,
    pub terminal: bool,
    pub successful: bool,
    pub transactions: Vec<StatusTransaction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct StatusTransaction {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explorer_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<u64>,
}

impl HumanDisplay for StatusOutput {
    fn display_human(&self, writer: &mut dyn Write, color: bool) -> io::Result<()> {
        use colored::Colorize;

        let status_colored = if self.successful {
            if color {
                self.status.green().bold().to_string()
            } else {
                self.status.clone()
            }
        } else if self.terminal {
            if color {
                self.status.red().bold().to_string()
            } else {
                self.status.clone()
            }
        } else if color {
            self.status.yellow().bold().to_string()
        } else {
            self.status.clone()
        };

        writeln!(writer, "\n  Status: {status_colored}")?;
        writeln!(writer, "  {}", self.status_detail)?;

        for tx in &self.transactions {
            if let Some(ref hash) = tx.tx_hash {
                writeln!(writer, "  Tx: {hash}")?;
            }
            if let Some(ref url) = tx.explorer_url {
                writeln!(writer, "  Explorer: {url}")?;
            }
        }

        if let Some(ref reason) = self.failure_reason {
            if color {
                writeln!(writer, "  Reason: {}", reason.red())?;
            } else {
                writeln!(writer, "  Reason: {reason}")?;
            }
        }

        Ok(())
    }
}

pub async fn run(
    args: &StatusArgs,
    output: &OutputHandler,
    global: &crate::GlobalOpts,
) -> Result<i32, CliError> {
    let config = config::load_config()?;

    let api_key = global
        .api_key
        .as_deref()
        .or(config.api.api_key.as_deref())
        .ok_or_else(CliError::api_key_missing)?
        .to_string();

    let client = ApiClient::new(api_key, global.timeout)?;

    // Auto-detect status type if not specified
    let status_type = args.r#type.unwrap_or_else(|| {
        // Heuristic: 0x-prefixed 66-char hash → likely EVM tx → cross-chain
        if args.hash.starts_with("0x") && args.hash.len() == 66 {
            StatusType::CrossChain
        } else {
            StatusType::Gasless
        }
    });

    match status_type {
        StatusType::Gasless => run_gasless_status(args, output, &client).await,
        StatusType::CrossChain => run_cross_chain_status(args, output, &client).await,
    }
}

async fn run_gasless_status(
    args: &StatusArgs,
    output: &OutputHandler,
    client: &ApiClient,
) -> Result<i32, CliError> {
    let chain_str = args.chain.as_deref().ok_or_else(|| CliError::Api {
        code: ErrorCode::InputInvalid,
        message: "Chain is required for gasless status. Use --chain <id>".into(),
        status: None,
        details: None,
        suggestion: None,
    })?;

    let chain_info = chain::resolve_chain(chain_str)?;
    let chain_id = chain_info.numeric_id().ok_or_else(|| CliError::Api {
        code: ErrorCode::InputInvalid,
        message: "Gasless status requires an EVM chain".into(),
        status: None,
        details: None,
        suggestion: None,
    })?;

    let metadata = Metadata::for_chain(chain_info);

    if args.poll {
        let spinner = output.spinner("Polling gasless status...");
        let cfg = crate::api::poll::PollConfig::new(
            args.poll_interval,
            args.poll_interval.saturating_mul(120),
            ErrorCode::TransactionTimeout,
        );
        let resp = crate::api::poll::poll_until_terminal(
            cfg,
            |elapsed, r: &crate::api::gasless::GaslessStatusResponse| {
                if let Some(s) = &spinner {
                    s.set_message(format!("Status: {} ({}s)", r.status, elapsed));
                }
            },
            || client.get_gasless_status(&args.hash, chain_id),
            |r| r.is_terminal(),
            || "Status polling timed out".to_string(),
        )
        .await?;

        if let Some(s) = spinner {
            s.finish_and_clear();
        }

        let result = gasless_to_output(&resp, chain_info);
        return Ok(output.emit_success("status", &result, metadata, Vec::new(), 0));
    }

    // Single check
    let spinner = output.spinner("Checking status...");
    let resp = client.get_gasless_status(&args.hash, chain_id).await?;

    if let Some(s) = spinner {
        s.finish_and_clear();
    }

    let result = gasless_to_output(&resp, chain_info);
    Ok(output.emit_success("status", &result, metadata, Vec::new(), 0))
}

fn gasless_to_output(
    resp: &crate::api::gasless::GaslessStatusResponse,
    chain_info: &chain::ChainInfo,
) -> StatusOutput {
    let transactions = resp
        .transactions
        .iter()
        .map(|t| StatusTransaction {
            chain_id: chain_info.numeric_id().map(|id| id.to_string()),
            chain_name: Some(chain_info.display_name.to_string()),
            tx_hash: t.hash.clone(),
            explorer_url: t.hash.as_ref().map(|h| chain_info.explorer_tx_url(h)),
            timestamp: t.timestamp,
        })
        .collect();

    let status_detail = match resp.status.as_str() {
        "pending" => "Trade is pending submission",
        "submitted" => "Trade has been submitted to the network",
        "succeeded" => "Trade transaction succeeded, waiting for confirmations",
        "confirmed" => "Trade confirmed on-chain",
        "failed" => "Trade failed",
        s => s,
    };

    StatusOutput {
        status: resp.status.clone(),
        status_detail: status_detail.to_string(),
        terminal: resp.is_terminal(),
        successful: resp.is_successful(),
        transactions,
        failure_reason: None,
    }
}

async fn run_cross_chain_status(
    args: &StatusArgs,
    output: &OutputHandler,
    client: &ApiClient,
) -> Result<i32, CliError> {
    let chain_str = args.chain.as_deref().ok_or_else(|| CliError::Api {
        code: ErrorCode::InputInvalid,
        message: "Origin chain is required for cross-chain status. Use --chain <id>".into(),
        status: None,
        details: None,
        suggestion: None,
    })?;

    let chain_info = chain::resolve_chain(chain_str)?;

    let metadata = Metadata::for_chain(chain_info);

    if args.poll {
        let spinner = output.spinner("Polling cross-chain status...");
        let cfg = crate::api::poll::PollConfig::new(
            args.poll_interval,
            args.poll_interval.saturating_mul(120),
            ErrorCode::BridgeTimeout,
        );
        let chain_id_str = chain_info.api_chain_id();
        let resp = crate::api::poll::poll_until_terminal(
            cfg,
            |elapsed, r: &crate::api::cross_chain::CrossChainStatusResponse| {
                if let Some(s) = &spinner {
                    s.set_message(format!("Status: {} ({}s)", r.status, elapsed));
                }
            },
            || client.get_cross_chain_status(&chain_id_str, &args.hash),
            |r| r.is_terminal(),
            || "Cross-chain status polling timed out".to_string(),
        )
        .await?;

        if let Some(s) = spinner {
            s.finish_and_clear();
        }

        let result = cross_chain_to_output(&resp);
        return Ok(output.emit_success("status", &result, metadata, Vec::new(), 0));
    }

    // Single check
    let spinner = output.spinner("Checking cross-chain status...");
    let resp = client
        .get_cross_chain_status(&chain_info.api_chain_id(), &args.hash)
        .await?;

    if let Some(s) = spinner {
        s.finish_and_clear();
    }

    let result = cross_chain_to_output(&resp);
    Ok(output.emit_success("status", &result, metadata, Vec::new(), 0))
}

fn json_chain_id_to_string(v: &serde_json::Value) -> Option<String> {
    v.as_u64()
        .map(|n| n.to_string())
        .or_else(|| v.as_str().map(|s| s.to_string()))
}

fn cross_chain_to_output(
    resp: &crate::api::cross_chain::CrossChainStatusResponse,
) -> StatusOutput {
    let transactions = resp
        .transactions
        .iter()
        .map(|t| {
            let chain_id_str = t.chain_id.as_ref().and_then(json_chain_id_to_string);
            let resolved = chain_id_str
                .as_deref()
                .and_then(|id| chain::resolve_chain(id).ok());
            let chain_name = resolved.map(|c| c.display_name.to_string());
            let explorer_url = match (&resolved, &t.tx_hash) {
                (Some(c), Some(hash)) => Some(c.explorer_tx_url(hash)),
                _ => None,
            };
            StatusTransaction {
                chain_id: chain_id_str,
                chain_name,
                tx_hash: t.tx_hash.clone(),
                explorer_url,
                timestamp: t.timestamp,
            }
        })
        .collect();

    let status_detail = match resp.status.as_str() {
        "origin_tx_pending" => "Origin transaction is pending",
        "origin_tx_succeeded" => "Origin transaction succeeded",
        "origin_tx_confirmed" => "Origin transaction confirmed, bridge is processing",
        "origin_tx_reverted" => "Origin transaction reverted",
        "bridge_pending" => "Bridge transfer is in progress",
        "bridge_filled" => "Bridge transfer complete — tokens delivered",
        "bridge_failed" => "Bridge transfer failed",
        "unknown" => "Status unknown",
        s => s,
    };

    StatusOutput {
        status: resp.status.clone(),
        status_detail: status_detail.to_string(),
        terminal: resp.is_terminal(),
        successful: resp.is_successful(),
        transactions,
        failure_reason: resp.failure_reason.clone(),
    }
}
