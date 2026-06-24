//! MPP-Tempo adapter over the `mpp` crate.
//!
//! Uses the per-request `PaymentExt::send_with_payment` API on a plain reqwest
//! client (no middleware, no auto-retry of a paid request). The `--max-payment`
//! cap — plus the expected currency (USDC.e) and chain (Tempo mainnet) — is
//! enforced **in-band**, inside a [`CappedTempoProvider`] wrapper whose `pay()`
//! is the single chokepoint the real (paid) challenge passes through *before*
//! any on-chain broadcast. This avoids the time-of-check/time-of-use gap a
//! separate unpaid pre-flight would have (the probe's challenge and the paid
//! challenge can differ).

use super::{to_payment_signer, PaymentReceipt};
use crate::error::{CliError, ErrorCode};
use alloy::primitives::{Address, U256};
use alloy::signers::local::PrivateKeySigner as EvmSigner;
use base64::{engine::general_purpose::STANDARD, Engine};
use mpp::client::tempo::charge::TempoCharge;
use mpp::client::{Fetch, PaymentProvider, TempoProvider};
use mpp::protocol::core::{PaymentChallenge, PaymentCredential};
use mpp::MppError;
// Payment path speaks reqwest 0.13 (what mpp is built on), aliased to coexist
// with the keyed API client's reqwest 0.12.
use reqwest_payments as reqwest;
use serde::Deserialize;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Tempo mainnet (chainId 4217) default RPC, matching the `mpp` crate's own
/// `DEFAULT_RPC_URL`. Overridable via `--tempo-rpc` or `[rpc].tempo` config.
const DEFAULT_TEMPO_RPC: &str = "https://rpc.tempo.xyz";
const TEMPO_CHAIN_ID: u64 = 4217;
/// USDC.e on Tempo mainnet — the only currency the `--max-payment` cap (in
/// 6-decimal base units) is denominated against. A challenge in any other
/// currency is refused fail-closed so the cap can't be silently misapplied to
/// a token with different decimals/value.
const TEMPO_USDC_E: &str = "0x20C000000000000000000000b9537d11c60E8b50";

/// Why the capped provider refused to pay (recorded before returning an error
/// so the caller can emit a precise, safe exit code — none of these spent
/// anything).
#[derive(Debug, Clone)]
enum Refusal {
    ExceedsCap { amount: U256, cap: U256 },
    WrongChain(u64),
    WrongCurrency(String),
    ChallengeUnparseable(String),
}

#[derive(Default)]
struct CapOutcome {
    /// The amount/chain actually authorized for payment (set only when the
    /// cap+currency+chain checks passed and we delegated to the inner pay).
    paid: Option<(U256, u64)>,
    /// Set instead when we refused — nothing was spent.
    refusal: Option<Refusal>,
}

/// Wraps [`TempoProvider`] and enforces the spend cap + expected currency/chain
/// on the genuine challenge inside `pay()`, before the inner provider signs or
/// broadcasts anything.
#[derive(Clone)]
struct CappedTempoProvider {
    inner: TempoProvider,
    cap: U256,
    expected_currency: Address,
    outcome: Arc<Mutex<CapOutcome>>,
}

impl PaymentProvider for CappedTempoProvider {
    fn supports(&self, method: &str, intent: &str) -> bool {
        self.inner.supports(method, intent)
    }

    fn accept_payment_header(&self) -> Option<String> {
        self.inner.accept_payment_header()
    }

