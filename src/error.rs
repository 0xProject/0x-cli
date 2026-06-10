use serde::Serialize;
use std::fmt;

/// Stable error codes that agents can match on.
/// Each code maps to a category and retryable flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    // Config errors
    ConfigNotFound,
    ConfigInvalid,
    ApiKeyMissing,
    WalletNotFound,
    WalletInvalid,
    // In test builds the keyring is stubbed out, so this variant is never
    // constructed — it's only built from the real keyring error path.
    #[cfg_attr(test, allow(dead_code))]
    KeyringUnavailable,

    // Input errors
    InputInvalid,
    ChainNotSupported,

    // Validation errors (from 0x API)
    InsufficientBalance,
    InsufficientAllowance,
    NoLiquidity,
    TokenNotSupported,
    SellAmountTooSmall,

    // Network errors
    NetworkError,
    NetworkTimeout,
    RpcError,
    ApiRateLimited,

    // API errors
    InternalServerError,
    ApiError,
    /// 401/403 where the API key is present but the user's plan doesn't
    /// include access to the endpoint they hit (e.g. Solana, cross-chain).
    ApiAccessDenied,

    // Signing errors
    SigningFailed,
    InvalidSignature,

    // Execution errors
    SimulationFailed,
    TransactionReverted,
    TransactionTimeout,
    #[allow(dead_code)]
    QuoteExpired,

    // Bridge errors
    #[allow(dead_code)]
    BridgeFailed,
    BridgeTimeout,

    // User errors
    UserCancelled,
}

impl ErrorCode {
    pub fn category(&self) -> &'static str {
        match self {
            Self::ConfigNotFound | Self::ConfigInvalid | Self::ApiKeyMissing => "config",
            Self::WalletNotFound | Self::WalletInvalid | Self::KeyringUnavailable => "config",
            Self::InputInvalid | Self::ChainNotSupported => "input",
            Self::InsufficientBalance
            | Self::InsufficientAllowance
            | Self::NoLiquidity
            | Self::TokenNotSupported
            | Self::SellAmountTooSmall => "validation",
            Self::NetworkError | Self::NetworkTimeout | Self::RpcError | Self::ApiRateLimited => {
                "network"
            }
            Self::InternalServerError | Self::ApiError | Self::ApiAccessDenied => "api",
            Self::SigningFailed | Self::InvalidSignature => "signing",
            Self::SimulationFailed
            | Self::TransactionReverted
            | Self::TransactionTimeout
            | Self::QuoteExpired => "execution",
            Self::BridgeFailed | Self::BridgeTimeout => "bridge",
            Self::UserCancelled => "input",
        }
    }

    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::NetworkError
                | Self::NetworkTimeout
                | Self::RpcError
                | Self::ApiRateLimited
                | Self::InternalServerError
                | Self::TransactionTimeout
                | Self::QuoteExpired
                | Self::BridgeTimeout
        )
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Self::ConfigNotFound
            | Self::ConfigInvalid
            | Self::WalletNotFound
            | Self::WalletInvalid
            | Self::KeyringUnavailable => 3,
            Self::ApiKeyMissing => 5,
            Self::InputInvalid | Self::ChainNotSupported => 2,
            Self::InsufficientBalance
            | Self::InsufficientAllowance
            | Self::NoLiquidity
            | Self::TokenNotSupported
            | Self::SellAmountTooSmall => 6,
            Self::NetworkError | Self::NetworkTimeout | Self::RpcError | Self::ApiRateLimited => 4,
            Self::InternalServerError | Self::ApiError => 4,
            Self::ApiAccessDenied => 5,
            Self::SigningFailed | Self::InvalidSignature => 1,
            Self::SimulationFailed => 10,
            Self::TransactionReverted => 11,
            Self::TransactionTimeout => 12,
            Self::QuoteExpired => 1,
            Self::BridgeFailed => 1,
            Self::BridgeTimeout => 12,
            Self::UserCancelled => 20,
        }
    }
}

