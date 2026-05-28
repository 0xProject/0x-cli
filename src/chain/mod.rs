pub mod evm;
pub mod solana;

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChainType {
    Evm,
    Svm,
}

/// Chain identifier that supports both numeric IDs and string names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum ChainId {
    Numeric(u64),
    Solana,
}

impl std::fmt::Display for ChainId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChainId::Numeric(id) => write!(f, "{id}"),
            ChainId::Solana => write!(f, "solana"),
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

    pub fn explorer_tx_url(&self, tx_hash: &str) -> String {
        format!("{}/tx/{}", self.explorer_url, tx_hash)
    }

    /// Get the numeric chain ID (for API calls). Returns None for Solana.
    pub fn numeric_id(&self) -> Option<u64> {
        match self.id {
            ChainId::Numeric(id) => Some(id),
            ChainId::Solana => None,
        }
    }

    /// Get the chain identifier for 0x API calls.
    /// For EVM: numeric string. For Solana: "solana".
    pub fn api_chain_id(&self) -> String {
        match self.id {
            ChainId::Numeric(id) => id.to_string(),
            ChainId::Solana => "solana".to_string(),
        }
    }
}

/// Static chain registry.
const CHAINS: &[ChainInfo] = &[
    ChainInfo {
        id: ChainId::Numeric(1),
        name: "ethereum",
        display_name: "Ethereum",
        native_token: "ETH",
        explorer_url: "https://etherscan.io",
        chain_type: ChainType::Evm,
    },
    ChainInfo {
        id: ChainId::Numeric(137),
        name: "polygon",
        display_name: "Polygon",
        native_token: "POL",
        explorer_url: "https://polygonscan.com",
        chain_type: ChainType::Evm,
    },
    ChainInfo {
        id: ChainId::Numeric(56),
        name: "bsc",
        display_name: "BNB Chain",
        native_token: "BNB",
        explorer_url: "https://bscscan.com",
        chain_type: ChainType::Evm,
    },
    ChainInfo {
        id: ChainId::Numeric(42161),
        name: "arbitrum",
        display_name: "Arbitrum",
        native_token: "ETH",
        explorer_url: "https://arbiscan.io",
        chain_type: ChainType::Evm,
    },
    ChainInfo {
        id: ChainId::Numeric(10),
        name: "optimism",
        display_name: "Optimism",
        native_token: "ETH",
        explorer_url: "https://optimistic.etherscan.io",
        chain_type: ChainType::Evm,
    },
    ChainInfo {
        id: ChainId::Numeric(8453),
        name: "base",
        display_name: "Base",
        native_token: "ETH",
        explorer_url: "https://basescan.org",
        chain_type: ChainType::Evm,
    },
    ChainInfo {
        id: ChainId::Numeric(43114),
        name: "avalanche",
        display_name: "Avalanche",
        native_token: "AVAX",
        explorer_url: "https://snowtrace.io",
        chain_type: ChainType::Evm,
    },
    ChainInfo {
        id: ChainId::Numeric(59144),
        name: "linea",
        display_name: "Linea",
        native_token: "ETH",
        explorer_url: "https://lineascan.build",
        chain_type: ChainType::Evm,
    },
    ChainInfo {
        id: ChainId::Numeric(534352),
        name: "scroll",
        display_name: "Scroll",
        native_token: "ETH",
        explorer_url: "https://scrollscan.com",
        chain_type: ChainType::Evm,
    },
    ChainInfo {
        id: ChainId::Numeric(81457),
        name: "blast",
        display_name: "Blast",
        native_token: "ETH",
        explorer_url: "https://blastscan.io",
        chain_type: ChainType::Evm,
    },
    ChainInfo {
        id: ChainId::Numeric(5000),
        name: "mantle",
        display_name: "Mantle",
        native_token: "MNT",
        explorer_url: "https://mantlescan.xyz",
        chain_type: ChainType::Evm,
    },
    ChainInfo {
        id: ChainId::Numeric(80094),
        name: "berachain",
        display_name: "Berachain",
        native_token: "BERA",
        explorer_url: "https://berascan.com",
        chain_type: ChainType::Evm,
    },
    ChainInfo {
        id: ChainId::Numeric(146),
        name: "sonic",
        display_name: "Sonic",
        native_token: "S",
        explorer_url: "https://sonicscan.org",
        chain_type: ChainType::Evm,
    },
    ChainInfo {
        id: ChainId::Numeric(130),
        name: "unichain",
        display_name: "Unichain",
        native_token: "ETH",
        explorer_url: "https://uniscan.xyz",
        chain_type: ChainType::Evm,
    },
    ChainInfo {
        id: ChainId::Numeric(480),
        name: "worldchain",
        display_name: "World Chain",
        native_token: "ETH",
        explorer_url: "https://worldscan.org",
        chain_type: ChainType::Evm,
    },
    ChainInfo {
        id: ChainId::Numeric(2741),
        name: "abstract",
        display_name: "Abstract",
        native_token: "ETH",
        explorer_url: "https://abscan.org",
        chain_type: ChainType::Evm,
    },
    ChainInfo {
        id: ChainId::Numeric(57073),
        name: "ink",
        display_name: "Ink",
        native_token: "ETH",
        explorer_url: "https://explorer.inkonchain.com",
        chain_type: ChainType::Evm,
    },
    ChainInfo {
        id: ChainId::Numeric(143),
        name: "monad",
        display_name: "Monad",
        native_token: "MON",
        explorer_url: "https://monadexplorer.com",
        chain_type: ChainType::Evm,
    },
    ChainInfo {
        id: ChainId::Numeric(999),
        name: "hyperevm",
        display_name: "HyperEVM",
        native_token: "HYPE",
        explorer_url: "https://hyperscan.xyz",
        chain_type: ChainType::Evm,
    },
    ChainInfo {
        id: ChainId::Solana,
        name: "solana",
        display_name: "Solana",
        native_token: "SOL",
        explorer_url: "https://solscan.io",
        chain_type: ChainType::Svm,
    },
];

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
/// For EVM: must start with 0x and be 42 chars (20 bytes hex).
/// For Solana: must be base58 encoded (32-44 chars, alphanumeric).
/// Returns Ok(()) if valid, Err with helpful message if not.
pub fn validate_token_address(token: &str, chain_info: &ChainInfo) -> Result<(), CliError> {
    if chain_info.is_evm() {
        if !token.starts_with("0x") || token.len() != 42 {
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
    } else if chain_info.is_solana()
        && (token.len() < 32 || token.len() > 44 || token.starts_with("0x"))
    {
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
                "Pass the amount in base units as a positive integer, e.g. 1000000 = 1 USDC (6 decimals), 1000000000000000000 = 1 ETH (18 decimals)".into(),
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
}
