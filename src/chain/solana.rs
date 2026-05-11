use crate::api::solana_swap::{ApiInstruction, SolanaSwapResponse};
use crate::error::{CliError, ErrorCode};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::address_lookup_table::AddressLookupTableAccount;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::message::{v0::Message as MessageV0, VersionedMessage};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::keypair::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::VersionedTransaction;
use std::str::FromStr;

/// Convert an API instruction to a native Solana instruction.
fn convert_instruction(api_ix: &ApiInstruction) -> Result<Instruction, CliError> {
    let program_id_bytes: [u8; 32] =
        api_ix.program_id.clone().try_into().map_err(|_| CliError::Api {
            code: ErrorCode::ApiError,
            message: "Invalid program_id length in swap instructions".into(),
            status: None,
            details: None,
            suggestion: None,
        })?;

    let accounts: Result<Vec<AccountMeta>, CliError> = api_ix
        .accounts
        .iter()
        .map(|a| {
            let pubkey_bytes: [u8; 32] = a.pubkey.clone().try_into().map_err(|_| CliError::Api {
                code: ErrorCode::ApiError,
                message: "Invalid pubkey length in swap instructions".into(),
                status: None,
                details: None,
                suggestion: None,
            })?;
            let pubkey = Pubkey::new_from_array(pubkey_bytes);
            Ok(if a.is_writable {
                AccountMeta::new(pubkey, a.is_signer)
            } else {
                AccountMeta::new_readonly(pubkey, a.is_signer)
            })
        })
        .collect();

    Ok(Instruction {
        program_id: Pubkey::new_from_array(program_id_bytes),
        accounts: accounts?,
        data: api_ix.data.clone(),
    })
}

/// Fetch address lookup tables from the Solana RPC.
async fn fetch_lookup_tables(
    rpc: &RpcClient,
    table_keys: &[String],
) -> Result<Vec<AddressLookupTableAccount>, CliError> {
    let mut tables = Vec::new();

    for key_str in table_keys {
        let key = Pubkey::from_str(key_str).map_err(|e| CliError::Api {
            code: ErrorCode::InputInvalid,
            message: format!("Invalid lookup table address '{key_str}': {e}"),
            status: None,
            details: None,
            suggestion: None,
        })?;

        let account = rpc.get_account(&key).await.map_err(|e| CliError::Transaction {
            code: ErrorCode::RpcError,
            message: format!("Failed to fetch lookup table {key_str}: {e}"),
            tx_hash: None,
            suggestion: None,
        })?;

        let table =
            solana_sdk::address_lookup_table::state::AddressLookupTable::deserialize(&account.data)
                .map_err(|e| CliError::Transaction {
                    code: ErrorCode::RpcError,
                    message: format!("Failed to deserialize lookup table {key_str}: {e}"),
                    tx_hash: None,
                    suggestion: None,
                })?;

        tables.push(AddressLookupTableAccount {
            key,
            addresses: table.addresses.to_vec(),
        });
    }

    Ok(tables)
}

