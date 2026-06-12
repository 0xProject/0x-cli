pub mod cross_chain;
pub mod evm_swap;
pub mod gasless;
pub mod poll;
pub mod solana_swap;
pub mod types;

use crate::error::{CliError, ErrorCode};
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{
    policies::ExponentialBackoff, RetryTransientMiddleware, Retryable, RetryableStrategy,
};
use std::time::Duration;

const SUPPORT_URL: &str = "https://docs.0x.org/docs/need-help/contact-support";

/// Default 0x API base URL. There is no public flag to override it — a wrong
/// override would be silently catastrophic (sign for the wrong chain, leak
/// the API key, etc.). Config profiles (`profiles.<name>.base_url`) are the
/// only supported override, and `client_for` announces the active profile.
pub const BASE_URL: &str = "https://api.0x.org";

/// Bound the body included with a parse-failure error. Solana responses embed
/// raw instruction byte arrays that otherwise spam the user's terminal.
const PARSE_ERROR_BODY_MAX: usize = 512;

/// Walk a 0x error response `data` object for a nested error code name. The v2
/// API has historically used top-level `name`, but older / nested validation
/// responses surface the actual cause inside `data.details[].code`,
/// `data.code`, or `data.name`. Returning the first hit is fine — these
/// responses don't combine multiple codes per error.
fn extract_nested_name(data: &serde_json::Value) -> Option<String> {
    if let Some(s) = data.get("name").and_then(|v| v.as_str()) {
        return Some(s.to_string());
    }
    if let Some(s) = data.get("code").and_then(|v| v.as_str()) {
        return Some(s.to_string());
    }
    if let Some(arr) = data.get("details").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(s) = item.get("code").and_then(|v| v.as_str()) {
                return Some(s.to_string());
            }
            if let Some(s) = item.get("name").and_then(|v| v.as_str()) {
                return Some(s.to_string());
            }
        }
    }
    None
}

pub fn truncate_for_error(body: &str) -> String {
    if body.len() <= PARSE_ERROR_BODY_MAX {
        body.to_string()
    } else {
        let mut end = PARSE_ERROR_BODY_MAX;
        while !body.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}… [truncated, {} bytes total]", &body[..end], body.len())
    }
}

/// Which 0x API product an HTTP path belongs to. Used to produce
/// endpoint-specific suggestions (e.g. point users at support when their plan
/// doesn't include Solana or cross-chain access).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndpointKind {
    EvmSwap,
    Gasless,
    Solana,
    CrossChain,
    Other,
}

impl EndpointKind {
    fn from_path(path: &str) -> Self {
        if path.starts_with("/solana/") {
            Self::Solana
        } else if path.starts_with("/cross-chain/") {
            Self::CrossChain
        } else if path.starts_with("/gasless/") {
            Self::Gasless
        } else if path.starts_with("/swap/") {
            Self::EvmSwap
        } else {
            Self::Other
        }
    }

    /// Human label used in "you may not have access to ..." messaging. Only
    /// endpoints that are gated by a separate plan/entitlement need one.
    fn entitlement_label(self) -> Option<&'static str> {
        match self {
            Self::Solana => Some("the Solana API"),
            Self::CrossChain => Some("the cross-chain API"),
            _ => None,
        }
    }
}

/// Retry policy for the 0x HTTP client. Matches the team's
/// `Retry429Strategy` (apps-rs/fee-wallets-ops/src/reqwest_utils.rs) but
/// widens the retry set to include 408 (Request Timeout), 425 (Too Early),
/// and 5xx server errors. Cloudflare edge errors (520-526) are deliberately
/// excluded — they indicate a sick origin and immediate retries make things
/// worse.
struct Retry0xStrategy;

impl RetryableStrategy for Retry0xStrategy {
    fn handle(
        &self,
        res: &std::result::Result<reqwest::Response, reqwest_middleware::Error>,
    ) -> Option<Retryable> {
        match res {
            Ok(resp) => {
                let code = resp.status().as_u16();
                if matches!(code, 408 | 425 | 429) {
                    Some(Retryable::Transient)
                } else if (520..=526).contains(&code) {
                    // Don't compound a CF edge failure with retries.
                    Some(Retryable::Fatal)
                } else if resp.status().is_server_error() {
                    Some(Retryable::Transient)
                } else {
                    None
                }
            }
            Err(reqwest_middleware::Error::Reqwest(e)) => {
                if e.is_timeout() || e.is_connect() {
                    Some(Retryable::Transient)
                } else {
                    Some(Retryable::Fatal)
                }
            }
            Err(_) => Some(Retryable::Fatal),
        }
    }
}

