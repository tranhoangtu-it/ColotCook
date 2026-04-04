//! Streaming client, multi-provider runtime, and turn-event utilities.
use std::io::{self, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use colotcook_api as api;
use colotcook_api::{
    ContentBlockDelta, InputContentBlock, InputMessage, MessageRequest, MessageResponse,
    OutputContentBlock, PromptCache, ProviderClient, StreamEvent as ApiStreamEvent, ToolChoice,
    ToolResultContentBlock,
};
use colotcook_runtime as runtime;
use colotcook_runtime::{
    ApiClient, ApiRequest, AssistantEvent, ContentBlock, ConversationMessage, MessageRole,
    PromptCacheEvent, RuntimeError,
};
use colotcook_tools::GlobalToolRegistry;
use serde_json::json;

use crate::arg_parsing::{filter_tool_specs, max_tokens_for_model, AllowedToolSet};
use crate::oauth_flow::resolve_cli_auth_source;
use crate::render::{MarkdownStreamState, TerminalRenderer};
use crate::tool_display::format_tool_call_start;
use crate::util::{
    extract_tool_path, first_visible_line, summarize_tool_payload, truncate_for_summary,
};

/// Interval between heartbeat ticks for long-running internal prompts.
#[allow(dead_code)] // Used by InternalPromptProgressRun heartbeat thread at runtime
pub(crate) const INTERNAL_PROGRESS_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(3);

/// Live progress state for an internal (background) prompt run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InternalPromptProgressState {
    pub(crate) command_label: &'static str,
    pub(crate) task_label: String,
    pub(crate) step: usize,
    pub(crate) phase: String,
    pub(crate) detail: Option<String>,
    pub(crate) saw_final_text: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Events emitted during the progress lifecycle of an internal prompt.
#[allow(dead_code)] // Variants used by InternalPromptProgressReporter/Run at runtime
pub(crate) enum InternalPromptProgressEvent {
    Started,
    Update,
    Heartbeat,
    Complete,
    Failed,
}

#[derive(Debug)]
struct InternalPromptProgressShared {
    state: Mutex<InternalPromptProgressState>,
    output_lock: Mutex<()>,
    started_at: Instant,
}

/// Shareable handle for updating and emitting progress during an internal prompt run.
#[derive(Debug, Clone)]
pub(crate) struct InternalPromptProgressReporter {
    shared: Arc<InternalPromptProgressShared>,
}

#[derive(Debug)]
/// Active run of an internal prompt with a managed heartbeat thread.
#[allow(dead_code)] // Constructed via InternalPromptProgressRun::start_ultraplan at runtime
pub(crate) struct InternalPromptProgressRun {
    reporter: InternalPromptProgressReporter,
    heartbeat_stop: Option<mpsc::Sender<()>>,
    heartbeat_handle: Option<thread::JoinHandle<()>>,
}

#[allow(dead_code)] // Methods used by InternalPromptProgressRun at runtime
impl InternalPromptProgressReporter {
    /// Create a reporter configured for a /ultraplan run.
    pub(crate) fn ultraplan(task: &str) -> Self {
        Self {
            shared: Arc::new(InternalPromptProgressShared {
                state: Mutex::new(InternalPromptProgressState {
                    command_label: "Ultraplan",
                    task_label: task.to_string(),
                    step: 0,
                    phase: "planning started".to_string(),
                    detail: Some(format!("task: {task}")),
                    saw_final_text: false,
                }),
                output_lock: Mutex::new(()),
                started_at: Instant::now(),
            }),
        }
    }

    /// Emit a progress event to the terminal.
    pub(crate) fn emit(&self, event: InternalPromptProgressEvent, error: Option<&str>) {
        let snapshot = self.snapshot();
        let line = format_internal_prompt_progress_line(event, &snapshot, self.elapsed(), error);
        self.write_line(&line);
    }

    /// Advance progress to the model-analysis phase.
    pub(crate) fn mark_model_phase(&self) {
        let snapshot = {
            let mut state = self
                .shared
                .state
                .lock()
                .expect("internal prompt progress state poisoned");
            state.step += 1;
            state.phase = if state.step == 1 {
                "analyzing request".to_string()
            } else {
                "reviewing findings".to_string()
            };
            state.detail = Some(format!("task: {}", state.task_label));
            state.clone()
        };
        self.write_line(&format_internal_prompt_progress_line(
            InternalPromptProgressEvent::Update,
            &snapshot,
            self.elapsed(),
            None,
        ));
    }

    /// Advance progress to a tool-execution phase.
    pub(crate) fn mark_tool_phase(&self, name: &str, input: &str) {
        let detail = describe_tool_progress(name, input);
        let snapshot = {
            let mut state = self
                .shared
                .state
                .lock()
                .expect("internal prompt progress state poisoned");
            state.step += 1;
            state.phase = format!("running {name}");
            state.detail = Some(detail);
            state.clone()
        };
        self.write_line(&format_internal_prompt_progress_line(
            InternalPromptProgressEvent::Update,
            &snapshot,
            self.elapsed(),
            None,
        ));
    }

    /// Record that the model produced text output.
    pub(crate) fn mark_text_phase(&self, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        let detail = truncate_for_summary(first_visible_line(trimmed), 120);
        let snapshot = {
            let mut state = self
                .shared
                .state
                .lock()
                .expect("internal prompt progress state poisoned");
            if state.saw_final_text {
                return;
            }
            state.saw_final_text = true;
            state.step += 1;
            state.phase = "drafting final plan".to_string();
            state.detail = (!detail.is_empty()).then_some(detail);
            state.clone()
        };
        self.write_line(&format_internal_prompt_progress_line(
            InternalPromptProgressEvent::Update,
            &snapshot,
            self.elapsed(),
            None,
        ));
    }

    fn emit_heartbeat(&self) {
        let snapshot = self.snapshot();
        self.write_line(&format_internal_prompt_progress_line(
            InternalPromptProgressEvent::Heartbeat,
            &snapshot,
            self.elapsed(),
            None,
        ));
    }

    fn snapshot(&self) -> InternalPromptProgressState {
        self.shared
            .state
            .lock()
            .expect("internal prompt progress state poisoned")
            .clone()
    }

    fn elapsed(&self) -> Duration {
        self.shared.started_at.elapsed()
    }

    fn write_line(&self, line: &str) {
        let _guard = self
            .shared
            .output_lock
            .lock()
            .expect("internal prompt progress output lock poisoned");
        let mut stdout = io::stdout();
        let _ = writeln!(stdout, "{line}");
        let _ = stdout.flush();
    }
}

#[allow(dead_code)] // Methods used by run_ultraplan REPL command at runtime
impl InternalPromptProgressRun {
    /// Start an ultraplan run with a heartbeat thread.
    pub(crate) fn start_ultraplan(task: &str) -> Self {
        let reporter = InternalPromptProgressReporter::ultraplan(task);
        reporter.emit(InternalPromptProgressEvent::Started, None);

        let (heartbeat_stop, heartbeat_rx) = mpsc::channel();
        let heartbeat_reporter = reporter.clone();
        let heartbeat_handle = thread::spawn(move || loop {
            match heartbeat_rx.recv_timeout(INTERNAL_PROGRESS_HEARTBEAT_INTERVAL) {
                Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => heartbeat_reporter.emit_heartbeat(),
            }
        });

        Self {
            reporter,
            heartbeat_stop: Some(heartbeat_stop),
            heartbeat_handle: Some(heartbeat_handle),
        }
    }

    /// Return a clone of the underlying reporter handle.
    pub(crate) fn reporter(&self) -> InternalPromptProgressReporter {
        self.reporter.clone()
    }

    /// Signal successful completion of the run.
    pub(crate) fn finish_success(&mut self) {
        self.stop_heartbeat();
        self.reporter
            .emit(InternalPromptProgressEvent::Complete, None);
    }

    /// Signal that the run failed with an error message.
    pub(crate) fn finish_failure(&mut self, error: &str) {
        self.stop_heartbeat();
        self.reporter
            .emit(InternalPromptProgressEvent::Failed, Some(error));
    }

    fn stop_heartbeat(&mut self) {
        if let Some(sender) = self.heartbeat_stop.take() {
            let _ = sender.send(());
        }
        if let Some(handle) = self.heartbeat_handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for InternalPromptProgressRun {
    fn drop(&mut self) {
        self.stop_heartbeat();
    }
}

/// Format a single progress-line string for the given event and state.
pub(crate) fn format_internal_prompt_progress_line(
    event: InternalPromptProgressEvent,
    snapshot: &InternalPromptProgressState,
    elapsed: Duration,
    error: Option<&str>,
) -> String {
    let elapsed_seconds = elapsed.as_secs();
    let step_label = if snapshot.step == 0 {
        "current step pending".to_string()
    } else {
        format!("current step {}", snapshot.step)
    };
    let mut status_bits = vec![step_label, format!("phase {}", snapshot.phase)];
    if let Some(detail) = snapshot
        .detail
        .as_deref()
        .filter(|detail| !detail.is_empty())
    {
        status_bits.push(detail.to_string());
    }
    let status = status_bits.join(" · ");
    match event {
        InternalPromptProgressEvent::Started => {
            format!(
                "🧭 {} status · planning started · {status}",
                snapshot.command_label
            )
        }
        InternalPromptProgressEvent::Update => {
            format!("… {} status · {status}", snapshot.command_label)
        }
        InternalPromptProgressEvent::Heartbeat => format!(
            "… {} heartbeat · {elapsed_seconds}s elapsed · {status}",
            snapshot.command_label
        ),
        InternalPromptProgressEvent::Complete => format!(
            "✔ {} status · completed · {elapsed_seconds}s elapsed · {} steps total",
            snapshot.command_label, snapshot.step
        ),
        InternalPromptProgressEvent::Failed => format!(
            "✘ {} status · failed · {elapsed_seconds}s elapsed · {}",
            snapshot.command_label,
            error.unwrap_or("unknown error")
        ),
    }
}

/// Return a short human-readable description of a tool call for progress display.
pub(crate) fn describe_tool_progress(name: &str, input: &str) -> String {
    let parsed: serde_json::Value =
        serde_json::from_str(input).unwrap_or(serde_json::Value::String(input.to_string()));
    match name {
        "bash" | "Bash" => {
            let command = parsed
                .get("command")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            if command.is_empty() {
                "running shell command".to_string()
            } else {
                format!("command {}", truncate_for_summary(command.trim(), 100))
            }
        }
        "read_file" | "Read" => format!("reading {}", extract_tool_path(&parsed)),
        "write_file" | "Write" => format!("writing {}", extract_tool_path(&parsed)),
        "edit_file" | "Edit" => format!("editing {}", extract_tool_path(&parsed)),
        "glob_search" | "Glob" => {
            let pattern = parsed
                .get("pattern")
                .and_then(|value| value.as_str())
                .unwrap_or("?");
            let scope = parsed
                .get("path")
                .and_then(|value| value.as_str())
                .unwrap_or(".");
            format!("glob `{pattern}` in {scope}")
        }
        "grep_search" | "Grep" => {
            let pattern = parsed
                .get("pattern")
                .and_then(|value| value.as_str())
                .unwrap_or("?");
            let scope = parsed
                .get("path")
                .and_then(|value| value.as_str())
                .unwrap_or(".");
            format!("grep `{pattern}` in {scope}")
        }
        "web_search" | "WebSearch" => parsed
            .get("query")
            .and_then(|value| value.as_str())
            .map_or_else(
                || "running web search".to_string(),
                |query| format!("query {}", truncate_for_summary(query, 100)),
            ),
        _ => {
            let summary = summarize_tool_payload(input);
            if summary.is_empty() {
                format!("running {name}")
            } else {
                format!("{name}: {summary}")
            }
        }
    }
}

/// Multi-provider AI client that dispatches to Anthropic, Gemini, `OpenAI`, etc.
pub(crate) struct MultiProviderRuntimeClient {
    runtime: tokio::runtime::Runtime,
    client: ProviderClient,
    model: String,
    enable_tools: bool,
    emit_output: bool,
    allowed_tools: Option<AllowedToolSet>,
    tool_registry: GlobalToolRegistry,
    progress_reporter: Option<InternalPromptProgressReporter>,
}

impl MultiProviderRuntimeClient {
    /// Construct a client bound to the given provider credential.
    pub(crate) fn new(
        session_id: &str,
        model: String,
        enable_tools: bool,
        emit_output: bool,
        allowed_tools: Option<AllowedToolSet>,
        tool_registry: GlobalToolRegistry,
        progress_reporter: Option<InternalPromptProgressReporter>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let anthropic_auth = resolve_cli_auth_source().ok();
        let client = ProviderClient::from_model_with_anthropic_auth(&model, anthropic_auth)?;
        let client = client.with_prompt_cache(PromptCache::new(session_id));
        Ok(Self {
            runtime: tokio::runtime::Runtime::new()?,
            client,
            model,
            enable_tools,
            emit_output,
            allowed_tools,
            tool_registry,
            progress_reporter,
        })
    }
}

impl ApiClient for MultiProviderRuntimeClient {
    #[allow(clippy::too_many_lines)]
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        if let Some(progress_reporter) = &self.progress_reporter {
            progress_reporter.mark_model_phase();
        }
        let message_request = MessageRequest {
            model: self.model.clone(),
            max_tokens: max_tokens_for_model(&self.model),
            messages: convert_messages(&request.messages),
            system: (!request.system_prompt.is_empty()).then(|| request.system_prompt.join("\n\n")),
            tools: self
                .enable_tools
                .then(|| filter_tool_specs(&self.tool_registry, self.allowed_tools.as_ref())),
            tool_choice: self.enable_tools.then_some(ToolChoice::Auto),
            stream: true,
        };

        self.runtime.block_on(async {
            let mut stream = self
                .client
                .stream_message(&message_request)
                .await
                .map_err(|error| RuntimeError::new(error.to_string()))?;
            let mut stdout = io::stdout();
            let mut sink = io::sink();
            let out: &mut dyn Write = if self.emit_output {
                &mut stdout
            } else {
                &mut sink
            };
            let renderer = TerminalRenderer::new();
            let mut markdown_stream = MarkdownStreamState::default();
            let mut events = Vec::new();
            let mut pending_tool: Option<(String, String, String)> = None;
            let mut saw_stop = false;

            while let Some(event) = stream
                .next_event()
                .await
                .map_err(|error| RuntimeError::new(error.to_string()))?
            {
                match event {
                    ApiStreamEvent::MessageStart(start) => {
                        for block in start.message.content {
                            push_output_block(block, out, &mut events, &mut pending_tool, true)?;
                        }
                    }
                    ApiStreamEvent::ContentBlockStart(start) => {
                        push_output_block(
                            start.content_block,
                            out,
                            &mut events,
                            &mut pending_tool,
                            true,
                        )?;
                    }
                    ApiStreamEvent::ContentBlockDelta(delta) => match delta.delta {
                        ContentBlockDelta::TextDelta { text } => {
                            if !text.is_empty() {
                                if let Some(progress_reporter) = &self.progress_reporter {
                                    progress_reporter.mark_text_phase(&text);
                                }
                                if let Some(rendered) = markdown_stream.push(&renderer, &text) {
                                    write!(out, "{rendered}")
                                        .and_then(|()| out.flush())
                                        .map_err(|error| RuntimeError::new(error.to_string()))?;
                                }
                                events.push(AssistantEvent::TextDelta(text));
                            }
                        }
                        ContentBlockDelta::InputJsonDelta { partial_json } => {
                            if let Some((_, _, input)) = &mut pending_tool {
                                input.push_str(&partial_json);
                            }
                        }
                        ContentBlockDelta::ThinkingDelta { .. }
                        | ContentBlockDelta::SignatureDelta { .. } => {}
                    },
                    ApiStreamEvent::ContentBlockStop(_) => {
                        if let Some(rendered) = markdown_stream.flush(&renderer) {
                            write!(out, "{rendered}")
                                .and_then(|()| out.flush())
                                .map_err(|error| RuntimeError::new(error.to_string()))?;
                        }
                        if let Some((id, name, input)) = pending_tool.take() {
                            if let Some(progress_reporter) = &self.progress_reporter {
                                progress_reporter.mark_tool_phase(&name, &input);
                            }
                            // Display tool call now that input is fully accumulated
                            writeln!(out, "\n{}", format_tool_call_start(&name, &input))
                                .and_then(|()| out.flush())
                                .map_err(|error| RuntimeError::new(error.to_string()))?;
                            events.push(AssistantEvent::ToolUse { id, name, input });
                        }
                    }
                    ApiStreamEvent::MessageDelta(delta) => {
                        events.push(AssistantEvent::Usage(delta.usage.token_usage()));
                    }
                    ApiStreamEvent::MessageStop(_) => {
                        saw_stop = true;
                        if let Some(rendered) = markdown_stream.flush(&renderer) {
                            write!(out, "{rendered}")
                                .and_then(|()| out.flush())
                                .map_err(|error| RuntimeError::new(error.to_string()))?;
                        }
                        events.push(AssistantEvent::MessageStop);
                    }
                }
            }

            push_prompt_cache_record(&self.client, &mut events);

            if !saw_stop
                && events.iter().any(|event| {
                    matches!(event, AssistantEvent::TextDelta(text) if !text.is_empty())
                        || matches!(event, AssistantEvent::ToolUse { .. })
                })
            {
                events.push(AssistantEvent::MessageStop);
            }

            if events
                .iter()
                .any(|event| matches!(event, AssistantEvent::MessageStop))
            {
                return Ok(events);
            }

            let response = self
                .client
                .send_message(&MessageRequest {
                    stream: false,
                    ..message_request.clone()
                })
                .await
                .map_err(|error| RuntimeError::new(error.to_string()))?;
            let mut events = response_to_events(response, out)?;
            push_prompt_cache_record(&self.client, &mut events);
            Ok(events)
        })
    }
}

/// Extract the final assistant text from a turn summary.
pub(crate) fn final_assistant_text(summary: &runtime::TurnSummary) -> String {
    summary
        .assistant_messages
        .last()
        .map(|message| {
            message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

/// Collect all tool-use events from a turn summary as JSON values.
pub(crate) fn collect_tool_uses(summary: &runtime::TurnSummary) -> Vec<serde_json::Value> {
    summary
        .assistant_messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, input } => Some(json!({
                "id": id,
                "name": name,
                "input": input,
            })),
            _ => None,
        })
        .collect()
}

/// Collect all tool-result events from a turn summary as JSON values.
pub(crate) fn collect_tool_results(summary: &runtime::TurnSummary) -> Vec<serde_json::Value> {
    summary
        .tool_results
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id,
                tool_name,
                output,
                is_error,
            } => Some(json!({
                "tool_use_id": tool_use_id,
                "tool_name": tool_name,
                "output": output,
                "is_error": is_error,
            })),
            _ => None,
        })
        .collect()
}

