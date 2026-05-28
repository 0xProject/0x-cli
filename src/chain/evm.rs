use crate::cli::ApprovalStrategy;
use crate::error::{CliError, ErrorCode};
use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::network::TransactionBuilder;
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol;
use std::str::FromStr;
use std::time::Duration;

// Minimal ERC-20 ABI for allowance checking and approvals
sol! {
    #[sol(rpc)]
    contract IERC20 {
        function approve(address spender, uint256 amount) external returns (bool);
        function allowance(address owner, address spender) external view returns (uint256);
        function balanceOf(address account) external view returns (uint256);
        function decimals() external view returns (uint8);
        function symbol() external view returns (string);
    }
}

/// Create a provider and execute a swap. Uses a concrete provider type internally.
pub struct EvmExecutor;

impl EvmExecutor {
    /// Fail closed if the RPC reports a chain_id that doesn't match the one
    /// the user selected via --chain. Catches "I pointed --rpc-url at an Arbitrum
    /// endpoint while passing --chain ethereum" type mistakes before any
    /// transaction (or approval) is signed.
    async fn verify_chain_id<P: Provider>(
        provider: &P,
        expected: u64,
        rpc_url: &str,
    ) -> Result<(), CliError> {
        let reported = provider
            .get_chain_id()
            .await
            .map_err(|e| CliError::Api {
                code: ErrorCode::RpcError,
                message: format!("Failed to read chain_id from RPC: {e}"),
                status: None,
                details: None,
                suggestion: Some("Check that the RPC endpoint is reachable".into()),
            })?;
        if reported != expected {
            return Err(CliError::Api {
                code: ErrorCode::ChainNotSupported,
                message: format!(
                    "RPC at {rpc_url} reports chain_id {reported}, but --chain selected {expected}. Refusing to sign cross-chain."
                ),
                status: None,
                details: None,
                suggestion: Some(
                    "Configure --rpc-url (or `0x config set rpc.<chain> <url>`) to match the selected chain".into(),
                ),
            });
        }
        Ok(())
    }

