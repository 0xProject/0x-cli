use crate::config::types::AppConfig;
use crate::error::{CliError, ErrorCode};
use solana_sdk::signer::keypair::Keypair;
use solana_sdk::signer::Signer;

/// Load a Solana keypair from CLI flag, env var, OS keyring, or config file.
///
/// Priority: `--wallet` flag → `ZEROX_SOLANA_KEYPAIR` env → OS keyring → config plaintext.
/// The keyring stores key material (base58 or JSON array). File paths are stored
/// in the config file because the path itself isn't secret. Keyring failures
/// fall through silently to the config file.
pub fn load_solana_keypair(config: &AppConfig, cli_wallet: Option<&str>) -> Result<Keypair, CliError> {
    let source = if let Some(wallet_arg) = cli_wallet {
        wallet_arg.to_string()
    } else if let Ok(env_val) = std::env::var("ZEROX_SOLANA_KEYPAIR") {
        env_val
    } else if let Some(keyring_val) = crate::wallet::keyring_store::get(
        crate::wallet::keyring_store::keys::WALLET_SOLANA,
    )
    .unwrap_or(None)
    {
        keyring_val
    } else if let Some(ref config_val) = config.wallet.solana {
        config_val.clone()
    } else {
        return Err(CliError::Wallet {
            code: ErrorCode::WalletNotFound,
            message: "No Solana wallet configured. Set via --wallet, ZEROX_SOLANA_KEYPAIR env var, or 'config set wallet.solana <path>'".into(),
        });
    };

    // Try loading as file path first
    if std::path::Path::new(&source).exists() {
        return load_from_json_file(&source);
    }

    // Try as JSON array string: [1,2,3,...]
    if source.starts_with('[') {
        return load_from_json_string(&source);
    }

    // Try as base58-encoded secret key
    load_from_base58(&source)
}

fn load_from_json_file(path: &str) -> Result<Keypair, CliError> {
    let contents = std::fs::read_to_string(path).map_err(|e| CliError::Wallet {
        code: ErrorCode::WalletInvalid,
        message: format!("Failed to read keypair file '{path}': {e}"),
    })?;
    load_from_json_string(&contents)
}

fn load_from_json_string(json: &str) -> Result<Keypair, CliError> {
    let bytes: Vec<u8> = serde_json::from_str(json).map_err(|e| CliError::Wallet {
        code: ErrorCode::WalletInvalid,
        message: format!("Failed to parse keypair JSON: {e}"),
    })?;
    Keypair::try_from(bytes.as_slice()).map_err(|e| CliError::Wallet {
        code: ErrorCode::WalletInvalid,
        message: format!("Invalid keypair bytes: {e}"),
    })
}

fn load_from_base58(s: &str) -> Result<Keypair, CliError> {
    let bytes = bs58::decode(s).into_vec().map_err(|e| CliError::Wallet {
        code: ErrorCode::WalletInvalid,
        message: format!("Invalid base58 keypair: {e}"),
    })?;
    Keypair::try_from(bytes.as_slice()).map_err(|e| CliError::Wallet {
        code: ErrorCode::WalletInvalid,
        message: format!("Invalid Solana keypair: {e}"),
    })
}

/// Get the base58 pubkey string.
pub fn pubkey_string(keypair: &Keypair) -> String {
    keypair.pubkey().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_from_json_string() {
        // A valid 64-byte keypair as JSON array
        let keypair = Keypair::new();
        let bytes = keypair.to_bytes();
        let json = serde_json::to_string(&bytes.to_vec()).unwrap();

        let loaded = load_from_json_string(&json).unwrap();
        assert_eq!(loaded.pubkey(), keypair.pubkey());
    }

    #[test]
    fn test_load_from_base58() {
        let keypair = Keypair::new();
        let bytes = keypair.to_bytes();
        let b58 = bs58::encode(&bytes).into_string();

        let loaded = load_from_base58(&b58).unwrap();
        assert_eq!(loaded.pubkey(), keypair.pubkey());
    }

    #[test]
    fn test_no_wallet_configured() {
        let config = AppConfig::default();
        assert!(load_solana_keypair(&config, None).is_err());
    }

    #[test]
    fn test_invalid_base58() {
        assert!(load_from_base58("not-valid-base58!!!").is_err());
    }
}