/// Translate a `reqwest_middleware::Error` (the only thing `RequestBuilder::send`
/// returns after retries exhaust) into our typed `CliError`.
fn map_send_error(e: &reqwest_middleware::Error) -> CliError {
    let inner = match e {
        reqwest_middleware::Error::Reqwest(e) => e,
        reqwest_middleware::Error::Middleware(_) => {
            return CliError::Api {
                code: ErrorCode::NetworkError,
                message: format!("HTTP middleware error: {e}"),
                status: None,
                details: None,
                suggestion: None,
            };
        }
    };
    if inner.is_timeout() {
        CliError::Timeout {
            code: ErrorCode::NetworkTimeout,
            message: format!("Request timed out after retries: {inner}"),
        }
    } else if inner.is_connect() {
        CliError::Api {
            code: ErrorCode::NetworkError,
            message: format!("Connection failed after retries: {inner}"),
            status: None,
            details: None,
            suggestion: Some("Check your network connection".into()),
        }
    } else {
        CliError::Api {
            code: ErrorCode::NetworkError,
            message: format!("HTTP request failed: {inner}"),
            status: None,
            details: None,
            suggestion: None,
        }
    }
}

/// Central HTTP client for 0x API calls.
///
/// Headers are baked into the underlying `reqwest::Client` so the API key is
/// validated exactly once at construction time and reused for every request.
/// `0x-version: v2` and `content-type: application/json` are sent on every
/// request — the Solana API ignores `0x-version` so it's harmless there.
///
/// Retries are handled by `reqwest-retry`'s `RetryTransientMiddleware`. The
/// strategy retries 408/425/429 and 5xx (excluding Cloudflare 520-526, which
/// indicate a sick origin and shouldn't be hammered) with exponential
/// backoff. 4xx error bodies are mapped to typed `CliError`s by `send`.
pub struct ApiClient {
    client: ClientWithMiddleware,
    base_url: String,
}

impl ApiClient {
    pub fn new(api_key: String, timeout_secs: u64) -> Result<Self, CliError> {
        // Reject keys that won't serialize as an HTTP header value up-front so
        // we never silently send an empty `0x-api-key`, which the API answers
        // with a misleading "missing API key" 401.
        let mut api_key_header = HeaderValue::from_str(&api_key).map_err(|_| CliError::Config {
            code: ErrorCode::ApiKeyMissing,
            message: "API key contains characters that are not valid in an HTTP header (e.g. newline or non-ASCII). Re-set with '0x config set api_key <key>'.".into(),
        })?;
        // Mark the header sensitive so middleware-driven logs/tracing don't
        // emit the raw value.
        api_key_header.set_sensitive(true);

        let mut headers = HeaderMap::new();
        headers.insert("0x-api-key", api_key_header);
        headers.insert("0x-version", HeaderValue::from_static("v2"));
        headers.insert("content-type", HeaderValue::from_static("application/json"));

        let inner = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .default_headers(headers)
            .build()
            .map_err(|e| CliError::Config {
                code: ErrorCode::NetworkError,
                message: format!("Failed to create HTTP client: {e}"),
            })?;

        // Match the prior hand-rolled retry budget: 3 attempts, ~1s/2s backoff.
        let retry_policy = ExponentialBackoff::builder().build_with_max_retries(3);
        let retry_middleware =
            RetryTransientMiddleware::new_with_policy_and_strategy(retry_policy, Retry0xStrategy);
        let client = ClientBuilder::new(inner).with(retry_middleware).build();

        Ok(Self {
            client,
            base_url: BASE_URL.to_string(),
        })
    }

