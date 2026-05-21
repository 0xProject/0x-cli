pub mod types;

use crate::chain::ChainInfo;
use crate::error::{CliError, ErrorCode};
use std::fs;
use std::path::PathBuf;
use types::AppConfig;

/// Resolve the RPC URL for a chain: config (by name or numeric id) → built-in
/// default for well-known chains. Errors when none of the above produces a URL.
/// The `--rpc-url` flag and `ZEROX_RPC_URL` env var are handled at the clap
/// layer and reach this resolver via [`resolve_rpc_url_with_override`].
pub fn resolve_rpc_url(config: &AppConfig, chain_info: &ChainInfo) -> Result<String, CliError> {
    if let Some(url) = config.rpc.get(chain_info.name) {
        return Ok(url.clone());
    }

    if let Some(id) = chain_info.numeric_id() {
        if let Some(url) = config.rpc.get(&id.to_string()) {
            return Ok(url.clone());
        }
    }

    let default = match chain_info.name {
        "ethereum" => Some("https://eth.llamarpc.com"),
        "base" => Some("https://base.llamarpc.com"),
        "arbitrum" => Some("https://arb1.arbitrum.io/rpc"),
        "optimism" => Some("https://mainnet.optimism.io"),
        "polygon" => Some("https://polygon-rpc.com"),
        "bsc" => Some("https://bsc-dataseed.binance.org"),
        "avalanche" => Some("https://api.avax.network/ext/bc/C/rpc"),
        "solana" => Some("https://api.mainnet-beta.solana.com"),
        _ => None,
    };

    default.map(|s| s.to_string()).ok_or_else(|| CliError::Config {
        code: ErrorCode::ConfigNotFound,
        message: format!(
            "No RPC URL configured for chain '{}'. Set one with: 0x config set rpc.{} <url>",
            chain_info.display_name, chain_info.name
        ),
    })
}

/// Resolve the RPC URL, preferring the CLI override (`--rpc-url`) when set.
/// Falls back to [`resolve_rpc_url`] (env → config → built-in default).
pub fn resolve_rpc_url_with_override(
    override_url: Option<&str>,
    config: &AppConfig,
    chain_info: &ChainInfo,
) -> Result<String, CliError> {
    if let Some(url) = override_url {
        return Ok(url.to_string());
    }
    resolve_rpc_url(config, chain_info)
}

/// Best-effort version of [`resolve_rpc_url_with_override`]: returns `None`
/// when neither the override nor the config produces a URL.
pub fn try_resolve_rpc_url_with_override(
    override_url: Option<&str>,
    config: &AppConfig,
    chain_info: &ChainInfo,
) -> Option<String> {
    resolve_rpc_url_with_override(override_url, config, chain_info).ok()
}

/// Returns the config directory path: ~/.0x-config/
pub fn config_dir() -> PathBuf {
    dirs::home_dir()
        .expect("Could not determine home directory")
        .join(".0x-config")
}

/// Returns the config file path: ~/.0x-config/config.toml
pub fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

/// Load configuration from disk, with environment variable overrides.
pub fn load_config() -> Result<AppConfig, CliError> {
    let path = config_file();

    let mut config = if path.exists() {
        let contents = fs::read_to_string(&path).map_err(|e| CliError::Config {
            code: ErrorCode::ConfigInvalid,
            message: format!("Failed to read config file: {e}"),
        })?;
        toml::from_str::<AppConfig>(&contents).map_err(|e| CliError::Config {
            code: ErrorCode::ConfigInvalid,
            message: format!("Failed to parse config file: {e}"),
        })?
    } else {
        AppConfig::default()
    };

    // Environment variable overrides
    if let Ok(key) = std::env::var("ZEROX_API_KEY") {
        config.api.api_key = Some(key);
    }
    if let Ok(key) = std::env::var("ZEROX_EVM_PRIVATE_KEY") {
        config.wallet.evm = Some(key);
    }
    if let Ok(key) = std::env::var("ZEROX_SOLANA_KEYPAIR") {
        config.wallet.solana = Some(key);
    }
    if let Ok(chain) = std::env::var("ZEROX_DEFAULT_CHAIN") {
        config.defaults.chain = Some(chain);
    }

    Ok(config)
}

