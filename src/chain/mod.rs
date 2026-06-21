pub mod evm;
pub mod retry;
pub mod solana;
pub mod tron;

use crate::error::CliError;
use crate::output::human::DataTable;
use crate::output::HumanDisplay;
use serde::Serialize;
use std::io::{self, Write};

/// Information about a supported blockchain.
#[derive(Debug, Clone, Serialize)]
pub struct ChainInfo {
    pub id: ChainId,
    pub name: &'static str,
    pub display_name: &'static str,
    pub native_token: &'static str,
    pub explorer_url: &'static str,
    pub chain_type: ChainType,
    /// Built-in public RPC URL for this chain. `None` for chains where no
    /// canonical public RPC is stable enough to ship; users must
    /// `0x config set rpc.<chain> <url>` for those. Surfaced in
    /// `0x chains` so agents can see what URL the CLI falls back to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_rpc_url: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChainType {
    Evm,
    Svm,
    Tvm,
}

/// Chain identifier that supports both numeric IDs and the special "solana"
/// string identifier the 0x API uses for SVM. JSON encoding is hand-rolled so
/// `Numeric(8453)` becomes the JSON number `8453` and `Solana` becomes the
/// JSON string `"solana"` — the help text on `0x chains` documents this as
/// `id: number|string`. (A derived `#[serde(untagged)]` would emit `null` for
/// the unit variant, which would silently break agents matching on the field.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainId {
    Numeric(u64),
    Solana,
    Tron,
}

impl std::fmt::Display for ChainId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChainId::Numeric(id) => write!(f, "{id}"),
            ChainId::Solana => write!(f, "solana"),
            ChainId::Tron => write!(f, "tron"),
        }
    }
}

impl Serialize for ChainId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            ChainId::Numeric(id) => serializer.serialize_u64(*id),
            ChainId::Solana => serializer.serialize_str("solana"),
            ChainId::Tron => serializer.serialize_str("tron"),
        }
    }
}

impl ChainInfo {
    pub fn is_solana(&self) -> bool {
        self.chain_type == ChainType::Svm
    }

    pub fn is_evm(&self) -> bool {
        self.chain_type == ChainType::Evm
    }

    pub fn is_tron(&self) -> bool {
        self.chain_type == ChainType::Tvm
    }

    pub fn explorer_tx_url(&self, tx_hash: &str) -> String {
        if self.chain_type == ChainType::Tvm {
            format!("{}/#/transaction/{}", self.explorer_url, tx_hash)
        } else {
            format!("{}/tx/{}", self.explorer_url, tx_hash)
        }
    }

    /// Get the numeric chain ID (for API calls). Returns None for Solana.
    pub fn numeric_id(&self) -> Option<u64> {
        match self.id {
            ChainId::Numeric(id) => Some(id),
            ChainId::Solana => None,
            ChainId::Tron => None,
        }
    }

    /// Numeric chain ID for code paths that require an EVM chain. Returns a
    /// structured error instead of panicking when the chain is non-EVM, so
    /// command handlers that branched on EVM-vs-Solana earlier degrade
    /// gracefully if that invariant ever breaks.
    pub fn evm_chain_id(&self) -> Result<u64, crate::error::CliError> {
        self.numeric_id()
            .ok_or_else(|| crate::error::CliError::Api {
                code: crate::error::ErrorCode::InputInvalid,
                message: format!("'{}' is not an EVM chain", self.name),
                status: None,
                details: None,
                suggestion: Some("Use --chain with an EVM chain like 'base' or 'ethereum'".into()),
            })
    }

    /// Get the chain identifier for 0x API calls.
    /// For EVM: numeric string. For Solana: "solana". For Tron: "tron".
    pub fn api_chain_id(&self) -> String {
        match self.id {
            ChainId::Numeric(id) => id.to_string(),
            ChainId::Solana => "solana".to_string(),
            ChainId::Tron => "tron".to_string(),
        }
    }
}

