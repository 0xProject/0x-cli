use std::future::Future;
use std::time::Duration;

/// Retry an idempotent async operation up to `max_attempts` times with
/// exponential backoff (200ms → 400ms → 800ms by default). Retries on
/// **any** error, so only call this for idempotent operations where
/// re-execution is safe — RPC reads (allowance, chain_id, simulation,
/// blockhash) yes; transaction broadcasts no.
///
/// Failures between attempts are logged at `warn` so a user running with
/// `-v` can see the retries.
pub async fn with_retry<F, Fut, T, E>(max_attempts: u32, mut f: F) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    debug_assert!(max_attempts >= 1, "max_attempts must be at least 1");
    let mut delay = Duration::from_millis(200);
    let mut attempt: u32 = 1;
    loop {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if attempt >= max_attempts {
                    return Err(e);
                }
                tracing::warn!(
                    attempt,
                    max_attempts,
                    error = %e,
                    "RPC call failed; retrying after backoff"
                );
                tokio::time::sleep(delay).await;
                delay = delay.saturating_mul(2);
                attempt += 1;
            }
        }
    }
}

/// Standard retry budget for idempotent RPC reads: 3 total attempts
/// (initial + 2 retries), 200ms → 400ms backoff before each retry. Total
/// worst-case latency added on top of the call: ~600ms.
pub const DEFAULT_RPC_RETRIES: u32 = 3;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn succeeds_on_first_try() {
        let calls = Arc::new(AtomicU32::new(0));
        let calls_clone = calls.clone();
        let res: Result<i32, &'static str> = with_retry(3, || {
            let c = calls_clone.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(42)
            }
        })
        .await;
        assert_eq!(res, Ok(42));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retries_then_succeeds() {
        let calls = Arc::new(AtomicU32::new(0));
        let calls_clone = calls.clone();
        let res: Result<&'static str, &'static str> = with_retry(3, || {
            let c = calls_clone.clone();
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst) + 1;
                if n < 3 {
                    Err("transient")
                } else {
                    Ok("done")
                }
            }
        })
        .await;
        assert_eq!(res, Ok("done"));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn gives_up_after_max_attempts() {
        let calls = Arc::new(AtomicU32::new(0));
        let calls_clone = calls.clone();
        let res: Result<(), &'static str> = with_retry(3, || {
            let c = calls_clone.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err("always fails")
            }
        })
        .await;
        assert_eq!(res, Err("always fails"));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }
}
