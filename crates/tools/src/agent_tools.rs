//! Agent and sub-agent orchestration system.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use colotcook_api as api;
use colotcook_api::{
    max_tokens_for_model, resolve_model_alias, ContentBlockDelta, InputContentBlock, InputMessage,
    MessageRequest, MessageResponse, OutputContentBlock, ProviderClient,
    StreamEvent as ApiStreamEvent, ToolChoice, ToolDefinition, ToolResultContentBlock,
};
use colotcook_runtime as runtime;
use colotcook_runtime::{
    load_system_prompt, ApiClient, ApiRequest, AssistantEvent, ContentBlock, ConversationMessage,
    ConversationRuntime, MessageRole, PermissionMode, PermissionPolicy, PromptCacheEvent,
    RuntimeError, Session, ToolError, ToolExecutor,
};
use serde_json::Value;

use crate::search_tools::canonical_tool_token;
use crate::types::{AgentInput, AgentOutput};
use crate::{execute_tool, mvp_tool_specs, ToolSpec};

/// Job descriptor for a sub-agent.
#[derive(Debug, Clone)]
pub(crate) struct AgentJob {
    pub manifest: AgentOutput,
    pub prompt: String,
    pub system_prompt: Vec<String>,
    pub allowed_tools: BTreeSet<String>,
    pub mcp_bridge: Option<McpBridge>,
}

pub(crate) const DEFAULT_AGENT_MODEL: &str = "claude-opus-4-6";
pub(crate) const DEFAULT_AGENT_SYSTEM_DATE: &str = "2026-03-31";
pub(crate) const DEFAULT_AGENT_MAX_ITERATIONS: usize = 32;

/// Global MCP bridge that sub-agents inherit from the main agent.
/// Set once during CLI startup via `set_global_mcp_bridge()`.
pub(crate) static GLOBAL_MCP_BRIDGE: std::sync::OnceLock<McpBridge> = std::sync::OnceLock::new();

/// Register the MCP bridge so that sub-agents can access MCP tools.
/// Should be called once from the CLI startup after the `McpServerManager` is created.
pub fn set_global_mcp_bridge(bridge: McpBridge) {
    let _ = GLOBAL_MCP_BRIDGE.set(bridge);
}

pub(crate) fn execute_agent(input: AgentInput) -> Result<AgentOutput, String> {
    execute_agent_with_spawn(input, spawn_agent_job)
}

pub(crate) fn execute_agent_with_spawn<F>(
    input: AgentInput,
    spawn_fn: F,
) -> Result<AgentOutput, String>
where
    F: FnOnce(AgentJob) -> Result<(), String>,
{
    if input.description.trim().is_empty() {
        return Err(String::from("description must not be empty"));
    }
    if input.prompt.trim().is_empty() {
        return Err(String::from("prompt must not be empty"));
    }

    let agent_id = make_agent_id();
    let output_dir = agent_store_dir()?;
    std::fs::create_dir_all(&output_dir).map_err(|error| error.to_string())?;
    let output_file = output_dir.join(format!("{agent_id}.md"));
    let manifest_file = output_dir.join(format!("{agent_id}.json"));
    let normalized_subagent_type = normalize_subagent_type(input.subagent_type.as_deref());
    let model = resolve_agent_model(input.model.as_deref());
    let agent_name = input
        .name
        .as_deref()
        .map(slugify_agent_name)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| slugify_agent_name(&input.description));
    let created_at = iso8601_now();
    let system_prompt = build_agent_system_prompt(&normalized_subagent_type)?;
    let allowed_tools = allowed_tools_for_subagent(&normalized_subagent_type);

    let output_contents = format!(
        "# Agent Task

- id: {}
- name: {}
- description: {}
- subagent_type: {}
- created_at: {}

## Prompt

{}
",
        agent_id, agent_name, input.description, normalized_subagent_type, created_at, input.prompt
    );
    std::fs::write(&output_file, output_contents).map_err(|error| error.to_string())?;

    let manifest = AgentOutput {
        agent_id,
        name: agent_name,
        description: input.description,
        subagent_type: Some(normalized_subagent_type),
        model: Some(model),
        status: String::from("running"),
        output_file: output_file.display().to_string(),
        manifest_file: manifest_file.display().to_string(),
        created_at: created_at.clone(),
        started_at: Some(created_at),
        completed_at: None,
        error: None,
    };
    write_agent_manifest(&manifest)?;

    let manifest_for_spawn = manifest.clone();
    let job = AgentJob {
        manifest: manifest_for_spawn,
        prompt: input.prompt,
        system_prompt,
        allowed_tools,
        mcp_bridge: GLOBAL_MCP_BRIDGE.get().cloned(),
    };
    if let Err(error) = spawn_fn(job) {
        let error = format!("failed to spawn sub-agent: {error}");
        persist_agent_terminal_state(&manifest, "failed", None, Some(error.clone()))?;
        return Err(error);
    }

    Ok(manifest)
}