    async fn pay(&self, challenge: &PaymentChallenge) -> Result<PaymentCredential, MppError> {
        let charge = match TempoCharge::from_challenge(challenge) {
            Ok(c) => c,
            Err(e) => {
                self.record(Refusal::ChallengeUnparseable(e.to_string()));
                return Err(e);
            }
        };
        let amount = charge.amount();
        let chain_id = charge.chain_id();
        let currency = charge.currency();

        if chain_id != TEMPO_CHAIN_ID {
            self.record(Refusal::WrongChain(chain_id));
            return Err(MppError::InvalidConfig(format!(
                "MPP challenge chainId {chain_id} is not Tempo mainnet ({TEMPO_CHAIN_ID}) — refusing"
            )));
        }
        if currency != self.expected_currency {
            self.record(Refusal::WrongCurrency(currency.to_string()));
            return Err(MppError::InvalidConfig(format!(
                "MPP challenge currency {currency} is not the expected USDC.e — refusing"
            )));
        }
        if amount > self.cap {
            self.record(Refusal::ExceedsCap { amount, cap: self.cap });
            return Err(MppError::InvalidConfig(format!(
                "MPP payment {amount} base units exceeds --max-payment cap {} — refusing before broadcast",
                self.cap
            )));
        }

        // Within cap and bound to USDC.e on Tempo — authorize the broadcast.
        self.outcome.lock().unwrap_or_else(|e| e.into_inner()).paid = Some((amount, chain_id));
        self.inner.pay(challenge).await
    }
}

impl CappedTempoProvider {
    fn record(&self, r: Refusal) {
        self.outcome.lock().unwrap_or_else(|e| e.into_inner()).refusal = Some(r);
    }
}

pub(super) async fn fetch<T: serde::de::DeserializeOwned>(
    signer: &EvmSigner,
    url: &str,
    query: &[(&str, &str)],
    max_payment: U256,
    timeout_secs: u64,
    tempo_rpc: Option<&str>,
) -> Result<(T, PaymentReceipt), CliError> {
    let payer_addr = format!("{:?}", signer.address());
    // Tempo RPC resolution (flag → `[rpc].tempo` config) is done by the caller;
    // fall back to the canonical mainnet endpoint here.
    let rpc = tempo_rpc.unwrap_or(DEFAULT_TEMPO_RPC).to_string();

    let expected_currency =
        Address::from_str(TEMPO_USDC_E).expect("TEMPO_USDC_E is a valid address literal");

    let inner_provider = TempoProvider::new(to_payment_signer(signer)?, &rpc).map_err(|e| {
        CliError::Config {
            code: ErrorCode::ConfigInvalid,
            message: format!("Failed to initialize Tempo provider (rpc={rpc}): {e}"),
        }
    })?;
    let outcome = Arc::new(Mutex::new(CapOutcome::default()));
    let provider = CappedTempoProvider {
        inner: inner_provider,
        cap: max_payment,
        expected_currency,
        outcome: Arc::clone(&outcome),
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs.max(120)))
        .build()
        .map_err(|e| CliError::Config {
            code: ErrorCode::NetworkError,
            message: format!("Failed to build MPP HTTP client: {e}"),
        })?;

    // The on-chain push broadcast happens inside send_with_payment against the
    // Tempo RPC, which the reqwest timeout doesn't bound — so wrap the whole
    // flow in an explicit deadline.
    let broadcast_deadline = Duration::from_secs(timeout_secs.max(120).saturating_add(60));
    let send = client.get(url).query(query).send_with_payment(&provider);
    let response = match tokio::time::timeout(broadcast_deadline, send).await {
        Ok(Ok(resp)) => resp,
        Ok(Err(http_err)) => {
            // Prefer the in-band refusal reason (nothing spent) over the
            // crate's generic payment error.
            if let Some(err) = refusal_to_error(&outcome) {
                return Err(err);
            }
            return Err(map_mpp_http_error(http_err));
        }
        Err(_) => {
            return Err(CliError::Transaction {
                code: ErrorCode::PaymentSettlementFailed,
                message: format!(
                    "MPP payment timed out after {}s (Tempo broadcast may have started)",
                    broadcast_deadline.as_secs()
                ),
                tx_hash: None,
                suggestion: Some(
                    "Check your wallet on Tempo before retrying — a transfer may have broadcast."
                        .into(),
                ),
            });
        }
    };

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(CliError::Transaction {
            code: ErrorCode::PaymentSettlementFailed,
            message: format!(
                "MPP gateway returned {} after payment: {}",
                status.as_u16(),
                crate::api::truncate_for_error(&body)
            ),
            tx_hash: None,
            suggestion: Some(
                "The Tempo payment may have broadcast without a usable response — check your wallet before retrying.".into(),
            ),
        });
    }

    let tx_hash = response
        .headers()
        .get("payment-receipt")
        .and_then(|h| h.to_str().ok())
        .and_then(extract_receipt_tx);

    let (amount, chain_id) = outcome
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .paid
        .map(|(a, c)| (Some(a.to_string()), c))
        .unwrap_or((None, TEMPO_CHAIN_ID));

    let body = response.text().await.map_err(|e| CliError::Api {
        code: ErrorCode::PaymentSettlementFailed,
        message: format!("Failed to read gateway response body: {e}"),
        status: Some(status.as_u16()),
        details: None,
        suggestion: None,
    })?;
    let value: T = serde_json::from_str(&body).map_err(|e| parse_err(&body, e))?;

    let receipt = PaymentReceipt {
        method: super::PaymentMethod::MppTempo.as_str(),
        network: Some(format!("tempo:{chain_id}")),
        tx_hash,
        // The payer is authoritatively the wallet we signed with.
        payer: Some(payer_addr),
        amount_base_units: amount,
    };
    Ok((value, receipt))
}

