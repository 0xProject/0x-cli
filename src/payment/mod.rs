//! Agent-payment support: pay per request through the 0x agent gateway
//! (`https://agent.api.0x.org`) instead of using a `0x-api-key`.
//!
//! Two open standards are supported (Phase 1):
//! - **x402** (`--pay x402-evm`): an EIP-3009 `transferWithAuthorization`
//!   signature sent in the `PAYMENT-SIGNATURE` header. Handled by the
//!   `x402-reqwest` crate via [`x402`].
//! - **MPP** (`--pay mpp`): the Machine Payments Protocol over Tempo
//!   (chainId 4217), challenge→credential→receipt with a "push" broadcast.
//!   Handled by the `mpp` crate via [`mpp`].
//!
//! Both gateway endpoints proxy the same Swap API v2 AllowanceHolder
//! price/quote, so the response bodies deserialize into the existing
//! [`crate::api::evm_swap`] types — only the transport (no API key, a 402
//! payment handshake) differs from [`crate::api::ApiClient`].
//!
//! ## Safety
//! `--max-payment` is enforced **client-side before any signature or
//! broadcast** (x402: in the payment selector; MPP: in an unpaid pre-flight
//! that reads the challenge amount). Failures map to the dedicated
//! `PAYMENT_*` error codes so an agent can tell "refused, nothing spent"
//! (exit 41) from "money spent, no result" (exit 43).

pub mod mpp;
pub mod x402;

use crate::error::{CliError, ErrorCode};
use alloy::primitives::U256;
use alloy::signers::local::PrivateKeySigner as EvmSigner;
use serde::Serialize;

/// Agent-gateway base URL. Hardcoded with no env/config override on purpose:
/// payments are signed against whatever host this resolves to, so a
/// redirectable base URL would be a payment-redirect risk. A staging hook, if
/// ever needed, should be a deliberate, reviewed addition.
const AGENT_BASE_URL: &str = "https://agent.api.0x.org";

/// USDC / USDC.e are 6-decimal tokens; `--max-payment` is given in USD and
/// converted to base units against this.
const USDC_DECIMALS: u32 = 6;

/// Which agent-payment rail to use. The variant set is intentionally open to a
/// future `X402Solana` without reshaping callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaymentMethod {
    /// x402 over an EVM payment network (Base), EIP-3009 exact scheme.
    X402Evm,
    /// MPP over Tempo mainnet (chainId 4217), push settlement.
    MppTempo,
}

impl PaymentMethod {
    /// Stable label for output / receipts.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::X402Evm => "x402-evm",
            Self::MppTempo => "mpp",
        }
    }

    /// Gateway path prefix for this method's AllowanceHolder endpoints.
    fn gateway_prefix(self) -> &'static str {
        match self {
            Self::X402Evm => "/v1/x402",
            Self::MppTempo => "/v1/mpp-tempo",
        }
    }

    /// Full gateway URL for `price` or `quote` under this method.
    fn endpoint_url(self, quote: bool) -> String {
        let leaf = if quote {
            "swap-allowance-holder-quote"
        } else {
            "swap-allowance-holder-price"
        };
        format!("{AGENT_BASE_URL}{}/{leaf}/", self.gateway_prefix())
    }
}

/// A protocol-agnostic view of the settlement the gateway returns
/// (`PAYMENT-RESPONSE` for x402, `Payment-Receipt` / event for MPP). Every
/// field is best-effort: agents get whatever the rail exposed, and at minimum
/// the payer address (which the CLI always knows — it signed).
#[derive(Debug, Clone, Serialize)]
pub struct PaymentReceipt {
    /// Payment method label (`x402-evm` / `mpp`).
    pub method: &'static str,
    /// CAIP-2-style network the payment settled on, when known
    /// (e.g. `eip155:8453`, `tempo:4217`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    /// On-chain payment transaction hash, when the rail reports one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_hash: Option<String>,
    /// The paying wallet address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payer: Option<String>,
    /// Amount paid, in the asset's base units (USDC/USDC.e have 6 decimals).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount_base_units: Option<String>,
}

