use crate::commands::swap::truncate_address;
use crate::error::{CliError, ErrorCode};
use alloy::primitives::Address;
use alloy::providers::ProviderBuilder;
use alloy::sol;
use std::collections::HashMap;
use std::str::FromStr;

sol! {
    #[sol(rpc)]
    contract IERC20Read {
        function decimals() external view returns (uint8);
        function symbol() external view returns (string);
    }
}

/// Cached token metadata (symbol + decimals).
#[derive(Debug, Clone)]
pub struct TokenMeta {
    pub symbol: String,
    pub decimals: u8,
}

impl Default for TokenMeta {
    fn default() -> Self {
        Self {
            symbol: "???".to_string(),
            decimals: 18,
        }
    }
}

/// In-memory cache for token metadata, keyed by (chain_id, address).
pub struct TokenCache {
    cache: HashMap<String, TokenMeta>,
}

impl TokenCache {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// Resolve token metadata for an EVM token address.
    /// Uses the cache if available, otherwise queries the chain.
    pub async fn resolve_evm(
        &mut self,
        rpc_url: &str,
        token_address: &str,
    ) -> TokenMeta {
        let key = token_address.to_lowercase();

        // Check cache first
        if let Some(meta) = self.cache.get(&key) {
            return meta.clone();
        }

        // Check if it's a native token (ETH/etc — no contract to call)
        if is_native_token(token_address) {
            let meta = native_token_meta(token_address);
            self.cache.insert(key, meta.clone());
            return meta;
        }

        // Query chain
        let meta = match query_evm_token(rpc_url, token_address).await {
            Ok(m) => m,
            Err(_) => {
                // Fallback: use 18 decimals and truncated address as symbol
                eprintln!(
                    "Warning: Could not resolve token metadata for {}. Using 18 decimals (amounts may display incorrectly).",
                    truncate_address(token_address)
                );
                TokenMeta {
                    symbol: truncate_address(token_address),
                    decimals: 18,
                }
            }
        };

        self.cache.insert(key, meta.clone());
        meta
    }

}

/// Query an EVM chain for token decimals and symbol.
async fn query_evm_token(rpc_url: &str, token_address: &str) -> Result<TokenMeta, CliError> {
    let addr = Address::from_str(token_address).map_err(|e| CliError::Api {
        code: ErrorCode::InputInvalid,
        message: format!("Invalid token address '{token_address}': {e}"),
        status: None,
        details: None,
        suggestion: None,
    })?;

    let provider = ProviderBuilder::new()
        .connect(rpc_url)
        .await
        .map_err(|e| CliError::Api {
            code: ErrorCode::RpcError,
            message: format!("Failed to connect to RPC for token metadata: {e}"),
            status: None,
            details: None,
            suggestion: None,
        })?;

    let contract = IERC20Read::new(addr, &provider);

    let decimals = contract
        .decimals()
        .call()
        .await
        .unwrap_or(18);

    let symbol = contract
        .symbol()
        .call()
        .await
        .unwrap_or_else(|_| truncate_address(token_address));

    Ok(TokenMeta { symbol, decimals })
}

/// Check if a token address is the native token (ETH, etc.).
fn is_native_token(address: &str) -> bool {
    let lower = address.to_lowercase();
    // Common native token representations
    lower == "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
        || lower == "0x0000000000000000000000000000000000000000"
        || lower == "eth"
        || lower == "matic"
        || lower == "bnb"
        || lower == "avax"
}

/// Get metadata for native tokens.
fn native_token_meta(address: &str) -> TokenMeta {
    let lower = address.to_lowercase();
    if lower.contains("eth") || lower == "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee" {
        TokenMeta {
            symbol: "ETH".to_string(),
            decimals: 18,
        }
    } else {
        TokenMeta {
            symbol: address.to_uppercase(),
            decimals: 18,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_native_token_detection() {
        assert!(is_native_token("0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE"));
        assert!(is_native_token("ETH"));
        assert!(!is_native_token("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"));
    }

    #[test]
    fn test_native_token_meta() {
        let meta = native_token_meta("ETH");
        assert_eq!(meta.symbol, "ETH");
        assert_eq!(meta.decimals, 18);
    }

    #[test]
    fn test_cache_returns_cached_value() {
        let mut cache = TokenCache::new();
        let key = "0xtest".to_lowercase();
        cache.cache.insert(
            key,
            TokenMeta {
                symbol: "TEST".to_string(),
                decimals: 6,
            },
        );

        // Should return cached value from the internal cache
        let meta = cache.cache.get("0xtest").unwrap();
        assert_eq!(meta.symbol, "TEST");
        assert_eq!(meta.decimals, 6);
    }
}
