//! Tron (TVM) address codec, transaction building, signing, and broadcast.
//! Tron is supported in cross-chain swaps only.

use crate::error::{CliError, ErrorCode};
use sha2::{Digest, Sha256};

/// Tron mainnet address version byte. Every base58check Tron address decodes
/// to `0x41 ++ 20-byte-address ++ 4-byte-checksum`.
const TRON_VERSION_BYTE: u8 = 0x41;

fn sha256d(bytes: &[u8]) -> [u8; 32] {
    let first = Sha256::digest(bytes);
    let second = Sha256::digest(first);
    let mut out = [0u8; 32];
    out.copy_from_slice(&second);
    out
}

fn invalid(msg: impl Into<String>) -> CliError {
    CliError::Api {
        code: ErrorCode::InputInvalid,
        message: msg.into(),
        status: None,
        details: None,
        suggestion: Some(
            "Use a base58check Tron address starting with 'T', e.g. TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t".into(),
        ),
    }
}

/// Decode a base58check Tron address (`T…`) into its 21-byte `0x41`-prefixed
/// form. Validates length, version byte, and the 4-byte double-SHA256 checksum.
pub fn base58check_to_21(addr: &str) -> Result<[u8; 21], CliError> {
    let raw = bs58::decode(addr)
        .into_vec()
        .map_err(|_| invalid(format!("'{addr}' is not valid base58")))?;
    if raw.len() != 25 {
        return Err(invalid(format!("'{addr}' is not a 25-byte Tron address")));
    }
    let (payload, checksum) = raw.split_at(21);
    if payload[0] != TRON_VERSION_BYTE {
        return Err(invalid(format!("'{addr}' has wrong Tron version byte")));
    }
    let expected = &sha256d(payload)[..4];
    if expected != checksum {
        return Err(invalid(format!("'{addr}' has an invalid checksum")));
    }
    let mut out = [0u8; 21];
    out.copy_from_slice(payload);
    Ok(out)
}

/// Encode a 21-byte `0x41`-prefixed address back to a base58check `T…` string.
pub fn addr21_to_base58check(addr: &[u8; 21]) -> String {
    let checksum = &sha256d(addr)[..4];
    let mut full = Vec::with_capacity(25);
    full.extend_from_slice(addr);
    full.extend_from_slice(checksum);
    bs58::encode(full).into_string()
}

/// True iff `addr` is a structurally valid base58check Tron address.
pub fn is_valid_tron_address(addr: &str) -> bool {
    base58check_to_21(addr).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // USDT-TRC20 contract address — a known-good base58check vector.
    const USDT: &str = "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t";

    #[test]
    fn test_base58check_roundtrip() {
        let bytes = base58check_to_21(USDT).expect("decode");
        assert_eq!(bytes[0], 0x41, "version byte must be 0x41");
        assert_eq!(addr21_to_base58check(&bytes), USDT);
    }

    #[test]
    fn test_rejects_bad_checksum() {
        // Flip the last character to corrupt the checksum.
        let bad = "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6X";
        assert!(base58check_to_21(bad).is_err());
        assert!(!is_valid_tron_address(bad));
    }

    #[test]
    fn test_rejects_evm_shaped() {
        assert!(!is_valid_tron_address("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"));
    }

    #[test]
    fn test_is_valid_true() {
        assert!(is_valid_tron_address(USDT));
    }
}
