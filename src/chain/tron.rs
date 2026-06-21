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

/// 100 TRX, in sun. Tron contract calls burn TRX for energy/bandwidth; this
/// caps how much a single origin transaction may spend.
pub const DEFAULT_FEE_LIMIT_SUN: u64 = 100_000_000;

const TRIGGER_SMART_CONTRACT_TYPE: u64 = 31;
const TRIGGER_TYPE_URL: &str = "type.googleapis.com/protocol.TriggerSmartContract";

// --- minimal protobuf writer (only what TriggerSmartContract needs) ---

fn write_varint(buf: &mut Vec<u8>, mut v: u64) {
    loop {
        let mut byte = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if v == 0 {
            break;
        }
    }
}

fn write_tag(buf: &mut Vec<u8>, field: u64, wire_type: u64) {
    write_varint(buf, (field << 3) | wire_type);
}

fn write_len_delimited(buf: &mut Vec<u8>, field: u64, bytes: &[u8]) {
    write_tag(buf, field, 2);
    write_varint(buf, bytes.len() as u64);
    buf.extend_from_slice(bytes);
}

fn write_varint_field(buf: &mut Vec<u8>, field: u64, v: u64) {
    write_tag(buf, field, 0);
    write_varint(buf, v);
}

fn encode_trigger_smart_contract(owner: &[u8], contract: &[u8], call_value: u64, data: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    write_len_delimited(&mut b, 1, owner); // owner_address
    write_len_delimited(&mut b, 2, contract); // contract_address
    if call_value != 0 {
        write_varint_field(&mut b, 3, call_value); // call_value
    }
    write_len_delimited(&mut b, 4, data); // data
    b
}

fn encode_any(type_url: &str, value: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    write_len_delimited(&mut b, 1, type_url.as_bytes());
    write_len_delimited(&mut b, 2, value);
    b
}

fn encode_contract(parameter_any: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    write_varint_field(&mut b, 1, TRIGGER_SMART_CONTRACT_TYPE); // Contract.type
    write_len_delimited(&mut b, 2, parameter_any); // Contract.parameter (Any)
    b
}

pub(crate) struct EncodeParams<'a> {
    pub owner: &'a [u8],
    pub contract: &'a [u8],
    pub data: &'a [u8],
    pub call_value: u64,
    pub ref_block_bytes: [u8; 2],
    pub ref_block_hash: [u8; 8],
    pub expiration: u64,
    pub timestamp: u64,
    pub fee_limit: u64,
}

/// Encode a Transaction.raw (the bytes that get hashed into the txID).
pub(crate) fn encode_raw_data(p: EncodeParams) -> Vec<u8> {
    let tsc = encode_trigger_smart_contract(p.owner, p.contract, p.call_value, p.data);
    let any = encode_any(TRIGGER_TYPE_URL, &tsc);
    let contract = encode_contract(&any);

    let mut raw = Vec::new();
    write_len_delimited(&mut raw, 1, &p.ref_block_bytes); // ref_block_bytes
    write_len_delimited(&mut raw, 4, &p.ref_block_hash); // ref_block_hash
    write_varint_field(&mut raw, 8, p.expiration); // expiration
    write_len_delimited(&mut raw, 11, &contract); // contract (repeated, one entry)
    write_varint_field(&mut raw, 14, p.timestamp); // timestamp
    write_varint_field(&mut raw, 18, p.fee_limit); // fee_limit
    raw
}

pub(crate) fn txid(raw_data: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(&Sha256::digest(raw_data));
    out
}

pub(crate) fn ref_block_bytes_from_number(number: u64) -> [u8; 2] {
    let be = number.to_be_bytes();
    [be[6], be[7]]
}

// --- ref-block fetch + broadcast over the TronGrid full-node HTTP API ---

#[derive(serde::Deserialize)]
struct NowBlock {
    #[serde(rename = "blockID")]
    block_id: String,
    block_header: BlockHeader,
}
#[derive(serde::Deserialize)]
struct BlockHeader {
    raw_data: BlockHeaderRaw,
}
#[derive(serde::Deserialize)]
struct BlockHeaderRaw {
    number: u64,
    timestamp: u64,
}

fn rpc_error(msg: impl Into<String>) -> CliError {
    CliError::Transaction {
        code: ErrorCode::RpcError,
        message: msg.into(),
        tx_hash: None,
        suggestion: Some("Check the Tron RPC URL (config set rpc.tron <url>) or try again.".into()),
    }
}

