//! Telemetry sinks and session tracing for HTTP requests and analytics events.
//!
//! Provides [`TelemetrySink`] implementations for in-memory testing and JSONL file output,
//! plus [`SessionTracer`] for structured per-session event recording.

use std::fmt::{Debug, Formatter};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";
pub const DEFAULT_APP_NAME: &str = "claude-code";
pub const DEFAULT_RUNTIME: &str = "rust";
pub const DEFAULT_AGENTIC_BETA: &str = "claude-code-20250219";
pub const DEFAULT_PROMPT_CACHING_SCOPE_BETA: &str = "prompt-caching-scope-2026-01-05";

/// Identifies the application making API requests (name, version, runtime).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientIdentity {
    pub app_name: String,
    pub app_version: String,
    pub runtime: String,
}

impl ClientIdentity {
    /// Creates a new identity with the given app name and version; runtime defaults to `"rust"`.
    #[must_use]
    pub fn new(app_name: impl Into<String>, app_version: impl Into<String>) -> Self {
        Self {
            app_name: app_name.into(),
            app_version: app_version.into(),
            runtime: DEFAULT_RUNTIME.to_string(),
        }
    }

    /// Returns a copy with the runtime field replaced.
    #[must_use]
    pub fn with_runtime(mut self, runtime: impl Into<String>) -> Self {
        self.runtime = runtime.into();
        self
    }

    /// Formats the `User-Agent` header value as `app_name/app_version`.
    #[must_use]
    pub fn user_agent(&self) -> String {
        format!("{}/{}", self.app_name, self.app_version)
    }
}

impl Default for ClientIdentity {
    fn default() -> Self {
        Self::new(DEFAULT_APP_NAME, env!("CARGO_PKG_VERSION"))
    }
}

/// Header and body configuration for Anthropic API requests (version, betas, extra fields).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnthropicRequestProfile {
    pub anthropic_version: String,
    pub client_identity: ClientIdentity,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub betas: Vec<String>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub extra_body: Map<String, Value>,
}

impl AnthropicRequestProfile {
    /// Creates a profile with default version and standard beta flags.
    #[must_use]
    pub fn new(client_identity: ClientIdentity) -> Self {
        Self {
            anthropic_version: DEFAULT_ANTHROPIC_VERSION.to_string(),
            client_identity,
            betas: vec![
                DEFAULT_AGENTIC_BETA.to_string(),
                DEFAULT_PROMPT_CACHING_SCOPE_BETA.to_string(),
            ],
            extra_body: Map::new(),
        }
    }

    /// Appends a beta feature flag if not already present.
    #[must_use]
    pub fn with_beta(mut self, beta: impl Into<String>) -> Self {
        let beta = beta.into();
        if !self.betas.contains(&beta) {
            self.betas.push(beta);
        }
        self
    }

    /// Inserts an arbitrary key-value pair that will be merged into the JSON request body.
    #[must_use]
    pub fn with_extra_body(mut self, key: impl Into<String>, value: Value) -> Self {
        self.extra_body.insert(key.into(), value);
        self
    }

    /// Returns the HTTP header name/value pairs that should be attached to every request.
    #[must_use]
    pub fn header_pairs(&self) -> Vec<(String, String)> {
        let mut headers = vec![
            (
                "anthropic-version".to_string(),
                self.anthropic_version.clone(),
            ),
            ("user-agent".to_string(), self.client_identity.user_agent()),
        ];
        if !self.betas.is_empty() {
            headers.push(("anthropic-beta".to_string(), self.betas.join(",")));
        }
        headers
    }

    /// Serializes `request` to JSON and merges `extra_body` fields and beta flags into the object.
    pub fn render_json_body<T: Serialize>(&self, request: &T) -> Result<Value, serde_json::Error> {
        let mut body = serde_json::to_value(request)?;
        let object = body.as_object_mut().ok_or_else(|| {
            serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "request body must serialize to a JSON object",
            ))
        })?;
        for (key, value) in &self.extra_body {
            object.insert(key.clone(), value.clone());
        }
        if !self.betas.is_empty() {
            object.insert(
                "betas".to_string(),
                Value::Array(self.betas.iter().cloned().map(Value::String).collect()),
            );
        }
        Ok(body)
    }
}

impl Default for AnthropicRequestProfile {
    fn default() -> Self {
        Self::new(ClientIdentity::default())
    }
}

/// A namespaced analytics event with an action name and optional key-value properties.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnalyticsEvent {
    pub namespace: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub properties: Map<String, Value>,
}

impl AnalyticsEvent {
    /// Creates an event with the given namespace and action, and an empty properties map.
    #[must_use]
    pub fn new(namespace: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            action: action.into(),
            properties: Map::new(),
        }
    }

    /// Inserts an additional property into this event.
    #[must_use]
    pub fn with_property(mut self, key: impl Into<String>, value: Value) -> Self {
        self.properties.insert(key.into(), value);
        self
    }
}

