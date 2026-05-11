use crate::config::{self, types::AppConfig, SecretStorage};
use crate::error::CliError;
use crate::output::{HumanDisplay, OutputHandler};
use crate::output::envelope::Metadata;
use serde::Serialize;
use std::io::{self, Write};

fn keyring_has(name: &str) -> bool {
    matches!(crate::wallet::keyring_store::get(name), Ok(Some(_)))
}

/// Show the config file path.
pub fn run_path(output: &OutputHandler) -> Result<i32, CliError> {
    let path = config::config_dir();
    let data = ConfigPath {
        path: path.display().to_string(),
    };
    output
        .success("config path", &data, Metadata::default(), Vec::new())
        .map_err(|e| CliError::config(crate::error::ErrorCode::Unknown, e.to_string()))
}

#[derive(Serialize)]
struct ConfigPath {
    path: String,
}

impl HumanDisplay for ConfigPath {
    fn display_human(&self, writer: &mut dyn Write, _color: bool) -> io::Result<()> {
        writeln!(writer, "{}", self.path)
    }
}

/// Show the full config (redacted).
pub fn run_show(output: &OutputHandler) -> Result<i32, CliError> {
    let config = config::load_config()?;
    let redacted = config.redacted();

    output
        .success("config show", &redacted, Metadata::default(), Vec::new())
        .map_err(|e| CliError::config(crate::error::ErrorCode::Unknown, e.to_string()))
}

impl HumanDisplay for AppConfig {
    fn display_human(&self, writer: &mut dyn Write, _color: bool) -> io::Result<()> {
        let toml = toml::to_string_pretty(self).unwrap_or_else(|_| format!("{self:?}"));
        writeln!(writer, "{toml}")
    }
}

/// Set a config value. Wallet secrets are routed to the OS keyring by default;
/// pass `plaintext = true` to write them into the config file instead.
pub fn run_set(key: &str, value: &str, plaintext: bool, output: &OutputHandler) -> Result<i32, CliError> {
    let mut config = config::load_config()?;
    let storage = config::set_config_value(&mut config, key, value, plaintext)?;
    config::save_config(&config)?;

    let data = ConfigSetResult {
        key: key.to_string(),
        value: if key.contains("wallet") || key == "api_key" {
            "***redacted***".to_string()
        } else {
            value.to_string()
        },
        storage,
    };

    let storage_note = match data.storage {
        SecretStorage::Keyring => " (stored in OS keyring)",
        SecretStorage::Config => "",
    };
    output.info(&format!("Set {key} successfully{storage_note}"));
    output
        .success("config set", &data, Metadata::default(), Vec::new())
        .map_err(|e| CliError::config(crate::error::ErrorCode::Unknown, e.to_string()))
}

#[derive(Serialize)]
struct ConfigSetResult {
    key: String,
    value: String,
    storage: SecretStorage,
}

impl HumanDisplay for ConfigSetResult {
    fn display_human(&self, writer: &mut dyn Write, _color: bool) -> io::Result<()> {
        let storage_note = match self.storage {
            SecretStorage::Keyring => " (keyring)",
            SecretStorage::Config => "",
        };
        writeln!(writer, "Set {} = {}{}", self.key, self.value, storage_note)
    }
}

/// Get a config value.
pub fn run_get(key: &str, output: &OutputHandler) -> Result<i32, CliError> {
    let config = config::load_config()?;
    let value = config::get_config_value(&config, key)?;

    let data = ConfigGetResult {
        key: key.to_string(),
        value: value.clone(),
    };

    output
        .success("config get", &data, Metadata::default(), Vec::new())
        .map_err(|e| CliError::config(crate::error::ErrorCode::Unknown, e.to_string()))
}

#[derive(Serialize)]
struct ConfigGetResult {
    key: String,
    value: String,
}

impl HumanDisplay for ConfigGetResult {
    fn display_human(&self, writer: &mut dyn Write, _color: bool) -> io::Result<()> {
        writeln!(writer, "{}", self.value)
    }
}

/// Remove a config value (clears both config file and keyring entries for
/// wallet keys).
pub fn run_unset(key: &str, output: &OutputHandler) -> Result<i32, CliError> {
    let mut config = config::load_config()?;
    let changed = config::unset_config_value(&mut config, key)?;
    config::save_config(&config)?;

    let data = ConfigUnsetResult {
        key: key.to_string(),
        changed,
    };

    if changed {
        output.info(&format!("Removed {key}"));
    } else {
        output.info(&format!("{key} was not set; nothing to remove"));
    }
    output
        .success("config unset", &data, Metadata::default(), Vec::new())
        .map_err(|e| CliError::config(crate::error::ErrorCode::Unknown, e.to_string()))
}