    /// Ensure `spender` has at least `sell_amount` allowance to spend `sell_token`
    /// on behalf of the signer. Sends an approval tx and waits for confirmation
    /// when allowance is insufficient. Returns whether an approval was sent.
    ///
    /// In `dry_run` mode, reports the gap on stderr and returns `Ok(false)` —
    /// no on-chain tx is sent. Callers using dry-run for "simulate the swap"
    /// must therefore tolerate a real transferFrom revert if allowance is short.
    #[allow(clippy::too_many_arguments)]
    pub async fn ensure_allowance(
        rpc_url: &str,
        chain_id: u64,
        signer: PrivateKeySigner,
        sell_token: &str,
        spender: &str,
        sell_amount: &str,
        approval_strategy: ApprovalStrategy,
        dry_run: bool,
        on_status: &dyn Fn(&str),
    ) -> Result<bool, CliError> {
        let address = signer.address();

        let provider = ProviderBuilder::new()
            .wallet(signer)
            .connect(rpc_url)
            .await
            .map_err(|e| CliError::Api {
                code: ErrorCode::RpcError,
                message: format!("Failed to connect to RPC: {e}"),
                status: None,
                details: None,
                suggestion: Some(format!("Check the RPC URL: {rpc_url}")),
            })?;

        Self::verify_chain_id(&provider, chain_id, rpc_url).await?;

        let token_addr = Address::from_str(sell_token).map_err(|e| CliError::Api {
            code: ErrorCode::InputInvalid,
            message: format!("Invalid sell token address: {e}"),
            status: None,
            details: None,
            suggestion: None,
        })?;
        let spender_addr = Address::from_str(spender).map_err(|e| CliError::Api {
            code: ErrorCode::InputInvalid,
            message: format!("Invalid spender address: {e}"),
            status: None,
            details: None,
            suggestion: None,
        })?;
        let sell_amount_u256 = U256::from_str(sell_amount).map_err(|e| CliError::Api {
            code: ErrorCode::InputInvalid,
            message: format!("Invalid sell amount '{sell_amount}': {e}"),
            status: None,
            details: None,
            suggestion: None,
        })?;

        on_status("Checking token allowance...");
        let contract = IERC20::new(token_addr, &provider);
        let current_allowance = contract
            .allowance(address, spender_addr)
            .call()
            .await
            .map_err(|e| CliError::Transaction {
                code: ErrorCode::RpcError,
                message: format!("Failed to check allowance: {e}"),
                tx_hash: None,
                suggestion: None,
            })?;

        if current_allowance >= sell_amount_u256 {
            return Ok(false);
        }

        if dry_run {
            on_status("Approval needed (skipped in dry-run)");
            return Ok(false);
        }

        let approve_amount = match approval_strategy {
            ApprovalStrategy::Exact => sell_amount_u256,
            ApprovalStrategy::Unlimited => U256::MAX,
        };

        on_status("Sending token approval...");
        let pending = contract
            .approve(spender_addr, approve_amount)
            .send()
            .await
            .map_err(|e| CliError::Transaction {
                code: ErrorCode::SigningFailed,
                message: format!("Failed to send approval: {e}"),
                tx_hash: None,
                suggestion: None,
            })?;

        let approval_hash = format!("{:?}", pending.tx_hash());
        on_status(&format!("Approval tx sent: {approval_hash}"));

        let receipt = tokio::time::timeout(
            Duration::from_secs(120),
            pending.get_receipt(),
        )
        .await;

        match receipt {
            Ok(Ok(r)) => {
                if !r.status() {
                    return Err(CliError::Transaction {
                        code: ErrorCode::TransactionReverted,
                        message: "Approval transaction reverted".into(),
                        tx_hash: Some(approval_hash),
                        suggestion: None,
                    });
                }
            }
            Ok(Err(e)) => {
                return Err(CliError::Transaction {
                    code: ErrorCode::TransactionTimeout,
                    message: format!("Approval receipt error: {e}"),
                    tx_hash: Some(approval_hash),
                    suggestion: Some("Transaction was sent. Check your block explorer to verify.".into()),
                });
            }
            Err(_) => {
                return Err(CliError::Transaction {
                    code: ErrorCode::TransactionTimeout,
                    message: "Approval sent but confirmation timed out after 120s".into(),
                    tx_hash: Some(approval_hash),
                    suggestion: Some("Transaction IS on-chain. Check your block explorer to verify.".into()),
                });
            }
        }

        on_status("Approval confirmed");
        Ok(true)
    }

