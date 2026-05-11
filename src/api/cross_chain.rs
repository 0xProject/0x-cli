use super::ApiClient;
use crate::error::CliError;
use serde::{Deserialize, Serialize};

/// Response from GET /cross-chain/quotes
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrossChainQuotesResponse {
    #[serde(default)]
    pub liquidity_available: bool,
    pub quotes: Vec<CrossChainQuote>,
    #[serde(default)]
    pub zid: Option<String>,
    #[serde(default)]
    pub origin_chain: Option<String>,
    #[serde(default)]
    pub destination_chain: Option<String>,
    #[serde(default)]
    pub allowance_target: Option<String>,
    #[serde(default)]
    pub sell_token: Option<serde_json::Value>,
    #[serde(default)]
    pub buy_token: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrossChainQuote {
    pub sell_amount: String,
    pub buy_amount: String,
    pub min_buy_amount: String,
    pub steps: Vec<CrossChainStep>,
    pub transaction: CrossChainTransaction,
    pub gas_costs: Option<serde_json::Value>,
    pub issues: Option<CrossChainIssues>,
    pub estimated_time_seconds: Option<u64>,
    pub quote_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrossChainStep {
    #[serde(rename = "type")]
    pub step_type: String,
    pub chain_id: Option<serde_json::Value>, // Can be number or string
    pub sell_token: Option<String>,
    pub buy_token: Option<String>,
    pub sell_amount: Option<String>,
    pub buy_amount: Option<String>,
    pub provider: Option<String>,
    pub estimated_time_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrossChainTransaction {
    pub chain_type: String, // "evm" or "svm"
    pub details: CrossChainTxDetails,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrossChainTxDetails {
    // EVM fields
    pub to: Option<String>,
    pub data: Option<String>,
    pub gas: Option<String>,
    pub gas_price: Option<String>,
    pub value: Option<String>,

    // SVM fields
    pub serialized_transaction: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrossChainIssues {
    pub allowance: Option<CrossChainAllowance>,
    pub balance: Option<serde_json::Value>,
    #[serde(default)]
    pub simulation_incomplete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossChainAllowance {
    pub actual: String,
    pub spender: String,
}

/// Response from GET /cross-chain/status
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrossChainStatusResponse {
    pub status: String,
    #[serde(default)]
    pub transactions: Vec<CrossChainStatusTx>,
    #[serde(default)]
    pub failure_reason: Option<String>,
    #[serde(default)]
    pub bridge: Option<String>,
    #[serde(default)]
    pub steps: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    pub zid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrossChainStatusTx {
    pub chain_id: Option<serde_json::Value>,
    pub tx_hash: Option<String>,
    pub timestamp: Option<u64>,
}

impl CrossChainStatusResponse {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            "bridge_filled" | "bridge_failed" | "origin_tx_reverted"
        )
    }

    pub fn is_successful(&self) -> bool {
        self.status == "bridge_filled"
    }
}

impl CrossChainQuote {
    /// Get the bridge provider name from the steps.
    pub fn bridge_provider(&self) -> String {
        self.steps
            .iter()
            .find(|s| s.step_type == "bridge")
            .and_then(|s| s.provider.clone())
            .unwrap_or_else(|| "unknown".to_string())
    }

    pub fn estimated_time_display(&self) -> String {
        match self.estimated_time_seconds {
            Some(s) if s < 60 => format!("~{s}s"),
            Some(s) if s < 3600 => format!("~{} min", s / 60),
            Some(s) => format!("~{} hr", s / 3600),
            None => "unknown".to_string(),
        }
    }
}

impl ApiClient {
    /// Get cross-chain quotes
    #[allow(clippy::too_many_arguments)]
    pub async fn get_cross_chain_quotes(
        &self,
        origin_chain: &str,
        destination_chain: &str,
        sell_token: &str,
        buy_token: &str,
        sell_amount: &str,
        origin_address: &str,
        slippage_bps: Option<u32>,
        sort_by: Option<&str>,
        max_quotes: Option<u8>,
    ) -> Result<CrossChainQuotesResponse, CliError> {
        let slippage_str = slippage_bps.unwrap_or(100).to_string();
        let max_quotes_str = max_quotes.unwrap_or(3).to_string();
        let sort = sort_by.unwrap_or("price");

        let params: Vec<(&str, &str)> = vec![
            ("originChain", origin_chain),
            ("destinationChain", destination_chain),
            ("sellToken", sell_token),
            ("buyToken", buy_token),
            ("sellAmount", sell_amount),
            ("originAddress", origin_address),
            ("slippageBps", &slippage_str),
            ("sortQuotesBy", sort),
            ("maxNumQuotes", &max_quotes_str),
        ];

        self.get("/cross-chain/quotes", &params).await
    }

    /// Get cross-chain status
    pub async fn get_cross_chain_status(
        &self,
        origin_chain: &str,
        origin_tx_hash: &str,
    ) -> Result<CrossChainStatusResponse, CliError> {
        self.get(
            "/cross-chain/status",
            &[
                ("originChain", origin_chain),
                ("originTxHash", origin_tx_hash),
            ],
        )
        .await
    }
}
