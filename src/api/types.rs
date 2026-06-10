use crate::error::{CliError, ErrorCode};
use serde::{Deserialize, Serialize};

/// A token amount with raw (base units), formatted (human-readable), and optional USD value.
/// This is the canonical representation used in all CLI output.
///
/// `formatted` is `None` when decimals are unknown — better to omit than to
/// display a wrong value scaled at the wrong precision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenAmount {
    /// Amount in base units (wei, lamports, etc.) — always a string for big number safety
    pub raw: String,
    /// Human-readable amount with decimals applied
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formatted: Option<String>,
    /// USD value estimate (best-effort, null if unavailable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usd_value: Option<String>,
}

impl TokenAmount {
    pub fn new(raw: &str, decimals: u8) -> Self {
        Self {
            raw: raw.to_string(),
            formatted: Some(format_amount(raw, decimals)),
            usd_value: None,
        }
    }

    /// Use when the token's decimals are unknown — emits the raw amount only.
    pub fn unknown_decimals(raw: &str) -> Self {
        Self {
            raw: raw.to_string(),
            formatted: None,
            usd_value: None,
        }
    }

    /// Construct a [`TokenAmount`] honoring whether decimals are known.
    pub fn from_optional_decimals(raw: &str, decimals: Option<u8>) -> Self {
        match decimals {
            Some(d) => Self::new(raw, d),
            None => Self::unknown_decimals(raw),
        }
    }