impl ErrorCode {
    /// Stable wire name used in the JSON envelope and the `Display` impl.
    /// Must match the `#[serde(rename_all = "SCREAMING_SNAKE_CASE")]` output —
    /// `cargo test test_error_code_names` enforces that.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ConfigNotFound => "CONFIG_NOT_FOUND",
            Self::ConfigInvalid => "CONFIG_INVALID",
            Self::ApiKeyMissing => "API_KEY_MISSING",
            Self::WalletNotFound => "WALLET_NOT_FOUND",
            Self::WalletInvalid => "WALLET_INVALID",
            Self::KeyringUnavailable => "KEYRING_UNAVAILABLE",
            Self::InputInvalid => "INPUT_INVALID",
            Self::ChainNotSupported => "CHAIN_NOT_SUPPORTED",
            Self::InsufficientBalance => "INSUFFICIENT_BALANCE",
            Self::InsufficientAllowance => "INSUFFICIENT_ALLOWANCE",
            Self::NoLiquidity => "NO_LIQUIDITY",
            Self::TokenNotSupported => "TOKEN_NOT_SUPPORTED",
            Self::SellAmountTooSmall => "SELL_AMOUNT_TOO_SMALL",
            Self::NetworkError => "NETWORK_ERROR",
            Self::NetworkTimeout => "NETWORK_TIMEOUT",
            Self::RpcError => "RPC_ERROR",
            Self::ApiRateLimited => "API_RATE_LIMITED",
            Self::InternalServerError => "INTERNAL_SERVER_ERROR",
            Self::ApiError => "API_ERROR",
            Self::ApiAccessDenied => "API_ACCESS_DENIED",
            Self::SigningFailed => "SIGNING_FAILED",
            Self::InvalidSignature => "INVALID_SIGNATURE",
            Self::SimulationFailed => "SIMULATION_FAILED",
            Self::TransactionReverted => "TRANSACTION_REVERTED",
            Self::TransactionTimeout => "TRANSACTION_TIMEOUT",
            Self::QuoteExpired => "QUOTE_EXPIRED",
            Self::BridgeFailed => "BRIDGE_FAILED",
            Self::BridgeTimeout => "BRIDGE_TIMEOUT",
            Self::UserCancelled => "USER_CANCELLED",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The main CLI error type.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("{message}")]
    Config { code: ErrorCode, message: String },

    #[error("{message}")]
    Api {
        code: ErrorCode,
        message: String,
        status: Option<u16>,
        details: Option<serde_json::Value>,
        suggestion: Option<String>,
    },

    #[error("{message}")]
    Wallet { code: ErrorCode, message: String },

    #[error("{message}")]
    Transaction {
        code: ErrorCode,
        message: String,
        tx_hash: Option<String>,
        suggestion: Option<String>,
    },

    #[error("Operation timed out: {message}")]
    Timeout { code: ErrorCode, message: String },

    #[error("Cancelled by user")]
    UserCancelled,
}

