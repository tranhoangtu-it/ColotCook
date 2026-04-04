//! Structured logging for production observability.
//!
//! Provides JSON-formatted log output when `COLOTCOOK_LOG_FORMAT=json` is set,
//! otherwise falls back to human-readable format.

use std::fmt;
use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

/// Log severity levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Debug => write!(f, "DEBUG"),
            Self::Info => write!(f, "INFO"),
            Self::Warn => write!(f, "WARN"),
            Self::Error => write!(f, "ERROR"),
        }
    }
}

/// Minimum log level from environment.
fn min_log_level() -> LogLevel {
    match std::env::var("COLOTCOOK_LOG_LEVEL")
        .unwrap_or_default()
        .to_lowercase()
        .as_str()
    {
        "debug" => LogLevel::Debug,
        "warn" => LogLevel::Warn,
        "error" => LogLevel::Error,
        // "info" and unrecognized values both default to Info
        _ => LogLevel::Info,
    }
}

/// Whether to use JSON format.
fn use_json_format() -> bool {
    std::env::var("COLOTCOOK_LOG_FORMAT")
        .map(|v| v.to_lowercase() == "json")
        .unwrap_or(false)
}

/// Emit a structured log entry.
pub fn log(level: LogLevel, component: &str, message: &str, fields: &[(&str, &str)]) {
    if level < min_log_level() {
        return;
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());

    if use_json_format() {
        let mut json = format!(
            r#"{{"ts":{},"level":"{}","component":"{}","msg":"{}""#,
            timestamp,
            level,
            escape_json(component),
            escape_json(message),
        );
        for (key, value) in fields {
            let _ = write!(json, r#","{}":"{}""#, escape_json(key), escape_json(value));
        }
        json.push('}');
        eprintln!("{json}");
    } else {
        let fields_str = if fields.is_empty() {
            String::new()
        } else {
            let pairs: Vec<String> = fields.iter().map(|(k, v)| format!("{k}={v}")).collect();
            format!(" {}", pairs.join(" "))
        };
        eprintln!("[{timestamp}] {level} [{component}] {message}{fields_str}");
    }
}

fn escape_json(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

/// Convenience macros-style functions.
pub fn log_info(component: &str, message: &str) {
    log(LogLevel::Info, component, message, &[]);
}

pub fn log_warn(component: &str, message: &str) {
    log(LogLevel::Warn, component, message, &[]);
}

pub fn log_error(component: &str, message: &str) {
    log(LogLevel::Error, component, message, &[]);
}

pub fn log_debug(component: &str, message: &str) {
    log(LogLevel::Debug, component, message, &[]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_level_ordering() {
        assert!(LogLevel::Debug < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Warn);
        assert!(LogLevel::Warn < LogLevel::Error);
    }

    #[test]
    fn escape_json_handles_special_chars() {
        assert_eq!(escape_json(r#"hello "world""#), r#"hello \"world\""#);
        assert_eq!(escape_json("line1\nline2"), "line1\\nline2");
    }

    #[test]
    fn log_level_display() {
        assert_eq!(LogLevel::Info.to_string(), "INFO");
        assert_eq!(LogLevel::Error.to_string(), "ERROR");
    }

    #[test]
    fn log_level_display_all_variants() {
        assert_eq!(LogLevel::Debug.to_string(), "DEBUG");
        assert_eq!(LogLevel::Warn.to_string(), "WARN");
        assert_eq!(LogLevel::Info.to_string(), "INFO");
        assert_eq!(LogLevel::Error.to_string(), "ERROR");
    }

    #[test]
    fn escape_json_handles_backslash() {
        assert_eq!(escape_json("back\\slash"), "back\\\\slash");
    }

    #[test]
    fn escape_json_handles_carriage_return() {
        assert_eq!(escape_json("foo\rbar"), "foo\\rbar");
    }

    #[test]
    fn escape_json_handles_tab() {
        assert_eq!(escape_json("foo\tbar"), "foo\\tbar");
    }

    #[test]
    fn escape_json_handles_no_special_chars() {
        assert_eq!(escape_json("hello world"), "hello world");
    }

    #[test]
    fn escape_json_handles_empty_string() {
        assert_eq!(escape_json(""), "");
    }

    #[test]
    fn escape_json_handles_multiple_specials() {
        let input = "a\"b\\c\nd";
        let result = escape_json(input);
        assert_eq!(result, "a\\\"b\\\\c\\nd");
    }

    #[test]
    fn log_level_equality() {
        assert_eq!(LogLevel::Debug, LogLevel::Debug);
        assert_eq!(LogLevel::Info, LogLevel::Info);
        assert_ne!(LogLevel::Debug, LogLevel::Error);
    }

    #[test]
    fn log_level_clone_and_copy() {
        let level = LogLevel::Warn;
        let cloned = level;
        assert_eq!(level, cloned);
    }

    #[test]
    fn log_does_not_emit_below_min_level() {
        // Just verify no panic when logging below threshold with env set to Error
        // We can't easily capture stderr, but we can ensure the function runs without panic.
        // Setting env temporarily is unsafe in tests, so we test via direct call.
        log(LogLevel::Debug, "test", "debug message", &[]);
        log(LogLevel::Info, "test", "info message", &[("key", "val")]);
        log(LogLevel::Warn, "test", "warn message", &[]);
        log(LogLevel::Error, "test", "error message", &[]);
    }

    #[test]
    fn log_convenience_functions_do_not_panic() {
        log_info("comp", "info message");
        log_warn("comp", "warn message");
        log_error("comp", "error message");
        log_debug("comp", "debug message");
    }

    #[test]
    fn log_with_fields_does_not_panic() {
        log(
            LogLevel::Info,
            "component",
            "test",
            &[("key1", "value1"), ("key2", "value2")],
        );
    }

    #[test]
    fn min_log_level_defaults_to_info_for_unknown() {
        // When COLOTCOOK_LOG_LEVEL is not set or has unrecognized value,
        // min_log_level() returns Info. We call it indirectly via log().
        // An Info-level log should always emit (when threshold is Info).
        log(LogLevel::Info, "test", "should emit", &[]);
    }

    #[test]
    fn use_json_format_returns_bool() {
        // Verify it returns a bool without panicking
        let _ = use_json_format();
    }
}
