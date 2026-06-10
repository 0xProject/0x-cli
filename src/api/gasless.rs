use super::ApiClient;
use crate::error::{CliError, ErrorCode};
use serde::{Deserialize, Serialize};

fn no_liquidity_error() -> CliError {
    CliError::Api {
        code: ErrorCode::NoLiquidity,
        message: "No liquidity available for this gasless pair".into(),
        status: None,
        details: None,
        suggestion: Some("Try a different token pair or amount, or try without --gasless".into()),
    }
}

/// Convert a raw gasless response into the typed struct. When the response is
/// the reduced no-liquidity shape — either `{ liquidityAvailable: false }` (bool
/// or string), or a response missing the trade/amount fields entirely —
/// surface a clean `NoLiquidity` error instead of a parse failure.
fn parse_gasless<T: serde::de::DeserializeOwned>(raw: serde_json::Value) -> Result<T, CliError> {
    let liquidity = raw.get("liquidityAvailable");
    let liquidity_false = matches!(liquidity, Some(serde_json::Value::Bool(false)))
        || matches!(liquidity, Some(serde_json::Value::String(s)) if s.eq_ignore_ascii_case("false"));
    if liquidity_false {
        return Err(no_liquidity_error());
    }
    // A 200 that lacks sellToken/buyToken means the API quietly degraded to
    // the no-liquidity envelope without the explicit flag — also treat as
    // NoLiquidity rather than a confusing parse error.
    let missing_core_fields = raw.get("sellToken").is_none() || raw.get("buyToken").is_none();
    if missing_core_fields {
        return Err(no_liquidity_error());
    }
    serde_json::from_value(raw.clone()).map_err(|e| CliError::Api {
        code: ErrorCode::ApiError,
        message: format!("Failed to parse gasless response: {e}"),
        status: None,
        details: Some(
            serde_json::json!({ "body_preview": super::truncate_for_error(&raw.to_string()) }),
        ),
        suggestion: None,
    })
}

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

        let raw: serde_json::Value = self.get("/gasless/price", &params).await?;
        parse_gasless(raw)
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

        let raw: serde_json::Value = self.get("/gasless/quote", &params).await?;
        parse_gasless(raw)
    }

    /// Submit a gasless swap
    pub async fn submit_gasless(
        &self,
        request: &GaslessSubmitRequest,
    ) -> Result<GaslessSubmitResponse, CliError> {
        self.post("/gasless/submit", request).await
    }

    /// Get gasless trade status. Validates `trade_hash` strictly up front so a
    /// typo can't be silently rewritten into a malformed-but-different
    /// request: anything but hex digits + an optional `0x` prefix is
    /// rejected. (The original implementation stripped non-hex chars, which
    /// could turn `0xabc/../foo` into `0xabcfo` and quietly query the wrong
    /// trade.)
    pub async fn get_gasless_status(
        &self,
        trade_hash: &str,
        chain_id: u64,
    ) -> Result<GaslessStatusResponse, CliError> {
        validate_trade_hash(trade_hash)?;
        let chain_id_str = chain_id.to_string();
        let path = format!("/gasless/status/{trade_hash}");
        self.get(&path, &[("chainId", chain_id_str.as_str())]).await
    }
}

/// Reject a gasless trade hash that isn't a pure hex string (with an
/// optional `0x` prefix). 0x's gasless trade hashes are 32-byte hashes (66
/// chars with the prefix, 64 without), but we accept any all-hex length here
/// because the API has historically used both forms.
fn validate_trade_hash(trade_hash: &str) -> Result<(), CliError> {
    let body = trade_hash
        .strip_prefix("0x")
        .or_else(|| trade_hash.strip_prefix("0X"))
        .unwrap_or(trade_hash);
    if body.is_empty() || !body.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(CliError::Api {
            code: ErrorCode::InputInvalid,
            message: format!(
                "Trade hash '{trade_hash}' is not a valid hex string (expected 0x-prefixed hex)"
            ),
            status: None,
            details: None,
            suggestion: Some(
                "Re-check the value emitted by `0x swap --gasless` (the `trade_hash` field)."
                    .into(),
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trade_hash_validation_accepts_0x_prefixed_hex() {
        assert!(validate_trade_hash(
            "0xabcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
        )
        .is_ok());
        assert!(validate_trade_hash("0xABCDEF").is_ok());
        // Bare hex (no prefix) is also accepted — the 0x API has used both.
        assert!(validate_trade_hash("abcdef").is_ok());
    }

    #[test]
    fn trade_hash_validation_rejects_path_injection() {
        let err = validate_trade_hash("0xabc/../foo").unwrap_err();
        assert_eq!(err.code(), ErrorCode::InputInvalid);
    }

    #[test]
    fn trade_hash_validation_rejects_empty() {
        assert!(validate_trade_hash("").is_err());
        assert!(validate_trade_hash("0x").is_err());
    }

    #[test]
    fn trade_hash_validation_rejects_non_hex() {
        assert!(validate_trade_hash("0xghij").is_err());
        assert!(validate_trade_hash("hello world").is_err());
    }
}
