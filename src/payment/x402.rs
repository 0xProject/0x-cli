//! x402-EVM adapter over the `x402-reqwest` crate.
//!
//! Wraps a `reqwest` client with the x402 middleware: on a `402` it parses the
//! `PAYMENT-REQUIRED` challenge, signs an EIP-3009 `transferWithAuthorization`
//! (V2 EIP-155 "exact" scheme), and retries with the `PAYMENT-SIGNATURE`
//! header. The `--max-payment` cap is enforced inside the payment selector,
//! **before any signature**.

use super::{to_payment_signer, PaymentReceipt};
use crate::error::{CliError, ErrorCode};
use alloy::primitives::U256;
use alloy::signers::local::PrivateKeySigner as EvmSigner;
use base64::{engine::general_purpose::STANDARD, Engine};
// The payment path speaks reqwest 0.13 (what x402-reqwest/mpp are built on),
// aliased so it doesn't collide with the keyed API client's reqwest 0.12.
use reqwest_payments as reqwest;
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use x402_chain_eip155::V2Eip155ExactClient;
use x402_reqwest::{ReqwestWithPayments, ReqwestWithPaymentsBuild, X402Client};
use x402_types::proto::PaymentRequired;
use x402_types::scheme::client::{
    PaymentCandidate, PaymentCandidateSigner, PaymentSelector, X402Error, X402SchemeClient,
};
use x402_types::scheme::X402SchemeId;

/// USDC on Base — the only asset the `--max-payment` cap (6-decimal base units)
/// is denominated against for x402. The gateway always offers Base USDC (or
/// Solana, which this Phase-1 EVM client doesn't register). Binding the asset
/// stops the 6-decimal cap from being misapplied to a token with different
/// decimals/value.
const BASE_USDC: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
/// Base mainnet, CAIP-2 `eip155:8453` — the x402 payment network. Bound
/// alongside the asset so a candidate carrying the USDC *address* on a
/// different chain can't slip through the cap (belt-and-suspenders; the signed
/// EIP-712 domain also carries chainId).
const BASE_CHAIN_NAMESPACE: &str = "eip155";
const BASE_CHAIN_REFERENCE: &str = "8453";

/// What the selector saw, captured so the caller can produce a precise error
/// after the middleware finishes: distinguish "no payable scheme offered"
/// (exit 40) from "cheapest option exceeded the cap" (exit 41) — in both cases
/// nothing was signed or spent.
#[derive(Debug, Default)]
struct SelectionOutcome {
    /// The selector ran at least once (i.e. a 402 was parsed into candidates).
    invoked: bool,
    /// Number of candidates the registered scheme produced.
    candidate_count: usize,
    /// Cheapest candidate amount seen, in asset base units.
    min_amount: Option<U256>,
}

/// Picks the cheapest candidate within the cap, recording what it saw. Returns
/// `None` (which the middleware turns into an error) when nothing is payable
/// within the cap — we then classify *why* from the captured outcome.
struct CappedSelector {
    cap: U256,
    /// Asset the cap is denominated in; candidates in any other asset are
    /// ineligible (fail-closed) so the 6-decimal cap can't be misapplied.
    expected_asset: String,
    outcome: Arc<Mutex<SelectionOutcome>>,
}

impl PaymentSelector for CappedSelector {
    fn select<'a>(&self, candidates: &'a [PaymentCandidate]) -> Option<&'a PaymentCandidate> {
        // Only consider candidates in the expected asset. `candidate_count` /
        // `min_amount` reflect these eligible ones so the over-cap vs.
        // no-scheme classification stays meaningful.
        let eligible: Vec<&PaymentCandidate> = candidates
            .iter()
            .filter(|c| {
                c.asset.eq_ignore_ascii_case(&self.expected_asset)
                    && c.chain_id.namespace() == BASE_CHAIN_NAMESPACE
                    && c.chain_id.reference() == BASE_CHAIN_REFERENCE
            })
            .collect();
        {
            let mut o = self.outcome.lock().unwrap_or_else(|e| e.into_inner());
            o.invoked = true;
            o.candidate_count = eligible.len();
            o.min_amount = eligible.iter().map(|c| c.amount).min();
        }
        eligible
            .into_iter()
            .filter(|c| c.amount <= self.cap)
            .min_by(|a, b| a.amount.cmp(&b.amount))
    }
}