    /// Override the API base URL — config profiles and tests only.
    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
        self
    }

    /// GET request. Retries on 408/425/429/5xx via middleware.
    pub async fn get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<T, CliError> {
        let url = format!("{}{}", self.base_url, path);
        let req = self.client.get(&url).query(params);
        self.send(path, req).await
    }

    /// POST with a JSON body. Retries on 408/425/429/5xx via middleware.
    pub async fn post<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, CliError> {
        let url = format!("{}{}", self.base_url, path);
        let req = self.client.post(&url).json(body);
        self.send(path, req).await
    }

    /// Send a request and decode the response. Retries on transient failures
    /// (408/425/429/5xx, network errors) are handled by the
    /// `RetryTransientMiddleware`; this function only sees the final response
    /// (or the final network error after retries are exhausted).
    #[tracing::instrument(skip(self, req), fields(path = path, status))]
    async fn send<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        req: reqwest_middleware::RequestBuilder,
    ) -> Result<T, CliError> {
        let endpoint = EndpointKind::from_path(path);

        let response = req.send().await.map_err(|e| map_send_error(&e))?;

        let status = response.status();
        let code = status.as_u16();
        tracing::Span::current().record("status", code);

        // 451 (Unavailable For Legal Reasons) — geo-restriction. Map to
        // ApiAccessDenied so the user gets the "contact support" hint and the
        // right exit code. Not retried by the middleware.
        if code == 451 {
            let body = response.text().await.unwrap_or_default();
            return Err(self.map_api_error(endpoint, code, &body));
        }

        // Cloudflare edge errors (520-526): upstream origin issue. Surfaced
        // as a non-retryable server error — the middleware deliberately
        // excludes this range so we don't hammer a sick edge.
        if (520..=526).contains(&code) {
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::Api {
                code: ErrorCode::InternalServerError,
                message: format!("0x API edge error ({code}): {}", truncate_for_error(&body)),
                status: Some(code),
                details: None,
                suggestion: Some(
                    "The 0x API edge is reporting an upstream issue. Wait a few minutes before retrying."
                        .into(),
                ),
            });
        }

        // After retries: 408/425/429 surface as their typed errors, not
        // generic 4xx. Same with other 5xx.
        if code == 408 || code == 425 || code == 429 {
            let (err_code, message, suggestion) = match code {
                408 => (
                    ErrorCode::NetworkTimeout,
                    "0x API responded with 408 (request timeout) after retries".to_string(),
                    Some("Wait a moment and try again".into()),
                ),
                425 => (
                    ErrorCode::ApiRateLimited,
                    "0x API responded with 425 (too early) after retries".to_string(),
                    Some("Wait a moment and try again".into()),
                ),
                _ => (
                    ErrorCode::ApiRateLimited,
                    "Rate limited by 0x API after retries".to_string(),
                    Some("Wait a moment and try again".into()),
                ),
            };
            return Err(CliError::Api {
                code: err_code,
                message,
                status: Some(code),
                details: None,
                suggestion,
            });
        }

        if status.is_server_error() {
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::Api {
                code: ErrorCode::InternalServerError,
                message: format!(
                    "0x API server error ({status}) after retries: {}",
                    truncate_for_error(&body)
                ),
                status: Some(code),
                details: serde_json::from_str(&body).ok(),
                suggestion: Some("This is a server-side issue, try again later".into()),
            });
        }

        if status.is_client_error() {
            let body = response.text().await.unwrap_or_default();
            return Err(self.map_api_error(endpoint, code, &body));
        }

        // Success — parse response body.
        let body = response.text().await.map_err(|e| CliError::Api {
            code: ErrorCode::NetworkError,
            message: format!("Failed to read response body: {e}"),
            status: Some(code),
            details: None,
            suggestion: None,
        })?;

        serde_json::from_str::<T>(&body).map_err(|e| CliError::Api {
            code: ErrorCode::ApiError,
            message: format!("Failed to parse API response: {e}"),
            status: Some(code),
            details: Some(serde_json::json!({ "body_preview": truncate_for_error(&body) })),
            suggestion: None,
        })
    }

    /// Map a 4xx API error response to a CliError with the appropriate error code.
    fn map_api_error(&self, endpoint: EndpointKind, status: u16, body: &str) -> CliError {
        // Try to parse the 0x API error format
        #[derive(serde::Deserialize)]
        struct ApiErrorResponse {
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            message: Option<String>,
            #[serde(default)]
            data: Option<serde_json::Value>,
        }

        let parsed: Option<ApiErrorResponse> = serde_json::from_str(body).ok();

        // Endpoints that are gated by a separate plan (Solana, cross-chain) get
        // a "contact support" path on auth failures — those users typically
        // have a valid key that just doesn't include the product.
        let auth_default = if endpoint.entitlement_label().is_some() {
            ErrorCode::ApiAccessDenied
        } else {
            ErrorCode::ApiKeyMissing
        };

        // 451 (geo-block) is plan/region access, not a bad key.
        let auth_default = if status == 451 {
            ErrorCode::ApiAccessDenied
        } else {
            auth_default
        };

        let (code, message, details) = if let Some(ref err) = parsed {
            let nested_name = err.data.as_ref().and_then(extract_nested_name);
            let primary_name = err.name.as_deref().or(nested_name.as_deref());
            let code = match primary_name {
                Some("INSUFFICIENT_BALANCE") => ErrorCode::InsufficientBalance,
                Some("INSUFFICIENT_BALANCE_OR_ALLOWANCE") => ErrorCode::InsufficientBalance,
                Some("INSUFFICIENT_ASSET_LIQUIDITY") | Some("NO_LIQUIDITY") => {
                    ErrorCode::NoLiquidity
                }
                Some("INSUFFICIENT_ALLOWANCE") => ErrorCode::InsufficientAllowance,
                Some("INPUT_INVALID") | Some("SWAP_VALIDATION_FAILED") => ErrorCode::InputInvalid,
                Some("RECIPIENT_NOT_SUPPORTED") => ErrorCode::TokenNotSupported,
                Some("TOKEN_NOT_SUPPORTED")
                | Some("SELL_TOKEN_NOT_AUTHORIZED_FOR_TRADE")
                | Some("BUY_TOKEN_NOT_AUTHORIZED_FOR_TRADE") => ErrorCode::TokenNotSupported,
                Some("SELL_AMOUNT_TOO_SMALL") => ErrorCode::SellAmountTooSmall,
                Some("USER_NOT_AUTHORIZED") | Some("TAKER_NOT_AUTHORIZED_FOR_TRADE") => {
                    ErrorCode::InvalidSignature
                }
                _ => {
                    if status == 401 || status == 403 || status == 451 {
                        auth_default
                    } else {
                        ErrorCode::ApiError
                    }
                }
            };

            let msg = err
                .message
                .clone()
                .unwrap_or_else(|| format!("API error ({status})"));
            let details = err.data.clone();
            (code, msg, details)
        } else if status == 401 || status == 403 || status == 451 {
            (
                auth_default,
                format!("API error ({status}): {}", truncate_for_error(body)),
                None,
            )
        } else {
            (
                ErrorCode::ApiError,
                format!("API error ({status}): {}", truncate_for_error(body)),
                None,
            )
        };

        let suggestion = match code {
            ErrorCode::InsufficientBalance => {
                Some("Fund your wallet with more tokens or reduce the amount".into())
            }
            ErrorCode::InsufficientAllowance => Some(
                "Run the swap with --approval exact (or unlimited) so the CLI approves the spender automatically".into(),
            ),
            ErrorCode::NoLiquidity => {
                Some("Try a different token pair, a smaller amount, or a different chain".into())
            }
            ErrorCode::ApiKeyMissing => {
                Some("Set your API key with '0x config set api_key <key>'".into())
            }
            ErrorCode::ApiAccessDenied => {
                let product = endpoint.entitlement_label().unwrap_or("this endpoint");
                Some(format!(
                    "If your API key is correct, you might not have access to {product}. Contact 0x support: {SUPPORT_URL}"
                ))
            }
            ErrorCode::TokenNotSupported => {
                Some("Check the token address is correct for this chain".into())
            }
            ErrorCode::SellAmountTooSmall => Some("Increase the sell amount".into()),
            _ => None,
        };

        CliError::Api {
            code,
            message,
            status: Some(status),
            details,
            suggestion,
        }
    }
}