/// An immutable trace record emitted by [`SessionTracer`] for a named event within a session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionTraceRecord {
    pub session_id: String,
    pub sequence: u64,
    pub name: String,
    pub timestamp_ms: u64,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub attributes: Map<String, Value>,
}

/// All event variants that can be recorded through a [`TelemetrySink`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TelemetryEvent {
    HttpRequestStarted {
        session_id: String,
        attempt: u32,
        method: String,
        path: String,
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        attributes: Map<String, Value>,
    },
    HttpRequestSucceeded {
        session_id: String,
        attempt: u32,
        method: String,
        path: String,
        status: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        attributes: Map<String, Value>,
    },
    HttpRequestFailed {
        session_id: String,
        attempt: u32,
        method: String,
        path: String,
        error: String,
        retryable: bool,
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        attributes: Map<String, Value>,
    },
    Analytics(AnalyticsEvent),
    SessionTrace(SessionTraceRecord),
}

/// Trait for recording telemetry events; must be `Send + Sync` for use across async tasks.
pub trait TelemetrySink: Send + Sync {
    /// Records a single telemetry event.
    fn record(&self, event: TelemetryEvent);
}

/// In-memory [`TelemetrySink`] that buffers events for test assertions.
#[derive(Default)]
pub struct MemoryTelemetrySink {
    events: Mutex<Vec<TelemetryEvent>>,
}

impl MemoryTelemetrySink {
    /// Returns a snapshot of all recorded events.
    #[must_use]
    pub fn events(&self) -> Vec<TelemetryEvent> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl TelemetrySink for MemoryTelemetrySink {
    fn record(&self, event: TelemetryEvent) {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    }
}

/// [`TelemetrySink`] that appends events as newline-delimited JSON to a log file.
pub struct JsonlTelemetrySink {
    path: PathBuf,
    file: Mutex<File>,
}

impl Debug for JsonlTelemetrySink {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonlTelemetrySink")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl JsonlTelemetrySink {
    /// Opens (or creates) a JSONL log file at `path`, creating parent directories as needed.
    pub fn new(path: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            path,
            file: Mutex::new(file),
        })
    }

    /// Returns the path to the JSONL log file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl TelemetrySink for JsonlTelemetrySink {
    fn record(&self, event: TelemetryEvent) {
        let Ok(line) = serde_json::to_string(&event) else {
            return;
        };
        let mut file = self
            .file
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }
}

/// Emits structured trace records and HTTP lifecycle events for a single session.
#[derive(Clone)]
pub struct SessionTracer {
    session_id: String,
    sequence: Arc<AtomicU64>,
    sink: Arc<dyn TelemetrySink>,
}

impl Debug for SessionTracer {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionTracer")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

impl SessionTracer {
    /// Creates a tracer that forwards events to `sink` under the given session identifier.
    #[must_use]
    pub fn new(session_id: impl Into<String>, sink: Arc<dyn TelemetrySink>) -> Self {
        Self {
            session_id: session_id.into(),
            sequence: Arc::new(AtomicU64::new(0)),
            sink,
        }
    }

    /// Returns the session identifier associated with this tracer.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Emits a named trace record with an auto-incrementing sequence number and current timestamp.
    pub fn record(&self, name: impl Into<String>, attributes: Map<String, Value>) {
        let record = SessionTraceRecord {
            session_id: self.session_id.clone(),
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
            name: name.into(),
            timestamp_ms: current_timestamp_ms(),
            attributes,
        };
        self.sink.record(TelemetryEvent::SessionTrace(record));
    }

    /// Records that an HTTP request attempt has started.
    pub fn record_http_request_started(
        &self,
        attempt: u32,
        method: impl Into<String>,
        path: impl Into<String>,
        attributes: Map<String, Value>,
    ) {
        let method = method.into();
        let path = path.into();
        self.sink.record(TelemetryEvent::HttpRequestStarted {
            session_id: self.session_id.clone(),
            attempt,
            method: method.clone(),
            path: path.clone(),
            attributes: attributes.clone(),
        });
        self.record(
            "http_request_started",
            merge_trace_fields(method, path, attempt, attributes),
        );
    }

    /// Records that an HTTP request attempt succeeded with the given status code.
    pub fn record_http_request_succeeded(
        &self,
        attempt: u32,
        method: impl Into<String>,
        path: impl Into<String>,
        status: u16,
        request_id: Option<String>,
        attributes: Map<String, Value>,
    ) {
        let method = method.into();
        let path = path.into();
        self.sink.record(TelemetryEvent::HttpRequestSucceeded {
            session_id: self.session_id.clone(),
            attempt,
            method: method.clone(),
            path: path.clone(),
            status,
            request_id: request_id.clone(),
            attributes: attributes.clone(),
        });
        let mut trace_attributes = merge_trace_fields(method, path, attempt, attributes);
        trace_attributes.insert("status".to_string(), Value::from(status));
        if let Some(request_id) = request_id {
            trace_attributes.insert("request_id".to_string(), Value::String(request_id));
        }
        self.record("http_request_succeeded", trace_attributes);
    }

