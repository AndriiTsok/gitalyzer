//! Shared HTTP retry plumbing (RFC 0003 R9): up to 3 attempts total for
//! transient failures (HTTP 429/5xx, connect/timeout errors), exponential
//! backoff with cheap jitter, honoring `Retry-After`. Request timing is
//! logged at debug level — never headers or bodies (RFC 0007 R7).

use std::time::{Duration, Instant};

use reqwest::{RequestBuilder, StatusCode};

use super::ProviderError;

/// Retry budget and pacing; injectable so tests run in milliseconds.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Total attempts, including the first (RFC 0003 R9: 3).
    pub max_attempts: u32,
    /// Base delay doubled per retry.
    pub base_delay: Duration,
    /// Hard cap on any single delay (also caps `Retry-After`).
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
        }
    }
}

impl RetryPolicy {
    /// Delay before retry number `attempt` (0-based), preferring the
    /// server-provided `Retry-After` when present.
    fn delay(&self, attempt: u32, retry_after: Option<Duration>) -> Duration {
        if let Some(after) = retry_after {
            return after.min(self.max_delay);
        }
        let exponential = self.base_delay.saturating_mul(1u32 << attempt.min(16));
        (exponential + jitter()).min(self.max_delay)
    }
}

/// Cheap std-only jitter (0–250 ms) derived from the clock's sub-second
/// noise; good enough to de-synchronize concurrent batch retries.
fn jitter() -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    Duration::from_millis(u64::from(nanos % 250))
}

/// Send a request (rebuilt per attempt via `build`) under the retry policy.
/// Returns the final status and body text; non-transient error statuses are
/// returned to the caller for interpretation, not retried.
pub(crate) async fn send_with_retry(
    build: impl Fn() -> RequestBuilder,
    policy: &RetryPolicy,
    provider: &'static str,
) -> Result<(StatusCode, String), ProviderError> {
    let mut attempt: u32 = 0;
    loop {
        let started = Instant::now();
        match build().send().await {
            Ok(response) => {
                let status = response.status();
                let retry_after = response
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .map(Duration::from_secs);
                let body = response.text().await.unwrap_or_default();
                tracing::debug!(
                    provider,
                    status = status.as_u16(),
                    attempt = attempt + 1,
                    elapsed_ms = %started.elapsed().as_millis(),
                    "provider request completed"
                );

                let transient = status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error();
                if !transient {
                    return Ok((status, body));
                }
                if attempt + 1 >= policy.max_attempts {
                    return Err(ProviderError::RetriesExhausted {
                        provider,
                        attempts: policy.max_attempts,
                        status: status.as_u16(),
                    });
                }
                tokio::time::sleep(policy.delay(attempt, retry_after)).await;
                attempt += 1;
            }
            Err(error) => {
                let transient = error.is_timeout() || error.is_connect();
                tracing::debug!(
                    provider,
                    attempt = attempt + 1,
                    elapsed_ms = %started.elapsed().as_millis(),
                    transient,
                    "provider request errored: {error}"
                );
                if !transient || attempt + 1 >= policy.max_attempts {
                    return Err(ProviderError::Transport {
                        provider,
                        source: error,
                    });
                }
                tokio::time::sleep(policy.delay(attempt, None)).await;
                attempt += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_wins_and_is_capped() {
        let policy = RetryPolicy {
            max_attempts: 3,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_secs(5),
        };
        assert_eq!(
            policy.delay(0, Some(Duration::from_secs(2))),
            Duration::from_secs(2)
        );
        assert_eq!(
            policy.delay(0, Some(Duration::from_secs(600))),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn backoff_grows_exponentially_up_to_the_cap() {
        let policy = RetryPolicy {
            max_attempts: 5,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(3),
        };
        assert!(policy.delay(0, None) >= Duration::from_secs(1));
        assert!(policy.delay(1, None) >= Duration::from_secs(2));
        assert_eq!(policy.delay(4, None), Duration::from_secs(3), "capped");
    }
}