/// Static chain registry. `default_rpc_url` is the built-in public RPC
/// each chain team documents; `None` for chains where the canonical
/// public endpoint isn't stable enough to ship — users configure their own.
const CHAINS: &[ChainInfo] = &[
    ChainInfo {
        id: ChainId::Numeric(1),
        name: "ethereum",
        display_name: "Ethereum",
        native_token: "ETH",
        explorer_url: "https://etherscan.io",
        chain_type: ChainType::Evm,
        default_rpc_url: Some("https://eth.merkle.io"),
    },
    ChainInfo {
        id: ChainId::Numeric(137),
        name: "polygon",
        display_name: "Polygon",
        native_token: "POL",
        explorer_url: "https://polygonscan.com",
        chain_type: ChainType::Evm,
        default_rpc_url: Some("https://polygon.drpc.org"),
    },
    ChainInfo {
        id: ChainId::Numeric(56),
        name: "bsc",
        display_name: "BNB Chain",
        native_token: "BNB",
        explorer_url: "https://bscscan.com",
        chain_type: ChainType::Evm,
        default_rpc_url: Some("https://56.rpc.thirdweb.com"),
    },
    ChainInfo {
        id: ChainId::Numeric(42161),
        name: "arbitrum",
        display_name: "Arbitrum",
        native_token: "ETH",
        explorer_url: "https://arbiscan.io",
        chain_type: ChainType::Evm,
        default_rpc_url: Some("https://arb1.arbitrum.io/rpc"),
    },
    ChainInfo {
        id: ChainId::Numeric(10),
        name: "optimism",
        display_name: "Optimism",
        native_token: "ETH",
        explorer_url: "https://optimistic.etherscan.io",
        chain_type: ChainType::Evm,
        default_rpc_url: Some("https://mainnet.optimism.io"),
    },
    ChainInfo {
        id: ChainId::Numeric(8453),
        name: "base",
        display_name: "Base",
        native_token: "ETH",
        explorer_url: "https://basescan.org",
        chain_type: ChainType::Evm,
        default_rpc_url: Some("https://mainnet.base.org"),
    },
    ChainInfo {
        id: ChainId::Numeric(43114),
        name: "avalanche",
        display_name: "Avalanche",
        native_token: "AVAX",
        explorer_url: "https://snowtrace.io",
        chain_type: ChainType::Evm,
        default_rpc_url: Some("https://api.avax.network/ext/bc/C/rpc"),
    },
    ChainInfo {
        id: ChainId::Numeric(59144),
        name: "linea",
        display_name: "Linea",
        native_token: "ETH",
        explorer_url: "https://lineascan.build",
        chain_type: ChainType::Evm,
        default_rpc_url: Some("https://rpc.linea.build"),
    },
    ChainInfo {
        id: ChainId::Numeric(534352),
        name: "scroll",
        display_name: "Scroll",
        native_token: "ETH",
        explorer_url: "https://scrollscan.com",
        chain_type: ChainType::Evm,
        default_rpc_url: Some("https://rpc.scroll.io"),
    },
    ChainInfo {
        id: ChainId::Numeric(81457),
        name: "blast",
        display_name: "Blast",
        native_token: "ETH",
        explorer_url: "https://blastscan.io",
        chain_type: ChainType::Evm,
        default_rpc_url: Some("https://rpc.blast.io"),
    },
    ChainInfo {
        id: ChainId::Numeric(5000),
        name: "mantle",
        display_name: "Mantle",
        native_token: "MNT",
        explorer_url: "https://mantlescan.xyz",
        chain_type: ChainType::Evm,
        default_rpc_url: Some("https://rpc.mantle.xyz"),
    },
    ChainInfo {
        id: ChainId::Numeric(80094),
        name: "berachain",
        display_name: "Berachain",
        native_token: "BERA",
        explorer_url: "https://berascan.com",
        chain_type: ChainType::Evm,
        default_rpc_url: Some("https://rpc.berachain.com"),
    },
    ChainInfo {
        id: ChainId::Numeric(146),
        name: "sonic",
        display_name: "Sonic",
        native_token: "S",
        explorer_url: "https://sonicscan.org",
        chain_type: ChainType::Evm,
        default_rpc_url: Some("https://rpc.soniclabs.com"),
    },
    ChainInfo {
        id: ChainId::Numeric(130),
        name: "unichain",
        display_name: "Unichain",
        native_token: "ETH",
        explorer_url: "https://uniscan.xyz",
        chain_type: ChainType::Evm,
        default_rpc_url: Some("https://mainnet.unichain.org"),
    },
    ChainInfo {
        id: ChainId::Numeric(480),
        name: "worldchain",
        display_name: "World Chain",
        native_token: "ETH",
        explorer_url: "https://worldscan.org",
        chain_type: ChainType::Evm,
        default_rpc_url: Some("https://worldchain-mainnet.g.alchemy.com/public"),
    },
    ChainInfo {
        id: ChainId::Numeric(2741),
        name: "abstract",
        display_name: "Abstract",
        native_token: "ETH",
        explorer_url: "https://abscan.org",
        chain_type: ChainType::Evm,
        default_rpc_url: Some("https://api.mainnet.abs.xyz"),
    },
    ChainInfo {
        id: ChainId::Numeric(57073),
        name: "ink",
        display_name: "Ink",
        native_token: "ETH",
        explorer_url: "https://explorer.inkonchain.com",
        chain_type: ChainType::Evm,
        default_rpc_url: Some("https://rpc-gel.inkonchain.com"),
    },
    ChainInfo {
        id: ChainId::Numeric(143),
        name: "monad",
        display_name: "Monad",
        native_token: "MON",
        explorer_url: "https://monadexplorer.com",
        chain_type: ChainType::Evm,
        default_rpc_url: Some("https://rpc.monad.xyz"),
    },
    ChainInfo {
        id: ChainId::Numeric(999),
        name: "hyperevm",
        display_name: "HyperEVM",
        native_token: "HYPE",
        explorer_url: "https://hyperscan.xyz",
        chain_type: ChainType::Evm,
        default_rpc_url: Some("https://rpc.hyperliquid.xyz/evm"),
    },
    ChainInfo {
        id: ChainId::Solana,
        name: "solana",
        display_name: "Solana",
        native_token: "SOL",
        explorer_url: "https://solscan.io",
        chain_type: ChainType::Svm,
        default_rpc_url: Some("https://api.mainnet-beta.solana.com"),
    },
    ChainInfo {
        id: ChainId::Tron,
        name: "tron",
        display_name: "Tron",
        native_token: "TRX",
        explorer_url: "https://tronscan.org",
        chain_type: ChainType::Tvm,
        default_rpc_url: Some("https://api.trongrid.io"),
    },
];