pub(crate) fn spawn_agent_job(job: AgentJob) -> Result<(), String> {
    let thread_name = format!("colotcook-agent-{}", job.manifest.agent_id);
    std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_agent_job(&job)));
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    let _ =
                        persist_agent_terminal_state(&job.manifest, "failed", None, Some(error));
                }
                Err(_) => {
                    let _ = persist_agent_terminal_state(
                        &job.manifest,
                        "failed",
                        None,
                        Some(String::from("sub-agent thread panicked")),
                    );
                }
            }
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub(crate) fn run_agent_job(job: &AgentJob) -> Result<(), String> {
    let mut runtime = build_agent_runtime(job)?.with_max_iterations(DEFAULT_AGENT_MAX_ITERATIONS);
    let summary = runtime
        .run_turn(job.prompt.clone(), None)
        .map_err(|error| error.to_string())?;
    let final_text = final_assistant_text(&summary);
    persist_agent_terminal_state(&job.manifest, "completed", Some(final_text.as_str()), None)
}

pub(crate) fn build_agent_runtime(
    job: &AgentJob,
) -> Result<ConversationRuntime<ProviderRuntimeClient, SubagentToolExecutor>, String> {
    let model = job
        .manifest
        .model
        .clone()
        .unwrap_or_else(|| DEFAULT_AGENT_MODEL.to_string());
    let allowed_tools = job.allowed_tools.clone();
    let api_client = ProviderRuntimeClient::new(model, allowed_tools.clone())?;
    let mut tool_executor = SubagentToolExecutor::new(allowed_tools);

    // Give sub-agents access to MCP tools if a bridge is available.
    if let Some(ref bridge) = job.mcp_bridge {
        tool_executor = tool_executor.with_mcp_bridge(bridge.clone());
    }

    Ok(ConversationRuntime::new(
        Session::new(),
        api_client,
        tool_executor,
        agent_permission_policy(),
        job.system_prompt.clone(),
    ))
}

pub(crate) fn build_agent_system_prompt(subagent_type: &str) -> Result<Vec<String>, String> {
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let mut prompt = load_system_prompt(
        cwd,
        DEFAULT_AGENT_SYSTEM_DATE.to_string(),
        std::env::consts::OS,
        "unknown",
    )
    .map_err(|error| error.to_string())?;
    prompt.push(format!(
        "You are a background sub-agent of type `{subagent_type}`. Work only on the delegated task, use only the tools available to you, do not ask the user questions, and finish with a concise result."
    ));
    Ok(prompt)
}

pub(crate) fn resolve_agent_model(model: Option<&str>) -> String {
    model
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .unwrap_or(DEFAULT_AGENT_MODEL)
        .to_string()
}

