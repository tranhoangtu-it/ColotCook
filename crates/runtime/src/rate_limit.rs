//! Token-bucket rate limiter for API calls.
//!
//! Prevents accidental cost spikes by limiting the rate of API requests.
//! Configurable via environment variables or runtime config.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Default maximum requests per minute.
const DEFAULT_MAX_REQUESTS_PER_MINUTE: u32 = 60;
/// Default maximum tokens per minute (input + output).
const DEFAULT_MAX_TOKENS_PER_MINUTE: u32 = 1_000_000;
/// Environment variable for custom request rate limit.
const RATE_LIMIT_RPM_ENV: &str = "COLOTCOOK_RATE_LIMIT_RPM";
/// Environment variable for custom token rate limit.
const RATE_LIMIT_TPM_ENV: &str = "COLOTCOOK_RATE_LIMIT_TPM";

/// Rate limiter configuration.
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    pub max_requests_per_minute: u32,
    pub max_tokens_per_minute: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            max_requests_per_minute: std::env::var(RATE_LIMIT_RPM_ENV)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(DEFAULT_MAX_REQUESTS_PER_MINUTE),
            max_tokens_per_minute: std::env::var(RATE_LIMIT_TPM_ENV)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(DEFAULT_MAX_TOKENS_PER_MINUTE),
        }
    }
}

/// Token-bucket based rate limiter.
#[derive(Debug)]
pub struct RateLimiter {
    config: RateLimitConfig,
    state: Mutex<RateLimiterState>,
}

#[derive(Debug)]
struct RateLimiterState {
    request_timestamps: Vec<Instant>,
    token_window_start: Instant,
    tokens_in_window: u32,
}

/// Result of a rate limit check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RateLimitDecision {
    /// Request is allowed.
    Allowed,
    /// Request is denied; wait for the specified duration.
    RetryAfter(Duration),
}

impl RateLimiter {
    #[must_use]
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            state: Mutex::new(RateLimiterState {
                request_timestamps: Vec::new(),
                token_window_start: Instant::now(),
                tokens_in_window: 0,
            }),
        }
    }

    /// Check if a request with the given token count is allowed.
    pub fn check_request(&self, estimated_tokens: u32) -> RateLimitDecision {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        let window = Duration::from_secs(60);

        // Clean up old request timestamps outside the window.
        state
            .request_timestamps
            .retain(|ts| now.duration_since(*ts) < window);

        // Check request count limit.
        if state.request_timestamps.len() >= self.config.max_requests_per_minute as usize {
            let oldest = state.request_timestamps[0];
            let retry_after = window.saturating_sub(now.duration_since(oldest));
            return RateLimitDecision::RetryAfter(retry_after);
        }

        // Reset token window if needed.
        if now.duration_since(state.token_window_start) >= window {
            state.token_window_start = now;
            state.tokens_in_window = 0;
        }

        // Check token limit.
        if state.tokens_in_window + estimated_tokens > self.config.max_tokens_per_minute {
            let elapsed = now.duration_since(state.token_window_start);
            let retry_after = window.saturating_sub(elapsed);
            return RateLimitDecision::RetryAfter(retry_after);
        }

        // Record the request.
        state.request_timestamps.push(now);
        state.tokens_in_window += estimated_tokens;

        RateLimitDecision::Allowed
    }

    /// Record actual token usage after a request completes.
    pub fn record_usage(&self, actual_tokens: u32) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Adjust if actual usage differs from estimate.
        // This is a simple "add the actual" approach.
        state.tokens_in_window = state.tokens_in_window.saturating_add(actual_tokens);
    }

    /// Get the current rate limit configuration.
    #[must_use]
    pub fn config(&self) -> &RateLimitConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_requests_within_limit() {
        let limiter = RateLimiter::new(RateLimitConfig {
            max_requests_per_minute: 5,
            max_tokens_per_minute: 10000,
        });
        for _ in 0..5 {
            assert_eq!(limiter.check_request(100), RateLimitDecision::Allowed);
        }
    }

    #[test]
    fn denies_requests_over_limit() {
        let limiter = RateLimiter::new(RateLimitConfig {
            max_requests_per_minute: 2,
            max_tokens_per_minute: 10000,
        });
        assert_eq!(limiter.check_request(100), RateLimitDecision::Allowed);
        assert_eq!(limiter.check_request(100), RateLimitDecision::Allowed);
        assert!(matches!(
            limiter.check_request(100),
            RateLimitDecision::RetryAfter(_)
        ));
    }

    #[test]
    fn denies_requests_over_token_limit() {
        let limiter = RateLimiter::new(RateLimitConfig {
            max_requests_per_minute: 100,
            max_tokens_per_minute: 500,
        });
        assert_eq!(limiter.check_request(300), RateLimitDecision::Allowed);
        assert_eq!(limiter.check_request(100), RateLimitDecision::Allowed);
        // 400 used, requesting 200 more would exceed 500
        assert!(matches!(
            limiter.check_request(200),
            RateLimitDecision::RetryAfter(_)
        ));
    }

    #[test]
    fn default_config_uses_environment_or_fallback() {
        let config = RateLimitConfig::default();
        assert!(config.max_requests_per_minute > 0);
        assert!(config.max_tokens_per_minute > 0);
    }
}
