use crate::error::{CliError, ErrorDetail};
use chrono::Utc;
use serde::Serialize;

/// The JSON envelope wrapping every CLI response.
/// This is the contract AI agents rely on.
#[derive(Debug, Serialize)]
pub struct CliOutput<T: Serialize> {
    /// Envelope schema version (bump on breaking changes)
    pub version: &'static str,
    /// Command that produced this output
    pub command: String,
    /// ISO 8601 timestamp
    pub timestamp: String,
    /// Wall-clock milliseconds elapsed
    pub duration_ms: u64,
    /// Process exit code
    pub exit_code: i32,
    /// "success" or "error"
    pub status: &'static str,
    /// Present on success
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    /// Present on error
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorDetail>,
    /// Non-fatal warnings (always an array)
    pub warnings: Vec<Warning>,
    /// Tracking and context metadata
    pub metadata: Metadata,
}

#[derive(Debug, Clone, Serialize)]
pub struct Warning {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Metadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_name: Option<String>,
    pub api_version: &'static str,
    /// 0x request tracking ID from API responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zid: Option<String>,
    /// Agent-payment settlement, present only when the command paid per
    /// request via `--pay` (x402 / MPP) instead of an API key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment: Option<crate::payment::PaymentReceipt>,
}

impl Default for Metadata {
    fn default() -> Self {
        Self {
            chain_id: None,
            chain_name: None,
            api_version: "v2",
            zid: None,
            payment: None,
        }
    }
}

impl Metadata {
    /// Build metadata for a chain-scoped command. Use this anywhere a command
    /// would otherwise repeat
    /// `Metadata { chain_id: ..., chain_name: ..., ..Default::default() }`.
    pub fn for_chain(chain: &crate::chain::ChainInfo) -> Self {
        Self {
            chain_id: chain.numeric_id(),
            chain_name: Some(chain.display_name.to_string()),
            ..Default::default()
        }
    }
}

impl<T: Serialize> CliOutput<T> {
    /// Build a success envelope. `exit_code` is what the process will actually
    /// return — used by callers like the swap flow to report 25 (needs
    /// confirmation), 30 (dry-run), or 11 (final state non-success) so the
    /// envelope's `exit_code` field doesn't lie to downstream agents.
    pub fn success(
        command: &str,
        data: T,
        duration_ms: u64,
        exit_code: i32,
        metadata: Metadata,
    ) -> Self {
        Self {
            version: "1",
            command: command.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            duration_ms,
            exit_code,
            status: "success",
            data: Some(data),
            error: None,
            warnings: Vec::new(),
            metadata,
        }
    }

    pub fn with_warnings(mut self, warnings: Vec<Warning>) -> Self {
        self.warnings = warnings;
        self
    }
}

impl CliOutput<serde_json::Value> {
    pub fn error(command: &str, err: &CliError, duration_ms: u64, metadata: Metadata) -> Self {
        let detail = ErrorDetail::from(err);
        Self {
            version: "1",
            command: command.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            duration_ms,
            exit_code: err.exit_code(),
            status: "error",
            data: None,
            error: Some(detail),
            warnings: Vec::new(),
            metadata,
        }
    }
}