/// Turn a recorded in-band refusal into a precise CliError. All refusals happen
/// *before* `inner.pay`, so nothing was spent.
fn refusal_to_error(outcome: &Arc<Mutex<CapOutcome>>) -> Option<CliError> {
    // Recover through a poisoned lock rather than dropping the refusal — losing
    // it would downgrade a safe "nothing spent" (exit 41) into a misleading
    // "money may have moved" (exit 43). That's the dangerous direction.
    let refusal = outcome
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .refusal
        .clone()?;
    Some(match refusal {
        Refusal::ExceedsCap { amount, cap } => CliError::Config {
            code: ErrorCode::PaymentExceedsLimit,
            message: format!(
                "MPP payment of {amount} base units exceeds --max-payment cap {cap} — refused before broadcasting. Nothing was spent."
            ),
        },
        Refusal::WrongChain(cid) => CliError::Api {
            code: ErrorCode::PaymentChallengeInvalid,
            message: format!("MPP challenge was for chainId {cid}, not Tempo mainnet ({TEMPO_CHAIN_ID}) — refused. Nothing was spent."),
            status: Some(402),
            details: None,
            suggestion: None,
        },
        Refusal::WrongCurrency(c) => CliError::Api {
            code: ErrorCode::PaymentChallengeInvalid,
            message: format!("MPP challenge currency {c} is not the expected USDC.e — refused. Nothing was spent."),
            status: Some(402),
            details: None,
            suggestion: None,
        },
        Refusal::ChallengeUnparseable(e) => CliError::Api {
            code: ErrorCode::PaymentChallengeInvalid,
            message: format!("MPP challenge could not be parsed: {e}"),
            status: Some(402),
            details: None,
            suggestion: None,
        },
    })
}

/// Best-effort tx-hash extraction from a `Payment-Receipt` header. Prefer the
/// reliable JSON / base64-JSON forms; fall back to scanning for a 0x… hash.
fn extract_receipt_tx(header: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Receipt {
        #[serde(alias = "txHash", alias = "transaction", alias = "tx_hash")]
        tx: Option<String>,
    }
    if let Ok(r) = serde_json::from_str::<Receipt>(header) {
        if r.tx.is_some() {
            return r.tx;
        }
    }
    if let Ok(bytes) = STANDARD.decode(header) {
        if let Ok(r) = serde_json::from_slice::<Receipt>(&bytes) {
            if r.tx.is_some() {
                return r.tx;
            }
        }
    }
    if let Some(pos) = header.find("0x") {
        let candidate = &header[pos..];
        let hex_len = candidate
            .chars()
            .skip(2)
            .take_while(|c| c.is_ascii_hexdigit())
            .count();
        if hex_len == 64 {
            return Some(candidate[..66].to_string());
        }
    }
    None
}

