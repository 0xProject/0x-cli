use crate::commands::swap::truncate_address;
use crate::error::{CliError, ErrorCode};
use crate::output::envelope::Warning;
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

/// Warning code surfaced when token metadata can't be resolved via RPC.
pub const WARN_METADATA_UNRESOLVED: &str = "TOKEN_METADATA_UNRESOLVED";

/// Cached token metadata (symbol + decimals).
#[derive(Debug, Clone)]
pub struct TokenMeta {
    pub symbol: String,
    pub decimals: u8,
}

/// Resolve metadata for a sell/buy token pair on an EVM chain.
/// Pushes a `TOKEN_METADATA_UNRESOLVED` warning into `warnings` for each token
/// that couldn't be resolved (either because no RPC is configured or the RPC
/// call failed). Returns the metadata for each token (None on failure).
pub async fn resolve_pair_evm(
    cache: &mut TokenCache,
    rpc_url: Option<&str>,
    chain_id: u64,
    sell_token: &str,
    buy_token: &str,
    warnings: &mut Vec<Warning>,
) -> (Option<TokenMeta>, Option<TokenMeta>) {
    let (sell, buy) = match rpc_url {
        Some(rpc) => (
            cache.resolve_evm(rpc, chain_id, sell_token).await,
            cache.resolve_evm(rpc, chain_id, buy_token).await,
        ),
        None => (None, None),
    };
    if sell.is_none() {
        warnings.push(unresolved_warning(sell_token, "sell"));
    }
    // Dedup: a pair where sell and buy are the same address only deserves one
    // warning. (Rare in real swaps but happens in tests and analytics tooling.)
    if buy.is_none() && sell_token.to_lowercase() != buy_token.to_lowercase() {
        warnings.push(unresolved_warning(buy_token, "buy"));
    }
    (sell, buy)
}

fn unresolved_warning(token: &str, side: &str) -> Warning {
    Warning {
        code: WARN_METADATA_UNRESOLVED.into(),
        message: format!(
            "Could not resolve metadata for {side} token {}. Showing raw amount; configure an RPC with `0x config set rpc.<chain> <url>` or pass --rpc-url.",
            truncate_address(token)
        ),
    }
}

/// In-memory cache for token metadata, keyed by (chain_id, lowercased address).
/// The chain_id in the key prevents wrong-chain metadata reuse — the same
/// address can exist on multiple EVM chains with different decimals/symbols.
pub struct TokenCache {
    cache: HashMap<(u64, String), TokenMeta>,
}

impl Default for TokenCache {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenCache {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// Resolve token metadata for an EVM token address on a specific chain.
    /// Returns `None` when the RPC lookup fails — callers should surface a
    /// warning and avoid formatting amounts at an assumed decimal count.
    pub async fn resolve_evm(
        &mut self,
        rpc_url: &str,
        chain_id: u64,
        token_address: &str,
    ) -> Option<TokenMeta> {
        let key = (chain_id, token_address.to_lowercase());

        if let Some(meta) = self.cache.get(&key) {
            return Some(meta.clone());
        }

        if is_native_token(token_address) {
            let meta = native_token_meta(token_address);
            self.cache.insert(key, meta.clone());
            return Some(meta);
        }

        match query_evm_token(rpc_url, token_address).await {
            Ok(m) => {
                self.cache.insert(key, m.clone());
                Some(m)
            }
            Err(_) => None,
        }
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

    // `decimals()` is what makes formatted amounts truthful; if it fails we
    // must propagate the error so the caller can surface a warning and skip
    // formatting. Symbol is cosmetic and can fall back to a truncated address.
    let decimals = contract
        .decimals()
        .call()
        .await
        .map_err(|e| CliError::Api {
            code: ErrorCode::RpcError,
            message: format!("Failed to read ERC-20 decimals() for {token_address}: {e}"),
            status: None,
            details: None,
            suggestion: None,
        })?;

    let symbol = contract
        .symbol()
        .call()
        .await
        .unwrap_or_else(|_| truncate_address(token_address));

    Ok(TokenMeta { symbol, decimals })
}

/// Canonical "native token" sentinel (Etherscan / 0x convention).
const NATIVE_PSEUDO_ADDRESS: &str = "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
/// Some integrations represent the native asset as the zero address. We accept
/// both because the 0x API has historically echoed back either form.
const ZERO_ADDRESS: &str = "0x0000000000000000000000000000000000000000";

/// Check if a token address is the native-asset sentinel. Exact match against
/// the two canonical pseudo-addresses — anything else (including arbitrary
/// hex addresses that happen to contain the substring "eth") goes through the
/// regular ERC-20 metadata lookup.
fn is_native_token(address: &str) -> bool {
    let lower = address.to_lowercase();
    matches!(lower.as_str(), NATIVE_PSEUDO_ADDRESS | ZERO_ADDRESS)
}

/// Best-effort metadata for the native sentinel. We don't know the chain here,
/// so we can't pick the actual ticker (ETH / POL / BNB / …); the symbol falls
/// back to "NATIVE" and the caller is expected to overlay the chain's
/// `native_token` from `ChainInfo` when rendering for humans. Decimals are 18
/// across every EVM chain the CLI currently supports.
fn native_token_meta(_address: &str) -> TokenMeta {
    TokenMeta {
        symbol: "NATIVE".to_string(),
        decimals: 18,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_native_token_detection() {
        // Canonical pseudo-addresses, case-insensitive.
        assert!(is_native_token(
            "0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE"
        ));
        assert!(is_native_token(
            "0x0000000000000000000000000000000000000000"
        ));
        // Real ERC-20 addresses must not match, even if their hex happens to
        // contain the substring "eth" (the old contains() check matched
        // 0x3 ... ETH ... → 0x3...eth... — see USDT on Ethereum).
        assert!(!is_native_token(
            "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
        ));
        assert!(!is_native_token(
            "0xdAC17F958D2ee523a2206206994597C13D831ec7"
        ));
        // Raw tickers never reach this function (validate_token_address blocks
        // anything without an 0x prefix), so they should not be recognized.
        assert!(!is_native_token("ETH"));
    }

    #[test]
    fn test_native_token_meta() {
        let meta = native_token_meta(NATIVE_PSEUDO_ADDRESS);
        assert_eq!(meta.symbol, "NATIVE");
        assert_eq!(meta.decimals, 18);
    }

    #[test]
    fn test_cache_returns_cached_value() {
        let mut cache = TokenCache::new();
        cache.cache.insert(
            (1, "0xtest".to_string()),
            TokenMeta {
                symbol: "TEST".to_string(),
                decimals: 6,
            },
        );

        let meta = cache.cache.get(&(1, "0xtest".to_string())).unwrap();
        assert_eq!(meta.symbol, "TEST");
        assert_eq!(meta.decimals, 6);
    }

    #[test]
    fn test_cache_isolates_by_chain_id() {
        let mut cache = TokenCache::new();
        // Same address, different chains can have different decimals
        // (e.g. wrapped tokens that bridged with non-standard decimals).
        cache.cache.insert(
            (1, "0xabc".to_string()),
            TokenMeta {
                symbol: "MAINNET".to_string(),
                decimals: 18,
            },
        );
        cache.cache.insert(
            (137, "0xabc".to_string()),
            TokenMeta {
                symbol: "POLY".to_string(),
                decimals: 6,
            },
        );

        assert_eq!(
            cache.cache.get(&(1, "0xabc".to_string())).unwrap().decimals,
            18
        );
        assert_eq!(
            cache
                .cache
                .get(&(137, "0xabc".to_string()))
                .unwrap()
                .decimals,
            6
        );
    }
}