/// Read-only view of every supported chain. Consumers iterate this for
/// menus / config wizards / docs without needing access to the private
/// constant.
pub fn all_chains() -> &'static [ChainInfo] {
    CHAINS
}

/// Resolve a chain from a name or ID string.
pub fn resolve_chain(input: &str) -> Result<&'static ChainInfo, CliError> {
    let trimmed = input.trim();
    let input_lower = trimmed.to_lowercase();

    // Try exact name match
    if let Some(chain) = CHAINS.iter().find(|c| c.name == input_lower) {
        return Ok(chain);
    }

    // Try numeric ID match
    if let Ok(id) = trimmed.parse::<u64>() {
        if let Some(chain) = CHAINS
            .iter()
            .find(|c| matches!(c.id, ChainId::Numeric(cid) if cid == id))
        {
            return Ok(chain);
        }
    }

    Err(CliError::chain_not_supported(input))
}

/// Validate that a token looks like a valid address for the given chain type.
/// For EVM: must start with `0x` and be 42 chars of hex (20-byte address).
/// For Solana: must base58-decode to exactly 32 bytes (a `Pubkey`). The
/// length-only check we used to do here accepted obvious junk like a
/// 32-char string of `!!!!` and only failed later at the API.
pub fn validate_token_address(token: &str, chain_info: &ChainInfo) -> Result<(), CliError> {
    if chain_info.is_evm() {
        let valid_evm = token.len() == 42
            && token.starts_with("0x")
            && token[2..].chars().all(|c| c.is_ascii_hexdigit());
        if !valid_evm {
            return Err(CliError::Api {
                code: crate::error::ErrorCode::InputInvalid,
                message: format!("'{token}' is not a valid EVM token address"),
                status: None,
                details: None,
                suggestion: Some(
                    "Use the full contract address (0x + 40 hex chars), e.g. 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 for USDC on Base".into()
                ),
            });
        }
    } else if chain_info.is_solana() {
        // Length pre-check filters obvious junk and avoids hammering bs58 with
        // megabytes of data. The 32-byte check is the real guard.
        let plausible_length = (32..=44).contains(&token.len()) && !token.starts_with("0x");
        let valid_pubkey =
            plausible_length && matches!(bs58::decode(token).into_vec(), Ok(b) if b.len() == 32);
        if !valid_pubkey {
            return Err(CliError::Api {
                code: crate::error::ErrorCode::InputInvalid,
                message: format!("'{token}' is not a valid Solana token address"),
                status: None,
                details: None,
                suggestion: Some(
                    "Use the base58 mint address, e.g. EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v for USDC on Solana".into()
                ),
            });
        }
    } else if chain_info.is_tron() {
        if !crate::chain::tron::is_valid_tron_address(token) {
            return Err(CliError::Api {
                code: crate::error::ErrorCode::InputInvalid,
                message: format!("'{token}' is not a valid Tron token address"),
                status: None,
                details: None,
                suggestion: Some(
                    "Use the base58check TRC20 address, e.g. TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t for USDT on Tron".into(),
                ),
            });
        }
    }
    Ok(())
}