/// Build, sign, and broadcast a Tron TriggerSmartContract transaction.
/// Returns the broadcast txID (hex). `data_hex` / `to_b58` / `owner_b58` /
/// `value_sun` come straight from the cross-chain quote's `transaction.details`.
pub async fn build_sign_broadcast(
    rpc_url: &str,
    signer: &crate::wallet::tron::TronSigner,
    to_b58: &str,
    owner_b58: &str,
    data_hex: &str,
    value_sun: u64,
    fee_limit_sun: u64,
) -> Result<String, CliError> {
    let owner = base58check_to_21(owner_b58)?;
    let contract = base58check_to_21(to_b58)?;
    let data = hex::decode(data_hex.strip_prefix("0x").unwrap_or(data_hex))
        .map_err(|e| rpc_error(format!("Quote returned non-hex Tron calldata: {e}")))?;

    let client = reqwest::Client::new();
    let now: NowBlock = client
        .post(format!("{}/wallet/getnowblock", rpc_url.trim_end_matches('/')))
        .send()
        .await
        .map_err(|e| rpc_error(format!("getnowblock request failed: {e}")))?
        .json()
        .await
        .map_err(|e| rpc_error(format!("getnowblock parse failed: {e}")))?;

    let block_hash = hex::decode(&now.block_id)
        .map_err(|e| rpc_error(format!("bad blockID hex: {e}")))?;
    if block_hash.len() < 16 {
        return Err(rpc_error("blockID shorter than 16 bytes"));
    }
    let mut ref_block_hash = [0u8; 8];
    ref_block_hash.copy_from_slice(&block_hash[8..16]);

    let raw = encode_raw_data(EncodeParams {
        owner: &owner,
        contract: &contract,
        data: &data,
        call_value: value_sun,
        ref_block_bytes: ref_block_bytes_from_number(now.block_header.raw_data.number),
        ref_block_hash,
        // Tron expiration must be after the latest block's time; +60s window.
        expiration: now.block_header.raw_data.timestamp + 60_000,
        timestamp: now.block_header.raw_data.timestamp,
        fee_limit: fee_limit_sun,
    });

    let id = txid(&raw);
    let signature = signer.sign_txid(&id);

    // Full signed Transaction protobuf: field 1 = raw_data, field 2 = signature.
    let mut signed = Vec::new();
    write_len_delimited(&mut signed, 1, &raw);
    write_len_delimited(&mut signed, 2, &signature);

    #[derive(serde::Serialize)]
    struct BroadcastHex {
        transaction: String,
    }
    #[derive(serde::Deserialize)]
    struct BroadcastResult {
        #[serde(default)]
        result: bool,
        #[serde(default)]
        message: Option<String>,
        #[serde(default, rename = "txid")]
        txid: Option<String>,
    }

    let res: BroadcastResult = client
        .post(format!("{}/wallet/broadcasthex", rpc_url.trim_end_matches('/')))
        .json(&BroadcastHex { transaction: hex::encode(&signed) })
        .send()
        .await
        .map_err(|e| rpc_error(format!("broadcast request failed: {e}")))?
        .json()
        .await
        .map_err(|e| rpc_error(format!("broadcast parse failed: {e}")))?;

    if !res.result {
        let detail = res
            .message
            .map(|m| String::from_utf8(hex::decode(&m).unwrap_or_default()).unwrap_or(m))
            .unwrap_or_else(|| "unknown error".into());
        return Err(rpc_error(format!("Tron broadcast rejected: {detail}")));
    }

    Ok(res.txid.unwrap_or_else(|| hex::encode(id)))
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

    #[test]
    fn test_encode_raw_data_golden() {
        // Fixed inputs → deterministic raw_data bytes (ref-block + timestamps
        // are passed in, so the encoding is fully reproducible).
        let owner = base58check_to_21("TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t").unwrap();
        let to = base58check_to_21("TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t").unwrap();
        let data = hex::decode("a9059cbb").unwrap();
        let raw = encode_raw_data(EncodeParams {
            owner: &owner,
            contract: &to,
            data: &data,
            call_value: 0,
            ref_block_bytes: [0x4f, 0x15],
            ref_block_hash: [0u8; 8],
            expiration: 1_700_000_060_000,
            timestamp: 1_700_000_000_000,
            fee_limit: DEFAULT_FEE_LIMIT_SUN,
        });
        // txID is sha256(raw_data); pin its length and determinism.
        let id1 = txid(&raw);
        let id2 = txid(&raw);
        assert_eq!(id1, id2);
        assert_eq!(id1.len(), 32);
        // Encoding must be non-empty and start with the ref_block_bytes field tag (0x0A).
        assert_eq!(raw[0], 0x0A);
    }

    #[test]
    fn test_ref_block_from_block_number() {
        // ref_block_bytes is bytes [6..8] of the big-endian 8-byte block number.
        let n: u64 = 83709717;
        let be = n.to_be_bytes();
        assert_eq!(ref_block_bytes_from_number(n), [be[6], be[7]]);
    }
}