pub(crate) fn allowed_tools_for_subagent(subagent_type: &str) -> BTreeSet<String> {
    let tools = match subagent_type {
        "Explore" => vec![
            "read_file",
            "glob_search",
            "grep_search",
            "WebFetch",
            "WebSearch",
            "ToolSearch",
            "Skill",
            "StructuredOutput",
        ],
        "Plan" => vec![
            "read_file",
            "glob_search",
            "grep_search",
            "WebFetch",
            "WebSearch",
            "ToolSearch",
            "Skill",
            "TodoWrite",
            "StructuredOutput",
            "SendUserMessage",
        ],
        "Verification" => vec![
            "bash",
            "read_file",
            "glob_search",
            "grep_search",
            "WebFetch",
            "WebSearch",
            "ToolSearch",
            "TodoWrite",
            "StructuredOutput",
            "SendUserMessage",
            "PowerShell",
        ],
        "claw-guide" => vec![
            "read_file",
            "glob_search",
            "grep_search",
            "WebFetch",
            "WebSearch",
            "ToolSearch",
            "Skill",
            "StructuredOutput",
            "SendUserMessage",
        ],
        "statusline-setup" => vec![
            "bash",
            "read_file",
            "write_file",
            "edit_file",
            "glob_search",
            "grep_search",
            "ToolSearch",
        ],
        _ => vec![
            "bash",
            "read_file",
            "write_file",
            "edit_file",
            "glob_search",
            "grep_search",
            "WebFetch",
            "WebSearch",
            "TodoWrite",
            "Skill",
            "ToolSearch",
            "NotebookEdit",
            "Sleep",
            "SendUserMessage",
            "Config",
            "StructuredOutput",
            "REPL",
            "PowerShell",
        ],
    };
    tools.into_iter().map(str::to_string).collect()
}

pub(crate) fn agent_permission_policy() -> PermissionPolicy {
    mvp_tool_specs().into_iter().fold(
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        |policy, spec| policy.with_tool_requirement(spec.name, spec.required_permission),
    )
}

// ---------------------------------------------------------------------------
// Multi-Agent Orchestrator
// ---------------------------------------------------------------------------

/// Request to spawn an agent as part of an orchestrated batch.
pub struct AgentSpawnRequest {
    pub description: String,
    pub prompt: String,
    pub subagent_type: Option<String>,
    pub model: Option<String>,
}

/// Result from a completed agent in an orchestrated batch.
#[derive(Debug, Clone)]
pub struct AgentResult {
    pub agent_id: String,
    pub name: String,
    pub status: String,
    pub output: Option<String>,
    pub error: Option<String>,
}

/// Orchestrates parallel execution of multiple sub-agents, waits for completion,
/// and collects results.
pub struct AgentOrchestrator;

