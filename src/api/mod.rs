pub mod cross_chain;
pub mod evm_swap;
pub mod gasless;
pub mod poll;
pub mod solana_swap;
pub mod types;

use crate::error::{CliError, ErrorCode};
use reqwest::header::{HeaderMap, HeaderValue};
use std::time::Duration;

/// Central HTTP client for 0x API calls.
/// Handles headers, authentication, and retry with exponential backoff.
pub struct ApiClient {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl ApiClient {
    pub fn new(api_key: String, timeout_secs: u64) -> Result<Self, CliError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| CliError::Config {
                code: ErrorCode::NetworkError,
                message: format!("Failed to create HTTP client: {e}"),
            })?;

        Ok(Self {
            client,
            api_key,
            base_url: "https://api.0x.org".to_string(),
        })
    }

    /// Build default headers for 0x API v2 calls.
    fn default_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "0x-api-key",
            HeaderValue::from_str(&self.api_key).unwrap_or_else(|_| HeaderValue::from_static("")),
        );
        headers.insert("0x-version", HeaderValue::from_static("v2"));
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        headers
    }

    /// Build headers without the version header (for Solana API).
    fn solana_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "0x-api-key",
            HeaderValue::from_str(&self.api_key).unwrap_or_else(|_| HeaderValue::from_static("")),
        );
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        headers
    }

    /// GET request with retry.
    pub async fn get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<T, CliError> {
        let url = format!("{}{}", self.base_url, path);
        self.request_with_retry(|| {
            self.client
                .get(&url)
                .headers(self.default_headers())
                .query(params)
        })
        .await
    }

    /// POST request with retry.
    pub async fn post<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, CliError> {
        let url = format!("{}{}", self.base_url, path);
        self.request_with_retry(|| {
            self.client
                .post(&url)
                .headers(self.default_headers())
                .json(body)
        })
        .await
    }

    /// POST request without version header (for Solana API).
    pub async fn post_solana<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, CliError> {
        let url = format!("{}{}", self.base_url, path);
        self.request_with_retry(|| {
            self.client
                .post(&url)
                .headers(self.solana_headers())
                .json(body)
        })
        .await
    }

    /// Execute a request with retry logic (3 attempts, exponential backoff on 429/5xx).
    async fn request_with_retry<T, F>(&self, build_request: F) -> Result<T, CliError>
    where
        T: serde::de::DeserializeOwned,
        F: Fn() -> reqwest::RequestBuilder,
    {
        let max_retries = 3;
        let mut last_error = None;

        for attempt in 0..max_retries {
            if attempt > 0 {
                let delay = Duration::from_secs(1 << (attempt - 1)); // 1s, 2s
                tokio::time::sleep(delay).await;
            }

            let response = match build_request().send().await {
                Ok(resp) => resp,
                Err(e) => {
                    if e.is_timeout() {
                        last_error = Some(CliError::Timeout {
                            code: ErrorCode::NetworkTimeout,
                            message: format!("Request timed out: {e}"),
                        });
                        continue;
                    }
                    if e.is_connect() {
                        last_error = Some(CliError::Api {
                            code: ErrorCode::NetworkError,
                            message: format!("Connection failed: {e}"),
                            status: None,
                            details: None,
                            suggestion: Some("Check your network connection".into()),
                        });
                        continue;
                    }
                    return Err(CliError::Api {
                        code: ErrorCode::NetworkError,
                        message: format!("HTTP request failed: {e}"),
                        status: None,
                        details: None,
                        suggestion: None,
                    });
                }
            };

            let status = response.status();

            // Retry on 429 (rate limit) and 5xx (server error)
            if status.as_u16() == 429 {
                last_error = Some(CliError::Api {
                    code: ErrorCode::ApiRateLimited,
                    message: "Rate limited by 0x API".into(),
                    status: Some(429),
                    details: None,
                    suggestion: Some("Wait a moment and try again".into()),
                });
                continue;
            }
            if status.is_server_error() {
                let body = response.text().await.unwrap_or_default();
                last_error = Some(CliError::Api {
                    code: ErrorCode::InternalServerError,
                    message: format!("0x API server error ({status}): {body}"),
                    status: Some(status.as_u16()),
                    details: serde_json::from_str(&body).ok(),
                    suggestion: Some("This is a server-side issue, try again later".into()),
                });
                continue;
            }

            // Client errors (4xx) — don't retry
            if status.is_client_error() {
                let body = response.text().await.unwrap_or_default();
                return Err(self.map_api_error(status.as_u16(), &body));
            }

            // Success — parse response
            let body = response.text().await.map_err(|e| CliError::Api {
                code: ErrorCode::NetworkError,
                message: format!("Failed to read response body: {e}"),
                status: Some(status.as_u16()),
                details: None,
                suggestion: None,
            })?;

            return serde_json::from_str::<T>(&body).map_err(|e| CliError::Api {
                code: ErrorCode::ApiError,
                message: format!("Failed to parse API response: {e}"),
                status: Some(status.as_u16()),
                details: serde_json::from_str(&body).ok(),
                suggestion: None,
            });
        }

        Err(last_error.unwrap_or_else(|| CliError::Api {
            code: ErrorCode::NetworkError,
            message: "Request failed after all retries".into(),
            status: None,
            details: None,
            suggestion: None,
        }))
    }

    /// Map a 4xx API error response to a CliError with the appropriate error code.
    fn map_api_error(&self, status: u16, body: &str) -> CliError {
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

        let (code, message, details) = if let Some(ref err) = parsed {
            let code = match err.name.as_deref() {
                Some("INSUFFICIENT_BALANCE") => ErrorCode::InsufficientBalance,
                Some("INSUFFICIENT_BALANCE_OR_ALLOWANCE") => ErrorCode::InsufficientBalance,
                Some("INSUFFICIENT_ALLOWANCE") => ErrorCode::InsufficientAllowance,
                Some("INPUT_INVALID") => ErrorCode::InputInvalid,
                Some("TOKEN_NOT_SUPPORTED") | Some("SELL_TOKEN_NOT_AUTHORIZED_FOR_TRADE")
                | Some("BUY_TOKEN_NOT_AUTHORIZED_FOR_TRADE") => ErrorCode::TokenNotSupported,
                Some("SELL_AMOUNT_TOO_SMALL") => ErrorCode::SellAmountTooSmall,
                Some("USER_NOT_AUTHORIZED") | Some("TAKER_NOT_AUTHORIZED_FOR_TRADE") => {
                    ErrorCode::InvalidSignature
                }
                _ => {
                    if status == 401 || status == 403 {
                        ErrorCode::ApiKeyMissing
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
        } else {
            (
                ErrorCode::ApiError,
                format!("API error ({status}): {body}"),
                None,
            )
        };

        let suggestion = match code {
            ErrorCode::InsufficientBalance => {
                Some("Fund your wallet with more tokens or reduce the amount".into())
            }
            ErrorCode::ApiKeyMissing => {
                Some("Set your API key with '0x config set api_key <key>'".into())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    fn make_client() -> ApiClient {
        ApiClient {
            client: reqwest::Client::new(),
            api_key: "test".to_string(),
            base_url: "https://api.0x.org".to_string(),
        }
    }

    #[test]
    fn test_map_api_error_insufficient_balance() {
        let client = make_client();
        let body = r#"{"name": "INSUFFICIENT_BALANCE", "message": "Not enough tokens"}"#;
        let err = client.map_api_error(400, body);
        assert_eq!(err.code(), ErrorCode::InsufficientBalance);
    }

    #[test]
    fn test_map_api_error_token_not_supported() {
        let client = make_client();
        let body = r#"{"name": "TOKEN_NOT_SUPPORTED", "message": "Token not found"}"#;
        let err = client.map_api_error(400, body);
        assert_eq!(err.code(), ErrorCode::TokenNotSupported);
    }

    #[test]
    fn test_map_api_error_unknown_name() {
        let client = make_client();
        let body = r#"{"name": "SOME_FUTURE_ERROR", "message": "Something new"}"#;
        let err = client.map_api_error(400, body);
        assert_eq!(err.code(), ErrorCode::ApiError);
    }

    #[test]
    fn test_map_api_error_auth_401() {
        let client = make_client();
        let body = r#"{"message": "Unauthorized"}"#;
        let err = client.map_api_error(401, body);
        assert_eq!(err.code(), ErrorCode::ApiKeyMissing);
    }

    #[test]
    fn test_map_api_error_malformed_json() {
        let client = make_client();
        let body = "not json at all";
        let err = client.map_api_error(400, body);
        assert_eq!(err.code(), ErrorCode::ApiError);
    }

    #[test]
    fn test_map_api_error_sell_amount_too_small() {
        let client = make_client();
        let body = r#"{"name": "SELL_AMOUNT_TOO_SMALL", "message": "Min 1000"}"#;
        let err = client.map_api_error(400, body);
        assert_eq!(err.code(), ErrorCode::SellAmountTooSmall);
    }
}
