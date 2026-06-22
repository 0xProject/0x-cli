pub mod chains;
pub mod config_cmd;
pub mod cross_chain;
pub mod gasless;
pub mod price;
pub mod skill;
pub mod solana_swap;
pub mod status;
pub mod swap;

use crate::error::{CliError, ErrorCode};

/// Error for `--buy-amount` (exact-out) used where only exact-in is supported
/// (Solana, gasless, cross-chain). `context` names the unsupported path.
pub fn exact_out_unsupported(context: &str) -> CliError {
    CliError::Api {
        code: ErrorCode::InputInvalid,
        message: format!("--buy-amount (exact-out) is not supported for {context}"),
        status: None,
        details: None,
        suggestion: Some(
            "Exact-out is available for EVM same-chain swaps only. Use --amount to \
             specify the sell amount instead."
                .into(),
        ),
    }
}
