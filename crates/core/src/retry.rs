use std::future::Future;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::time::sleep;
use tracing::warn;

const MAX_BACKOFF: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryDecision {
    Retry,
    Stop,
}

pub async fn retry_with_backoff<T, F, Fut>(
    attempts: usize,
    initial_delay: Duration,
    operation: &str,
    mut f: F,
) -> Result<T>
where
    F: FnMut(usize) -> Fut,
    Fut: Future<Output = Result<T>>,
{
    if attempts == 0 {
        anyhow::bail!("retry_with_backoff requires attempts >= 1");
    }

    let mut delay = initial_delay;
    for attempt in 1..=attempts {
        match f(attempt).await {
            Ok(value) => return Ok(value),
            Err(err) => {
                if attempt == attempts {
                    return Err(err).with_context(|| {
                        format!("{} failed after {} attempts", operation, attempts)
                    });
                }

                warn!(
                    "{} failed on attempt {}/{}: {}. Retrying in {:?}",
                    operation, attempt, attempts, err, delay
                );

                if !delay.is_zero() {
                    sleep(delay).await;
                }
                delay = (delay * 2).min(MAX_BACKOFF);
            }
        }
    }

    unreachable!("retry loop always returns before completion")
}

pub async fn retry_with_backoff_classified<T, F, Fut, C>(
    attempts: usize,
    initial_delay: Duration,
    operation: &str,
    mut classify: C,
    mut f: F,
) -> Result<T>
where
    F: FnMut(usize) -> Fut,
    Fut: Future<Output = Result<T>>,
    C: FnMut(&anyhow::Error) -> RetryDecision,
{
    if attempts == 0 {
        anyhow::bail!("retry_with_backoff_classified requires attempts >= 1");
    }

    let mut delay = initial_delay;
    for attempt in 1..=attempts {
        match f(attempt).await {
            Ok(value) => return Ok(value),
            Err(err) => {
                let decision = classify(&err);
                if attempt == attempts || decision == RetryDecision::Stop {
                    return Err(err).with_context(|| {
                        if decision == RetryDecision::Stop {
                            format!(
                                "{} failed with non-retryable error on attempt {}",
                                operation, attempt
                            )
                        } else {
                            format!("{} failed after {} attempts", operation, attempts)
                        }
                    });
                }

                warn!(
                    "{} failed on attempt {}/{}: {}. Retrying in {:?}",
                    operation, attempt, attempts, err, delay
                );

                if !delay.is_zero() {
                    sleep(delay).await;
                }
                delay = (delay * 2).min(MAX_BACKOFF);
            }
        }
    }

    unreachable!("retry loop always returns before completion")
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use super::{retry_with_backoff, retry_with_backoff_classified, RetryDecision};

    #[tokio::test]
    async fn returns_success_without_retry() {
        let value = retry_with_backoff(3, Duration::ZERO, "test-op", |_| async {
            Ok::<_, anyhow::Error>(42usize)
        })
        .await
        .unwrap();

        assert_eq!(value, 42);
    }

    #[tokio::test]
    async fn retries_until_success() {
        let count = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&count);

        let value = retry_with_backoff(4, Duration::ZERO, "flaky-op", move |_| {
            let counter = Arc::clone(&counter);
            async move {
                let current = counter.fetch_add(1, Ordering::SeqCst);
                if current < 2 {
                    anyhow::bail!("transient failure");
                }
                Ok::<_, anyhow::Error>("ok")
            }
        })
        .await
        .unwrap();

        assert_eq!(value, "ok");
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn returns_error_after_all_attempts() {
        let err = retry_with_backoff(2, Duration::ZERO, "broken-op", |_| async {
            Err::<(), _>(anyhow::anyhow!("still failing"))
        })
        .await
        .unwrap_err();

        assert!(err
            .to_string()
            .contains("broken-op failed after 2 attempts"));
    }

    #[tokio::test]
    async fn stops_on_non_retryable_error() {
        let err = retry_with_backoff_classified(
            3,
            Duration::ZERO,
            "non-retryable-op",
            |_| RetryDecision::Stop,
            |_| async { Err::<(), _>(anyhow::anyhow!("invalid input")) },
        )
        .await
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("failed with non-retryable error on attempt 1"),
            "unexpected error: {err}"
        );
    }
}
