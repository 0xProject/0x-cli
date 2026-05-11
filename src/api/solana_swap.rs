use super::ApiClient;
use crate::error::CliError;
use serde::{Deserialize, Serialize};

/// Request body for POST /solana/swap-instructions
#[derive(Debug, Serialize)]
pub struct SolanaSwapRequest {
    pub token_in: String,
    pub token_out: String,
    pub amount_in: u64,
    pub slippage_bps: u32,
    pub taker: String,
}

/// Response from /solana/swap-instructions
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SolanaSwapResponse {
    pub instructions: Vec<ApiInstruction>,
    pub amount_out: u64,
    #[serde(default)]
    pub address_lookup_tables: Vec<String>,
    #[serde(default)]
    pub zid: Option<String>,
}

/// An instruction returned by the Solana swap API.
/// program_id and pubkeys are byte arrays (32 bytes each).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiInstruction {
    pub program_id: Vec<u8>,
    pub accounts: Vec<ApiAccountMeta>,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiAccountMeta {
    pub pubkey: Vec<u8>,
    pub is_signer: bool,
    pub is_writable: bool,
}

impl ApiClient {
    /// Get Solana swap instructions
    pub async fn get_solana_swap(
        &self,
        request: &SolanaSwapRequest,
    ) -> Result<SolanaSwapResponse, CliError> {
        self.post_solana("/solana/swap-instructions", request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_solana_request() {
        let req = SolanaSwapRequest {
            token_in: "So11111111111111111111111111111111111111112".to_string(),
            token_out: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            amount_in: 1_000_000,
            slippage_bps: 100,
            taker: "HN7cABqLq46Es1jh92dQQisAi5UuGZb7t4HPfrheB7fL".to_string(),
        };

        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["token_in"], "So11111111111111111111111111111111111111112");
        assert_eq!(json["amount_in"], 1_000_000);
        assert_eq!(json["slippage_bps"], 100);
    }
}
