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

/// `--pay` was used on a non-EVM chain. The agent gateway only proxies the EVM
/// AllowanceHolder price/quote.
pub fn pay_requires_evm() -> CliError {
    CliError::Api {
        code: ErrorCode::InputInvalid,
        message: "--pay (agent payments) is only supported on EVM chains".into(),
        status: None,
        details: None,
        suggestion: Some(
            "The agent gateway proxies the EVM AllowanceHolder swap only. Use an EVM chain like --chain base."
                .into(),
        ),
    }
}

/// `--pay` combined with a flag it can't coexist with (e.g. `--gasless`).
pub fn pay_incompatible(flag: &str) -> CliError {
    CliError::Api {
        code: ErrorCode::InputInvalid,
        message: format!("--pay cannot be combined with {flag}"),
        status: None,
        details: None,
        suggestion: Some(
            "Agent payments route through the EVM AllowanceHolder gateway, which doesn't support that mode."
                .into(),
        ),
    }
}

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
