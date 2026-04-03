use std::env::VarError;
use std::fmt::{Display, Formatter};
use std::time::Duration;

/// Error type for all API-layer failures, including auth, HTTP, JSON, and retry exhaustion.
#[derive(Debug)]
pub enum ApiError {
    /// Required credential environment variables are absent for the given provider.
    MissingCredentials {
        provider: &'static str,
        env_vars: &'static [&'static str],
    },
    /// A saved OAuth token has expired with no refresh token available.
    ExpiredOAuthToken,
    /// Authentication was rejected by the remote server.
    Auth(String),
    /// The credential environment variable exists but could not be read.
    InvalidApiKeyEnv(VarError),
    /// Underlying HTTP transport error from `reqwest`.
    Http(reqwest::Error),
    /// I/O error encountered while reading credentials or cache files.
    Io(std::io::Error),
    /// JSON serialization/deserialization failure.
    Json(serde_json::Error),
    /// The API returned a non-2xx HTTP status code.
    Api {
        status: reqwest::StatusCode,
        error_type: Option<String>,
        message: Option<String>,
        body: String,
        retryable: bool,
    },
    /// All retry attempts failed; wraps the last underlying error.
    RetriesExhausted {
        attempts: u32,
        last_error: Box<ApiError>,
    },
    /// An SSE frame did not conform to the expected format.
    InvalidSseFrame(&'static str),
    /// Exponential backoff delay calculation overflowed for the given attempt.
    BackoffOverflow {
        attempt: u32,
        base_delay: Duration,
    },
}

impl ApiError {
    /// Constructs a [`ApiError::MissingCredentials`] for the given provider and env-var list.
    #[must_use]
    pub const fn missing_credentials(
        provider: &'static str,
        env_vars: &'static [&'static str],
    ) -> Self {
        Self::MissingCredentials { provider, env_vars }
    }

    /// Returns `true` if this error can reasonably be retried (network timeouts, transient 5xx).
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Http(error) => error.is_connect() || error.is_timeout() || error.is_request(),
            Self::Api { retryable, .. } => *retryable,
            Self::RetriesExhausted { last_error, .. } => last_error.is_retryable(),
            Self::MissingCredentials { .. }
            | Self::ExpiredOAuthToken
            | Self::Auth(_)
            | Self::InvalidApiKeyEnv(_)
            | Self::Io(_)
            | Self::Json(_)
            | Self::InvalidSseFrame(_)
            | Self::BackoffOverflow { .. } => false,
        }
    }
}

impl Display for ApiError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingCredentials { provider, env_vars } => write!(
                f,
                "missing {provider} credentials; export {} before calling the {provider} API",
                env_vars.join(" or ")
            ),
            Self::ExpiredOAuthToken => {
                write!(
                    f,
                    "saved OAuth token is expired and no refresh token is available"
                )
            }
            Self::Auth(message) => write!(f, "auth error: {message}"),
            Self::InvalidApiKeyEnv(error) => {
                write!(f, "failed to read credential environment variable: {error}")
            }
            Self::Http(error) => write!(f, "http error: {error}"),
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Json(error) => write!(f, "json error: {error}"),
            Self::Api {
                status,
                error_type,
                message,
                body,
                ..
            } => match (error_type, message) {
                (Some(error_type), Some(message)) => {
                    write!(f, "api returned {status} ({error_type}): {message}")
                }
                _ => write!(f, "api returned {status}: {body}"),
            },
            Self::RetriesExhausted {
                attempts,
                last_error,
            } => write!(f, "api failed after {attempts} attempts: {last_error}"),
            Self::InvalidSseFrame(message) => write!(f, "invalid sse frame: {message}"),
            Self::BackoffOverflow {
                attempt,
                base_delay,
            } => write!(
                f,
                "retry backoff overflowed on attempt {attempt} with base delay {base_delay:?}"
            ),
        }
    }
}

impl std::error::Error for ApiError {}

impl From<reqwest::Error> for ApiError {
    fn from(value: reqwest::Error) -> Self {
        Self::Http(value)
    }
}

impl From<std::io::Error> for ApiError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<VarError> for ApiError {
    fn from(value: VarError) -> Self {
        Self::InvalidApiKeyEnv(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn display_missing_credentials_single_var() {
        let err = ApiError::missing_credentials("TestProvider", &["TEST_API_KEY"]);
        let msg = err.to_string();
        assert!(msg.contains("TestProvider"));
        assert!(msg.contains("TEST_API_KEY"));
    }

    #[test]
    fn display_missing_credentials_multiple_vars() {
        let err = ApiError::missing_credentials("TestProvider", &["KEY_A", "KEY_B"]);
        let msg = err.to_string();
        assert!(msg.contains("KEY_A"));
        assert!(msg.contains("KEY_B"));
    }

    #[test]
    fn display_expired_oauth_token() {
        let msg = ApiError::ExpiredOAuthToken.to_string();
        assert!(msg.contains("expired"));
    }

    #[test]
    fn display_retries_exhausted() {
        let inner = ApiError::Auth("bad token".to_string());
        let err = ApiError::RetriesExhausted {
            attempts: 3,
            last_error: Box::new(inner),
        };
        let msg = err.to_string();
        assert!(msg.contains("3"));
        assert!(msg.contains("bad token"));
    }

    #[test]
    fn from_io_error_wraps_as_io_variant() {
        let io_err = io::Error::new(io::ErrorKind::NotFound, "missing file");
        let api_err = ApiError::from(io_err);
        assert!(matches!(api_err, ApiError::Io(_)));
        assert!(api_err.to_string().contains("missing file"));
    }

    #[test]
    fn io_variant_is_not_retryable() {
        let io_err = io::Error::new(io::ErrorKind::PermissionDenied, "denied");
        let api_err = ApiError::from(io_err);
        assert!(!api_err.is_retryable());
    }
}