impl AgentOrchestrator {
    /// Spawn multiple agents in parallel and wait for all of them to complete.
    ///
    /// Returns results in the same order as the input requests.
    #[allow(clippy::too_many_lines)] // Will be split in Phase 2
    pub fn run_parallel(requests: Vec<AgentSpawnRequest>, timeout: Duration) -> Vec<AgentResult> {
        let mut handles: Vec<(AgentOutput, std::thread::JoinHandle<()>)> = Vec::new();

        for req in requests {
            let input = AgentInput {
                description: req.description,
                prompt: req.prompt.clone(),
                subagent_type: req.subagent_type,
                model: req.model,
                name: None,
            };

            let normalized_subagent_type = normalize_subagent_type(input.subagent_type.as_deref());
            let model = resolve_agent_model(input.model.as_deref());
            let agent_id = make_agent_id();
            let Ok(output_dir) = agent_store_dir() else {
                continue;
            };
            let _ = std::fs::create_dir_all(&output_dir);
            let output_file = output_dir.join(format!("{agent_id}.md"));
            let manifest_file = output_dir.join(format!("{agent_id}.json"));
            let agent_name = slugify_agent_name(&input.description);
            let created_at = iso8601_now();
            let Ok(system_prompt) = build_agent_system_prompt(&normalized_subagent_type) else {
                continue;
            };
            let allowed_tools = allowed_tools_for_subagent(&normalized_subagent_type);

            let output_contents = format!(
                "# Agent Task\n\n- id: {}\n- name: {}\n\n## Prompt\n\n{}\n",
                agent_id, agent_name, input.prompt
            );
            let _ = std::fs::write(&output_file, output_contents);

            let manifest = AgentOutput {
                agent_id: agent_id.clone(),
                name: agent_name,
                description: input.description,
                subagent_type: Some(normalized_subagent_type),
                model: Some(model),
                status: String::from("running"),
                output_file: output_file.display().to_string(),
                manifest_file: manifest_file.display().to_string(),
                created_at: created_at.clone(),
                started_at: Some(created_at),
                completed_at: None,
                error: None,
            };
            let _ = write_agent_manifest(&manifest);

            let job = AgentJob {
                manifest: manifest.clone(),
                prompt: req.prompt,
                system_prompt,
                allowed_tools,
                mcp_bridge: GLOBAL_MCP_BRIDGE.get().cloned(),
            };

            let handle = std::thread::Builder::new()
                .name(format!("colotcook-agent-{agent_id}"))
                .spawn(move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        run_agent_job(&job)
                    }));
                    match result {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            let _ = persist_agent_terminal_state(
                                &job.manifest,
                                "failed",
                                None,
                                Some(error),
                            );
                        }
                        Err(_) => {
                            let _ = persist_agent_terminal_state(
                                &job.manifest,
                                "failed",
                                None,
                                Some(String::from("sub-agent thread panicked")),
                            );
                        }
                    }
                });

            if let Ok(h) = handle {
                handles.push((manifest, h));
            }
        }

        // Wait for all agents to complete with timeout.
        let deadline = Instant::now() + timeout;
        let mut results = Vec::with_capacity(handles.len());

        for (manifest, handle) in handles {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                results.push(AgentResult {
                    agent_id: manifest.agent_id,
                    name: manifest.name,
                    status: String::from("timeout"),
                    output: None,
                    error: Some(String::from("agent timed out")),
                });
                continue;
            }

            // Park the current thread until the agent finishes or we hit the deadline.
            let join_result = {
                let _parker = std::thread::current();
                // Simple polling with short sleeps (agent threads are typically short-lived).
                let start = Instant::now();
                loop {
                    if handle.is_finished() {
                        break handle.join();
                    }
                    if start.elapsed() >= remaining {
                        break Err(Box::new("timeout") as Box<dyn std::any::Any + Send>);
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            };

            // Read the final manifest to get the actual status and output.
            let final_manifest = std::fs::read_to_string(&manifest.manifest_file)
                .ok()
                .and_then(|s| serde_json::from_str::<AgentOutput>(&s).ok());

            let (status, output, error) = match final_manifest {
                Some(m) => {
                    let output = std::fs::read_to_string(&m.output_file).ok();
                    (m.status, output, m.error)
                }
                None => match join_result {
                    Ok(()) => (String::from("completed"), None, None),
                    Err(_) => (
                        String::from("failed"),
                        None,
                        Some(String::from("agent thread panicked")),
                    ),
                },
            };

            results.push(AgentResult {
                agent_id: manifest.agent_id,
                name: manifest.name,
                status,
                output,
                error,
            });
        }

        results
    }
}