#[derive(Serialize)]
struct ConfigUnsetResult {
    key: String,
    changed: bool,
}

impl HumanDisplay for ConfigUnsetResult {
    fn display_human(&self, writer: &mut dyn Write, _color: bool) -> io::Result<()> {
        if self.changed {
            writeln!(writer, "Removed {}", self.key)
        } else {
            writeln!(writer, "{} was not set", self.key)
        }
    }
}

/// Interactive config wizard.
pub fn run_init(output: &OutputHandler) -> Result<i32, CliError> {
    output.info("Setting up 0x CLI configuration...\n");

    let mut config = config::load_config().unwrap_or_default();

    // API key
    let api_key: String = dialoguer::Input::new()
        .with_prompt("0x API key (get one at https://dashboard.0x.org)")
        .allow_empty(true)
        .with_initial_text(config.api.api_key.as_deref().unwrap_or(""))
        .interact_text()
        .map_err(|_| CliError::UserCancelled)?;
    if !api_key.is_empty() {
        config::set_config_value(&mut config, "api_key", &api_key, false)?;
    }

    // Default chain
    let chains = vec![
        "base", "ethereum", "arbitrum", "optimism", "polygon", "solana",
    ];
    let default_idx = config
        .defaults
        .chain
        .as_ref()
        .and_then(|c| chains.iter().position(|&name| name == c))
        .unwrap_or(0);

    let chain_idx = dialoguer::Select::new()
        .with_prompt("Default chain")
        .items(&chains)
        .default(default_idx)
        .interact()
        .map_err(|_| CliError::UserCancelled)?;
    config::set_config_value(&mut config, "defaults.chain", chains[chain_idx], false)?;

    // EVM wallet (optional) — written to OS keyring by default. If one is
    // already stored, hint at that and treat empty input as "keep current".
    let evm_in_keyring = keyring_has(crate::wallet::keyring_store::keys::WALLET_EVM);
    let evm_prompt = if evm_in_keyring {
        "EVM private key (hex, optional — current: stored in keyring; submit empty to keep)"
    } else {
        "EVM private key (hex, optional)"
    };
    let evm_key: String = dialoguer::Input::new()
        .with_prompt(evm_prompt)
        .allow_empty(true)
        .interact_text()
        .map_err(|_| CliError::UserCancelled)?;
    if !evm_key.is_empty() {
        let storage = config::set_config_value(&mut config, "wallet.evm", &evm_key, false)?;
        if matches!(storage, SecretStorage::Keyring) {
            output.info("  → stored EVM wallet in OS keyring");
        }
    }

    // Solana wallet (optional) — file paths stay in config, key material goes to keyring.
    let sol_in_keyring = keyring_has(crate::wallet::keyring_store::keys::WALLET_SOLANA);
    let sol_initial = if sol_in_keyring {
        ""
    } else {
        config.wallet.solana.as_deref().unwrap_or("")
    };
    let sol_prompt = if sol_in_keyring {
        "Solana keypair file path or base58 secret (current: stored in keyring; submit empty to keep)"
    } else {
        "Solana keypair file path or base58 secret (optional)"
    };
    let sol_value: String = dialoguer::Input::new()
        .with_prompt(sol_prompt)
        .allow_empty(true)
        .with_initial_text(sol_initial)
        .interact_text()
        .map_err(|_| CliError::UserCancelled)?;
    if !sol_value.is_empty() {
        let storage = config::set_config_value(&mut config, "wallet.solana", &sol_value, false)?;
        if matches!(storage, SecretStorage::Keyring) {
            output.info("  → stored Solana wallet in OS keyring");
        }
    }

    config::save_config(&config)?;

    let data = ConfigInitResult {
        config_path: config::config_file().display().to_string(),
    };

    output.info(&format!(
        "\nConfig saved to {}",
        config::config_file().display()
    ));

    output
        .success("config init", &data, Metadata::default(), Vec::new())
        .map_err(|e| CliError::config(crate::error::ErrorCode::Unknown, e.to_string()))
}

#[derive(Serialize)]
struct ConfigInitResult {
    config_path: String,
}

impl HumanDisplay for ConfigInitResult {
    fn display_human(&self, writer: &mut dyn Write, color: bool) -> io::Result<()> {
        if color {
            writeln!(
                writer,
                "{}",
                colored::Colorize::green("Configuration complete!")
            )
        } else {
            writeln!(writer, "Configuration complete!")
        }
    }
}