/// Re-cases `payload.authorization.to` in a signed x402 payload to its EIP-55
/// checksummed form. The `x402-chain-eip155` client serializes the recipient as
/// a lowercase address, but the 0x agent gateway's facilitator compares the
/// recipient string **case-sensitively** against the checksummed `payTo` it
/// advertised — so a lowercase recipient is rejected with "Payment
/// authorization recipient does not match project requirements" before any
/// settlement. Re-casing the JSON string does **not** invalidate the EIP-712
/// signature (it authorizes the 20 address bytes, not their text), so this is a
/// safe, transport-level normalization.
fn checksum_recipient(payload_b64: &str) -> Result<String, X402Error> {
    let bytes = STANDARD
        .decode(payload_b64.trim())
        .map_err(|e| X402Error::SigningError(format!("payload base64 decode: {e}")))?;
    let mut value: serde_json::Value = serde_json::from_slice(&bytes)?;

    if let Some(to) = value
        .get_mut("payload")
        .and_then(|p| p.get_mut("authorization"))
        .and_then(|a| a.get_mut("to"))
    {
        if let Some(addr) = to.as_str() {
            let checksummed = addr
                .parse::<alloy::primitives::Address>()
                .map_err(|e| X402Error::SigningError(format!("recipient parse: {e}")))?
                .to_checksum(None);
            *to = serde_json::Value::String(checksummed);
        }
    }

    let reencoded = serde_json::to_vec(&value)?;
    Ok(STANDARD.encode(reencoded))
}

/// Wraps a candidate's signer so the produced payload has a checksummed
/// recipient. See [`checksum_recipient`].
struct ChecksumFixingSigner {
    inner: Box<dyn PaymentCandidateSigner + Send + Sync>,
}

#[async_trait::async_trait]
impl PaymentCandidateSigner for ChecksumFixingSigner {
    async fn sign_payment(&self) -> Result<String, X402Error> {
        let signed = self.inner.sign_payment().await?;
        checksum_recipient(&signed)
    }
}

/// Wraps a scheme client (e.g. [`V2Eip155ExactClient`]) so every candidate it
/// produces signs through [`ChecksumFixingSigner`]. Identity (namespace /
/// scheme / version) is delegated unchanged so the middleware still routes 402
/// challenges to it.
struct ChecksumFixingScheme<C> {
    inner: C,
}

impl<C: X402SchemeId> X402SchemeId for ChecksumFixingScheme<C> {
    fn x402_version(&self) -> u8 {
        self.inner.x402_version()
    }
    fn namespace(&self) -> &str {
        self.inner.namespace()
    }
    fn scheme(&self) -> &str {
        self.inner.scheme()
    }
}

impl<C: X402SchemeClient> X402SchemeClient for ChecksumFixingScheme<C> {
    fn accept(&self, payment_required: &PaymentRequired) -> Vec<PaymentCandidate> {
        self.inner
            .accept(payment_required)
            .into_iter()
            .map(|c| PaymentCandidate {
                signer: Box::new(ChecksumFixingSigner { inner: c.signer }),
                chain_id: c.chain_id,
                asset: c.asset,
                amount: c.amount,
                scheme: c.scheme,
                x402_version: c.x402_version,
                pay_to: c.pay_to,
            })
            .collect()
    }
}

/// Settlement data from the `PAYMENT-RESPONSE` header (base64 JSON).
#[derive(Debug, Default, Deserialize)]
struct X402Settlement {
    #[serde(default)]
    transaction: Option<String>,
    #[serde(default)]
    network: Option<String>,
}

