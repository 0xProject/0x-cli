//! Helpers for building trade-output payloads. Every command that produces a
//! `SwapOutput`, `CrossChainOutput`, `GaslessSwapOutput`, or `PriceResult`
//! used to repeat
//!
//! ```ignore
//! TokenInfo { address: …, symbol: sell_sym.clone(), decimals: sell_dec },
//! TokenAmount::from_optional_decimals(&quote.sell_amount, sell_dec),
//! TokenAmount::from_optional_decimals(&quote.buy_amount, buy_dec),
//! TokenAmount::from_optional_decimals(&quote.min_buy_amount, buy_dec),
//! ```
//!
//! for each side. `SideMeta` is the one place where the decimals/symbols live
//! during a command's body, so the call sites read as plain English.

use crate::api::types::{TokenAmount, TokenInfo};
use crate::token_cache::TokenMeta;

/// Metadata for one side of a trade (sell or buy).
///
/// Address comes from the 0x quote (already normalized). `symbol` and
/// `decimals` come from token metadata resolution and are `None` when that
/// resolution failed — in which case `amount` falls back to a raw-string
/// representation that the CLI tags as "decimals unknown" instead of silently
/// formatting at the wrong scale.
#[derive(Debug, Clone)]
pub struct SideMeta {
    pub address: String,
    pub symbol: Option<String>,
    pub decimals: Option<u8>,
}

impl SideMeta {
    /// New side from raw fields. Prefer [`Self::from_meta`] when constructing
    /// from `TokenCache` output.
    pub fn new(address: String, symbol: Option<String>, decimals: Option<u8>) -> Self {
        Self {
            address,
            symbol,
            decimals,
        }
    }

    /// Build a side from a `TokenCache` lookup. `meta = None` means metadata
    /// resolution failed; the side will be address-only with raw amounts.
    pub fn from_meta(address: String, meta: Option<TokenMeta>) -> Self {
        let (symbol, decimals) = match meta {
            Some(m) => (Some(m.symbol), Some(m.decimals)),
            None => (None, None),
        };
        Self {
            address,
            symbol,
            decimals,
        }
    }

    /// Address with no metadata. Used on Solana, where on-chain metadata isn't
    /// looked up.
    pub fn address_only(address: String) -> Self {
        Self::new(address, None, None)
    }

    /// `TokenInfo` clone for embedding in an output struct.
    pub fn token_info(&self) -> TokenInfo {
        TokenInfo {
            address: self.address.clone(),
            symbol: self.symbol.clone(),
            decimals: self.decimals,
        }
    }

    /// Format a raw base-unit amount using this side's decimals.
    pub fn amount(&self, raw: &str) -> TokenAmount {
        TokenAmount::from_optional_decimals(raw, self.decimals)
    }

    /// Human-readable label for this side: the symbol if we resolved one,
    /// otherwise the raw address. The output structs use this for the
    /// fall-through case in `display_human`.
    pub fn label(&self) -> &str {
        self.symbol.as_deref().unwrap_or(self.address.as_str())
    }
}
