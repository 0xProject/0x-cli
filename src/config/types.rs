use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Current config schema version. Bump when a change to `AppConfig` needs a
/// migration (renamed/retyped fields, semantic changes) and add the
/// transformation step in `config::validate_and_migrate`.
pub const CONFIG_VERSION: u32 = 1;

/// Serde default for files written before the version field existed —
/// version-less configs are v1 by definition.
fn default_config_version() -> u32 {
    1
}

/// Top-level CLI configuration stored in ~/.0x-config/config.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Schema version. Stamped to [`CONFIG_VERSION`] on every save; files
    /// from a *newer* CLI are rejected on load instead of being silently
    /// misread. Plain value — TOML requires these before any table.
    #[serde(default = "default_config_version")]
    pub version: u32,

    /// Profile applied when `--profile` isn't passed. Declared before the
    /// table fields — TOML requires plain values to precede tables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_profile: Option<String>,

    #[serde(default)]
    pub api: ApiConfig,

    #[serde(default)]
    pub defaults: Defaults,

    /// Chain name → RPC URL
    #[serde(default)]
    pub rpc: HashMap<String, String>,

    #[serde(default)]
    pub wallet: WalletConfig,

    /// Named environment overrides; unset fields fall back to the default
    /// `[api]` section at resolution time.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub profiles: HashMap<String, Profile>,

    #[serde(default)]
    pub telemetry: TelemetryConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            // Derived Default would zero this — fresh configs must carry the
            // current schema version.
            version: CONFIG_VERSION,
            active_profile: None,
            api: ApiConfig::default(),
            defaults: Defaults::default(),
            rpc: HashMap::new(),
            wallet: WalletConfig::default(),
            profiles: HashMap::new(),
            telemetry: TelemetryConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ApiConfig {
    /// 0x API key
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Profile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,

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

/// Anonymous usage telemetry settings. Telemetry is opt-out (default on) but
/// only ever sends when an Amplitude key is compiled into the binary — dev and
/// CI builds have none, so they are inert. See `crate::telemetry`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryConfig {
    /// Whether anonymous usage stats may be sent. User-settable via
    /// `0x config set telemetry.enabled false` (also overridable by the
    /// `ZEROX_TELEMETRY` / `DO_NOT_TRACK` env vars).
    #[serde(default = "default_telemetry_enabled")]
    pub enabled: bool,

    /// Random anonymous install token, generated once on first activation.
    /// Deliberately *not* a device or hardware identifier — just a fresh UUID
    /// so events from the same install group together. Settable only by the
    /// CLI, but visible in `config show` so users can see exactly what
    /// identifies them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_id: Option<String>,
}

fn default_telemetry_enabled() -> bool {
    true
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enabled: default_telemetry_enabled(),
            install_id: None,
        }
    }
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

        for profile in copy.profiles.values_mut() {
            if let Some(key) = profile.api_key.as_deref() {
                profile.api_key = Some(redact_string(key));
            }
        }

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
        assert_eq!(config.version, CONFIG_VERSION);
        assert_eq!(config.defaults.slippage_bps, 100);
        assert_eq!(config.defaults.approval_type, "exact");
        assert!(config.api.api_key.is_none());
        assert!(config.rpc.is_empty());
    }

    /// Files written before the version field existed must load as v1, not
    /// fail to parse and not default to 0.
    #[test]
    fn test_versionless_toml_loads_as_v1() {
        let parsed: AppConfig = toml::from_str(
            r#"
            [api]
            api_key = "test-key"

            [defaults]
            slippage_bps = 100
            approval_type = "exact"
            "#,
        )
        .unwrap();
        assert_eq!(parsed.version, 1);
    }

    #[test]
    fn test_version_roundtrips_through_toml() {
        let toml_str = toml::to_string_pretty(&AppConfig::default()).unwrap();
        assert!(toml_str.contains("version = 1"), "got: {toml_str}");
        let parsed: AppConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.version, CONFIG_VERSION);
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
            version: CONFIG_VERSION,
            active_profile: None,
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
            profiles: HashMap::new(),
            telemetry: TelemetryConfig::default(),
        };

        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: AppConfig = toml::from_str(&toml_str).unwrap();

        assert_eq!(parsed.api.api_key, config.api.api_key);
        assert_eq!(parsed.defaults.chain, config.defaults.chain);
        assert_eq!(parsed.defaults.slippage_bps, config.defaults.slippage_bps);
        assert_eq!(parsed.rpc.get("base"), config.rpc.get("base"));
    }

    #[test]
    fn test_profiles_roundtrip_and_redaction() {
        let mut config = AppConfig::default();
        config.api.api_key = Some("prod-key-12345678".to_string());
        config.active_profile = Some("stg".to_string());
        config.profiles.insert(
            "stg".to_string(),
            Profile {
                base_url: Some("https://staging.example.com".to_string()),
                api_key: Some("stg-key-12345678".to_string()),
            },
        );

        // active_profile is a plain value and must serialize before the table
        // fields — to_string_pretty errors if struct order puts it after them.
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: AppConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.active_profile.as_deref(), Some("stg"));
        let stg = parsed.profiles.get("stg").unwrap();
        assert_eq!(stg.base_url.as_deref(), Some("https://staging.example.com"));
        assert_eq!(stg.api_key.as_deref(), Some("stg-key-12345678"));

        let redacted = config.redacted();
        let stg = redacted.profiles.get("stg").unwrap();
        assert_eq!(stg.api_key.as_deref(), Some("stg-...5678"));
        assert_eq!(stg.base_url.as_deref(), Some("https://staging.example.com"));
    }

    #[test]
    fn test_telemetry_defaults_on_for_existing_configs() {
        // A config file written before telemetry existed (no [telemetry]
        // table) must deserialize with telemetry enabled and no install id.
        let parsed: AppConfig = toml::from_str(
            r#"
            [api]
            api_key = "test-key"

            [defaults]
            slippage_bps = 100
            approval_type = "exact"
            "#,
        )
        .unwrap();
        assert!(parsed.telemetry.enabled);
        assert!(parsed.telemetry.install_id.is_none());
    }
}