pub(crate) fn write_agent_manifest(manifest: &AgentOutput) -> Result<(), String> {
    std::fs::write(
        &manifest.manifest_file,
        serde_json::to_string_pretty(manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

pub(crate) fn persist_agent_terminal_state(
    manifest: &AgentOutput,
    status: &str,
    result: Option<&str>,
    error: Option<String>,
) -> Result<(), String> {
    append_agent_output(
        &manifest.output_file,
        &format_agent_terminal_output(status, result, error.as_deref()),
    )?;
    let mut next_manifest = manifest.clone();
    next_manifest.status = status.to_string();
    next_manifest.completed_at = Some(iso8601_now());
    next_manifest.error = error;
    write_agent_manifest(&next_manifest)
}

pub(crate) fn append_agent_output(path: &str, suffix: &str) -> Result<(), String> {
    use std::io::Write as _;

    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    file.write_all(suffix.as_bytes())
        .map_err(|error| error.to_string())
}

pub(crate) fn format_agent_terminal_output(
    status: &str,
    result: Option<&str>,
    error: Option<&str>,
) -> String {
    let mut sections = vec![format!("\n## Result\n\n- status: {status}\n")];
    if let Some(result) = result.filter(|value| !value.trim().is_empty()) {
        sections.push(format!("\n### Final response\n\n{}\n", result.trim()));
    }
    if let Some(error) = error.filter(|value| !value.trim().is_empty()) {
        sections.push(format!("\n### Error\n\n{}\n", error.trim()));
    }
    sections.join("")
}

pub(crate) struct ProviderRuntimeClient {
    pub runtime: tokio::runtime::Runtime,
    pub client: ProviderClient,
    pub model: String,
    pub allowed_tools: BTreeSet<String>,
}

impl ProviderRuntimeClient {
    #[allow(clippy::needless_pass_by_value)]
    fn new(model: String, allowed_tools: BTreeSet<String>) -> Result<Self, String> {
        let model = resolve_model_alias(&model).clone();
        let client = ProviderClient::from_model(&model).map_err(|error| error.to_string())?;
        Ok(Self {
            runtime: tokio::runtime::Runtime::new().map_err(|error| error.to_string())?,
            client,
            model,
            allowed_tools,
        })
    }
}

impl ApiClient for ProviderRuntimeClient {
    #[allow(clippy::too_many_lines)]
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        let tools = tool_specs_for_allowed_tools(Some(&self.allowed_tools))
            .into_iter()
            .map(|spec| ToolDefinition {
                name: spec.name.to_string(),
                description: Some(spec.description.to_string()),
                input_schema: spec.input_schema,
            })
            .collect::<Vec<_>>();
        let message_request = MessageRequest {
            model: self.model.clone(),
            max_tokens: max_tokens_for_model(&self.model),
            messages: convert_messages(&request.messages),
            system: (!request.system_prompt.is_empty()).then(|| request.system_prompt.join("\n\n")),
            tools: (!tools.is_empty()).then_some(tools),
            tool_choice: (!self.allowed_tools.is_empty()).then_some(ToolChoice::Auto),
            stream: true,
        };

        self.runtime.block_on(async {
            let mut stream = self
                .client
                .stream_message(&message_request)
                .await
                .map_err(|error| RuntimeError::new(error.to_string()))?;
            let mut events = Vec::new();
            let mut pending_tools: BTreeMap<u32, (String, String, String)> = BTreeMap::new();
            let mut saw_stop = false;

            while let Some(event) = stream
                .next_event()
                .await
                .map_err(|error| RuntimeError::new(error.to_string()))?
            {
                match event {
                    ApiStreamEvent::MessageStart(start) => {
                        for block in start.message.content {
                            push_output_block(block, 0, &mut events, &mut pending_tools, true);
                        }
                    }
                    ApiStreamEvent::ContentBlockStart(start) => {
                        push_output_block(
                            start.content_block,
                            start.index,
                            &mut events,
                            &mut pending_tools,
                            true,
                        );
                    }
                    ApiStreamEvent::ContentBlockDelta(delta) => match delta.delta {
                        ContentBlockDelta::TextDelta { text } => {
                            if !text.is_empty() {
                                events.push(AssistantEvent::TextDelta(text));
                            }
                        }
                        ContentBlockDelta::InputJsonDelta { partial_json } => {
                            if let Some((_, _, input)) = pending_tools.get_mut(&delta.index) {
                                input.push_str(&partial_json);
                            }
                        }
                        ContentBlockDelta::ThinkingDelta { .. }
                        | ContentBlockDelta::SignatureDelta { .. } => {}
                    },
                    ApiStreamEvent::ContentBlockStop(stop) => {
                        if let Some((id, name, input)) = pending_tools.remove(&stop.index) {
                            events.push(AssistantEvent::ToolUse { id, name, input });
                        }
                    }
                    ApiStreamEvent::MessageDelta(delta) => {
                        events.push(AssistantEvent::Usage(delta.usage.token_usage()));
                    }
                    ApiStreamEvent::MessageStop(_) => {
                        saw_stop = true;
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
            let mut events = response_to_events(response);
            push_prompt_cache_record(&self.client, &mut events);
            Ok(events)
        })
    }
}

/// Bridge for sub-agents to access MCP tools through the shared `McpServerManager`.
#[derive(Clone)]
pub struct McpBridge {
    pub manager: std::sync::Arc<std::sync::Mutex<runtime::McpServerManager>>,
    pub mcp_runtime: std::sync::Arc<tokio::runtime::Runtime>,
}

impl McpBridge {
    /// Create a new MCP bridge from a shared manager and dedicated runtime.
    pub fn new(
        manager: std::sync::Arc<std::sync::Mutex<runtime::McpServerManager>>,
        mcp_runtime: std::sync::Arc<tokio::runtime::Runtime>,
    ) -> Self {
        Self {
            manager,
            mcp_runtime,
        }
    }

    /// Execute an MCP tool call, blocking the current thread.
    fn call_tool(&self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        let arguments: Option<Value> = if input.is_empty() || input == "{}" {
            None
        } else {
            Some(
                serde_json::from_str(input)
                    .map_err(|e| ToolError::new(format!("invalid MCP tool input JSON: {e}")))?,
            )
        };

        let tool_name_owned = tool_name.to_string();
        let mut manager = self
            .manager
            .lock()
            .map_err(|e| ToolError::new(format!("MCP manager lock poisoned: {e}")))?;

        let result = self
            .mcp_runtime
            .block_on(manager.call_tool(&tool_name_owned, arguments));

        match result {
            Ok(response) => {
                if let Some(ref error) = response.error {
                    return Err(ToolError::new(format!(
                        "MCP JSON-RPC error {}: {}",
                        error.code, error.message
                    )));
                }
                match response.result {
                    Some(call_result) => serde_json::to_string(&call_result).map_err(|e| {
                        ToolError::new(format!("failed to serialize MCP result: {e}"))
                    }),
                    None => Err(ToolError::new("MCP tool call returned no result")),
                }
            }
            Err(error) => Err(ToolError::new(format!("MCP tool call failed: {error}"))),
        }
    }
}

impl std::fmt::Debug for McpBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpBridge").finish_non_exhaustive()
    }
}

pub(crate) struct SubagentToolExecutor {
    pub allowed_tools: BTreeSet<String>,
    pub mcp_bridge: Option<McpBridge>,
}

impl SubagentToolExecutor {
    pub(crate) fn new(allowed_tools: BTreeSet<String>) -> Self {
        Self {
            allowed_tools,
            mcp_bridge: None,
        }
    }

    fn with_mcp_bridge(mut self, bridge: McpBridge) -> Self {
        self.mcp_bridge = Some(bridge);
        self
    }
}

impl ToolExecutor for SubagentToolExecutor {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        if !self.allowed_tools.contains(tool_name) {
            return Err(ToolError::new(format!(
                "tool `{tool_name}` is not enabled for this sub-agent"
            )));
        }

        // Route MCP tool calls through the bridge.
        if tool_name.starts_with("mcp__") {
            return match &self.mcp_bridge {
                Some(bridge) => bridge.call_tool(tool_name, input),
                None => Err(ToolError::new(
                    "MCP tools are not available for this sub-agent",
                )),
            };
        }

        let value = serde_json::from_str(input)
            .map_err(|error| ToolError::new(format!("invalid tool input JSON: {error}")))?;
        execute_tool(tool_name, &value).map_err(ToolError::new)
    }
}

pub(crate) fn tool_specs_for_allowed_tools(
    allowed_tools: Option<&BTreeSet<String>>,
) -> Vec<ToolSpec> {
    mvp_tool_specs()
        .into_iter()
        .filter(|spec| allowed_tools.is_none_or(|allowed| allowed.contains(spec.name)))
        .collect()
}

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

pub(crate) fn push_output_block(
    block: OutputContentBlock,
    block_index: u32,
    events: &mut Vec<AssistantEvent>,
    pending_tools: &mut BTreeMap<u32, (String, String, String)>,
    streaming_tool_input: bool,
) {
    match block {
        OutputContentBlock::Text { text } => {
            if !text.is_empty() {
                events.push(AssistantEvent::TextDelta(text));
            }
        }
        OutputContentBlock::ToolUse { id, name, input } => {
            let initial_input = if streaming_tool_input
                && input.is_object()
                && input.as_object().is_some_and(serde_json::Map::is_empty)
            {
                String::new()
            } else {
                input.to_string()
            };
            pending_tools.insert(block_index, (id, name, initial_input));
        }
        OutputContentBlock::Thinking { .. } | OutputContentBlock::RedactedThinking { .. } => {}
    }
}

pub(crate) fn response_to_events(response: MessageResponse) -> Vec<AssistantEvent> {
    let mut events = Vec::new();
    let mut pending_tools = BTreeMap::new();

    for (index, block) in response.content.into_iter().enumerate() {
        let index = u32::try_from(index).expect("response block index overflow");
        push_output_block(block, index, &mut events, &mut pending_tools, false);
        if let Some((id, name, input)) = pending_tools.remove(&index) {
            events.push(AssistantEvent::ToolUse { id, name, input });
        }
    }

    events.push(AssistantEvent::Usage(response.usage.token_usage()));
    events.push(AssistantEvent::MessageStop);
    events
}

pub(crate) fn push_prompt_cache_record(client: &ProviderClient, events: &mut Vec<AssistantEvent>) {
    if let Some(record) = client.take_last_prompt_cache_record() {
        if let Some(event) = prompt_cache_record_to_runtime_event(record) {
            events.push(AssistantEvent::PromptCache(event));
        }
    }
}

pub(crate) fn prompt_cache_record_to_runtime_event(
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

pub(crate) fn agent_store_dir() -> Result<std::path::PathBuf, String> {
    if let Ok(path) = std::env::var("COLOTCOOK_AGENT_STORE") {
        return Ok(std::path::PathBuf::from(path));
    }
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    if let Some(workspace_root) = cwd.ancestors().nth(2) {
        return Ok(workspace_root.join(".colotcook-agents"));
    }
    Ok(cwd.join(".colotcook-agents"))
}

pub(crate) fn make_agent_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("agent-{nanos}")
}

pub(crate) fn slugify_agent_name(description: &str) -> String {
    let mut out = description
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    out.trim_matches('-').chars().take(32).collect()
}

pub(crate) fn normalize_subagent_type(subagent_type: Option<&str>) -> String {
    let trimmed = subagent_type.map(str::trim).unwrap_or_default();
    if trimmed.is_empty() {
        return String::from("general-purpose");
    }

    match canonical_tool_token(trimmed).as_str() {
        "general" | "generalpurpose" | "generalpurposeagent" => String::from("general-purpose"),
        "explore" | "explorer" | "exploreagent" => String::from("Explore"),
        "plan" | "planagent" => String::from("Plan"),
        "verification" | "verificationagent" | "verify" | "verifier" => {
            String::from("Verification")
        }
        "clawguide" | "clawguideagent" | "guide" => String::from("claw-guide"),
        "statusline" | "statuslinesetup" => String::from("statusline-setup"),
        _ => trimmed.to_string(),
    }
}

pub(crate) fn iso8601_now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}
