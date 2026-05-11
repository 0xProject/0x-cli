use serde::{Deserialize, Serialize};

/// A token amount with raw (base units), formatted (human-readable), and optional USD value.
/// This is the canonical representation used in all CLI output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenAmount {
    /// Amount in base units (wei, lamports, etc.) — always a string for big number safety
    pub raw: String,
    /// Human-readable amount with decimals applied
    pub formatted: String,
    /// USD value estimate (best-effort, null if unavailable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usd_value: Option<String>,
}

impl TokenAmount {
    pub fn new(raw: &str, decimals: u8) -> Self {
        Self {
            raw: raw.to_string(),
            formatted: format_amount(raw, decimals),
            usd_value: None,
        }
    }
}

/// Token information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenInfo {
    pub address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decimals: Option<u8>,
}

/// Allowance issue from the 0x API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowanceIssue {
    /// Current allowance amount
    pub actual: String,
    /// Address to approve (NEVER use the contract address directly)
    pub spender: String,
}

/// Issues reported by the 0x API.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Issues {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowance: Option<AllowanceIssue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance: Option<serde_json::Value>,
    #[serde(rename = "simulationIncomplete", default)]
    pub simulation_incomplete: bool,
}

/// Fee information from the 0x API.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Fees {
    #[serde(rename = "zeroExFee", skip_serializing_if = "Option::is_none")]
    pub zero_ex_fee: Option<FeeDetail>,
    #[serde(rename = "integratorFee", skip_serializing_if = "Option::is_none")]
    pub integrator_fee: Option<FeeDetail>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeeDetail {
    pub amount: String,
    pub token: String,
    #[serde(rename = "type")]
    pub fee_type: Option<String>,
}

/// Route/source information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteSource {
    pub name: String,
    pub proportion: String,
}

/// Compute an indicative `buy/sell` rate string from raw base-unit amounts.
/// Returns `"N/A"` when sell is zero. Uses widening precision for small rates
/// so sub-cent rates remain legible.
pub fn compute_rate(sell_amount: &str, buy_amount: &str) -> String {
    let sell: f64 = sell_amount.parse().unwrap_or(1.0);
    let buy: f64 = buy_amount.parse().unwrap_or(0.0);
    if sell == 0.0 {
        return "N/A".to_string();
    }
    let rate = buy / sell;
    if rate > 1000.0 {
        format!("{rate:.2}")
    } else if rate > 1.0 {
        format!("{rate:.6}")
    } else {
        format!("{rate:.10}")
    }
}

/// Format a raw amount string with decimals.
/// "1000000" with 6 decimals → "1.000000"
pub fn format_amount(raw: &str, decimals: u8) -> String {
    let decimals = decimals as usize;
    if decimals == 0 {
        return raw.to_string();
    }

    // Remove leading zeros but keep at least one digit
    let raw = raw.trim_start_matches('0');
    let raw = if raw.is_empty() { "0" } else { raw };

    if raw.len() <= decimals {
        // Need to pad with leading zeros: "123" with 6 decimals → "0.000123"
        let zeros = decimals - raw.len();
        format!("0.{}{}", "0".repeat(zeros), raw)
    } else {
        let split_at = raw.len() - decimals;
        let integer = &raw[..split_at];
        let fraction = &raw[split_at..];
        format!("{integer}.{fraction}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_amount() {
        assert_eq!(format_amount("1000000", 6), "1.000000");
        assert_eq!(format_amount("100000000", 6), "100.000000");
        assert_eq!(format_amount("123", 6), "0.000123");
        assert_eq!(format_amount("0", 6), "0.000000");
        assert_eq!(format_amount("1000000000000000000", 18), "1.000000000000000000");
        assert_eq!(format_amount("500000000000000000", 18), "0.500000000000000000");
        assert_eq!(format_amount("42", 0), "42");
    }

    #[test]
    fn test_format_amount_edge_cases() {
        // Leading zeros get stripped correctly
        assert_eq!(format_amount("000123", 6), "0.000123");
        assert_eq!(format_amount("0000001", 6), "0.000001");
        // Very small amount with 18 decimals
        assert_eq!(format_amount("100", 18), "0.000000000000000100");
        // Single digit
        assert_eq!(format_amount("1", 6), "0.000001");
        // Large number
        assert_eq!(format_amount("99999999999", 6), "99999.999999");
    }

    #[test]
    fn test_token_amount_new() {
        let amount = TokenAmount::new("1000000", 6);
        assert_eq!(amount.raw, "1000000");
        assert_eq!(amount.formatted, "1.000000");
        assert!(amount.usd_value.is_none());
    }
}
