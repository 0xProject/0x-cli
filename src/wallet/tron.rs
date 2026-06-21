use crate::config::types::AppConfig;
use crate::error::{CliError, ErrorCode};
use alloy::primitives::keccak256;
use alloy::signers::k256::ecdsa::SigningKey;

/// A loaded Tron signer: a secp256k1 key plus its derived base58check address.
pub struct TronSigner {
    signing_key: SigningKey,
    address: String,
}

impl TronSigner {
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Sign a 32-byte Tron txID, returning a 65-byte `r ‖ s ‖ recovery_id`
    /// signature (recovery id is 0/1, NOT the EVM 27/28 convention).
    pub fn sign_txid(&self, txid: &[u8; 32]) -> [u8; 65] {
        let (sig, recid) = self
            .signing_key
            .sign_prehash_recoverable(txid)
            .expect("signing a 32-byte prehash cannot fail");
        let mut out = [0u8; 65];
        out[..64].copy_from_slice(&sig.to_bytes());
        out[64] = recid.to_byte();
        out
    }
}

/// Derive the base58check Tron address from a secp256k1 signing key:
/// `base58check(0x41 ++ keccak256(uncompressed_pubkey[1..])[12..])`.
fn derive_address(signing_key: &SigningKey) -> String {
    let verifying_key = signing_key.verifying_key();
    let point = verifying_key.to_encoded_point(false); // 0x04 ++ X(32) ++ Y(32)
    let hash = keccak256(&point.as_bytes()[1..]);
    let mut addr21 = [0u8; 21];
    addr21[0] = 0x41;
    addr21[1..].copy_from_slice(&hash[12..]);
    crate::chain::tron::addr21_to_base58check(&addr21)
}

/// Load a Tron signer from CLI flag, env var, or OS keyring.
///
/// Priority: `--wallet` flag → `ZEROX_TRON_PRIVATE_KEY` env → OS keyring.
/// (Config-file fallback is wired in once `AppConfig.wallet.tron` exists.)
pub fn load_tron_signer(
    config: &AppConfig,
    cli_wallet: Option<&str>,
) -> Result<TronSigner, CliError> {
    let _ = config; // config-field fallback added in the config task
    let key = if let Some(wallet_arg) = cli_wallet {
        wallet_arg.to_string()
    } else if let Ok(env_key) = std::env::var("ZEROX_TRON_PRIVATE_KEY") {
        env_key
    } else if let Some(keyring_key) =
        crate::wallet::keyring_store::get(crate::wallet::keyring_store::keys::WALLET_TRON)
            .unwrap_or(None)
    {
        keyring_key
    } else {
        return Err(CliError::Wallet {
            code: ErrorCode::WalletNotFound,
            message: "No Tron wallet configured. Set via --wallet, ZEROX_TRON_PRIVATE_KEY env var, or 'config set wallet.tron <key>'".into(),
        });
    };

    let hex_str = key.strip_prefix("0x").or_else(|| key.strip_prefix("0X")).unwrap_or(&key);
    let bytes = hex::decode(hex_str).map_err(|e| CliError::Wallet {
        code: ErrorCode::WalletInvalid,
        message: format!("Invalid Tron private key (expected hex): {e}"),
    })?;
    let signing_key = SigningKey::from_slice(&bytes).map_err(|e| CliError::Wallet {
        code: ErrorCode::WalletInvalid,
        message: format!("Invalid Tron private key: {e}"),
    })?;
    let address = derive_address(&signing_key);
    Ok(TronSigner { signing_key, address })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::AppConfig;

    // Hardhat account #0 private key (also a valid secp256k1 key for Tron).
    const PK: &str = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

    #[test]
    fn test_load_and_derive_address() {
        let signer = load_tron_signer(&AppConfig::default(), Some(PK)).unwrap();
        // Known vector: Hardhat #0 key → EVM 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
        // → Tron base58check(0x41 ‖ those 20 bytes), computed independently.
        assert_eq!(signer.address(), "TYBNgWfhGuNzdLtjKtxXTfskAhTbMcqbaG");
    }

    #[test]
    fn test_sign_txid_is_65_bytes() {
        let signer = load_tron_signer(&AppConfig::default(), Some(PK)).unwrap();
        let sig = signer.sign_txid(&[7u8; 32]);
        assert_eq!(sig.len(), 65);
        assert!(sig[64] == 0 || sig[64] == 1, "recovery id must be 0 or 1");
    }

    #[test]
    fn test_no_wallet_errors() {
        assert!(load_tron_signer(&AppConfig::default(), None).is_err());
    }
}