/// Collect prompt-cache events from a turn summary as JSON values.
pub(crate) fn collect_prompt_cache_events(
    summary: &runtime::TurnSummary,
) -> Vec<serde_json::Value> {
    summary
        .prompt_cache_events
        .iter()
        .map(|event| {
            json!({
                "unexpected": event.unexpected,
                "reason": event.reason,
                "previous_cache_read_input_tokens": event.previous_cache_read_input_tokens,
                "current_cache_read_input_tokens": event.current_cache_read_input_tokens,
                "token_drop": event.token_drop,
            })
        })
        .collect()
}

/// Append an output block from a message response to the event list.
pub(crate) fn push_output_block(
    block: OutputContentBlock,
    out: &mut (impl Write + ?Sized),
    events: &mut Vec<AssistantEvent>,
    pending_tool: &mut Option<(String, String, String)>,
    streaming_tool_input: bool,
) -> Result<(), RuntimeError> {
    match block {
        OutputContentBlock::Text { text } => {
            if !text.is_empty() {
                let rendered = TerminalRenderer::new().markdown_to_ansi(&text);
                write!(out, "{rendered}")
                    .and_then(|()| out.flush())
                    .map_err(|error| RuntimeError::new(error.to_string()))?;
                events.push(AssistantEvent::TextDelta(text));
            }
        }
        OutputContentBlock::ToolUse { id, name, input } => {
            // During streaming, the initial content_block_start has an empty input ({}).
            // The real input arrives via input_json_delta events. In
            // non-streaming responses, preserve a legitimate empty object.
            let initial_input = if streaming_tool_input
                && input.is_object()
                && input.as_object().is_some_and(serde_json::Map::is_empty)
            {
                String::new()
            } else {
                input.to_string()
            };
            *pending_tool = Some((id, name, initial_input));
        }
        OutputContentBlock::Thinking { .. } | OutputContentBlock::RedactedThinking { .. } => {}
    }
    Ok(())
}

