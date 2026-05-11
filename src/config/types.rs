use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Top-level CLI configuration stored in ~/.0x-config/config.toml
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub api: ApiConfig,

    #[serde(default)]
    pub defaults: Defaults,

    /// Chain name → RPC URL
    #[serde(default)]
    pub rpc: HashMap<String, String>,

    #[serde(default)]
    pub wallet: WalletConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ApiConfig {
    /// 0x API key
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Defaults {
    /// Default chain (name or ID)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain: Option<String>,

    /// Default slippage in basis points
    pub slippage_bps: u32,

    /// Default token approval strategy
    pub approval_type: String,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            chain: None,
            slippage_bps: 100,
            approval_type: "exact".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WalletConfig {
    /// EVM private key (hex string)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evm: Option<String>,

    /// Solana keypair file path or base58 string
    #[serde(skip_serializing_if = "Option::is_none")]
    pub solana: Option<String>,
}

impl AppConfig {
    /// Create a redacted copy for display (secrets masked). Wallet fields also
    /// reflect keyring storage: when no plaintext value is present but the
    /// keyring holds one, the field reads `<stored in keyring>`.
    pub fn redacted(&self) -> Self {
        let mut copy = self.clone();

        if copy.api.api_key.is_some() {
            copy.api.api_key = Some(redact_string(
                copy.api.api_key.as_deref().unwrap_or_default(),
            ));
        }

        copy.wallet.evm = match copy.wallet.evm {
            Some(_) => Some("***redacted***".to_string()),
            None if keyring_has(crate::wallet::keyring_store::keys::WALLET_EVM) => {
                Some("<stored in keyring>".to_string())
            }
            None => None,
        };

        copy.wallet.solana = match copy.wallet.solana {
            Some(ref s) if is_path_like(s) => copy.wallet.solana.clone(),
            Some(_) => Some("***redacted***".to_string()),
            None if keyring_has(crate::wallet::keyring_store::keys::WALLET_SOLANA) => {
                Some("<stored in keyring>".to_string())
            }
            None => None,
        };

        copy
    }
}

/// Heuristic: treat a value as a filesystem path rather than secret key
/// material when it contains a path separator or ends in `.json`. The path
/// itself isn't sensitive — the contents of the file it points to are.
pub(crate) fn is_path_like(s: &str) -> bool {
    s.contains('/') || s.contains('\\') || s.ends_with(".json")
}

fn keyring_has(name: &str) -> bool {
    matches!(crate::wallet::keyring_store::get(name), Ok(Some(_)))
}

fn redact_string(s: &str) -> String {
    if s.len() <= 8 {
        "***".to_string()
    } else {
        format!("{}...{}", &s[..4], &s[s.len() - 4..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = AppConfig::default();
        assert_eq!(config.defaults.slippage_bps, 100);
        assert_eq!(config.defaults.approval_type, "exact");
        assert!(config.api.api_key.is_none());
        assert!(config.rpc.is_empty());
    }

    #[test]
    fn test_redacted_config() {
        let config = AppConfig {
            api: ApiConfig {
                api_key: Some("abcdef1234567890".to_string()),
            },
            wallet: WalletConfig {
                evm: Some("0xdeadbeef".to_string()),
                solana: Some("/home/user/.config/solana/id.json".to_string()),
            },
            ..Default::default()
        };

        let redacted = config.redacted();
        assert_eq!(redacted.api.api_key.unwrap(), "abcd...7890");
        assert_eq!(redacted.wallet.evm.unwrap(), "***redacted***");
        // File paths should NOT be redacted
        assert_eq!(
            redacted.wallet.solana.unwrap(),
            "/home/user/.config/solana/id.json"
        );
    }

    #[test]
    fn test_roundtrip_toml() {
        let config = AppConfig {
            api: ApiConfig {
                api_key: Some("test-key".to_string()),
            },
            defaults: Defaults {
                chain: Some("base".to_string()),
                slippage_bps: 50,
                approval_type: "unlimited".to_string(),
            },
            rpc: {
                let mut m = std::collections::HashMap::new();
                m.insert("base".to_string(), "https://base.llamarpc.com".to_string());
                m
            },
            wallet: WalletConfig {
                evm: Some("0xdeadbeef".to_string()),
                solana: None,
            },
        };

        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: AppConfig = toml::from_str(&toml_str).unwrap();

        assert_eq!(parsed.api.api_key, config.api.api_key);
        assert_eq!(parsed.defaults.chain, config.defaults.chain);
        assert_eq!(parsed.defaults.slippage_bps, config.defaults.slippage_bps);
        assert_eq!(parsed.rpc.get("base"), config.rpc.get("base"));
    }
}