/// Build, sign, simulate, and send a Solana swap transaction.
pub async fn execute_solana_swap(
    rpc_url: &str,
    keypair: &Keypair,
    swap_response: &SolanaSwapResponse,
    dry_run: bool,
    on_status: &dyn Fn(&str),
) -> Result<SolanaSwapResult, CliError> {
    let rpc = RpcClient::new_with_commitment(rpc_url.to_string(), CommitmentConfig::confirmed());

    // Step 1: Convert API instructions
    on_status("Building transaction...");
    let instructions: Result<Vec<Instruction>, _> = swap_response
        .instructions
        .iter()
        .map(convert_instruction)
        .collect();
    let instructions = instructions?;

    // Step 2: Fetch address lookup tables
    let lookup_tables = if swap_response.address_lookup_tables.is_empty() {
        Vec::new()
    } else {
        on_status("Fetching address lookup tables...");
        fetch_lookup_tables(&rpc, &swap_response.address_lookup_tables).await?
    };

    // Step 3: Get recent blockhash
    on_status("Fetching blockhash...");
    let blockhash = rpc.get_latest_blockhash().await.map_err(|e| CliError::Transaction {
        code: ErrorCode::RpcError,
        message: format!("Failed to get recent blockhash: {e}"),
        tx_hash: None,
        suggestion: None,
    })?;

    // Step 4: Compile V0 message
    let v0_message = MessageV0::try_compile(
        &keypair.pubkey(),
        &instructions,
        &lookup_tables,
        blockhash,
    )
    .map_err(|e| CliError::Transaction {
        code: ErrorCode::SigningFailed,
        message: format!("Failed to compile transaction message: {e}"),
        tx_hash: None,
        suggestion: None,
    })?;

    // Step 5: Sign
    on_status("Signing transaction...");
    let tx = VersionedTransaction::try_new(VersionedMessage::V0(v0_message), &[keypair])
        .map_err(|e| CliError::Transaction {
            code: ErrorCode::SigningFailed,
            message: format!("Failed to sign transaction: {e}"),
            tx_hash: None,
            suggestion: None,
        })?;

    // Step 6: Simulate
    on_status("Simulating transaction...");
    let sim_result = rpc.simulate_transaction(&tx).await.map_err(|e| CliError::Transaction {
        code: ErrorCode::SimulationFailed,
        message: format!("Transaction simulation failed: {e}"),
        tx_hash: None,
        suggestion: None,
    })?;

    if let Some(err) = sim_result.value.err {
        let logs = sim_result.value.logs.unwrap_or_default();
        return Err(CliError::Transaction {
            code: ErrorCode::SimulationFailed,
            message: format!("Transaction simulation failed: {err:?}"),
            tx_hash: None,
            suggestion: Some(format!(
                "Simulation logs:\n{}",
                logs.iter()
                    .take(10)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n")
            )),
        });
    }

    if dry_run {
        return Ok(SolanaSwapResult::DryRun);
    }

    // Step 7: Send
    on_status("Sending transaction...");
    let signature = rpc
        .send_transaction(&tx)
        .await
        .map_err(|e| CliError::Transaction {
            code: ErrorCode::SigningFailed,
            message: format!("Failed to send transaction: {e}"),
            tx_hash: None,
            suggestion: None,
        })?;

    let sig_str = signature.to_string();
    on_status(&format!("Transaction sent: {sig_str}"));

    // Step 8: Confirm
    on_status("Waiting for confirmation...");
    rpc.confirm_transaction_with_commitment(&signature, CommitmentConfig::confirmed())
        .await
        .map_err(|e| CliError::Transaction {
            code: ErrorCode::TransactionTimeout,
            message: format!("Transaction not confirmed: {e}"),
            tx_hash: Some(sig_str.clone()),
            suggestion: Some("Check the transaction status on Solscan".into()),
        })?;

    Ok(SolanaSwapResult::Success { signature: sig_str })
}

pub enum SolanaSwapResult {
    Success { signature: String },
    DryRun,
}

/// Sign a pre-serialized transaction (for cross-chain Solana origin).
pub fn sign_preserialized_transaction(
    base64_tx: &str,
    keypair: &Keypair,
) -> Result<VersionedTransaction, CliError> {
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, base64_tx)
        .map_err(|e| CliError::Transaction {
            code: ErrorCode::SigningFailed,
            message: format!("Failed to decode base64 transaction: {e}"),
            tx_hash: None,
            suggestion: None,
        })?;

    let mut tx: VersionedTransaction = bincode::deserialize(&bytes).map_err(|e| CliError::Transaction {
        code: ErrorCode::SigningFailed,
        message: format!("Failed to deserialize Solana transaction: {e}"),
        tx_hash: None,
        suggestion: None,
    })?;

    // Sign the message
    let msg_bytes = tx.message.serialize();
    let signature = keypair.sign_message(&msg_bytes);

    // Replace the first signature (fee payer)
    if tx.signatures.is_empty() {
        return Err(CliError::Transaction {
            code: ErrorCode::SigningFailed,
            message: "Pre-built transaction has no signature slots".into(),
            tx_hash: None,
            suggestion: None,
        });
    }
    tx.signatures[0] = signature;

    Ok(tx)
}