/// Convert a `MessageResponse` into a list of `AssistantEvent`s.
pub(crate) fn response_to_events(
    response: MessageResponse,
    out: &mut (impl Write + ?Sized),
) -> Result<Vec<AssistantEvent>, RuntimeError> {
    let mut events = Vec::new();
    let mut pending_tool = None;

    for block in response.content {
        push_output_block(block, out, &mut events, &mut pending_tool, false)?;
        if let Some((id, name, input)) = pending_tool.take() {
            events.push(AssistantEvent::ToolUse { id, name, input });
        }
    }

    events.push(AssistantEvent::Usage(response.usage.token_usage()));
    events.push(AssistantEvent::MessageStop);
    Ok(events)
}

/// Append a prompt-cache record to the event list if one exists on the client.
pub(crate) fn push_prompt_cache_record(client: &ProviderClient, events: &mut Vec<AssistantEvent>) {
    if let Some(record) = client.take_last_prompt_cache_record() {
        if let Some(event) = prompt_cache_record_to_runtime_event(record) {
            events.push(AssistantEvent::PromptCache(event));
        }
    }
}

fn prompt_cache_record_to_runtime_event(
    record: api::PromptCacheRecord,
) -> Option<PromptCacheEvent> {
    let cache_break = record.cache_break?;
    Some(PromptCacheEvent {
        unexpected: cache_break.unexpected,
        reason: cache_break.reason,
        previous_cache_read_input_tokens: cache_break.previous_cache_read_input_tokens,
        current_cache_read_input_tokens: cache_break.current_cache_read_input_tokens,
        token_drop: cache_break.token_drop,
    })
}