/// Save configuration to disk, creating the directory if needed.
/// Sets secure permissions (0700 dir, 0600 file) on Unix.
pub fn save_config(config: &AppConfig) -> Result<(), CliError> {
    let dir = config_dir();
    let path = config_file();

    // Create directory with 0700 permissions
    if !dir.exists() {
        fs::create_dir_all(&dir).map_err(|e| CliError::Config {
            code: ErrorCode::ConfigInvalid,
            message: format!("Failed to create config directory: {e}"),
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).map_err(|e| {
                CliError::Config {
                    code: ErrorCode::ConfigInvalid,
                    message: format!("Failed to set directory permissions: {e}"),
                }
            })?;
        }
    }

    let toml_str = toml::to_string_pretty(config).map_err(|e| CliError::Config {
        code: ErrorCode::ConfigInvalid,
        message: format!("Failed to serialize config: {e}"),
    })?;

    // Atomic write: write to temp file, then rename
    let tmp_path = path.with_extension("toml.tmp");
    fs::write(&tmp_path, &toml_str).map_err(|e| CliError::Config {
        code: ErrorCode::ConfigInvalid,
        message: format!("Failed to write config file: {e}"),
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o600)).map_err(|e| {
            CliError::Config {
                code: ErrorCode::ConfigInvalid,
                message: format!("Failed to set file permissions: {e}"),
            }
        })?;
    }

    fs::rename(&tmp_path, &path).map_err(|e| CliError::Config {
        code: ErrorCode::ConfigInvalid,
        message: format!("Failed to save config file: {e}"),
    })?;

    Ok(())
}

/// Where a wallet secret was written. Surfaced to the user so they know
/// whether it landed in the keyring or in `~/.0x-config/config.toml`.
#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretStorage {
    Keyring,
    Config,
}

/// Set a config value by dot-notation key. Wallet secrets are written to the
/// OS keyring by default; pass `plaintext = true` to write them into the
/// config file. The flag has no effect on non-secret keys. Returns where any
/// secret ended up so the caller can display it.
///
/// Solana wallet values that look like a file path are always written to the
/// config file — the path itself isn't sensitive; the contents of the file are.
pub fn set_config_value(
    config: &mut AppConfig,
    key: &str,
    value: &str,
    plaintext: bool,
) -> Result<SecretStorage, CliError> {
    match key {
        "api_key" => {
            config.api.api_key = Some(value.to_string());
            Ok(SecretStorage::Config)
        }
        "defaults.chain" => {
            config.defaults.chain = Some(value.to_string());
            Ok(SecretStorage::Config)
        }
        "defaults.slippage_bps" => {
            config.defaults.slippage_bps =
                value.parse().map_err(|_| CliError::Config {
                    code: ErrorCode::InputInvalid,
                    message: format!("Invalid slippage value: {value}"),
                })?;
            Ok(SecretStorage::Config)
        }
        "defaults.approval_type" => {
            if value != "exact" && value != "unlimited" {
                return Err(CliError::Config {
                    code: ErrorCode::InputInvalid,
                    message: format!(
                        "Invalid approval type '{value}'. Must be 'exact' or 'unlimited'"
                    ),
                });
            }
            config.defaults.approval_type = value.to_string();
            Ok(SecretStorage::Config)
        }
        "wallet.evm" => set_wallet_secret(
            crate::wallet::keyring_store::keys::WALLET_EVM,
            value,
            plaintext,
            |v| config.wallet.evm = v,
        ),
        "wallet.solana" => {
            // File paths are not secret — keep them in the config file.
            if crate::config::types::is_path_like(value) {
                config.wallet.solana = Some(value.to_string());
                // Also clear any stale keyring entry so file-path mode is consistent.
                let _ = crate::wallet::keyring_store::delete(
                    crate::wallet::keyring_store::keys::WALLET_SOLANA,
                );
                Ok(SecretStorage::Config)
            } else {
                set_wallet_secret(
                    crate::wallet::keyring_store::keys::WALLET_SOLANA,
                    value,
                    plaintext,
                    |v| config.wallet.solana = v,
                )
            }
        }
        key if key.starts_with("rpc.") => {
            let chain = key.strip_prefix("rpc.").unwrap();
            config.rpc.insert(chain.to_string(), value.to_string());
            Ok(SecretStorage::Config)
        }
        _ => Err(CliError::Config {
            code: ErrorCode::InputInvalid,
            message: format!("Unknown config key: '{key}'"),
        }),
    }
}

