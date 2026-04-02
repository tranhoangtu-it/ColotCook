//! Structured logging for production observability.
//!
//! Provides JSON-formatted log output when `COLOTCOOK_LOG_FORMAT=json` is set,
//! otherwise falls back to human-readable format.

use std::fmt;
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
        "info" => LogLevel::Info,
        "warn" => LogLevel::Warn,
        "error" => LogLevel::Error,
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
            json.push_str(&format!(r#","{}":"{}""#, escape_json(key), escape_json(value)));
        }
        json.push('}');
        eprintln!("{json}");
    } else {
        let fields_str = if fields.is_empty() {
            String::new()
        } else {
            let pairs: Vec<String> = fields
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
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
}