impl CliError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Config { code, .. } => *code,
            Self::Api { code, .. } => *code,
            Self::Wallet { code, .. } => *code,
            Self::Transaction { code, .. } => *code,
            Self::Timeout { code, .. } => *code,
            Self::UserCancelled => ErrorCode::UserCancelled,
        }
    }

    pub fn exit_code(&self) -> i32 {
        self.code().exit_code()
    }

    pub fn suggestion(&self) -> Option<&str> {
        match self {
            Self::Api { suggestion, .. } => suggestion.as_deref(),
            Self::Transaction { suggestion, .. } => suggestion.as_deref(),
            Self::Config { code, .. } => match code {
                ErrorCode::ApiKeyMissing => {
                    Some("Run '0x config set api_key <your-key>' or set ZEROX_API_KEY env var")
                }
                ErrorCode::WalletNotFound => Some(
                    "Run '0x config set wallet.evm <private-key>' or set ZEROX_EVM_PRIVATE_KEY",
                ),
                ErrorCode::ConfigNotFound => Some("Run '0x config init' to set up your config"),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn details(&self) -> Option<&serde_json::Value> {
        match self {
            Self::Api { details, .. } => details.as_ref(),
            _ => None,
        }
    }

    // Convenience constructors
    pub fn config(code: ErrorCode, message: impl Into<String>) -> Self {
        Self::Config {
            code,
            message: message.into(),
        }
    }

    pub fn api_key_missing() -> Self {
        Self::Config {
            code: ErrorCode::ApiKeyMissing,
            message: "No API key configured".into(),
        }
    }

    pub fn chain_not_supported(chain: &str) -> Self {
        Self::Api {
            code: ErrorCode::ChainNotSupported,
            message: format!("Chain '{chain}' is not supported"),
            status: None,
            details: None,
            suggestion: Some("Run '0x chains' to see supported chains".into()),
        }
    }

    /// Append (or set) a hint on the suggestion field. Only the Api and
    /// Transaction variants carry a suggestion; everything else passes through
    /// unchanged. Used by the RPC layer to add a "configure a private RPC"
    /// line when a request fails on a built-in public endpoint.
    pub fn append_suggestion(self, extra: &str) -> Self {
        match self {
            Self::Api {
                code,
                message,
                status,
                details,
                suggestion,
            } => Self::Api {
                code,
                message,
                status,
                details,
                suggestion: Some(match suggestion {
                    Some(s) => format!("{s} {extra}"),
                    None => extra.to_string(),
                }),
            },
            Self::Transaction {
                code,
                message,
                tx_hash,
                suggestion,
            } => Self::Transaction {
                code,
                message,
                tx_hash,
                suggestion: Some(match suggestion {
                    Some(s) => format!("{s} {extra}"),
                    None => extra.to_string(),
                }),
            },
            other => other,
        }
    }
}

/// Whether an error code points at the RPC layer specifically (network,
/// timeouts, rate-limits) rather than user input or on-chain reverts. Used
/// to decide when to add "this was a public RPC" hints.
pub fn is_rpc_layer_failure(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::NetworkError
            | ErrorCode::NetworkTimeout
            | ErrorCode::RpcError
            | ErrorCode::ApiRateLimited
            | ErrorCode::TransactionTimeout
            | ErrorCode::InternalServerError
    )
}

/// Serializable error detail for JSON output.
#[derive(Debug, Serialize)]
pub struct ErrorDetail {
    pub code: ErrorCode,
    pub message: String,
    pub category: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

impl From<&CliError> for ErrorDetail {
    fn from(err: &CliError) -> Self {
        let code = err.code();
        Self {
            code,
            message: err.to_string(),
            category: code.category().to_string(),
            retryable: code.retryable(),
            details: err.details().cloned(),
            suggestion: err.suggestion().map(|s| s.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin every variant's wire name. If a new variant is added without
    /// updating `ErrorCode::as_str`, serde would still serialize it (via
    /// SCREAMING_SNAKE_CASE) but the `Display` impl would not — this catches
    /// that drift.
    #[test]
    fn as_str_matches_serde_for_every_variant() {
        let variants = [
            ErrorCode::ConfigNotFound,
            ErrorCode::ConfigInvalid,
            ErrorCode::ApiKeyMissing,
            ErrorCode::WalletNotFound,
            ErrorCode::WalletInvalid,
            ErrorCode::KeyringUnavailable,
            ErrorCode::InputInvalid,
            ErrorCode::ChainNotSupported,
            ErrorCode::InsufficientBalance,
            ErrorCode::InsufficientAllowance,
            ErrorCode::NoLiquidity,
            ErrorCode::TokenNotSupported,
            ErrorCode::SellAmountTooSmall,
            ErrorCode::NetworkError,
            ErrorCode::NetworkTimeout,
            ErrorCode::RpcError,
            ErrorCode::ApiRateLimited,
            ErrorCode::InternalServerError,
            ErrorCode::ApiError,
            ErrorCode::ApiAccessDenied,
            ErrorCode::SigningFailed,
            ErrorCode::InvalidSignature,
            ErrorCode::SimulationFailed,
            ErrorCode::TransactionReverted,
            ErrorCode::TransactionTimeout,
            ErrorCode::QuoteExpired,
            ErrorCode::BridgeFailed,
            ErrorCode::BridgeTimeout,
            ErrorCode::UserCancelled,
        ];
        for v in variants {
            let serde_name = serde_json::to_string(&v).unwrap();
            let serde_name = serde_name.trim_matches('"');
            assert_eq!(v.as_str(), serde_name, "drift on {v:?}");
            assert_eq!(v.to_string(), serde_name, "Display drift on {v:?}");
        }
    }
}