/// Validate that an amount string is a positive base-unit integer.
/// Rejects empty, signed, decimal, or non-digit input. Catches the common
/// confusion of passing a human-formatted value like "0.5" instead of "500000".
pub fn validate_base_unit_amount(amount: &str) -> Result<(), CliError> {
    if amount.is_empty() || !amount.chars().all(|c| c.is_ascii_digit()) {
        return Err(CliError::Api {
            code: crate::error::ErrorCode::InputInvalid,
            message: format!("'{amount}' is not a valid base-unit amount"),
            status: None,
            details: None,
            suggestion: Some(
                "Pass the amount in base units as a positive integer — the token's \
                 smallest unit, no decimals applied: a 6-decimal token uses 1000000 = 1.0, \
                 an 18-decimal token uses 1000000000000000000 = 1.0".into(),
            ),
        });
    }
    if amount.chars().all(|c| c == '0') {
        return Err(CliError::Api {
            code: crate::error::ErrorCode::InputInvalid,
            message: "Amount must be greater than 0".into(),
            status: None,
            details: None,
            suggestion: None,
        });
    }
    Ok(())
}

/// Chains list for human display.
pub struct ChainsList;

impl HumanDisplay for ChainsList {
    fn display_human(&self, writer: &mut dyn Write, color: bool) -> io::Result<()> {
        let table = DataTable {
            title: Some("Supported Chains".to_string()),
            headers: vec![
                "ID".into(),
                "Name".into(),
                "Network".into(),
                "Native Token".into(),
                "Explorer".into(),
                "Default RPC".into(),
            ],
            rows: CHAINS
                .iter()
                .map(|c| {
                    vec![
                        c.id.to_string(),
                        c.name.to_string(),
                        c.display_name.to_string(),
                        c.native_token.to_string(),
                        c.explorer_url.to_string(),
                        c.default_rpc_url.unwrap_or("—").to_string(),
                    ]
                })
                .collect(),
        };
        table.display_human(writer, color)
    }
}