fn set_wallet_secret(
    keyring_name: &str,
    value: &str,
    plaintext: bool,
    set_config_field: impl FnOnce(Option<String>),
) -> Result<SecretStorage, CliError> {
    if plaintext {
        // Clear any stale keyring entry so the new plaintext value isn't
        // shadowed by an older keyring secret on the read path.
        let _ = crate::wallet::keyring_store::delete(keyring_name);
        set_config_field(Some(value.to_string()));
        Ok(SecretStorage::Config)
    } else {
        crate::wallet::keyring_store::set(keyring_name, value)?;
        // Clear any plaintext copy of this secret from the config file.
        set_config_field(None);
        Ok(SecretStorage::Keyring)
    }
}

/// Remove a config value by dot-notation key. For wallet keys this also
/// deletes the matching OS keyring entry. Returns `true` if anything was
/// actually cleared (so callers can distinguish a no-op from a real change).
pub fn unset_config_value(config: &mut AppConfig, key: &str) -> Result<bool, CliError> {
    let mut changed = false;
    match key {
        "api_key" => changed = config.api.api_key.take().is_some(),
        "defaults.chain" => changed = config.defaults.chain.take().is_some(),
        "defaults.slippage_bps" => {
            // Reset to default rather than removing — the field isn't Option.
            if config.defaults.slippage_bps != types::Defaults::default().slippage_bps {
                config.defaults.slippage_bps = types::Defaults::default().slippage_bps;
                changed = true;
            }
        }
        "defaults.approval_type" => {
            if config.defaults.approval_type != types::Defaults::default().approval_type {
                config.defaults.approval_type = types::Defaults::default().approval_type;
                changed = true;
            }
        }
        "wallet.evm" => {
            changed |= config.wallet.evm.take().is_some();
            changed |= unset_keyring(crate::wallet::keyring_store::keys::WALLET_EVM);
        }
        "wallet.solana" => {
            changed |= config.wallet.solana.take().is_some();
            changed |= unset_keyring(crate::wallet::keyring_store::keys::WALLET_SOLANA);
        }
        key if key.starts_with("rpc.") => {
            let chain = key.strip_prefix("rpc.").unwrap();
            changed = config.rpc.remove(chain).is_some();
        }
        _ => {
            return Err(CliError::Config {
                code: ErrorCode::InputInvalid,
                message: format!("Unknown config key: '{key}'"),
            });
        }
    }
    Ok(changed)
}

/// Get a config value by dot-notation key.
///
/// Wallet keys are aware of OS keyring storage: when the config file has no
/// plaintext value but the keyring holds one, returns `"<stored in keyring>"`.
/// Secret material is never returned — only paths (`wallet.solana`) come back
/// verbatim, since paths themselves aren't sensitive.
pub fn get_config_value(config: &AppConfig, key: &str) -> Result<String, CliError> {
    let value = match key {
        "api_key" => config.api.api_key.clone(),
        "defaults.chain" => config.defaults.chain.clone(),
        "defaults.slippage_bps" => Some(config.defaults.slippage_bps.to_string()),
        "defaults.approval_type" => Some(config.defaults.approval_type.clone()),
        "wallet.evm" => match config.wallet.evm {
            Some(_) => Some("***redacted***".to_string()),
            None if keyring_has(crate::wallet::keyring_store::keys::WALLET_EVM) => {
                Some("<stored in keyring>".to_string())
            }
            None => None,
        },
        "wallet.solana" => match config.wallet.solana {
            Some(ref s) if crate::config::types::is_path_like(s) => Some(s.clone()),
            Some(_) => Some("***redacted***".to_string()),
            None if keyring_has(crate::wallet::keyring_store::keys::WALLET_SOLANA) => {
                Some("<stored in keyring>".to_string())
            }
            None => None,
        },
        key if key.starts_with("rpc.") => {
            let chain = key.strip_prefix("rpc.").unwrap();
            config.rpc.get(chain).cloned()
        }
        _ => {
            return Err(CliError::Config {
                code: ErrorCode::InputInvalid,
                message: format!("Unknown config key: '{key}'"),
            });
        }
    };

    value.ok_or_else(|| CliError::Config {
        code: ErrorCode::ConfigNotFound,
        message: format!("Config key '{key}' is not set"),
    })
}

fn keyring_has(name: &str) -> bool {
    matches!(crate::wallet::keyring_store::get(name), Ok(Some(_)))
}