fn map_mpp_http_error(e: mpp::client::HttpError) -> CliError {
    use mpp::client::HttpError;
    match e {
        HttpError::MissingChallenge
        | HttpError::NoSupportedChallenge(_)
        | HttpError::InvalidChallenge(_) => CliError::Api {
            code: ErrorCode::PaymentChallengeInvalid,
            message: format!("MPP challenge error: {e}"),
            status: Some(402),
            details: None,
            suggestion: None,
        },
        HttpError::InvalidCredential(_) | HttpError::CloneFailed => CliError::Transaction {
            code: ErrorCode::PaymentSigningFailed,
            message: format!("MPP credential error: {e}"),
            tx_hash: None,
            suggestion: None,
        },
        HttpError::Payment(_) => CliError::Transaction {
            code: ErrorCode::PaymentSettlementFailed,
            message: format!("MPP payment failed: {e}"),
            tx_hash: None,
            suggestion: Some(
                "If Tempo broadcast started, money may have moved — check your wallet. Ensure the wallet holds USDC.e and native gas on Tempo.".into(),
            ),
        },
        HttpError::Request(_) => CliError::Api {
            code: ErrorCode::NetworkError,
            message: format!("MPP request failed: {e}"),
            status: None,
            details: None,
            suggestion: Some("Check your network connection and the Tempo RPC URL.".into()),
        },
    }
}

fn parse_err(body: &str, e: serde_json::Error) -> CliError {
    CliError::Api {
        code: ErrorCode::ApiError,
        message: format!("Failed to parse gateway response: {e}"),
        status: None,
        details: Some(serde_json::json!({
            "body_preview": crate::api::truncate_for_error(body)
        })),
        suggestion: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tempo_usdc_e_is_a_valid_address() {
        assert!(Address::from_str(TEMPO_USDC_E).is_ok());
    }

    #[test]
    fn cap_comparison_is_base_units() {
        // $0.01 challenge (10000) is within a $0.05 cap (50000).
        let amount = U256::from_str("10000").unwrap();
        let cap = U256::from(50_000u64);
        assert!(amount <= cap);
    }

    #[test]
    fn refusal_over_cap_maps_to_exceeds_limit() {
        let outcome = Arc::new(Mutex::new(CapOutcome {
            paid: None,
            refusal: Some(Refusal::ExceedsCap {
                amount: U256::from(100_000u64),
                cap: U256::from(50_000u64),
            }),
        }));
        assert_eq!(
            refusal_to_error(&outcome).unwrap().code(),
            ErrorCode::PaymentExceedsLimit
        );
    }

    #[test]
    fn refusal_wrong_currency_maps_to_challenge_invalid() {
        let outcome = Arc::new(Mutex::new(CapOutcome {
            paid: None,
            refusal: Some(Refusal::WrongCurrency("0xdead".into())),
        }));
        assert_eq!(
            refusal_to_error(&outcome).unwrap().code(),
            ErrorCode::PaymentChallengeInvalid
        );
    }

    #[test]
    fn no_refusal_is_none() {
        let outcome = Arc::new(Mutex::new(CapOutcome::default()));
        assert!(refusal_to_error(&outcome).is_none());
    }

    #[test]
    fn extract_receipt_tx_prefers_json_then_hex() {
        let json = r#"{"txHash":"0xdeadbeef"}"#;
        assert_eq!(extract_receipt_tx(json).as_deref(), Some("0xdeadbeef"));
        let h = "0x".to_string() + &"a".repeat(64);
        assert_eq!(extract_receipt_tx(&h).as_deref(), Some(h.as_str()));
        assert!(extract_receipt_tx("no hash here").is_none());
    }
}
