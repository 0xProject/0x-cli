//! OS keyring access for wallet secrets. Wraps the `keyring` crate with our
//! error type and a stable service name. All callers should go through this
//! module rather than touching `keyring::Entry` directly.
//!
//! Conventions:
//! - Service: `"0x-cli"`
//! - User: a short stable key like `"wallet.evm"` or `"wallet.solana"`.
//!
//! In `cfg(test)` builds, every function short-circuits without touching the
//! real OS keyring so tests stay hermetic regardless of the dev machine's
//! credential store.

use crate::error::CliError;

/// Keys we store in the OS keyring.
pub mod keys {
    pub const WALLET_EVM: &str = "wallet.evm";
    pub const WALLET_SOLANA: &str = "wallet.solana";
    pub const WALLET_TRON: &str = "wallet.tron";
}

#[cfg(not(test))]
mod real {
    use super::*;
    use crate::error::ErrorCode;

    const SERVICE: &str = "0x-cli";

    pub fn get(key: &str) -> Result<Option<String>, CliError> {
        let entry = keyring::Entry::new(SERVICE, key).map_err(map_err)?;
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(map_err(e)),
        }
    }

    pub fn set(key: &str, value: &str) -> Result<(), CliError> {
        let entry = keyring::Entry::new(SERVICE, key).map_err(map_err)?;
        entry.set_password(value).map_err(map_err)
    }

    pub fn delete(key: &str) -> Result<(), CliError> {
        let entry = keyring::Entry::new(SERVICE, key).map_err(map_err)?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(map_err(e)),
        }
    }

    fn map_err(e: keyring::Error) -> CliError {
        CliError::Wallet {
            code: ErrorCode::KeyringUnavailable,
            message: format!(
                "OS keyring error: {e}. Use --plaintext on `config set`, or set ZEROX_EVM_PRIVATE_KEY / ZEROX_SOLANA_KEYPAIR."
            ),
        }
    }
}

/// Fetch a secret from the keyring. Returns `Ok(None)` when the entry doesn't
/// exist; `Err` only on actual keyring failures (denied access, keyring
/// daemon unavailable, etc.).
#[cfg(not(test))]
pub fn get(key: &str) -> Result<Option<String>, CliError> {
    real::get(key)
}

/// Store a secret in the keyring, overwriting any existing entry.
#[cfg(not(test))]
pub fn set(key: &str, value: &str) -> Result<(), CliError> {
    real::set(key, value)
}

/// Delete a keyring entry. `Ok(())` if the entry didn't exist.
#[cfg(not(test))]
pub fn delete(key: &str) -> Result<(), CliError> {
    real::delete(key)
}

// Test-mode stubs: short-circuit without touching the real OS keyring so
// `cargo test` is hermetic regardless of the developer's credential store.
#[cfg(test)]
pub fn get(_key: &str) -> Result<Option<String>, CliError> {
    Ok(None)
}
#[cfg(test)]
pub fn set(_key: &str, _value: &str) -> Result<(), CliError> {
    Ok(())
}
#[cfg(test)]
pub fn delete(_key: &str) -> Result<(), CliError> {
    Ok(())
}