/// Best-effort keyring deletion. Returns `true` only if an entry was actually
/// removed. Swallows keyring errors so an unavailable keyring (e.g. headless
/// Linux without DBus) doesn't block clearing the config-file half of the
/// secret — the user still wants their plaintext gone.
fn unset_keyring(name: &str) -> bool {
    let had_entry = matches!(crate::wallet::keyring_store::get(name), Ok(Some(_)));
    let _ = crate::wallet::keyring_store::delete(name);
    had_entry
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[allow(dead_code)]
    fn with_temp_home<F: FnOnce()>(f: F) {
        let tmp = TempDir::new().unwrap();
        std::env::set_var("HOME", tmp.path());
        f();
    }

    #[test]
    fn test_set_and_get_config_values() {
        let mut config = AppConfig::default();

        set_config_value(&mut config, "api_key", "test-key", false).unwrap();
        assert_eq!(config.api.api_key.as_deref(), Some("test-key"));

        set_config_value(&mut config, "defaults.chain", "base", false).unwrap();
        assert_eq!(config.defaults.chain.as_deref(), Some("base"));

        set_config_value(&mut config, "defaults.slippage_bps", "50", false).unwrap();
        assert_eq!(config.defaults.slippage_bps, 50);

        set_config_value(&mut config, "rpc.base", "https://base.example.com", false).unwrap();
        assert_eq!(
            config.rpc.get("base").unwrap(),
            "https://base.example.com"
        );

        // Get values
        assert_eq!(get_config_value(&config, "api_key").unwrap(), "test-key");
        assert_eq!(get_config_value(&config, "defaults.chain").unwrap(), "base");

        // Unknown key
        assert!(set_config_value(&mut config, "unknown.key", "val", false).is_err());
        assert!(get_config_value(&config, "unknown.key").is_err());
    }

    #[test]
    fn test_invalid_approval_type() {
        let mut config = AppConfig::default();
        assert!(set_config_value(&mut config, "defaults.approval_type", "bad", false).is_err());
        assert!(set_config_value(&mut config, "defaults.approval_type", "exact", false).is_ok());
        assert!(set_config_value(&mut config, "defaults.approval_type", "unlimited", false).is_ok());
    }

    #[test]
    fn test_solana_path_stays_in_config() {
        let mut config = AppConfig::default();
        let storage = set_config_value(
            &mut config,
            "wallet.solana",
            "/home/user/.config/solana/id.json",
            false,
        )
        .unwrap();
        assert!(matches!(storage, SecretStorage::Config));
        assert_eq!(
            config.wallet.solana.as_deref(),
            Some("/home/user/.config/solana/id.json")
        );
    }

    #[test]
    fn test_resolve_rpc_url_with_override_precedence() {
        use crate::chain::{resolve_chain, ChainId, ChainInfo, ChainType};

        let base = resolve_chain("base").unwrap();
        let mut config = AppConfig::default();

        // 1. Override wins over everything else, even when config has a value.
        config
            .rpc
            .insert("base".to_string(), "https://configured.example".to_string());
        let resolved = resolve_rpc_url_with_override(
            Some("https://override.example"),
            &config,
            base,
        )
        .unwrap();
        assert_eq!(resolved, "https://override.example");

        // 2. No override → fall back to config.
        let resolved = resolve_rpc_url_with_override(None, &config, base).unwrap();
        assert_eq!(resolved, "https://configured.example");

        // 3. No override and no config → fall back to the built-in default.
        let empty = AppConfig::default();
        let resolved = resolve_rpc_url_with_override(None, &empty, base).unwrap();
        assert_eq!(resolved, "https://base.llamarpc.com");

        // 4. No override, no config, no default → Err.
        let unknown = ChainInfo {
            id: ChainId::Numeric(999_999),
            name: "made-up-chain",
            display_name: "Made Up",
            native_token: "MUC",
            explorer_url: "",
            chain_type: ChainType::Evm,
        };
        assert!(resolve_rpc_url_with_override(None, &empty, &unknown).is_err());

        // 5. try_ variant returns None instead of Err for the same case.
        assert!(try_resolve_rpc_url_with_override(None, &empty, &unknown).is_none());
    }

    #[test]
    fn test_plaintext_flag_keeps_wallet_in_config() {
        let mut config = AppConfig::default();
        let storage = set_config_value(
            &mut config,
            "wallet.evm",
            "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
            true,
        )
        .unwrap();
        assert!(matches!(storage, SecretStorage::Config));
        assert!(config.wallet.evm.is_some());
    }
}