    /// Render for human output: the formatted amount when known, otherwise the
    /// raw amount tagged so the user can see decimals were missing.
    pub fn display(&self) -> String {
        match &self.formatted {
            Some(f) => f.clone(),
            None => format!("{} (raw, decimals unknown)", self.raw),
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

/// Balance issue from the 0x API: the taker doesn't hold enough of the sell
/// token. Reported inside a 200 quote/price response (`issues.balance`), not
/// as an API error — callers must check it explicitly before executing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceIssue {
    /// Sell token contract address
    pub token: String,
    /// Current taker balance in base units
    pub actual: String,
    /// Balance required for the swap to execute, in base units
    pub expected: String,
}

impl BalanceIssue {
    /// Convert the quote-reported shortfall into a structured
    /// `INSUFFICIENT_BALANCE` error (exit 6). Without this pre-flight
    /// conversion the swap proceeds and dies later as a confusing
    /// `SIMULATION_FAILED`.
    pub fn to_error(&self) -> CliError {
        CliError::Api {
            code: ErrorCode::InsufficientBalance,
            message: format!(
                "Insufficient sell token balance: wallet holds {} but the swap needs {} (token {})",
                self.actual, self.expected, self.token
            ),
            status: None,
            details: serde_json::to_value(self).ok(),
            suggestion: Some(
                "Fund the wallet with more of the sell token or reduce --amount".into(),
            ),
        }
    }
}

/// Issues reported by the 0x API.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Issues {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowance: Option<AllowanceIssue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance: Option<BalanceIssue>,
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

/// Render a base-unit amount when decimals are known, otherwise tag the raw
/// amount so the user knows it isn't human-formatted.
pub fn display_amount(raw: &str, decimals: Option<u8>) -> String {
    match decimals {
        Some(d) => format_amount(raw, d),
        None => format!("{raw} (raw, decimals unknown)"),
    }
}

/// Compute an indicative `buy/sell` rate string from raw base-unit amounts.
/// Returns `"N/A"` when sell is zero or either input isn't a parseable
/// non-negative number. Uses widening precision for small rates so sub-cent
/// rates remain legible.
///
/// Computed in arbitrary-precision decimal via `bigdecimal` so wei-scale or
/// lamport-scale amounts (up to ~80 digits) don't lose the low-order digits
/// the way an `f64` division would.
pub fn compute_rate(sell_amount: &str, buy_amount: &str) -> String {
    use bigdecimal::{BigDecimal, Zero};
    use std::str::FromStr;

    let sell = match BigDecimal::from_str(sell_amount) {
        Ok(n) if n > BigDecimal::zero() => n,
        _ => return "N/A".to_string(),
    };
    let buy = match BigDecimal::from_str(buy_amount) {
        Ok(n) if n >= BigDecimal::zero() => n,
        _ => return "N/A".to_string(),
    };

    // 20 fractional digits is enough to hold any rate the CLI displays
    // (worst case: tiny amount of an 18-decimal token vs. tiny amount of a
    // 6-decimal token → rate ~1e-12 — still plenty of room).
    let rate = (&buy / &sell).with_prec(40);

    // Bucket display precision by magnitude — matches the f64 version's
    // "{:.2}" / "{:.6}" / "{:.10}" tiers so the wire format doesn't shift.
    let thousand = BigDecimal::from(1000);
    let one = BigDecimal::from(1);
    let digits: u8 = if rate > thousand {
        2
    } else if rate > one {
        6
    } else {
        10
    };

    // BigDecimal's Display uses scientific notation for very-small values
    // ("5.0E-9"); reuse `format_amount` to render fixed-point instead so the
    // JSON envelope stays human-readable for sub-unit rates.
    let rounded = rate.with_scale(digits as i64);
    let (mantissa, exponent) = rounded.as_bigint_and_exponent();
    let mantissa_str = mantissa.to_string();
    debug_assert!(exponent >= 0, "with_scale produces non-negative exponent");
    debug_assert!(!mantissa_str.starts_with('-'), "rate is non-negative");
    format_amount(&mantissa_str, exponent as u8)
}

/// Format a raw amount string with decimals.
/// "1000000" with 6 decimals → "1.000000".
/// Falls back to a tagged raw string when the input isn't a plain non-negative
/// decimal integer (empty, non-digit, signed) — better to flag than silently
/// produce a misleading "0.000abc".
pub fn format_amount(raw: &str, decimals: u8) -> String {
    if raw.is_empty() || !raw.chars().all(|c| c.is_ascii_digit()) {
        return format!("{raw} (raw, not a base-unit integer)");
    }

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
        assert_eq!(
            format_amount("1000000000000000000", 18),
            "1.000000000000000000"
        );
        assert_eq!(
            format_amount("500000000000000000", 18),
            "0.500000000000000000"
        );
        assert_eq!(format_amount("42", 0), "42");
    }

    #[test]
    fn test_format_amount_rejects_malformed() {
        assert_eq!(format_amount("", 6), " (raw, not a base-unit integer)");
        assert_eq!(
            format_amount("abc", 6),
            "abc (raw, not a base-unit integer)"
        );
        assert_eq!(format_amount("-1", 6), "-1 (raw, not a base-unit integer)");
        assert_eq!(
            format_amount("1.5", 6),
            "1.5 (raw, not a base-unit integer)"
        );
        // Zero decimals with malformed input is still flagged.
        assert_eq!(
            format_amount("abc", 0),
            "abc (raw, not a base-unit integer)"
        );
    }

    #[test]
    fn test_compute_rate_edge_cases() {
        assert_eq!(compute_rate("0", "100"), "N/A");
        assert_eq!(compute_rate("", "100"), "N/A");
        assert_eq!(compute_rate("abc", "100"), "N/A");
        assert_eq!(compute_rate("100", "abc"), "N/A");
        assert_eq!(compute_rate("-1", "100"), "N/A");
        // sane happy paths still work (rate exactly 1.0 falls into the small-rate format)
        assert_eq!(compute_rate("1000000", "1000000"), "1.0000000000");
    }

    #[test]
    fn test_compute_rate_extreme_magnitudes() {
        // 1 wei sell vs ~10^25 wei buy — the old f64 version emitted a value
        // with ~15 significant digits of precision then formatted as
        // "{:.2}", losing the low-order ~10 digits. The BigDecimal version
        // keeps every integer digit; the post-decimal trailing zeroes come
        // from the `{:.2}` magnitude bucket.
        let rate = compute_rate("1", "9999999999999999999999999");
        assert!(rate.starts_with("9999999999999999999999999"), "got {rate}");

        // Wei-scale buy/sell pairs typical of a swap: 1 ETH sell for 5000 USDC
        // (1e18 wei vs 5e9 base units). Rate ≈ 5e-9, sub-1 bucket.
        let rate = compute_rate("1000000000000000000", "5000000000");
        assert_eq!(rate, "0.0000000050");
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
        assert_eq!(amount.formatted.as_deref(), Some("1.000000"));
        assert!(amount.usd_value.is_none());
    }

    #[test]
    fn test_token_amount_unknown_decimals() {
        let amount = TokenAmount::unknown_decimals("1000000");
        assert_eq!(amount.raw, "1000000");
        assert!(amount.formatted.is_none());
        assert_eq!(amount.display(), "1000000 (raw, decimals unknown)");
    }

    /// The API's quote-level `issues.balance` shape (`{token, actual,
    /// expected}`) must keep deserializing — the pre-flight balance check in
    /// every swap path depends on it.
    #[test]
    fn test_issues_balance_deserializes() {
        let issues: Issues = serde_json::from_str(
            r#"{
                "allowance": null,
                "balance": {
                    "token": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
                    "actual": "0",
                    "expected": "1000000"
                },
                "simulationIncomplete": false
            }"#,
        )
        .expect("quote issues with balance parse");
        let balance = issues.balance.expect("balance issue present");
        assert_eq!(balance.actual, "0");
        assert_eq!(balance.expected, "1000000");
    }

    /// The balance issue converts to a structured INSUFFICIENT_BALANCE error
    /// (exit 6) carrying the shortfall details — not a generic API error.
    #[test]
    fn test_balance_issue_to_error() {
        let issue = BalanceIssue {
            token: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".into(),
            actual: "0".into(),
            expected: "1000000".into(),
        };
        let err = issue.to_error();
        assert_eq!(err.code(), ErrorCode::InsufficientBalance);
        assert_eq!(err.exit_code(), 6);
        assert!(!err.code().retryable());
        let details = err.details().expect("details carry the shortfall");
        assert_eq!(details["actual"], "0");
        assert_eq!(details["expected"], "1000000");
        assert!(err.suggestion().is_some());
    }
}