    /// Records that an HTTP request attempt failed, noting whether the error is retryable.
    pub fn record_http_request_failed(
        &self,
        attempt: u32,
        method: impl Into<String>,
        path: impl Into<String>,
        error: impl Into<String>,
        retryable: bool,
        attributes: Map<String, Value>,
    ) {
        let method = method.into();
        let path = path.into();
        let error = error.into();
        self.sink.record(TelemetryEvent::HttpRequestFailed {
            session_id: self.session_id.clone(),
            attempt,
            method: method.clone(),
            path: path.clone(),
            error: error.clone(),
            retryable,
            attributes: attributes.clone(),
        });
        let mut trace_attributes = merge_trace_fields(method, path, attempt, attributes);
        trace_attributes.insert("error".to_string(), Value::String(error));
        trace_attributes.insert("retryable".to_string(), Value::Bool(retryable));
        self.record("http_request_failed", trace_attributes);
    }

    /// Emits an analytics event to the sink and also records it as a trace entry.
    pub fn record_analytics(&self, event: AnalyticsEvent) {
        let mut attributes = event.properties.clone();
        attributes.insert(
            "namespace".to_string(),
            Value::String(event.namespace.clone()),
        );
        attributes.insert("action".to_string(), Value::String(event.action.clone()));
        self.sink.record(TelemetryEvent::Analytics(event));
        self.record("analytics", attributes);
    }
}

fn merge_trace_fields(
    method: String,
    path: String,
    attempt: u32,
    mut attributes: Map<String, Value>,
) -> Map<String, Value> {
    attributes.insert("method".to_string(), Value::String(method));
    attributes.insert("path".to_string(), Value::String(path));
    attributes.insert("attempt".to_string(), Value::from(attempt));
    attributes
}

fn current_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_profile_emits_headers_and_merges_body() {
        let profile = AnthropicRequestProfile::new(
            ClientIdentity::new("claude-code", "1.2.3").with_runtime("rust-cli"),
        )
        .with_beta("tools-2026-04-01")
        .with_extra_body("metadata", serde_json::json!({"source": "test"}));

        assert_eq!(
            profile.header_pairs(),
            vec![
                (
                    "anthropic-version".to_string(),
                    DEFAULT_ANTHROPIC_VERSION.to_string()
                ),
                ("user-agent".to_string(), "claude-code/1.2.3".to_string()),
                (
                    "anthropic-beta".to_string(),
                    "claude-code-20250219,prompt-caching-scope-2026-01-05,tools-2026-04-01"
                        .to_string(),
                ),
            ]
        );

        let body = profile
            .render_json_body(&serde_json::json!({"model": "claude-sonnet"}))
            .expect("body should serialize");
        assert_eq!(
            body["metadata"]["source"],
            Value::String("test".to_string())
        );
        assert_eq!(
            body["betas"],
            serde_json::json!([
                "claude-code-20250219",
                "prompt-caching-scope-2026-01-05",
                "tools-2026-04-01"
            ])
        );
    }

    #[test]
    fn session_tracer_records_structured_events_and_trace_sequence() {
        let sink = Arc::new(MemoryTelemetrySink::default());
        let tracer = SessionTracer::new("session-123", sink.clone());

        tracer.record_http_request_started(1, "POST", "/v1/messages", Map::new());
        tracer.record_analytics(
            AnalyticsEvent::new("cli", "prompt_sent")
                .with_property("model", Value::String("claude-opus".to_string())),
        );

        let events = sink.events();
        assert!(matches!(
            &events[0],
            TelemetryEvent::HttpRequestStarted {
                session_id,
                attempt: 1,
                method,
                path,
                ..
            } if session_id == "session-123" && method == "POST" && path == "/v1/messages"
        ));
        assert!(matches!(
            &events[1],
            TelemetryEvent::SessionTrace(SessionTraceRecord { sequence: 0, name, .. })
            if name == "http_request_started"
        ));
        assert!(matches!(&events[2], TelemetryEvent::Analytics(_)));
        assert!(matches!(
            &events[3],
            TelemetryEvent::SessionTrace(SessionTraceRecord { sequence: 1, name, .. })
            if name == "analytics"
        ));
    }

    #[test]
    fn jsonl_sink_persists_events() {
        let path =
            std::env::temp_dir().join(format!("telemetry-jsonl-{}.log", current_timestamp_ms()));
        let sink = JsonlTelemetrySink::new(&path).expect("sink should create file");

        sink.record(TelemetryEvent::Analytics(
            AnalyticsEvent::new("cli", "turn_completed").with_property("ok", Value::Bool(true)),
        ));

        let contents = std::fs::read_to_string(&path).expect("telemetry log should be readable");
        assert!(contents.contains("\"type\":\"analytics\""));
        assert!(contents.contains("\"action\":\"turn_completed\""));

        let _ = std::fs::remove_file(path);
    }
}
