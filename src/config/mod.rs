pub mod types;

use crate::chain::ChainInfo;
use crate::error::{CliError, ErrorCode};
use std::fs;
use std::path::PathBuf;
use types::AppConfig;

/// Where the chosen RPC URL came from. Surfaced to write paths so they can
/// emit a warning when running against a public fallback endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcSource {
    /// From `--rpc-url` or `ZEROX_RPC_URL`.
    Override,
    /// From `rpc.<chain>` in the config file.
    Config,
    /// Built-in public RPC fallback for the chain. Public endpoints throttle
    /// and may return stale state; write paths warn.
    BuiltinDefault,
}

/// A resolved RPC URL plus where it came from.
#[derive(Debug, Clone)]
pub struct ResolvedRpc {
    pub url: String,
    pub source: RpcSource,
}

impl ResolvedRpc {
    /// When `self.source == BuiltinDefault` and `err` looks like an RPC-layer
    /// failure (timeouts, rate-limits, network), append a "configure a
    /// private RPC" hint to the error's suggestion field. Other sources or
    /// non-RPC errors pass through unchanged.
    ///
    /// The hint only fires when it's actionable: the user is on a public
    /// fallback AND the failure is plausibly caused by that endpoint's
    /// limits, rather than e.g. an on-chain revert or bad input.
    pub fn enrich_rpc_error(
        &self,
        err: crate::error::CliError,
        chain_info: &ChainInfo,
    ) -> crate::error::CliError {
        if self.source != RpcSource::BuiltinDefault
            || !crate::error::is_rpc_layer_failure(err.code())
        {
            return err;
        }
        err.append_suggestion(&format!(
            "This call used the built-in public RPC for {}. If you're hitting rate limits or timeouts, configure a private one: 0x config set rpc.{} <url>",
            chain_info.display_name, chain_info.name
        ))
    }
}

/// Resolve the 0x API key: `--api-key` flag first, then the config view
/// (which already has `ZEROX_API_KEY` overlaid by [`load_config`]). Errors
/// with the structured `API_KEY_MISSING` when neither is set — every command
/// needs this exact chain, so it lives here instead of being copy-pasted.
pub fn resolve_api_key(
    global: &crate::GlobalOpts,
    config: &AppConfig,
) -> Result<String, CliError> {
    global
        .api_key
        .as_deref()
        .or(config.api.api_key.as_deref())
        .map(str::to_string)
        .ok_or_else(CliError::api_key_missing)
}

/// A fully resolved API environment: which profile (if any) is active, the
/// base URL to hit, and the API key to send.
#[derive(Debug, Clone)]
pub struct ResolvedEnv {
    /// Active profile name; `None` means the default `[api]` section.
    pub profile: Option<String>,
    pub base_url: String,
    pub api_key: String,
}

/// Resolve the API environment: `--profile` / `ZEROX_PROFILE` first, then
/// `active_profile` from the config file, then the default `[api]` section.
/// Within a profile, unset fields fall back to the default section, so a
/// profile may override just the key or just the URL. The `--api-key` flag
/// (and `ZEROX_API_KEY`, which clap feeds into it) wins over both.
pub fn resolve_env(
    global: &crate::GlobalOpts,
    config: &AppConfig,
) -> Result<ResolvedEnv, CliError> {
    let name = global
        .profile
        .as_deref()
        .or(config.active_profile.as_deref());

    let profile = match name {
        Some(n) => Some(config.profiles.get(n).ok_or_else(|| {
            let available = if config.profiles.is_empty() {
                "none defined".to_string()
            } else {
                config.profiles.keys().cloned().collect::<Vec<_>>().join(", ")
            };
            CliError::Config {
                code: ErrorCode::ConfigNotFound,
                message: format!(
                    "Profile '{n}' is not defined (available: {available}). Define it with: 0x config set profiles.{n}.base_url <url>"
                ),
            }
        })?),
        None => None,
    };

    let api_key = global
        .api_key
        .as_deref()
        .or_else(|| profile.and_then(|p| p.api_key.as_deref()))
        .or(config.api.api_key.as_deref())
        .map(str::to_string)
        .ok_or_else(CliError::api_key_missing)?;

    let base_url = profile
        .and_then(|p| p.base_url.clone())
        .unwrap_or_else(|| crate::api::BASE_URL.to_string());

    Ok(ResolvedEnv {
        profile: name.map(str::to_string),
        base_url,
        api_key,
    })
}