impl Serialize for ChainsList {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        CHAINS.serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_by_name() {
        let chain = resolve_chain("base").unwrap();
        assert_eq!(chain.name, "base");
        assert_eq!(chain.numeric_id(), Some(8453));
    }

    #[test]
    fn test_resolve_by_id() {
        let chain = resolve_chain("8453").unwrap();
        assert_eq!(chain.name, "base");
    }

    #[test]
    fn test_resolve_solana() {
        let chain = resolve_chain("solana").unwrap();
        assert!(chain.is_solana());
        assert_eq!(chain.numeric_id(), None);
    }

    #[test]
    fn test_resolve_case_insensitive() {
        let chain = resolve_chain("Base").unwrap();
        assert_eq!(chain.name, "base");
    }

    #[test]
    fn test_resolve_unknown() {
        assert!(resolve_chain("unknown_chain").is_err());
        assert!(resolve_chain("99999").is_err());
    }

    #[test]
    fn test_explorer_tx_url() {
        let chain = resolve_chain("base").unwrap();
        assert_eq!(
            chain.explorer_tx_url("0xabc"),
            "https://basescan.org/tx/0xabc"
        );
    }

    #[test]
    fn test_validate_evm_token_address() {
        let base = resolve_chain("base").unwrap();
        // Canonical USDC on Base.
        assert!(validate_token_address("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913", base).is_ok());
        // Length wrong.
        assert!(validate_token_address("0xabc", base).is_err());
        // Missing 0x.
        assert!(validate_token_address("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913", base).is_err());
        // Non-hex body — pass-3 added this check; the prior length+prefix
        // gate would have accepted this.
        assert!(
            validate_token_address("0xZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ", base).is_err()
        );
    }

    #[test]
    fn test_validate_solana_token_address() {
        let solana = resolve_chain("solana").unwrap();
        // Canonical USDC mint on Solana.
        assert!(
            validate_token_address("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", solana).is_ok()
        );
        // EVM-shaped address rejected.
        assert!(
            validate_token_address("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913", solana).is_err()
        );
        // Pass-3 boundary: 32 chars but not base58-decodable to 32 bytes.
        // `!` isn't in the base58 alphabet — pre-pass-3 this passed.
        assert!(validate_token_address("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!", solana).is_err());
        // Too short.
        assert!(validate_token_address("abc", solana).is_err());
    }

    #[test]
    fn test_chain_id_serializes_as_number_or_solana_string() {
        // The `0x chains` JSON contract documents id as `number|string`. The
        // previous `#[serde(untagged)]` derive emitted `null` for the Solana
        // unit variant — this test pins the hand-rolled Serialize impl.
        assert_eq!(
            serde_json::to_string(&ChainId::Numeric(8453)).unwrap(),
            "8453"
        );
        assert_eq!(
            serde_json::to_string(&ChainId::Solana).unwrap(),
            "\"solana\""
        );
    }

    #[test]
    fn test_resolve_tron() {
        let chain = resolve_chain("tron").unwrap();
        assert!(chain.is_tron());
        assert!(!chain.is_evm());
        assert!(!chain.is_solana());
        assert_eq!(chain.numeric_id(), None);
        assert_eq!(chain.api_chain_id(), "tron");
    }

    #[test]
    fn test_chain_id_tron_serializes_as_string() {
        assert_eq!(
            serde_json::to_string(&ChainId::Tron).unwrap(),
            "\"tron\""
        );
    }

    #[test]
    fn test_tron_explorer_tx_url() {
        let chain = resolve_chain("tron").unwrap();
        assert_eq!(
            chain.explorer_tx_url("abc123"),
            "https://tronscan.org/#/transaction/abc123"
        );
    }

    #[test]
    fn test_validate_tron_token_address() {
        let tron = resolve_chain("tron").unwrap();
        assert!(validate_token_address("TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t", tron).is_ok());
        assert!(validate_token_address("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913", tron).is_err());
        assert!(validate_token_address("not-an-address", tron).is_err());
    }
}
