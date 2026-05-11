use super::ApiClient;
use crate::error::CliError;
use serde::{Deserialize, Serialize};

/// Gasless price response
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GaslessPriceResponse {
    pub sell_token: String,
    pub buy_token: String,
    pub sell_amount: String,
    pub buy_amount: String,
    pub min_buy_amount: String,
    #[serde(default)]
    pub liquidity_available: Option<bool>,
    #[serde(default)]
    pub issues: Option<GaslessIssues>,
}

/// Gasless quote response
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GaslessQuoteResponse {
    pub sell_token: String,
    pub buy_token: String,
    pub sell_amount: String,
    pub buy_amount: String,
    pub min_buy_amount: String,
    pub trade: Option<GaslessSignable>,
    pub approval: Option<GaslessSignable>,
    #[serde(default)]
    pub issues: Option<GaslessIssues>,
    #[serde(default)]
    pub liquidity_available: Option<bool>,
    #[serde(default)]
    pub zid: Option<String>,
    #[serde(default)]
    pub route: Option<serde_json::Value>,
    #[serde(default)]
    pub fees: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GaslessSignable {
    #[serde(rename = "type")]
    pub signable_type: String,
    pub eip712: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GaslessIssues {
    #[serde(default)]
    pub allowance: Option<serde_json::Value>,
}

/// Submit request body
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GaslessSubmitRequest {
    pub chain_id: u64,
    pub trade: GaslessSubmitSignable,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval: Option<GaslessSubmitSignable>,
}

#[derive(Debug, Serialize)]
pub struct GaslessSubmitSignable {
    #[serde(rename = "type")]
    pub signable_type: String,
    pub eip712: serde_json::Value,
    pub signature: SignatureSplit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureSplit {
    pub v: u8,
    pub r: String,
    pub s: String,
    pub signature_type: u8,
}

/// Gasless submit response
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GaslessSubmitResponse {
    pub trade_hash: String,
}

/// Gasless status response
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GaslessStatusResponse {
    pub status: String,
    #[serde(default)]
    pub transactions: Vec<GaslessTransaction>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub zid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GaslessTransaction {
    #[serde(default)]
    pub hash: Option<String>,
    #[serde(default)]
    pub timestamp: Option<u64>,
}

impl GaslessStatusResponse {
    /// Whether this is a terminal (final) state.
    pub fn is_terminal(&self) -> bool {
        matches!(self.status.as_str(), "confirmed" | "failed")
    }

    pub fn is_successful(&self) -> bool {
        self.status == "confirmed"
    }
}

impl ApiClient {
    /// Get gasless price
    pub async fn get_gasless_price(
        &self,
        chain_id: u64,
        sell_token: &str,
        buy_token: &str,
        sell_amount: &str,
        taker: Option<&str>,
    ) -> Result<GaslessPriceResponse, CliError> {
        let chain_id_str = chain_id.to_string();
        let mut params: Vec<(&str, &str)> = vec![
            ("chainId", &chain_id_str),
            ("sellToken", sell_token),
            ("buyToken", buy_token),
            ("sellAmount", sell_amount),
        ];
        if let Some(taker) = taker {
            params.push(("taker", taker));
        }

        self.get("/gasless/price", &params).await
    }

    /// Get gasless quote
    pub async fn get_gasless_quote(
        &self,
        chain_id: u64,
        sell_token: &str,
        buy_token: &str,
        sell_amount: &str,
        taker: &str,
    ) -> Result<GaslessQuoteResponse, CliError> {
        let chain_id_str = chain_id.to_string();
        let params: Vec<(&str, &str)> = vec![
            ("chainId", &chain_id_str),
            ("sellToken", sell_token),
            ("buyToken", buy_token),
            ("sellAmount", sell_amount),
            ("taker", taker),
        ];

        self.get("/gasless/quote", &params).await
    }

    /// Submit a gasless swap
    pub async fn submit_gasless(
        &self,
        request: &GaslessSubmitRequest,
    ) -> Result<GaslessSubmitResponse, CliError> {
        self.post("/gasless/submit", request).await
    }

    /// Get gasless trade status
    pub async fn get_gasless_status(
        &self,
        trade_hash: &str,
        chain_id: u64,
    ) -> Result<GaslessStatusResponse, CliError> {
        let chain_id_str = chain_id.to_string();
        // Validate hash format to prevent path injection
        let sanitized_hash = trade_hash
            .chars()
            .filter(|c| c.is_ascii_hexdigit() || *c == 'x' || *c == 'X')
            .collect::<String>();
        let path = format!("/gasless/status/{sanitized_hash}");
        self.get(&path, &[("chainId", chain_id_str.as_str())])
            .await
    }
}