/// Resolve an RPC URL, preferring `override_url` (CLI flag / env var), then
/// the config file's `rpc.<name>` or `rpc.<numeric_id>` entry, then the
/// chain's built-in public default. Errors only when none of the above
/// produces a URL — typically for newer chains we don't ship a default for.
pub fn resolve_rpc(
    override_url: Option<&str>,
    config: &AppConfig,
    chain_info: &ChainInfo,
) -> Result<ResolvedRpc, CliError> {
    if let Some(url) = override_url {
        return Ok(ResolvedRpc {
            url: url.to_string(),
            source: RpcSource::Override,
        });
    }
    if let Some(url) = config.rpc.get(chain_info.name) {
        return Ok(ResolvedRpc {
            url: url.clone(),
            source: RpcSource::Config,
        });
    }
    if let Some(id) = chain_info.numeric_id() {
        if let Some(url) = config.rpc.get(&id.to_string()) {
            return Ok(ResolvedRpc {
                url: url.clone(),
                source: RpcSource::Config,
            });
        }
    }
    if let Some(url) = chain_info.default_rpc_url {
        return Ok(ResolvedRpc {
            url: url.to_string(),
            source: RpcSource::BuiltinDefault,
        });
    }
    Err(CliError::Config {
        code: ErrorCode::ConfigNotFound,
        message: format!(
            "No RPC URL configured for chain '{}' and no built-in default available. Set one with: 0x config set rpc.{} <url>, or pass --rpc-url <url>",
            chain_info.display_name, chain_info.name
        ),
    })
}

/// String-only convenience wrapper around [`resolve_rpc`] for callers that
/// don't care about the source.
pub fn resolve_rpc_url_with_override(
    override_url: Option<&str>,
    config: &AppConfig,
    chain_info: &ChainInfo,
) -> Result<String, CliError> {
    resolve_rpc(override_url, config, chain_info).map(|r| r.url)
}

/// Best-effort version of [`resolve_rpc_url_with_override`]: returns `None`
/// when no URL can be resolved at all (a chain without a built-in default
/// and no user config).
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

/// Load configuration from disk only, with no environment-variable overrides.
/// Use this in any write path (config set/unset/init) — the env-overlaid
/// view from [`load_config`] would persist env-derived secrets back to disk
/// on the next `save_config`.
pub fn load_config_disk_only() -> Result<AppConfig, CliError> {
    let path = config_file();
    if path.exists() {
        let contents = fs::read_to_string(&path).map_err(|e| CliError::Config {
            code: ErrorCode::ConfigInvalid,
            message: format!("Failed to read config file: {e}"),
        })?;
        toml::from_str::<AppConfig>(&contents).map_err(|e| CliError::Config {
            code: ErrorCode::ConfigInvalid,
            message: format!("Failed to parse config file: {e}"),
        })
    } else {
        Ok(AppConfig::default())
    }
}

