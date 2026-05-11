//! Generic "poll until terminal" helper. Backs the cross-chain bridge tracker,
//! the gasless trade-status loop, and the `0x status --poll` command. Keeps
//! all three on the same timing semantics so users / agents see consistent
//! polling behaviour and consistent timeout errors.

use crate::error::{CliError, ErrorCode};
use std::future::Future;
use std::time::Duration;

/// Polling parameters. Defaults to 5 s interval, 10 min max-elapsed — the
/// historical cross-chain / gasless behaviour. Override either field as
/// needed; e.g. `0x status` honours the user's `--poll-interval` flag.
#[derive(Debug, Clone, Copy)]
pub struct PollConfig {
    pub interval: Duration,
    pub max_elapsed: Duration,
    /// Error code to use on timeout. Cross-chain wants `BridgeTimeout`;
    /// gasless / status want `TransactionTimeout`.
    pub timeout_code: ErrorCode,
}

impl PollConfig {
    pub fn new(interval_secs: u64, max_elapsed_secs: u64, timeout_code: ErrorCode) -> Self {
        Self {
            interval: Duration::from_secs(interval_secs),
            max_elapsed: Duration::from_secs(max_elapsed_secs),
            timeout_code,
        }
    }
}

/// Repeatedly call `fetch` until either `is_terminal` returns `true` (success)
/// or `cfg.max_elapsed` has been reached (returns `CliError::Timeout` with
/// `cfg.timeout_code` and `timeout_message`).
///
/// `on_status` is invoked with `(seconds_elapsed, &latest_response)` after
/// every successful fetch — typically used to update a spinner.
pub async fn poll_until_terminal<T, F, Fut>(
    cfg: PollConfig,
    on_status: impl Fn(u64, &T),
    mut fetch: F,
    is_terminal: impl Fn(&T) -> bool,
    timeout_message: impl FnOnce() -> String,
) -> Result<T, CliError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, CliError>>,
{
    let mut elapsed = Duration::ZERO;
    loop {
        let resp = fetch().await?;
        on_status(elapsed.as_secs(), &resp);
        if is_terminal(&resp) {
            return Ok(resp);
        }
        if elapsed >= cfg.max_elapsed {
            return Err(CliError::Timeout {
                code: cfg.timeout_code,
                message: timeout_message(),
            });
        }
        tokio::time::sleep(cfg.interval).await;
        elapsed += cfg.interval;
    }
}