pub(super) async fn fetch<T: serde::de::DeserializeOwned>(
    signer: &EvmSigner,
    url: &str,
    query: &[(&str, &str)],
    max_payment: U256,
    timeout_secs: u64,
) -> Result<(T, PaymentReceipt), CliError> {
    let payer_addr = format!("{:?}", signer.address());
    let payment_signer = to_payment_signer(signer)?;

    let outcome = Arc::new(Mutex::new(SelectionOutcome::default()));
    let selector = CappedSelector {
        cap: max_payment,
        expected_asset: BASE_USDC.to_string(),
        outcome: Arc::clone(&outcome),
    };
    let x402_client = X402Client::new()
        .with_selector(selector)
        .register(ChecksumFixingScheme {
            inner: V2Eip155ExactClient::new(payment_signer),
        });

    // A fresh client with ONLY the x402 middleware — deliberately no
    // reqwest-retry layer, so a transient failure never silently re-signs or
    // re-submits a paid request. Payment broadcasts can be slow; floor the
    // timeout generously.
    let inner = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs.max(60)))
        .build()
        .map_err(|e| CliError::Config {
            code: ErrorCode::NetworkError,
            message: format!("Failed to build x402 HTTP client: {e}"),
        })?;
    let http = inner.with_payments(x402_client).build();

    // The reqwest-middleware RequestBuilder doesn't proxy `.query()`, so fold
    // the params into the URL up front.
    let full_url = reqwest::Url::parse_with_params(url, query.iter().copied()).map_err(|e| {
        CliError::Config {
            code: ErrorCode::InputInvalid,
            message: format!("Failed to build gateway URL: {e}"),
        }
    })?;
    let result = http.get(full_url).send().await;

    let snapshot = {
        let o = outcome.lock().unwrap_or_else(|e| e.into_inner());
        (o.invoked, o.candidate_count, o.min_amount)
    };

    let response = match result {
        Ok(r) => r,
        Err(e) => return Err(classify_send_error(&e.to_string(), snapshot, max_payment)),
    };

    let status = response.status();
    if status.as_u16() == 402 {
        let (invoked, count, min_amount) = snapshot;
        let payment_submitted =
            invoked && count > 0 && min_amount.map(|m| m <= max_payment).unwrap_or(false);
        if payment_submitted {
            // A candidate within the cap was selected, signed, and submitted,
            // yet the gateway still returned 402. This is a settlement-side
            // rejection — surface the gateway's real error instead of
            // pretending no payable scheme was offered. Money may or may not
            // have moved.
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::Transaction {
                code: ErrorCode::PaymentSettlementFailed,
                message: format!(
                    "Agent gateway rejected the signed payment (402): {}",
                    crate::api::truncate_for_error(&body)
                ),
                tx_hash: None,
                suggestion: Some(
                    "Check your wallet before retrying — a payment may have been submitted.".into(),
                ),
            });
        }
        // Selector found nothing payable within the cap — nothing was signed.
        return Err(classify_unpaid(snapshot, max_payment));
    }
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(CliError::Transaction {
            code: ErrorCode::PaymentSettlementFailed,
            message: format!(
                "Agent gateway returned {} after payment: {}",
                status.as_u16(),
                crate::api::truncate_for_error(&body)
            ),
            tx_hash: None,
            suggestion: Some(
                "The payment may have settled without a usable response — check your wallet before retrying.".into(),
            ),
        });
    }

    let receipt = decode_receipt(response.headers(), &payer_addr, max_payment);

    let body = response.text().await.map_err(|e| CliError::Api {
        code: ErrorCode::PaymentSettlementFailed,
        message: format!("Failed to read gateway response body: {e}"),
        status: Some(status.as_u16()),
        details: None,
        suggestion: None,
    })?;
    let value: T = serde_json::from_str(&body).map_err(|e| CliError::Api {
        code: ErrorCode::ApiError,
        message: format!("Failed to parse gateway response: {e}"),
        status: Some(status.as_u16()),
        details: Some(serde_json::json!({
            "body_preview": crate::api::truncate_for_error(&body)
        })),
        suggestion: None,
    })?;

    Ok((value, receipt))
}