/// Load configuration from disk and overlay environment variables.
/// Use this for read/runtime paths (swap, price, status, etc.) where the
/// caller will not persist the result. **Do not pass the result to
/// `save_config`** — env-derived secrets would leak to disk.
pub fn load_config() -> Result<AppConfig, CliError> {
    let mut config = load_config_disk_only()?;

    // Environment variable overrides — applied to the in-memory view only.
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
            // Mirror the clap range on `0x swap --slippage` (0..=10000) so we
            // can't quietly persist a value that the swap CLI would later
            // refuse — the user would see "100 bps" stored and "20000
            // out-of-range" later.
            let parsed: u32 = value.parse().map_err(|_| CliError::Config {
                code: ErrorCode::InputInvalid,
                message: format!("Invalid slippage value: {value}"),
            })?;
            if parsed > 10000 {
                return Err(CliError::Config {
                    code: ErrorCode::InputInvalid,
                    message: format!(
                        "Slippage must be 0..=10000 bps (100 = 1%, 10000 = 100%); got {parsed}"
                    ),
                });
            }
            config.defaults.slippage_bps = parsed;
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

    fn global_with(profile: Option<&str>, api_key: Option<&str>) -> crate::GlobalOpts {
        crate::GlobalOpts {
            api_key: api_key.map(str::to_string),
            wallet: None,
            rpc_url: None,
            timeout: 30,
            yes: false,
            dry_run: false,
            verbose: false,
            profile: profile.map(str::to_string),
        }
    }

    #[test]
    fn test_resolve_env_precedence() {
        let mut config = AppConfig::default();
        config.api.api_key = Some("prod-key".to_string());
        config.profiles.insert(
            "stg".to_string(),
            types::Profile {
                base_url: Some("https://staging.example.com".to_string()),
                api_key: Some("stg-key".to_string()),
            },
        );

        // 1. No profile anywhere → default env.
        let env = resolve_env(&global_with(None, None), &config).unwrap();
        assert!(env.profile.is_none());
        assert_eq!(env.base_url, crate::api::BASE_URL);
        assert_eq!(env.api_key, "prod-key");

        // 2. --profile flag selects the profile.
        let env = resolve_env(&global_with(Some("stg"), None), &config).unwrap();
        assert_eq!(env.profile.as_deref(), Some("stg"));
        assert_eq!(env.base_url, "https://staging.example.com");
        assert_eq!(env.api_key, "stg-key");

        // 3. active_profile from config applies when no flag is passed,
        //    and the --profile flag wins over it.
        config.active_profile = Some("other".to_string());
        config.profiles.insert("other".to_string(), types::Profile::default());
        let env = resolve_env(&global_with(None, None), &config).unwrap();
        assert_eq!(env.profile.as_deref(), Some("other"));
        let env = resolve_env(&global_with(Some("stg"), None), &config).unwrap();
        assert_eq!(env.profile.as_deref(), Some("stg"));
        assert_eq!(env.base_url, "https://staging.example.com");
        config.active_profile = None;
        config.profiles.remove("other");

        // 4. --api-key / ZEROX_API_KEY beats the profile's key.
        let env = resolve_env(&global_with(Some("stg"), Some("flag-key")), &config).unwrap();
        assert_eq!(env.api_key, "flag-key");

        // 5. Profile without its own key falls back to the default key;
        //    profile without a base_url falls back to BASE_URL.
        config.profiles.insert("keyless".to_string(), types::Profile {
            base_url: None,
            api_key: None,
        });
        let env = resolve_env(&global_with(Some("keyless"), None), &config).unwrap();
        assert_eq!(env.api_key, "prod-key");
        assert_eq!(env.base_url, crate::api::BASE_URL);

        // 6. Unknown profile errors.
        assert!(resolve_env(&global_with(Some("nope"), None), &config).is_err());

        // 7. No key anywhere errors.
        config.api.api_key = None;
        assert!(resolve_env(&global_with(Some("keyless"), None), &config).is_err());
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
        assert_eq!(config.rpc.get("base").unwrap(), "https://base.example.com");

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
        assert!(
            set_config_value(&mut config, "defaults.approval_type", "unlimited", false).is_ok()
        );
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
    fn test_resolve_rpc_precedence() {
        use crate::chain::{resolve_chain, ChainId, ChainInfo, ChainType};

        let base = resolve_chain("base").unwrap();
        let mut config = AppConfig::default();

        // 1. Override wins over everything else, even when config has a value.
        config
            .rpc
            .insert("base".to_string(), "https://configured.example".to_string());
        let resolved = resolve_rpc(Some("https://override.example"), &config, base).unwrap();
        assert_eq!(resolved.url, "https://override.example");
        assert_eq!(resolved.source, RpcSource::Override);

        // 2. No override → fall back to config.
        let resolved = resolve_rpc(None, &config, base).unwrap();
        assert_eq!(resolved.url, "https://configured.example");
        assert_eq!(resolved.source, RpcSource::Config);

        // 3. No override and no config → fall back to the chain's built-in
        //    public default. Used to Err before defaults were introduced.
        let empty = AppConfig::default();
        let resolved = resolve_rpc(None, &empty, base).unwrap();
        assert_eq!(resolved.url, "https://mainnet.base.org");
        assert_eq!(resolved.source, RpcSource::BuiltinDefault);

        // 4. Unknown chain (no built-in default) → Err.
        let unknown = ChainInfo {
            id: ChainId::Numeric(999_999),
            name: "made-up-chain",
            display_name: "Made Up",
            native_token: "MUC",
            explorer_url: "",
            chain_type: ChainType::Evm,
            default_rpc_url: None,
        };
        assert!(resolve_rpc(None, &empty, &unknown).is_err());
        assert!(try_resolve_rpc_url_with_override(None, &empty, &unknown).is_none());

        // 5. Config entry by numeric id (e.g. `rpc.8453`) is also honored.
        let mut by_id = AppConfig::default();
        by_id
            .rpc
            .insert("8453".to_string(), "https://by-numeric.example".to_string());
        let resolved = resolve_rpc(None, &by_id, base).unwrap();
        assert_eq!(resolved.url, "https://by-numeric.example");
        assert_eq!(resolved.source, RpcSource::Config);
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
