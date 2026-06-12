use crate::config::{self, types::AppConfig, SecretStorage};
use crate::error::CliError;
use crate::output::envelope::Metadata;
use crate::output::{HumanDisplay, OutputHandler};
use serde::Serialize;
use std::io::{self, Write};
use std::process::Command;

const DASHBOARD_URL: &str = "https://dashboard.0x.org";

fn keyring_has(name: &str) -> bool {
    matches!(crate::wallet::keyring_store::get(name), Ok(Some(_)))
}

/// Show the config file path.
pub fn run_path(output: &OutputHandler) -> Result<i32, CliError> {
    let path = config::config_dir();
    let data = ConfigPath {
        path: path.display().to_string(),
    };
    Ok(output.emit_success("config path", &data, Metadata::default(), Vec::new(), 0))
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
    Ok(output.emit_success("config show", &redacted, Metadata::default(), Vec::new(), 0))
}

impl HumanDisplay for AppConfig {
    fn display_human(&self, writer: &mut dyn Write, _color: bool) -> io::Result<()> {
        let toml = toml::to_string_pretty(self).unwrap_or_else(|_| format!("{self:?}"));
        writeln!(writer, "{toml}")
    }
}

/// Set a config value. Wallet secrets are routed to the OS keyring by default;
/// pass `plaintext = true` to write them into the config file instead.
pub fn run_set(
    key: &str,
    value: &str,
    plaintext: bool,
    output: &OutputHandler,
) -> Result<i32, CliError> {
    // Disk-only view — never persist env-derived secrets back to the config file.
    let mut config = config::load_config_disk_only()?;
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
    Ok(output.emit_success("config set", &data, Metadata::default(), Vec::new(), 0))
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
    Ok(output.emit_success("config get", &data, Metadata::default(), Vec::new(), 0))
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
    let mut config = config::load_config_disk_only()?;
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
    Ok(output.emit_success("config unset", &data, Metadata::default(), Vec::new(), 0))
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

/// Switch the active profile. `default` clears the override so commands hit
/// the default `[api]` environment again.
pub fn run_use(name: &str, output: &OutputHandler) -> Result<i32, CliError> {
    let mut config = config::load_config_disk_only()?;
    let active = if name == "default" {
        config.active_profile = None;
        None
    } else {
        config::set_config_value(&mut config, "active_profile", name, false)?;
        Some(name.to_string())
    };
    config::save_config(&config)?;

    let data = ConfigUseResult {
        active_profile: active,
    };
    Ok(output.emit_success("config use", &data, Metadata::default(), Vec::new(), 0))
}

#[derive(Serialize)]
struct ConfigUseResult {
    active_profile: Option<String>,
}

impl HumanDisplay for ConfigUseResult {
    fn display_human(&self, writer: &mut dyn Write, _color: bool) -> io::Result<()> {
        match &self.active_profile {
            Some(n) => writeln!(writer, "Active profile: {n}"),
            None => writeln!(writer, "Using default environment"),
        }
    }
}

/// Interactive config wizard. When `browser` is true, opens
/// `https://dashboard.0x.org` in the OS default browser so the user can
/// grab an API key without leaving the terminal (copy/paste — no OAuth
/// round-trip).
pub fn run_init(output: &OutputHandler, browser: bool) -> Result<i32, CliError> {
    output.info("Setting up 0x CLI configuration...\n");

    let mut config = config::load_config_disk_only().unwrap_or_default();

    // API key — point users at the dashboard and (optionally) launch it.
    // We mask the input with `Password` so the key isn't echoed; empty
    // input keeps the current value when one is already configured.
    let api_already_set = config.api.api_key.is_some();
    if api_already_set {
        output.info(
            "API key already configured — submit empty to keep, or paste a new one to overwrite.",
        );
    } else {
        output.info(&format!("Get your 0x API key at {DASHBOARD_URL}"));
        if browser {
            match open_in_browser(DASHBOARD_URL) {
                Ok(()) => output.info("  → opened dashboard in your default browser"),
                Err(e) => output.info(&format!(
                    "  ! could not open browser ({e}); visit the URL manually"
                )),
            }
        }
    }
    let api_prompt = if api_already_set {
        "0x API key (current: stored; submit empty to keep)"
    } else {
        "Paste your 0x API key"
    };
    let api_key: String = dialoguer::Password::new()
        .with_prompt(api_prompt)
        .allow_empty_password(true)
        .interact()
        .map_err(|_| CliError::UserCancelled)?;
    if !api_key.is_empty() {
        config::set_config_value(&mut config, "api_key", &api_key, false)?;
    }

    // Default chain — show every supported chain so users see the full set,
    // not a hardcoded subset. Each row is `<name> (<display name>)` for
    // readability. Default selection is the chain currently in config (if
    // any), else `base` (the most-used in trading docs).
    let chains = crate::chain::all_chains();
    let chain_labels: Vec<String> = chains
        .iter()
        .map(|c| format!("{} ({})", c.name, c.display_name))
        .collect();
    let default_idx = config
        .defaults
        .chain
        .as_ref()
        .and_then(|c| chains.iter().position(|info| info.name == c))
        .or_else(|| chains.iter().position(|info| info.name == "base"))
        .unwrap_or(0);

    let chain_idx = dialoguer::Select::new()
        .with_prompt("Default chain")
        .items(&chain_labels)
        .default(default_idx)
        .interact()
        .map_err(|_| CliError::UserCancelled)?;
    config::set_config_value(&mut config, "defaults.chain", chains[chain_idx].name, false)?;

    // Default slippage — Input with the current value pre-filled. Empty
    // submission keeps whatever's already there; non-empty is validated
    // through set_config_value (clamped to 0..=10000).
    let current_slippage = config.defaults.slippage_bps;
    let slippage_input: String = dialoguer::Input::<String>::new()
        .with_prompt(format!(
            "Default slippage in basis points (100 = 1%, max 10000 = 100%; current: {current_slippage})"
        ))
        .allow_empty(true)
        .with_initial_text(current_slippage.to_string())
        .interact_text()
        .map_err(|_| CliError::UserCancelled)?;
    if !slippage_input.trim().is_empty() {
        config::set_config_value(
            &mut config,
            "defaults.slippage_bps",
            slippage_input.trim(),
            false,
        )?;
    }

    // Default approval type — Select so users see the two valid choices
    // explicitly. `exact` is the safer default: each swap approves only the
    // sell amount, so a future bug or compromised spender can't drain the
    // wallet.
    let approval_options = ["exact", "unlimited"];
    let approval_default_idx = approval_options
        .iter()
        .position(|opt| *opt == config.defaults.approval_type)
        .unwrap_or(0);
    let approval_idx = dialoguer::Select::new()
        .with_prompt("Default approval type (exact = per-swap; unlimited = approve once)")
        .items(approval_options)
        .default(approval_default_idx)
        .interact()
        .map_err(|_| CliError::UserCancelled)?;
    config::set_config_value(
        &mut config,
        "defaults.approval_type",
        approval_options[approval_idx],
        false,
    )?;

    // EVM wallet (optional) — written to OS keyring by default. If one is
    // already stored, hint at that and treat empty input as "keep current".
    // `Password` (not `Input`) so the hex secret isn't echoed to the terminal.
    let evm_in_keyring = keyring_has(crate::wallet::keyring_store::keys::WALLET_EVM);
    let evm_prompt = if evm_in_keyring {
        "EVM private key (hex, optional — current: stored in keyring; submit empty to keep)"
    } else {
        "EVM private key (hex, optional)"
    };
    let evm_key: String = dialoguer::Password::new()
        .with_prompt(evm_prompt)
        .allow_empty_password(true)
        .interact()
        .map_err(|_| CliError::UserCancelled)?;
    if !evm_key.is_empty() {
        let storage = config::set_config_value(&mut config, "wallet.evm", &evm_key, false)?;
        if matches!(storage, SecretStorage::Keyring) {
            output.info("  → stored EVM wallet in OS keyring");
        }
    }

    // Solana wallet (optional) — file paths stay in config (not sensitive),
    // key material (base58 / JSON array) goes to the keyring. Use Password so
    // we don't echo the base58 form; the file-path form is also not echoed,
    // which is acceptable since the user is typing what they intend either
    // way.
    let sol_in_keyring = keyring_has(crate::wallet::keyring_store::keys::WALLET_SOLANA);
    let sol_prompt = if sol_in_keyring {
        "Solana keypair file path or base58 secret (current: stored in keyring; submit empty to keep)"
    } else {
        "Solana keypair file path or base58 secret (optional)"
    };
    let sol_value: String = dialoguer::Password::new()
        .with_prompt(sol_prompt)
        .allow_empty_password(true)
        .interact()
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

    Ok(output.emit_success("config init", &data, Metadata::default(), Vec::new(), 0))
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

fn open_in_browser(url: &str) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(target_os = "linux")]
    let mut cmd = {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.args(["/c", "start", "", url]);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    return Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "browser auto-open is not supported on this platform",
    ));

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    {
        let status = cmd
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "browser launcher exited with {status}"
            )))
        }
    }
}
