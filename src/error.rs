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

    // Catch-all
    Unknown,
}

impl ErrorCode {
    pub fn category(&self) -> &'static str {
        match self {
            Self::ConfigNotFound | Self::ConfigInvalid | Self::ApiKeyMissing => "config",
            Self::WalletNotFound | Self::WalletInvalid | Self::KeyringUnavailable => "config",
            Self::InputInvalid | Self::ChainNotSupported => "input",
            Self::InsufficientBalance | Self::InsufficientAllowance | Self::NoLiquidity
            | Self::TokenNotSupported | Self::SellAmountTooSmall => "validation",
            Self::NetworkError | Self::NetworkTimeout | Self::RpcError | Self::ApiRateLimited => {
                "network"
            }
            Self::InternalServerError | Self::ApiError => "api",
            Self::SigningFailed | Self::InvalidSignature => "signing",
            Self::SimulationFailed | Self::TransactionReverted | Self::TransactionTimeout
            | Self::QuoteExpired => "execution",
            Self::BridgeFailed | Self::BridgeTimeout => "bridge",
            Self::UserCancelled => "input",
            Self::Unknown => "unknown",
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
            Self::ConfigNotFound | Self::ConfigInvalid
            | Self::WalletNotFound | Self::WalletInvalid | Self::KeyringUnavailable => 3,
            Self::ApiKeyMissing => 5,
            Self::InputInvalid | Self::ChainNotSupported => 2,
            Self::InsufficientBalance | Self::InsufficientAllowance | Self::NoLiquidity
            | Self::TokenNotSupported | Self::SellAmountTooSmall => 2,
            Self::NetworkError | Self::NetworkTimeout | Self::RpcError | Self::ApiRateLimited => 4,
            Self::InternalServerError | Self::ApiError => 4,
            Self::SigningFailed | Self::InvalidSignature => 1,
            Self::SimulationFailed => 10,
            Self::TransactionReverted => 11,
            Self::TransactionTimeout => 12,
            Self::QuoteExpired => 1,
            Self::BridgeFailed => 1,
            Self::BridgeTimeout => 12,
            Self::UserCancelled => 20,
            Self::Unknown => 1,
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Use the serde serialization for display
        let s = serde_json::to_string(self).unwrap_or_else(|_| "UNKNOWN".to_string());
        // Remove quotes from JSON string
        write!(f, "{}", s.trim_matches('"'))
    }
}

/// The main CLI error type.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("{message}")]
    Config {
        code: ErrorCode,
        message: String,
    },

    #[error("{message}")]
    Api {
        code: ErrorCode,
        message: String,
        status: Option<u16>,
        details: Option<serde_json::Value>,
        suggestion: Option<String>,
    },

    #[error("{message}")]
    Wallet {
        code: ErrorCode,
        message: String,
    },

    #[error("{message}")]
    Transaction {
        code: ErrorCode,
        message: String,
        tx_hash: Option<String>,
        suggestion: Option<String>,
    },

    #[error("Operation timed out: {message}")]
    Timeout {
        code: ErrorCode,
        message: String,
    },

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
