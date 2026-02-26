use std::future::Future;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl RetryPolicy {
    pub fn backoff_delay(&self, retry_index: u32) -> Duration {
        if self.base_delay.is_zero() {
            return Duration::from_millis(0);
        }
        let shift = retry_index.min(16);
        let multiplier = 1u32 << shift;
        self.base_delay
            .checked_mul(multiplier)
            .unwrap_or(self.max_delay)
            .min(self.max_delay)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CircuitBreakerPolicy {
    pub failure_threshold: u32,
    pub cooldown: Duration,
}

#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    policy: CircuitBreakerPolicy,
    consecutive_failures: u32,
    open_until: Option<Instant>,
}

impl CircuitBreaker {
    pub fn new(policy: CircuitBreakerPolicy) -> Self {
        Self {
            policy,
            consecutive_failures: 0,
            open_until: None,
        }
    }

    pub async fn wait_if_open(&mut self, label: &str) {
        if let Some(until) = self.open_until {
            let now = Instant::now();
            if until > now {
                let wait_for = until.duration_since(now);
                tracing::warn!(
                    "{label}: circuit breaker open, waiting {}ms",
                    wait_for.as_millis()
                );
                tokio::time::sleep(wait_for).await;
            }
            self.open_until = None;
            self.consecutive_failures = 0;
        }
    }

    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.open_until = None;
    }

    pub fn record_failure(&mut self) -> Option<Duration> {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if self.policy.failure_threshold == 0
            || self.consecutive_failures < self.policy.failure_threshold
        {
            return None;
        }
        self.consecutive_failures = 0;
        let cooldown = self.policy.cooldown;
        self.open_until = Some(Instant::now() + cooldown);
        Some(cooldown)
    }
}

pub async fn run_with_retry<T, F, Fut>(
    label: &str,
    retry_policy: RetryPolicy,
    circuit_breaker: &mut CircuitBreaker,
    mut operation: F,
) -> Result<T, String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, String>>,
{
    for attempt in 0..=retry_policy.max_retries {
        circuit_breaker.wait_if_open(label).await;

        match operation().await {
            Ok(value) => {
                circuit_breaker.record_success();
                return Ok(value);
            }
            Err(err) => {
                let opened_cooldown = circuit_breaker.record_failure();
                if attempt >= retry_policy.max_retries {
                    return Err(err);
                }

                if let Some(cooldown) = opened_cooldown {
                    tracing::warn!(
                        "{label}: circuit opened for {}ms after consecutive failures",
                        cooldown.as_millis()
                    );
                }

                let delay = retry_policy.backoff_delay(attempt);
                tracing::warn!(
                    "{label}: attempt {} failed: {err}; retrying in {}ms",
                    attempt + 1,
                    delay.as_millis()
                );
                tokio::time::sleep(delay).await;
            }
        }
    }

    Err(format!(
        "{label}: retry loop exhausted unexpectedly without result"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_delay_caps_at_max() {
        let policy = RetryPolicy {
            max_retries: 5,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(350),
        };

        assert_eq!(policy.backoff_delay(0), Duration::from_millis(100));
        assert_eq!(policy.backoff_delay(1), Duration::from_millis(200));
        assert_eq!(policy.backoff_delay(2), Duration::from_millis(350));
    }

    #[test]
    fn circuit_breaker_opens_on_threshold() {
        let mut breaker = CircuitBreaker::new(CircuitBreakerPolicy {
            failure_threshold: 2,
            cooldown: Duration::from_millis(50),
        });

        assert!(breaker.record_failure().is_none());
        let opened = breaker.record_failure();
        assert_eq!(opened, Some(Duration::from_millis(50)));
    }
}