/// Convert a `--max-payment` USD value (e.g. `0.05`) into USDC base units using
/// exact decimal arithmetic (no float). Rejects non-positive / unparseable /
/// over-precise input fail-closed — an unusable cap must never silently become
/// "unlimited".
pub fn max_payment_to_base_units(usd: &str) -> Result<U256, CliError> {
    let invalid = || CliError::Config {
        code: ErrorCode::InputInvalid,
        message: format!("--max-payment '{usd}' must be a positive USD amount with at most 6 decimals (e.g. 0.05)"),
    };

    let trimmed = usd.trim();
    if trimmed.is_empty() || trimmed.starts_with('-') || trimmed.starts_with('+') {
        return Err(invalid());
    }
    let (int_part, frac_part) = trimmed.split_once('.').unwrap_or((trimmed, ""));
    if !int_part.chars().all(|c| c.is_ascii_digit())
        || !frac_part.chars().all(|c| c.is_ascii_digit())
    {
        return Err(invalid());
    }
    // More than 6 fractional digits is sub-base-unit precision — refuse rather
    // than silently truncate it away.
    if frac_part.len() > USDC_DECIMALS as usize {
        return Err(invalid());
    }
    // Right-pad the fraction to exactly 6 digits, concatenate with the integer
    // part, and parse the whole thing as base units.
    let frac_padded = format!("{frac_part:0<width$}", width = USDC_DECIMALS as usize);
    let combined = format!("{int_part}{frac_padded}");
    let value = U256::from_str_radix(&combined, 10).map_err(|_| invalid())?;
    if value.is_zero() {
        return Err(CliError::Config {
            code: ErrorCode::InputInvalid,
            message: "--max-payment must be greater than 0".into(),
        });
    }
    Ok(value)
}

/// Reconstruct an alloy-2 `PrivateKeySigner` (what the payment crates expect)
/// from the CLI's alloy-1 signer. The two alloy majors coexist in the tree;
/// the raw 32-byte key is the version-agnostic bridge. Going through the byte
/// slice avoids relying on `B256` type identity across the two `alloy`
/// facades.
pub(crate) fn to_payment_signer(
    signer: &EvmSigner,
) -> Result<alloy_signer_local::PrivateKeySigner, CliError> {
    let bytes = signer.to_bytes();
    alloy_signer_local::PrivateKeySigner::from_slice(bytes.as_slice()).map_err(|e| {
        CliError::Transaction {
            code: ErrorCode::PaymentSigningFailed,
            message: format!("Failed to load payment signer: {e}"),
            tx_hash: None,
            suggestion: None,
        }
    })
}

/// Pay for and fetch one gateway price/quote response, deserialized into `T`.
///
/// `query` is the same parameter list the keyed API uses (chainId, sellToken,
/// …). `signer` is the EVM wallet that pays. `max_payment` is the
/// already-parsed cap in USDC base units. `tempo_rpc` is only consulted for
/// [`PaymentMethod::MppTempo`].
///
/// Returns the deserialized body plus a [`PaymentReceipt`]. The payment is a
/// single, non-idempotent pay-and-submit — there is no automatic retry that
/// could double-sign or double-broadcast.
pub async fn fetch<T: serde::de::DeserializeOwned>(
    method: PaymentMethod,
    signer: &EvmSigner,
    quote: bool,
    query: &[(&str, &str)],
    max_payment: U256,
    timeout_secs: u64,
    tempo_rpc: Option<&str>,
) -> Result<(T, PaymentReceipt), CliError> {
    let url = method.endpoint_url(quote);
    match method {
        PaymentMethod::X402Evm => {
            x402::fetch(signer, &url, query, max_payment, timeout_secs).await
        }
        PaymentMethod::MppTempo => {
            mpp::fetch(signer, &url, query, max_payment, timeout_secs, tempo_rpc).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_payment_parses_usd_to_base_units() {
        assert_eq!(max_payment_to_base_units("0.05").unwrap(), U256::from(50_000u64));
        assert_eq!(max_payment_to_base_units("1").unwrap(), U256::from(1_000_000u64));
        assert_eq!(
            max_payment_to_base_units("0.000001").unwrap(),
            U256::from(1u64)
        );
    }

    #[test]
    fn max_payment_rejects_bad_input_fail_closed() {
        assert!(max_payment_to_base_units("0").is_err());
        assert!(max_payment_to_base_units("-1").is_err());
        assert!(max_payment_to_base_units("abc").is_err());
        assert!(max_payment_to_base_units("").is_err());
        // Below one base unit must not round to zero / unlimited.
        assert!(max_payment_to_base_units("0.0000001").is_err());
    }

    #[test]
    fn endpoint_urls_match_gateway_layout() {
        assert_eq!(
            PaymentMethod::X402Evm.endpoint_url(false),
            "https://agent.api.0x.org/v1/x402/swap-allowance-holder-price/"
        );
        assert_eq!(
            PaymentMethod::MppTempo.endpoint_url(true),
            "https://agent.api.0x.org/v1/mpp-tempo/swap-allowance-holder-quote/"
        );
    }

    #[test]
    fn method_labels_are_stable() {
        assert_eq!(PaymentMethod::X402Evm.as_str(), "x402-evm");
        assert_eq!(PaymentMethod::MppTempo.as_str(), "mpp");
    }
}