/// Resolve the API environment and build the client for it, announcing a
/// non-default profile on stderr. Commands use this instead of
/// `ApiClient::new` so a staging override is never silently in effect.
pub fn client_for(
    global: &crate::GlobalOpts,
    config: &crate::config::types::AppConfig,
    output: &crate::output::OutputHandler,
) -> Result<ApiClient, CliError> {
    let env = crate::config::resolve_env(global, config)?;
    if let Some(name) = &env.profile {
        output.info(&format!("Profile '{name}' → {}", env.base_url));
    }
    Ok(ApiClient::new(env.api_key, global.timeout)?.with_base_url(env.base_url))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    fn make_client() -> ApiClient {
        // map_api_error only touches `base_url` and the request HTTP, neither
        // of which is exercised by the tests below — so a minimal client built
        // through the real constructor is enough.
        ApiClient::new("test".to_string(), 30).expect("api client")
    }

    #[test]
    fn test_map_api_error_insufficient_balance() {
        let client = make_client();
        let body = r#"{"name": "INSUFFICIENT_BALANCE", "message": "Not enough tokens"}"#;
        let err = client.map_api_error(EndpointKind::EvmSwap, 400, body);
        assert_eq!(err.code(), ErrorCode::InsufficientBalance);
    }

    #[test]
    fn test_map_api_error_token_not_supported() {
        let client = make_client();
        let body = r#"{"name": "TOKEN_NOT_SUPPORTED", "message": "Token not found"}"#;
        let err = client.map_api_error(EndpointKind::EvmSwap, 400, body);
        assert_eq!(err.code(), ErrorCode::TokenNotSupported);
    }

    #[test]
    fn test_map_api_error_unknown_name() {
        let client = make_client();
        let body = r#"{"name": "SOME_FUTURE_ERROR", "message": "Something new"}"#;
        let err = client.map_api_error(EndpointKind::EvmSwap, 400, body);
        assert_eq!(err.code(), ErrorCode::ApiError);
    }

    #[test]
    fn test_map_api_error_auth_401_on_evm_swap_is_api_key_missing() {
        let client = make_client();
        let body = r#"{"message": "Unauthorized"}"#;
        let err = client.map_api_error(EndpointKind::EvmSwap, 401, body);
        assert_eq!(err.code(), ErrorCode::ApiKeyMissing);
    }

    #[test]
    fn test_map_api_error_auth_403_on_solana_is_access_denied() {
        let client = make_client();
        let body = r#"{"message": "Forbidden"}"#;
        let err = client.map_api_error(EndpointKind::Solana, 403, body);
        assert_eq!(err.code(), ErrorCode::ApiAccessDenied);
        let suggestion = match &err {
            CliError::Api { suggestion, .. } => suggestion.as_deref().unwrap_or(""),
            _ => panic!("expected Api error"),
        };
        assert!(suggestion.contains("Solana"));
        assert!(suggestion.contains(SUPPORT_URL));
    }

    #[test]
    fn test_map_api_error_auth_403_on_cross_chain_is_access_denied() {
        let client = make_client();
        let body = "<some non-json forbidden body>";
        let err = client.map_api_error(EndpointKind::CrossChain, 403, body);
        assert_eq!(err.code(), ErrorCode::ApiAccessDenied);
        let suggestion = match &err {
            CliError::Api { suggestion, .. } => suggestion.as_deref().unwrap_or(""),
            _ => panic!("expected Api error"),
        };
        assert!(suggestion.contains("cross-chain"));
        assert!(suggestion.contains(SUPPORT_URL));
    }

    #[test]
    fn test_map_api_error_malformed_json() {
        let client = make_client();
        let body = "not json at all";
        let err = client.map_api_error(EndpointKind::EvmSwap, 400, body);
        assert_eq!(err.code(), ErrorCode::ApiError);
    }

    #[test]
    fn test_map_api_error_sell_amount_too_small() {
        let client = make_client();
        let body = r#"{"name": "SELL_AMOUNT_TOO_SMALL", "message": "Min 1000"}"#;
        let err = client.map_api_error(EndpointKind::EvmSwap, 400, body);
        assert_eq!(err.code(), ErrorCode::SellAmountTooSmall);
    }

    #[test]
    fn test_endpoint_kind_from_path() {
        assert_eq!(
            EndpointKind::from_path("/solana/swap-instructions"),
            EndpointKind::Solana
        );
        assert_eq!(
            EndpointKind::from_path("/cross-chain/quotes"),
            EndpointKind::CrossChain
        );
        assert_eq!(
            EndpointKind::from_path("/gasless/quote"),
            EndpointKind::Gasless
        );
        assert_eq!(
            EndpointKind::from_path("/swap/allowance-holder/quote"),
            EndpointKind::EvmSwap
        );
        assert_eq!(
            EndpointKind::from_path("/something-else"),
            EndpointKind::Other
        );
    }

    #[test]
    fn test_with_base_url_overrides_default() {
        let client = ApiClient::new("test".to_string(), 30)
            .expect("api client")
            .with_base_url("https://staging.example.com".to_string());
        assert_eq!(client.base_url, "https://staging.example.com");

        let client = ApiClient::new("test".to_string(), 30).expect("api client");
        assert_eq!(client.base_url, BASE_URL);
    }
}