/// Build the receipt from the `PAYMENT-RESPONSE` header. The payer is always
/// known (we signed); the rest is best-effort.
fn decode_receipt(
    headers: &reqwest::header::HeaderMap,
    payer_addr: &str,
    amount: U256,
) -> PaymentReceipt {
    let settlement = headers
        .get("payment-response")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| STANDARD.decode(h).ok())
        .and_then(|b| serde_json::from_slice::<X402Settlement>(&b).ok())
        .unwrap_or_default();

    PaymentReceipt {
        method: super::PaymentMethod::X402Evm.as_str(),
        network: settlement.network,
        tx_hash: settlement.transaction,
        // The payer is authoritatively the wallet we signed with — don't trust
        // a gateway-reported payer over what we know locally.
        payer: Some(payer_addr.to_string()),
        amount_base_units: Some(amount.to_string()),
    }
}

/// Map a `402` that survived the middleware (selector returned `None`) to a
/// precise, safe error. Nothing was signed in either branch.
fn classify_unpaid(
    snapshot: (bool, usize, Option<U256>),
    cap: U256,
) -> CliError {
    let (invoked, count, min_amount) = snapshot;
    if invoked && count > 0 {
        if let Some(min) = min_amount {
            if min > cap {
                return CliError::Config {
                    code: ErrorCode::PaymentExceedsLimit,
                    message: format!(
                        "Cheapest x402 payment is {min} base units but --max-payment cap is {cap} — refused before signing. Nothing was spent."
                    ),
                };
            }
        }
    }
    CliError::Api {
        code: ErrorCode::PaymentChallengeInvalid,
        message: "The agent gateway offered no x402 payment scheme this CLI can satisfy on EVM"
            .into(),
        status: Some(402),
        details: None,
        suggestion: Some("Ensure --pay x402-evm targets a gateway endpoint that accepts EVM (Base) USDC payment.".into()),
    }
}