    /// Check allowance, optionally approve, simulate, and send a swap transaction.
    /// `chain_id` binds the signed transaction to a specific chain — this both
    /// prevents replay onto a different chain and guards against a misconfigured
    /// `--rpc-url` pointing at the wrong network (we verify the RPC reports the
    /// same chain id before sending anything).
    #[allow(clippy::too_many_arguments)]
    pub async fn execute_swap(
        rpc_url: &str,
        chain_id: u64,
        signer: PrivateKeySigner,
        sell_token: &str,
        spender: Option<&str>,
        sell_amount: &str,
        approval_strategy: ApprovalStrategy,
        to: &str,
        data: &str,
        value: &str,
        gas: Option<&str>,
        gas_price: Option<&str>,
        dry_run: bool,
        on_status: &dyn Fn(&str),
    ) -> Result<SwapResult, CliError> {
        // Step 1: Ensure allowance if a spender is provided.
        if let Some(spender_addr_str) = spender {
            Self::ensure_allowance(
                rpc_url,
                chain_id,
                signer.clone(),
                sell_token,
                spender_addr_str,
                sell_amount,
                approval_strategy,
                dry_run,
                on_status,
            )
            .await?;
        }

        let address = signer.address();

        let provider = ProviderBuilder::new()
            .wallet(signer)
            .connect(rpc_url)
            .await
            .map_err(|e| CliError::Api {
                code: ErrorCode::RpcError,
                message: format!("Failed to connect to RPC: {e}"),
                status: None,
                details: None,
                suggestion: Some(format!("Check the RPC URL: {rpc_url}")),
            })?;

        Self::verify_chain_id(&provider, chain_id, rpc_url).await?;

        // Step 2: Build the swap transaction
        let to_addr = Address::from_str(to).map_err(|e| CliError::Api {
            code: ErrorCode::InputInvalid,
            message: format!("Invalid 'to' address: {e}"),
            status: None,
            details: None,
            suggestion: None,
        })?;

        let value_u256 = if value.is_empty() || value == "0" {
            U256::ZERO
        } else {
            U256::from_str(value).map_err(|e| CliError::Api {
                code: ErrorCode::InputInvalid,
                message: format!("Invalid transaction value '{value}': {e}"),
                status: None,
                details: None,
                suggestion: None,
            })?
        };
        let data_bytes = Bytes::from(
            hex::decode(data.strip_prefix("0x").unwrap_or(data)).map_err(|e| CliError::Api {
                code: ErrorCode::InputInvalid,
                message: format!("Invalid transaction data: {e}"),
                status: None,
                details: None,
                suggestion: None,
            })?,
        );

        let mut tx = TransactionRequest::default()
            .to(to_addr)
            .input(data_bytes.into())
            .value(value_u256)
            .from(address)
            .with_chain_id(chain_id);

        if let Some(gas_str) = gas {
            if let Ok(gas_val) = gas_str.parse::<u64>() {
                tx = tx.gas_limit(gas_val);
            }
        }
        if let Some(gp_str) = gas_price {
            if let Ok(gp_val) = gp_str.parse::<u128>() {
                tx = tx.gas_price(gp_val);
            }
        }

        // Step 3: Simulate
        on_status("Simulating transaction...");
        provider
            .call(tx.clone())
            .await
            .map_err(|e| CliError::Transaction {
                code: ErrorCode::SimulationFailed,
                message: format!("Transaction simulation failed: {e}"),
                tx_hash: None,
                suggestion: Some(
                    "The transaction would revert. Check token balance, slippage, and parameters."
                        .into(),
                ),
            })?;

        if dry_run {
            return Ok(SwapResult::DryRun);
        }

        // Step 4: Send
        on_status("Sending swap transaction...");
        let pending = provider
            .send_transaction(tx)
            .await
            .map_err(|e| CliError::Transaction {
                code: ErrorCode::SigningFailed,
                message: format!("Failed to send transaction: {e}"),
                tx_hash: None,
                suggestion: None,
            })?;

        let tx_hash = format!("{:?}", pending.tx_hash());
        on_status(&format!("Transaction sent: {tx_hash}"));

        // Step 5: Wait for receipt with explicit timeout
        on_status("Waiting for confirmation...");
        let receipt = tokio::time::timeout(
            Duration::from_secs(120),
            pending.get_receipt(),
        )
        .await;

        let receipt = match receipt {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                return Err(CliError::Transaction {
                    code: ErrorCode::TransactionTimeout,
                    message: format!("Transaction sent but receipt failed: {e}"),
                    tx_hash: Some(tx_hash),
                    suggestion: Some("Your transaction was sent to the network. Check the block explorer to verify its status.".into()),
                });
            }
            Err(_) => {
                return Err(CliError::Transaction {
                    code: ErrorCode::TransactionTimeout,
                    message: "Transaction sent but confirmation timed out after 120s".into(),
                    tx_hash: Some(tx_hash),
                    suggestion: Some("Your transaction IS on-chain but unconfirmed. Check the block explorer to verify.".into()),
                });
            }
        };

        if !receipt.status() {
            return Err(CliError::Transaction {
                code: ErrorCode::TransactionReverted,
                message: "Swap transaction reverted on-chain".into(),
                tx_hash: Some(tx_hash),
                suggestion: Some(
                    "The swap failed. Possible causes: slippage exceeded, insufficient balance, or token restrictions."
                        .into(),
                ),
            });
        }

        Ok(SwapResult::Success(SwapReceipt {
            tx_hash,
            gas_used: receipt.gas_used,
            effective_gas_price: receipt.effective_gas_price,
            block_number: receipt.block_number,
        }))
    }

}

/// Result of a swap execution.
pub enum SwapResult {
    Success(SwapReceipt),
    DryRun,
    /// Quote/preview only — nothing was simulated or sent. Distinct from
    /// `DryRun` so the JSON envelope can communicate "needs confirmation" vs
    /// "dry-run completed".
    Preview,
}

/// Receipt from a successful swap transaction.
#[derive(Debug)]
pub struct SwapReceipt {
    pub tx_hash: String,
    pub gas_used: u64,
    pub effective_gas_price: u128,
    pub block_number: Option<u64>,
}