/// Convert internal conversation messages into the API wire format.
pub(crate) fn convert_messages(messages: &[ConversationMessage]) -> Vec<InputMessage> {
    messages
        .iter()
        .filter_map(|message| {
            let role = match message.role {
                MessageRole::System | MessageRole::User | MessageRole::Tool => "user",
                MessageRole::Assistant => "assistant",
            };
            let content = message
                .blocks
                .iter()
                .map(|block| match block {
                    ContentBlock::Text { text } => InputContentBlock::Text { text: text.clone() },
                    ContentBlock::ToolUse { id, name, input } => InputContentBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: serde_json::from_str(input)
                            .unwrap_or_else(|_| serde_json::json!({ "raw": input })),
                    },
                    ContentBlock::ToolResult {
                        tool_use_id,
                        output,
                        is_error,
                        ..
                    } => InputContentBlock::ToolResult {
                        tool_use_id: tool_use_id.clone(),
                        content: vec![ToolResultContentBlock::Text {
                            text: output.clone(),
                        }],
                        is_error: *is_error,
                    },
                })
                .collect::<Vec<_>>();
            (!content.is_empty()).then(|| InputMessage {
                role: role.to_string(),
                content,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use colotcook_runtime::{
        AssistantEvent, ContentBlock, ConversationMessage, MessageRole, TokenUsage,
    };

    fn empty_summary() -> runtime::TurnSummary {
        runtime::TurnSummary {
            assistant_messages: vec![],
            tool_results: vec![],
            prompt_cache_events: vec![],
            usage: TokenUsage {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            },
            iterations: 0,
            auto_compaction: None,
        }
    }

    fn msg(role: MessageRole, blocks: Vec<ContentBlock>) -> ConversationMessage {
        ConversationMessage {
            role,
            blocks,
            usage: None,
        }
    }

    fn test_response(content: Vec<OutputContentBlock>) -> MessageResponse {
        MessageResponse {
            content,
            usage: colotcook_api::Usage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            },
            id: "msg_1".into(),
            kind: "message".into(),
            model: "test".into(),
            role: "assistant".into(),
            stop_reason: Some("end_turn".into()),
            stop_sequence: None,
            request_id: None,
        }
    }

    // --- convert_messages ---

    #[test]
    fn convert_messages_empty() {
        assert!(convert_messages(&[]).is_empty());
    }

    #[test]
    fn convert_messages_user_text() {
        let r = convert_messages(&[msg(
            MessageRole::User,
            vec![ContentBlock::Text { text: "hi".into() }],
        )]);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].role, "user");
    }

    #[test]
    fn convert_messages_assistant() {
        let r = convert_messages(&[msg(
            MessageRole::Assistant,
            vec![ContentBlock::Text { text: "a".into() }],
        )]);
        assert_eq!(r[0].role, "assistant");
    }

    #[test]
    fn convert_messages_system_to_user() {
        assert_eq!(
            convert_messages(&[msg(
                MessageRole::System,
                vec![ContentBlock::Text { text: "s".into() }]
            )])[0]
                .role,
            "user"
        );
    }

    #[test]
    fn convert_messages_tool_to_user() {
        assert_eq!(
            convert_messages(&[msg(
                MessageRole::Tool,
                vec![ContentBlock::Text { text: "o".into() }]
            )])[0]
                .role,
            "user"
        );
    }

    #[test]
    fn convert_messages_tool_use_block() {
        let r = convert_messages(&[msg(
            MessageRole::Assistant,
            vec![ContentBlock::ToolUse {
                id: "t".into(),
                name: "b".into(),
                input: r#"{"command":"ls"}"#.into(),
            }],
        )]);
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn convert_messages_invalid_json_fallback() {
        let r = convert_messages(&[msg(
            MessageRole::Assistant,
            vec![ContentBlock::ToolUse {
                id: "t".into(),
                name: "b".into(),
                input: "bad".into(),
            }],
        )]);
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn convert_messages_tool_result() {
        let r = convert_messages(&[msg(
            MessageRole::Tool,
            vec![ContentBlock::ToolResult {
                tool_use_id: "t".into(),
                tool_name: "b".into(),
                output: "ok".into(),
                is_error: false,
            }],
        )]);
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn convert_messages_empty_blocks_omitted() {
        assert!(convert_messages(&[msg(MessageRole::User, vec![])]).is_empty());
    }

    #[test]
    fn convert_messages_multiple() {
        let r = convert_messages(&[
            msg(
                MessageRole::User,
                vec![ContentBlock::Text { text: "q".into() }],
            ),
            msg(
                MessageRole::Assistant,
                vec![ContentBlock::Text { text: "a".into() }],
            ),
        ]);
        assert_eq!(r.len(), 2);
    }

    // --- final_assistant_text ---

    #[test]
    fn final_text_empty() {
        assert_eq!(final_assistant_text(&empty_summary()), "");
    }

    #[test]
    fn final_text_single() {
        let mut s = empty_summary();
        s.assistant_messages = vec![msg(
            MessageRole::Assistant,
            vec![ContentBlock::Text {
                text: "hello".into(),
            }],
        )];
        assert_eq!(final_assistant_text(&s), "hello");
    }

    #[test]
    fn final_text_uses_last() {
        let mut s = empty_summary();
        s.assistant_messages = vec![
            msg(
                MessageRole::Assistant,
                vec![ContentBlock::Text {
                    text: "first".into(),
                }],
            ),
            msg(
                MessageRole::Assistant,
                vec![ContentBlock::Text {
                    text: "second".into(),
                }],
            ),
        ];
        assert_eq!(final_assistant_text(&s), "second");
    }

    #[test]
    fn final_text_ignores_tool_use() {
        let mut s = empty_summary();
        s.assistant_messages = vec![msg(
            MessageRole::Assistant,
            vec![
                ContentBlock::ToolUse {
                    id: "t".into(),
                    name: "b".into(),
                    input: "{}".into(),
                },
                ContentBlock::Text {
                    text: "result".into(),
                },
            ],
        )];
        assert_eq!(final_assistant_text(&s), "result");
    }

    #[test]
    fn final_text_concatenates() {
        let mut s = empty_summary();
        s.assistant_messages = vec![msg(
            MessageRole::Assistant,
            vec![
                ContentBlock::Text { text: "p1".into() },
                ContentBlock::Text { text: "p2".into() },
            ],
        )];
        assert_eq!(final_assistant_text(&s), "p1p2");
    }

    // --- collect_tool_uses ---

    #[test]
    fn tool_uses_empty() {
        assert!(collect_tool_uses(&empty_summary()).is_empty());
    }

    #[test]
    fn tool_uses_captures() {
        let mut s = empty_summary();
        s.assistant_messages = vec![msg(
            MessageRole::Assistant,
            vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "bash".into(),
                input: "{}".into(),
            }],
        )];
        let u = collect_tool_uses(&s);
        assert_eq!(u.len(), 1);
        assert_eq!(u[0]["name"], "bash");
    }

    // --- collect_tool_results ---

    #[test]
    fn tool_results_empty() {
        assert!(collect_tool_results(&empty_summary()).is_empty());
    }

    #[test]
    fn tool_results_captures() {
        let mut s = empty_summary();
        s.tool_results = vec![msg(
            MessageRole::Tool,
            vec![ContentBlock::ToolResult {
                tool_use_id: "t1".into(),
                tool_name: "bash".into(),
                output: "ok".into(),
                is_error: false,
            }],
        )];
        let r = collect_tool_results(&s);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0]["tool_use_id"], "t1");
    }

    // --- collect_prompt_cache_events ---

    #[test]
    fn cache_events_empty() {
        assert!(collect_prompt_cache_events(&empty_summary()).is_empty());
    }

    #[test]
    fn cache_events_captures() {
        let mut s = empty_summary();
        s.prompt_cache_events = vec![runtime::PromptCacheEvent {
            unexpected: true,
            reason: "test".into(),
            previous_cache_read_input_tokens: 100,
            current_cache_read_input_tokens: 50,
            token_drop: 50,
        }];
        let e = collect_prompt_cache_events(&s);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0]["unexpected"], true);
    }

    // --- push_output_block ---

    #[test]
    fn output_block_text() {
        let mut events = Vec::new();
        let mut pt = None;
        push_output_block(
            OutputContentBlock::Text { text: "hi".into() },
            &mut io::sink(),
            &mut events,
            &mut pt,
            true,
        )
        .unwrap();
        assert!(matches!(&events[0], AssistantEvent::TextDelta(t) if t == "hi"));
    }

    #[test]
    fn output_block_empty_text() {
        let mut events = Vec::new();
        let mut pt = None;
        push_output_block(
            OutputContentBlock::Text { text: "".into() },
            &mut io::sink(),
            &mut events,
            &mut pt,
            true,
        )
        .unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn output_block_tool_streaming() {
        let mut events = Vec::new();
        let mut pt = None;
        push_output_block(
            OutputContentBlock::ToolUse {
                id: "t".into(),
                name: "b".into(),
                input: serde_json::json!({}),
            },
            &mut io::sink(),
            &mut events,
            &mut pt,
            true,
        )
        .unwrap();
        assert_eq!(pt.unwrap().2, "");
    }

    #[test]
    fn output_block_tool_non_streaming() {
        let mut events = Vec::new();
        let mut pt = None;
        push_output_block(
            OutputContentBlock::ToolUse {
                id: "t".into(),
                name: "b".into(),
                input: serde_json::json!({"k":"v"}),
            },
            &mut io::sink(),
            &mut events,
            &mut pt,
            false,
        )
        .unwrap();
        assert!(pt.unwrap().2.contains("k"));
    }

    #[test]
    fn output_block_thinking() {
        let mut events = Vec::new();
        let mut pt = None;
        push_output_block(
            OutputContentBlock::Thinking {
                thinking: "x".into(),
                signature: None,
            },
            &mut io::sink(),
            &mut events,
            &mut pt,
            true,
        )
        .unwrap();
        assert!(events.is_empty());
    }

    // --- response_to_events ---

    #[test]
    fn resp_events_text() {
        let events = response_to_events(
            test_response(vec![OutputContentBlock::Text { text: "hi".into() }]),
            &mut io::sink(),
        )
        .unwrap();
        assert!(events
            .iter()
            .any(|e| matches!(e, AssistantEvent::TextDelta(_))));
        assert!(events
            .iter()
            .any(|e| matches!(e, AssistantEvent::MessageStop)));
    }

    #[test]
    fn resp_events_tool_use() {
        let events = response_to_events(
            test_response(vec![OutputContentBlock::ToolUse {
                id: "t".into(),
                name: "bash".into(),
                input: serde_json::json!({"c":"l"}),
            }]),
            &mut io::sink(),
        )
        .unwrap();
        assert!(events
            .iter()
            .any(|e| matches!(e, AssistantEvent::ToolUse { name, .. } if name == "bash")));
    }

    // --- format_internal_prompt_progress_line ---

    fn snap(step: usize, phase: &str, detail: Option<&str>) -> InternalPromptProgressState {
        InternalPromptProgressState {
            command_label: "Ultraplan",
            task_label: "t".into(),
            step,
            phase: phase.into(),
            detail: detail.map(Into::into),
            saw_final_text: false,
        }
    }

    #[test]
    fn progress_started() {
        let l = format_internal_prompt_progress_line(
            InternalPromptProgressEvent::Started,
            &snap(0, "planning", Some("task: t")),
            Duration::from_secs(0),
            None,
        );
        assert!(l.contains("Ultraplan") && l.contains("current step pending"));
    }

    #[test]
    fn progress_update() {
        let l = format_internal_prompt_progress_line(
            InternalPromptProgressEvent::Update,
            &snap(3, "analyzing", None),
            Duration::from_secs(5),
            None,
        );
        assert!(l.contains("current step 3"));
    }

    #[test]
    fn progress_heartbeat() {
        let l = format_internal_prompt_progress_line(
            InternalPromptProgressEvent::Heartbeat,
            &snap(1, "w", None),
            Duration::from_secs(15),
            None,
        );
        assert!(l.contains("heartbeat") && l.contains("15s elapsed"));
    }

    #[test]
    fn progress_complete() {
        let mut s = snap(5, "done", None);
        s.saw_final_text = true;
        let l = format_internal_prompt_progress_line(
            InternalPromptProgressEvent::Complete,
            &s,
            Duration::from_secs(30),
            None,
        );
        assert!(l.contains("completed") && l.contains("5 steps total"));
    }

    #[test]
    fn progress_failed_with_err() {
        let l = format_internal_prompt_progress_line(
            InternalPromptProgressEvent::Failed,
            &snap(2, "x", None),
            Duration::from_secs(10),
            Some("timeout"),
        );
        assert!(l.contains("failed") && l.contains("timeout"));
    }

    #[test]
    fn progress_failed_unknown() {
        let l = format_internal_prompt_progress_line(
            InternalPromptProgressEvent::Failed,
            &snap(1, "x", None),
            Duration::from_secs(5),
            None,
        );
        assert!(l.contains("unknown error"));
    }

    // --- describe_tool_progress ---

    #[test]
    fn tool_prog_bash() {
        assert!(describe_tool_progress("bash", r#"{"command":"ls -la"}"#).contains("ls -la"));
    }
    #[test]
    fn tool_prog_bash_alias() {
        assert!(describe_tool_progress("Bash", r#"{"command":"echo hi"}"#).contains("echo hi"));
    }
    #[test]
    fn tool_prog_bash_empty() {
        assert_eq!(
            describe_tool_progress("bash", r#"{"command":""}"#),
            "running shell command"
        );
    }
    #[test]
    fn tool_prog_read() {
        assert!(describe_tool_progress("Read", r#"{"file_path":"main.rs"}"#).contains("reading"));
    }
    #[test]
    fn tool_prog_write() {
        assert!(describe_tool_progress("Write", r#"{"file_path":"o.txt"}"#).contains("writing"));
    }
    #[test]
    fn tool_prog_edit() {
        assert!(describe_tool_progress("Edit", r#"{"file_path":"lib.rs"}"#).contains("editing"));
    }
    #[test]
    fn tool_prog_glob() {
        let r = describe_tool_progress("Glob", r#"{"pattern":"*.rs","path":"src"}"#);
        assert!(r.contains("glob") && r.contains("*.rs"));
    }
    #[test]
    fn tool_prog_grep() {
        let r = describe_tool_progress("Grep", r#"{"pattern":"TODO","path":"src"}"#);
        assert!(r.contains("grep") && r.contains("TODO"));
    }
    #[test]
    fn tool_prog_web_search() {
        assert!(describe_tool_progress("web_search", r#"{"query":"rust"}"#).contains("rust"));
    }
    #[test]
    fn tool_prog_web_search_none() {
        assert_eq!(
            describe_tool_progress("WebSearch", "{}"),
            "running web search"
        );
    }
    #[test]
    fn tool_prog_unknown() {
        assert!(describe_tool_progress("custom", "{}").contains("custom"));
    }
    #[test]
    fn tool_prog_bad_json() {
        assert!(!describe_tool_progress("bash", "not json").is_empty());
    }

    // --- InternalPromptProgressReporter ---

    #[test]
    fn reporter_snapshot() {
        let r = InternalPromptProgressReporter::ultraplan("build");
        let s = r.snapshot();
        assert_eq!(s.command_label, "Ultraplan");
        assert_eq!(s.step, 0);
    }

    #[test]
    fn reporter_elapsed() {
        assert!(InternalPromptProgressReporter::ultraplan("t").elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn state_clone_eq() {
        let s = snap(2, "r", None);
        assert_eq!(s.clone(), s);
    }

    #[test]
    fn event_copy_eq() {
        let e = InternalPromptProgressEvent::Started;
        let c = e;
        assert_eq!(e, c);
    }

    // --- InternalPromptProgressReporter: mark_model_phase ---

    #[test]
    fn reporter_mark_model_phase_increments_step() {
        let r = InternalPromptProgressReporter::ultraplan("test task");
        r.mark_model_phase();
        let s = r.snapshot();
        assert_eq!(s.step, 1);
        assert_eq!(s.phase, "analyzing request");
    }

    #[test]
    fn reporter_mark_model_phase_second_call_reviewing() {
        let r = InternalPromptProgressReporter::ultraplan("test task");
        r.mark_model_phase();
        r.mark_model_phase();
        let s = r.snapshot();
        assert_eq!(s.step, 2);
        assert_eq!(s.phase, "reviewing findings");
    }

    // --- InternalPromptProgressReporter: mark_tool_phase ---

    #[test]
    fn reporter_mark_tool_phase_sets_phase() {
        let r = InternalPromptProgressReporter::ultraplan("task");
        r.mark_tool_phase("bash", r#"{"command":"ls"}"#);
        let s = r.snapshot();
        assert_eq!(s.step, 1);
        assert!(s.phase.contains("bash"));
    }

    #[test]
    fn reporter_mark_tool_phase_detail_includes_description() {
        let r = InternalPromptProgressReporter::ultraplan("task");
        r.mark_tool_phase("Read", r#"{"file_path":"main.rs"}"#);
        let s = r.snapshot();
        assert!(s.detail.as_ref().unwrap().contains("reading"));
    }

    // --- InternalPromptProgressReporter: mark_text_phase ---

    #[test]
    fn reporter_mark_text_phase_sets_saw_final_text() {
        let r = InternalPromptProgressReporter::ultraplan("task");
        r.mark_text_phase("Here is my plan...");
        let s = r.snapshot();
        assert!(s.saw_final_text);
        assert_eq!(s.phase, "drafting final plan");
    }

    #[test]
    fn reporter_mark_text_phase_ignores_empty() {
        let r = InternalPromptProgressReporter::ultraplan("task");
        r.mark_text_phase("   ");
        let s = r.snapshot();
        assert!(!s.saw_final_text);
        assert_eq!(s.step, 0);
    }

    #[test]
    fn reporter_mark_text_phase_only_fires_once() {
        let r = InternalPromptProgressReporter::ultraplan("task");
        r.mark_text_phase("first text");
        r.mark_text_phase("second text");
        let s = r.snapshot();
        assert_eq!(s.step, 1); // Only incremented once
    }

    // --- InternalPromptProgressReporter: emit ---

    #[test]
    fn reporter_emit_does_not_panic() {
        let r = InternalPromptProgressReporter::ultraplan("task");
        r.emit(InternalPromptProgressEvent::Started, None);
        r.emit(InternalPromptProgressEvent::Update, None);
        r.emit(InternalPromptProgressEvent::Heartbeat, None);
        r.emit(InternalPromptProgressEvent::Complete, None);
        r.emit(InternalPromptProgressEvent::Failed, Some("err"));
    }

    // --- InternalPromptProgressReporter: emit_heartbeat ---

    #[test]
    fn reporter_emit_heartbeat_does_not_panic() {
        let r = InternalPromptProgressReporter::ultraplan("task");
        r.emit_heartbeat();
    }

    // --- InternalPromptProgressReporter: write_line ---

    #[test]
    fn reporter_write_line_does_not_panic() {
        let r = InternalPromptProgressReporter::ultraplan("task");
        r.write_line("test output line");
    }

    // --- InternalPromptProgressRun ---

    #[test]
    fn progress_run_start_ultraplan_and_finish_success() {
        let mut run = InternalPromptProgressRun::start_ultraplan("test");
        let reporter = run.reporter();
        reporter.mark_model_phase();
        run.finish_success();
        // Should not hang or panic
    }

    #[test]
    fn progress_run_start_ultraplan_and_finish_failure() {
        let mut run = InternalPromptProgressRun::start_ultraplan("test");
        run.finish_failure("something went wrong");
        // Should not hang or panic
    }

    #[test]
    fn progress_run_reporter_returns_clone() {
        let run = InternalPromptProgressRun::start_ultraplan("test");
        let reporter = run.reporter();
        let s = reporter.snapshot();
        assert_eq!(s.command_label, "Ultraplan");
        drop(run);
    }

    #[test]
    fn progress_run_drop_stops_heartbeat() {
        let run = InternalPromptProgressRun::start_ultraplan("test");
        drop(run);
        // Should not hang or panic
    }

    // --- InternalPromptProgressState snapshot ---

    #[test]
    fn state_debug_impl() {
        let s = snap(1, "test", Some("detail"));
        let debug = format!("{s:?}");
        assert!(debug.contains("Ultraplan"));
    }

    // --- describe_tool_progress additional ---

    #[test]
    fn tool_prog_read_file_alias() {
        assert!(describe_tool_progress("read_file", r#"{"file_path":"lib.rs"}"#).contains("reading"));
    }

    #[test]
    fn tool_prog_write_file_alias() {
        assert!(describe_tool_progress("write_file", r#"{"file_path":"out.txt"}"#).contains("writing"));
    }

    #[test]
    fn tool_prog_edit_file_alias() {
        assert!(describe_tool_progress("edit_file", r#"{"file_path":"main.rs"}"#).contains("editing"));
    }

    #[test]
    fn tool_prog_glob_search_alias() {
        let r = describe_tool_progress("glob_search", r#"{"pattern":"*.rs","path":"."}"#);
        assert!(r.contains("glob"));
    }

    #[test]
    fn tool_prog_grep_search_alias() {
        let r = describe_tool_progress("grep_search", r#"{"pattern":"fn main","path":"src"}"#);
        assert!(r.contains("grep"));
    }

    #[test]
    fn tool_prog_web_search_with_query() {
        let r = describe_tool_progress("web_search", r#"{"query":"how to use rust async"}"#);
        assert!(r.contains("how to use rust async"));
    }

    #[test]
    fn tool_prog_unknown_with_payload() {
        let r = describe_tool_progress("my_tool", r#"{"key":"value"}"#);
        assert!(r.contains("my_tool"));
    }

    #[test]
    fn tool_prog_bash_no_command_key() {
        let r = describe_tool_progress("bash", r#"{"other":"thing"}"#);
        assert_eq!(r, "running shell command");
    }

    // --- format_internal_prompt_progress_line additional ---

    #[test]
    fn progress_line_with_detail() {
        let l = format_internal_prompt_progress_line(
            InternalPromptProgressEvent::Update,
            &snap(1, "working", Some("file: main.rs")),
            Duration::from_secs(3),
            None,
        );
        assert!(l.contains("file: main.rs"));
    }

    #[test]
    fn progress_line_empty_detail_omitted() {
        let l = format_internal_prompt_progress_line(
            InternalPromptProgressEvent::Update,
            &snap(1, "working", Some("")),
            Duration::from_secs(3),
            None,
        );
        // Should still work but not include empty detail
        assert!(l.contains("Ultraplan"));
    }

    #[test]
    fn progress_line_step_zero_pending() {
        let l = format_internal_prompt_progress_line(
            InternalPromptProgressEvent::Update,
            &snap(0, "init", None),
            Duration::from_secs(0),
            None,
        );
        assert!(l.contains("current step pending"));
    }

    // --- convert_messages additional ---

    #[test]
    fn convert_messages_tool_use_with_invalid_json() {
        let r = convert_messages(&[msg(
            MessageRole::Assistant,
            vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "bash".into(),
                input: "not valid json".into(),
            }],
        )]);
        assert_eq!(r.len(), 1);
        // The input should be wrapped in a {raw: ...} fallback
    }

    #[test]
    fn convert_messages_tool_result_with_error() {
        let r = convert_messages(&[msg(
            MessageRole::Tool,
            vec![ContentBlock::ToolResult {
                tool_use_id: "t1".into(),
                tool_name: "bash".into(),
                output: "error output".into(),
                is_error: true,
            }],
        )]);
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn convert_messages_mixed_blocks() {
        let r = convert_messages(&[msg(
            MessageRole::Assistant,
            vec![
                ContentBlock::Text { text: "text".into() },
                ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "bash".into(),
                    input: r#"{"command":"ls"}"#.into(),
                },
            ],
        )]);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].content.len(), 2);
    }

    // --- collect_tool_uses additional ---

    #[test]
    fn tool_uses_multiple() {
        let mut s = empty_summary();
        s.assistant_messages = vec![
            msg(
                MessageRole::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "bash".into(),
                    input: "{}".into(),
                }],
            ),
            msg(
                MessageRole::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "t2".into(),
                    name: "Read".into(),
                    input: "{}".into(),
                }],
            ),
        ];
        let u = collect_tool_uses(&s);
        assert_eq!(u.len(), 2);
    }

    #[test]
    fn tool_uses_skips_text_blocks() {
        let mut s = empty_summary();
        s.assistant_messages = vec![msg(
            MessageRole::Assistant,
            vec![
                ContentBlock::Text { text: "hi".into() },
                ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "bash".into(),
                    input: "{}".into(),
                },
            ],
        )];
        let u = collect_tool_uses(&s);
        assert_eq!(u.len(), 1);
    }

    // --- collect_tool_results additional ---

    #[test]
    fn tool_results_multiple() {
        let mut s = empty_summary();
        s.tool_results = vec![
            msg(
                MessageRole::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "t1".into(),
                    tool_name: "bash".into(),
                    output: "ok".into(),
                    is_error: false,
                }],
            ),
            msg(
                MessageRole::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "t2".into(),
                    tool_name: "Read".into(),
                    output: "content".into(),
                    is_error: false,
                }],
            ),
        ];
        let r = collect_tool_results(&s);
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn tool_results_with_error_flag() {
        let mut s = empty_summary();
        s.tool_results = vec![msg(
            MessageRole::Tool,
            vec![ContentBlock::ToolResult {
                tool_use_id: "t1".into(),
                tool_name: "bash".into(),
                output: "failure".into(),
                is_error: true,
            }],
        )];
        let r = collect_tool_results(&s);
        assert_eq!(r[0]["is_error"], true);
    }

    // --- collect_prompt_cache_events additional ---

    #[test]
    fn cache_events_multiple() {
        let mut s = empty_summary();
        s.prompt_cache_events = vec![
            runtime::PromptCacheEvent {
                unexpected: false,
                reason: "normal".into(),
                previous_cache_read_input_tokens: 200,
                current_cache_read_input_tokens: 100,
                token_drop: 100,
            },
            runtime::PromptCacheEvent {
                unexpected: true,
                reason: "break".into(),
                previous_cache_read_input_tokens: 500,
                current_cache_read_input_tokens: 0,
                token_drop: 500,
            },
        ];
        let e = collect_prompt_cache_events(&s);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0]["reason"], "normal");
        assert_eq!(e[1]["reason"], "break");
    }

    // --- push_output_block additional ---

    #[test]
    fn output_block_redacted_thinking() {
        let mut events = Vec::new();
        let mut pt = None;
        push_output_block(
            OutputContentBlock::RedactedThinking { data: "x".into() },
            &mut io::sink(),
            &mut events,
            &mut pt,
            true,
        )
        .unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn output_block_tool_use_with_real_input_streaming() {
        let mut events = Vec::new();
        let mut pt = None;
        push_output_block(
            OutputContentBlock::ToolUse {
                id: "t".into(),
                name: "bash".into(),
                input: serde_json::json!({"command": "echo hi"}),
            },
            &mut io::sink(),
            &mut events,
            &mut pt,
            true,
        )
        .unwrap();
        // Non-empty input during streaming should still be stored (only empty {} is cleared)
        assert!(pt.is_some());
    }

    // --- response_to_events additional ---

    #[test]
    fn resp_events_empty_content() {
        let events = response_to_events(test_response(vec![]), &mut io::sink()).unwrap();
        assert!(events
            .iter()
            .any(|e| matches!(e, AssistantEvent::MessageStop)));
        assert!(events
            .iter()
            .any(|e| matches!(e, AssistantEvent::Usage(_))));
    }

    #[test]
    fn resp_events_multiple_blocks() {
        let events = response_to_events(
            test_response(vec![
                OutputContentBlock::Text { text: "a".into() },
                OutputContentBlock::ToolUse {
                    id: "t".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"c":"l"}),
                },
                OutputContentBlock::Text { text: "b".into() },
            ]),
            &mut io::sink(),
        )
        .unwrap();
        let text_count = events
            .iter()
            .filter(|e| matches!(e, AssistantEvent::TextDelta(_)))
            .count();
        assert_eq!(text_count, 2);
        let tool_count = events
            .iter()
            .filter(|e| matches!(e, AssistantEvent::ToolUse { .. }))
            .count();
        assert_eq!(tool_count, 1);
    }

    // --- prompt_cache_record_to_runtime_event ---

    #[test]
    fn prompt_cache_record_to_event_none_cache_break() {
        let record = api::PromptCacheRecord {
            cache_break: None,
            stats: Default::default(),
        };
        assert!(prompt_cache_record_to_runtime_event(record).is_none());
    }

    #[test]
    fn prompt_cache_record_to_event_with_cache_break() {
        let record = api::PromptCacheRecord {
            cache_break: Some(api::CacheBreakEvent {
                unexpected: true,
                reason: "test reason".into(),
                previous_cache_read_input_tokens: 100,
                current_cache_read_input_tokens: 50,
                token_drop: 50,
            }),
            stats: Default::default(),
        };
        let event = prompt_cache_record_to_runtime_event(record).unwrap();
        assert!(event.unexpected);
        assert_eq!(event.reason, "test reason");
        assert_eq!(event.token_drop, 50);
    }

    // --- INTERNAL_PROGRESS_HEARTBEAT_INTERVAL ---

    #[test]
    fn heartbeat_interval_is_3_seconds() {
        assert_eq!(INTERNAL_PROGRESS_HEARTBEAT_INTERVAL, Duration::from_secs(3));
    }

    // --- InternalPromptProgressEvent variants ---

    #[test]
    fn progress_event_all_variants_debug() {
        let variants = [
            InternalPromptProgressEvent::Started,
            InternalPromptProgressEvent::Update,
            InternalPromptProgressEvent::Heartbeat,
            InternalPromptProgressEvent::Complete,
            InternalPromptProgressEvent::Failed,
        ];
        for v in &variants {
            let _ = format!("{v:?}");
        }
    }

    #[test]
    fn progress_event_equality() {
        assert_eq!(
            InternalPromptProgressEvent::Started,
            InternalPromptProgressEvent::Started
        );
        assert_ne!(
            InternalPromptProgressEvent::Started,
            InternalPromptProgressEvent::Failed
        );
    }
}