/// Map a middleware send error. Uses the captured selection outcome (rather
/// than fragile error-string matching) to keep the over-cap case precise.
fn classify_send_error(
    message: &str,
    snapshot: (bool, usize, Option<U256>),
    cap: U256,
) -> CliError {
    let (invoked, count, min_amount) = snapshot;
    if invoked {
        // The 402 was parsed, so this is a payment-decision outcome.
        if count == 0 {
            return classify_unpaid(snapshot, cap);
        }
        if let Some(min) = min_amount {
            if min > cap {
                return classify_unpaid(snapshot, cap);
            }
        }
        // A candidate within cap was selected → failure is in signing or the
        // paid resubmission. Money may or may not have moved.
        return CliError::Transaction {
            code: ErrorCode::PaymentSettlementFailed,
            message: format!("x402 payment failed after selecting a payable option: {message}"),
            tx_hash: None,
            suggestion: Some(
                "Check your wallet before retrying — a payment may have been submitted.".into(),
            ),
        };
    }
    // Selector never ran: either the 402 couldn't be parsed into payment
    // requirements, or it was a transport error before any 402. Distinguish so
    // an agent doesn't retry a malformed-challenge as if it were a network blip.
    let lower = message.to_lowercase();
    if lower.contains("parse") || lower.contains("402") {
        return CliError::Api {
            code: ErrorCode::PaymentChallengeInvalid,
            message: format!("x402 gateway returned a 402 this CLI couldn't parse: {message}"),
            status: Some(402),
            details: None,
            suggestion: None,
        };
    }
    CliError::Api {
        code: ErrorCode::NetworkError,
        message: format!("x402 request to the agent gateway failed: {message}"),
        status: None,
        details: None,
        suggestion: Some("Check your network connection and try again.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(invoked: bool, count: usize, min: Option<u64>) -> (bool, usize, Option<U256>) {
        (invoked, count, min.map(U256::from))
    }

    #[test]
    fn checksum_recipient_recases_to_eip55_keeping_other_fields() {
        // Lowercase `to`, like the x402-chain-eip155 client emits today.
        let payload = serde_json::json!({
            "x402Version": 2,
            "payload": {
                "signature": "0xabc",
                "authorization": {
                    "from": "0x1563915e194d8cfba1943570603f7606a3115508",
                    "to": "0xb15a55e85fdf5edc41b6c1eaf7813e2c6e6def59",
                    "value": "10000"
                }
            }
        });
        let b64 = STANDARD.encode(serde_json::to_vec(&payload).unwrap());

        let fixed_b64 = checksum_recipient(&b64).unwrap();
        let fixed: serde_json::Value =
            serde_json::from_slice(&STANDARD.decode(&fixed_b64).unwrap()).unwrap();

        // Recipient is now EIP-55 checksummed (matches the advertised payTo).
        assert_eq!(
            fixed["payload"]["authorization"]["to"],
            "0xb15a55e85FdF5edc41B6c1eaf7813e2c6e6def59"
        );
        // Signature and other fields are untouched.
        assert_eq!(fixed["payload"]["signature"], "0xabc");
        assert_eq!(fixed["x402Version"], 2);
        assert_eq!(
            fixed["payload"]["authorization"]["from"],
            "0x1563915e194d8cfba1943570603f7606a3115508"
        );
    }

    #[test]
    fn checksum_recipient_tolerates_missing_authorization() {
        // Must not panic / error if the shape is unexpected — pass through.
        let payload = serde_json::json!({"x402Version": 2, "payload": {}});
        let b64 = STANDARD.encode(serde_json::to_vec(&payload).unwrap());
        assert!(checksum_recipient(&b64).is_ok());
    }

    #[test]
    fn over_cap_maps_to_exceeds_limit_nothing_spent() {
        // Cheapest option 100000 base units, cap 50000 → refuse.
        let err = classify_unpaid(snap(true, 1, Some(100_000)), U256::from(50_000u64));
        assert_eq!(err.code(), ErrorCode::PaymentExceedsLimit);
    }

    #[test]
    fn no_candidates_maps_to_challenge_invalid() {
        let err = classify_unpaid(snap(true, 0, None), U256::from(50_000u64));
        assert_eq!(err.code(), ErrorCode::PaymentChallengeInvalid);
    }

    #[test]
    fn transport_error_before_402_is_network() {
        let err = classify_send_error("connection reset", snap(false, 0, None), U256::from(50_000u64));
        assert_eq!(err.code(), ErrorCode::NetworkError);
    }

    #[test]
    fn send_error_after_selection_is_settlement() {
        let err = classify_send_error("signing boom", snap(true, 1, Some(10_000)), U256::from(50_000u64));
        assert_eq!(err.code(), ErrorCode::PaymentSettlementFailed);
    }

    #[test]
    fn send_error_over_cap_is_exceeds_limit() {
        // The real path: selector returns None → middleware Err → classify via
        // the captured snapshot. Over-cap must map to "nothing spent".
        let err = classify_send_error(
            "No matching payment option found",
            snap(true, 1, Some(100_000)),
            U256::from(50_000u64),
        );
        assert_eq!(err.code(), ErrorCode::PaymentExceedsLimit);
    }

    #[test]
    fn unparseable_402_is_challenge_invalid_not_network() {
        let err = classify_send_error(
            "Failed to parse 402 response: bad json",
            snap(false, 0, None),
            U256::from(50_000u64),
        );
        assert_eq!(err.code(), ErrorCode::PaymentChallengeInvalid);
    }
}
