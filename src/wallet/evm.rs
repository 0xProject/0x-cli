use crate::config::types::AppConfig;
use crate::error::{CliError, ErrorCode};
use alloy::signers::local::PrivateKeySigner;
use std::str::FromStr;

/// Load an EVM signer from CLI flag, env var, OS keyring, or config file.
///
/// Priority: `--wallet` flag → `ZEROX_EVM_PRIVATE_KEY` env → OS keyring → config plaintext.
/// Keyring failures (no daemon, denied access, etc.) fall through silently to
/// the config file rather than aborting the load — `config set` is the place
/// where keyring errors should surface, not arbitrary read paths.
pub fn load_evm_signer(config: &AppConfig, cli_wallet: Option<&str>) -> Result<PrivateKeySigner, CliError> {
    let key = if let Some(wallet_arg) = cli_wallet {
        wallet_arg.to_string()
    } else if let Ok(env_key) = std::env::var("ZEROX_EVM_PRIVATE_KEY") {
        env_key
    } else if let Some(keyring_key) = crate::wallet::keyring_store::get(
        crate::wallet::keyring_store::keys::WALLET_EVM,
    )
    .unwrap_or(None)
    {
        keyring_key
    } else if let Some(ref config_key) = config.wallet.evm {
        config_key.clone()
    } else {
        return Err(CliError::Wallet {
            code: ErrorCode::WalletNotFound,
            message: "No EVM wallet configured. Set via --wallet, ZEROX_EVM_PRIVATE_KEY env var, or 'config set wallet.evm <key>'".into(),
        });
    };

    // Normalize: ensure 0x prefix
    let key = if key.starts_with("0x") || key.starts_with("0X") {
        key
    } else {
        format!("0x{key}")
    };

    PrivateKeySigner::from_str(&key).map_err(|e| CliError::Wallet {
        code: ErrorCode::WalletInvalid,
        message: format!("Invalid EVM private key: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_signer_from_hex() {
        let config = AppConfig {
            wallet: crate::config::types::WalletConfig {
                evm: Some("ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        let signer = load_evm_signer(&config, None).unwrap();
        // Known address for this private key (Hardhat account #0)
        let addr = format!("{:?}", signer.address()).to_lowercase();
        assert_eq!(addr, "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266");
    }

    #[test]
    fn test_load_signer_with_0x_prefix() {
        let config = AppConfig {
            wallet: crate::config::types::WalletConfig {
                evm: Some("0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        let signer = load_evm_signer(&config, None).unwrap();
        let addr = format!("{:?}", signer.address()).to_lowercase();
        assert_eq!(addr, "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266");
    }

    #[test]
    fn test_load_signer_cli_override() {
        let config = AppConfig::default();
        let signer = load_evm_signer(
            &config,
            Some("0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"),
        )
        .unwrap();
        let addr = format!("{:?}", signer.address()).to_lowercase();
        assert_eq!(addr, "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266");
    }

    #[test]
    fn test_load_signer_no_wallet() {
        let config = AppConfig::default();
        assert!(load_evm_signer(&config, None).is_err());
    }

    #[test]
    fn test_load_signer_invalid_key() {
        let config = AppConfig {
            wallet: crate::config::types::WalletConfig {
                evm: Some("not-a-valid-key".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(load_evm_signer(&config, None).is_err());
    }
}
