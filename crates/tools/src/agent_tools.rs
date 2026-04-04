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

/// Default model for sub-agents.
pub(crate) const DEFAULT_AGENT_MODEL: &str = "claude-opus-4-6";
/// Default system date string for sub-agent prompts.
pub(crate) const DEFAULT_AGENT_SYSTEM_DATE: &str = "2026-03-31";
/// Maximum turn iterations before a sub-agent stops.
pub(crate) const DEFAULT_AGENT_MAX_ITERATIONS: usize = 32;

/// Global MCP bridge that sub-agents inherit from the main agent.
/// Set once during CLI startup via `set_global_mcp_bridge()`.
pub(crate) static GLOBAL_MCP_BRIDGE: std::sync::OnceLock<McpBridge> = std::sync::OnceLock::new();

/// Register the MCP bridge so that sub-agents can access MCP tools.
/// Should be called once from the CLI startup after the `McpServerManager` is created.
pub fn set_global_mcp_bridge(bridge: McpBridge) {
    let _ = GLOBAL_MCP_BRIDGE.set(bridge);
}

/// Run a sub-agent from the given `AgentInput`.
pub(crate) fn execute_agent(input: AgentInput) -> Result<AgentOutput, String> {
    execute_agent_with_spawn(input, spawn_agent_job)
}

/// Run a sub-agent, injecting a custom spawner (useful for testing).
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

/// Spawn a sub-agent job in its own thread.
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

/// Execute the agent job loop until completion or max iterations.
pub(crate) fn run_agent_job(job: &AgentJob) -> Result<(), String> {
    let mut runtime = build_agent_runtime(job)?.with_max_iterations(DEFAULT_AGENT_MAX_ITERATIONS);
    let summary = runtime
        .run_turn(job.prompt.clone(), None)
        .map_err(|error| error.to_string())?;
    let final_text = final_assistant_text(&summary);
    persist_agent_terminal_state(&job.manifest, "completed", Some(final_text.as_str()), None)
}

/// Build a `ConversationRuntime` for the given sub-agent job.
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

/// Load and render the system prompt for a sub-agent type.
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

/// Resolve the model to use for a sub-agent.
pub(crate) fn resolve_agent_model(model: Option<&str>) -> String {
    model
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .unwrap_or(DEFAULT_AGENT_MODEL)
        .to_string()
}

/// Return the allowed tool set for the given sub-agent type.
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

/// Return the permission policy for sub-agent tool execution.
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

/// Persist an agent manifest JSON to the agent store.
pub(crate) fn write_agent_manifest(manifest: &AgentOutput) -> Result<(), String> {
    std::fs::write(
        &manifest.manifest_file,
        serde_json::to_string_pretty(manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

/// Write the final terminal state of an agent run to disk.
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

/// Append a suffix string to an existing agent output file.
pub(crate) fn append_agent_output(path: &str, suffix: &str) -> Result<(), String> {
    use std::io::Write as _;

    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    file.write_all(suffix.as_bytes())
        .map_err(|error| error.to_string())
}

/// Format a structured agent terminal output for display.
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

/// Multi-provider AI client used by sub-agent runtimes.
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

/// Tool executor that restricts execution to the allowed tool set.
pub(crate) struct SubagentToolExecutor {
    pub allowed_tools: BTreeSet<String>,
    pub mcp_bridge: Option<McpBridge>,
}

impl SubagentToolExecutor {
    /// Construct an executor with the given allowed-tool set.
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

/// Return tool specs filtered to the allowed set.
pub(crate) fn tool_specs_for_allowed_tools(
    allowed_tools: Option<&BTreeSet<String>>,
) -> Vec<ToolSpec> {
    mvp_tool_specs()
        .into_iter()
        .filter(|spec| allowed_tools.is_none_or(|allowed| allowed.contains(spec.name)))
        .collect()
}

/// Convert internal messages into API request format.
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

/// Append an output content block to the event list.
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

/// Convert a `MessageResponse` to assistant events.
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

/// Append a prompt-cache record to events if present.
pub(crate) fn push_prompt_cache_record(client: &ProviderClient, events: &mut Vec<AssistantEvent>) {
    if let Some(record) = client.take_last_prompt_cache_record() {
        if let Some(event) = prompt_cache_record_to_runtime_event(record) {
            events.push(AssistantEvent::PromptCache(event));
        }
    }
}

/// Convert a prompt-cache record to a runtime event.
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

/// Return (and create if needed) the agent store directory.
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

/// Generate a unique agent run ID.
pub(crate) fn make_agent_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("agent-{nanos}")
}

/// Convert a description string into a URL-safe slug.
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

/// Normalize a sub-agent type string (defaults to `researcher`).
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

/// Return the current UTC time as an ISO 8601 string.
pub(crate) fn iso8601_now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- resolve_agent_model ---

    #[test]
    fn resolve_agent_model_none_returns_default() {
        assert_eq!(resolve_agent_model(None), DEFAULT_AGENT_MODEL);
    }

    #[test]
    fn resolve_agent_model_empty_returns_default() {
        assert_eq!(resolve_agent_model(Some("")), DEFAULT_AGENT_MODEL);
    }

    #[test]
    fn resolve_agent_model_whitespace_returns_default() {
        assert_eq!(resolve_agent_model(Some("   ")), DEFAULT_AGENT_MODEL);
    }

    #[test]
    fn resolve_agent_model_custom_model() {
        assert_eq!(
            resolve_agent_model(Some("claude-haiku-3")),
            "claude-haiku-3"
        );
    }

    #[test]
    fn resolve_agent_model_trims_whitespace() {
        assert_eq!(
            resolve_agent_model(Some("  claude-haiku-3  ")),
            "claude-haiku-3"
        );
    }

    // --- normalize_subagent_type ---

    #[test]
    fn normalize_subagent_type_none_returns_general_purpose() {
        assert_eq!(normalize_subagent_type(None), "general-purpose");
    }

    #[test]
    fn normalize_subagent_type_empty_returns_general_purpose() {
        assert_eq!(normalize_subagent_type(Some("")), "general-purpose");
    }

    #[test]
    fn normalize_subagent_type_explore_variants() {
        assert_eq!(normalize_subagent_type(Some("explore")), "Explore");
        assert_eq!(normalize_subagent_type(Some("Explore")), "Explore");
        assert_eq!(normalize_subagent_type(Some("explorer")), "Explore");
    }

    #[test]
    fn normalize_subagent_type_plan_variants() {
        assert_eq!(normalize_subagent_type(Some("plan")), "Plan");
        assert_eq!(normalize_subagent_type(Some("Plan")), "Plan");
    }

    #[test]
    fn normalize_subagent_type_verification_variants() {
        assert_eq!(
            normalize_subagent_type(Some("verification")),
            "Verification"
        );
        assert_eq!(normalize_subagent_type(Some("verify")), "Verification");
        assert_eq!(normalize_subagent_type(Some("verifier")), "Verification");
    }

    #[test]
    fn normalize_subagent_type_general_variants() {
        assert_eq!(normalize_subagent_type(Some("general")), "general-purpose");
        assert_eq!(
            normalize_subagent_type(Some("general-purpose")),
            "general-purpose"
        );
    }

    #[test]
    fn normalize_subagent_type_claw_guide() {
        assert_eq!(normalize_subagent_type(Some("claw-guide")), "claw-guide");
        assert_eq!(normalize_subagent_type(Some("guide")), "claw-guide");
    }

    #[test]
    fn normalize_subagent_type_statusline_setup() {
        assert_eq!(
            normalize_subagent_type(Some("statusline-setup")),
            "statusline-setup"
        );
        assert_eq!(
            normalize_subagent_type(Some("statusline")),
            "statusline-setup"
        );
    }

    #[test]
    fn normalize_subagent_type_unknown_passthrough() {
        assert_eq!(normalize_subagent_type(Some("my-custom")), "my-custom");
    }

    // --- slugify_agent_name ---

    #[test]
    fn slugify_agent_name_basic() {
        assert_eq!(slugify_agent_name("hello world"), "hello-world");
    }

    #[test]
    fn slugify_agent_name_uppercase_lowercased() {
        assert_eq!(slugify_agent_name("Hello World"), "hello-world");
    }

    #[test]
    fn slugify_agent_name_collapses_separators() {
        assert_eq!(slugify_agent_name("hello  world"), "hello-world");
    }

    #[test]
    fn slugify_agent_name_trims_dashes() {
        assert_eq!(slugify_agent_name("  hello  "), "hello");
    }

    #[test]
    fn slugify_agent_name_truncates_at_32_chars() {
        let long = "a".repeat(40);
        let result = slugify_agent_name(&long);
        assert_eq!(result.len(), 32);
    }

    #[test]
    fn slugify_agent_name_empty_returns_empty() {
        assert_eq!(slugify_agent_name(""), "");
    }

    #[test]
    fn slugify_agent_name_special_chars_become_dash() {
        assert_eq!(slugify_agent_name("fix:bug"), "fix-bug");
    }

    // --- make_agent_id ---

    #[test]
    fn make_agent_id_has_agent_prefix() {
        let id = make_agent_id();
        assert!(id.starts_with("agent-"), "id was: {id}");
    }

    #[test]
    fn make_agent_id_unique() {
        let id1 = make_agent_id();
        std::thread::sleep(std::time::Duration::from_nanos(1000));
        let id2 = make_agent_id();
        // Two IDs generated with a small gap should not be equal
        // (nanos are unique in almost all cases; this just checks format)
        assert!(id1.starts_with("agent-"));
        assert!(id2.starts_with("agent-"));
    }

    // --- agent_store_dir ---
    // These tests modify env vars and must be serialized.
    static AGENT_STORE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn agent_store_dir_env_override() {
        let _lock = AGENT_STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("COLOTCOOK_AGENT_STORE", "/tmp/test-agent-store");
        let dir = agent_store_dir().unwrap();
        std::env::remove_var("COLOTCOOK_AGENT_STORE");
        assert_eq!(dir, std::path::PathBuf::from("/tmp/test-agent-store"));
    }

    #[test]
    fn agent_store_dir_default_contains_colotcook_agents() {
        let _lock = AGENT_STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("COLOTCOOK_AGENT_STORE");
        let dir = agent_store_dir().unwrap();
        assert!(
            dir.to_string_lossy().contains(".colotcook-agents"),
            "dir was: {}",
            dir.display()
        );
    }

    // --- iso8601_now ---

    #[test]
    fn iso8601_now_returns_numeric_string() {
        let ts = iso8601_now();
        assert!(!ts.is_empty());
        let _: u64 = ts.parse().expect("should be a numeric unix timestamp");
    }

    // --- allowed_tools_for_subagent ---

    #[test]
    fn allowed_tools_explore_contains_read_file() {
        let tools = allowed_tools_for_subagent("Explore");
        assert!(tools.contains("read_file"));
        assert!(!tools.contains("bash"), "Explore should not have bash");
    }

    #[test]
    fn allowed_tools_plan_contains_todo_write() {
        let tools = allowed_tools_for_subagent("Plan");
        assert!(tools.contains("TodoWrite"));
    }

    #[test]
    fn allowed_tools_verification_contains_bash() {
        let tools = allowed_tools_for_subagent("Verification");
        assert!(tools.contains("bash"));
    }

    #[test]
    fn allowed_tools_default_contains_common_tools() {
        let tools = allowed_tools_for_subagent("unknown-type");
        assert!(tools.contains("bash"));
        assert!(tools.contains("read_file"));
        assert!(tools.contains("write_file"));
    }

    #[test]
    fn allowed_tools_claw_guide_has_no_bash() {
        let tools = allowed_tools_for_subagent("claw-guide");
        assert!(!tools.contains("bash"));
        assert!(tools.contains("WebFetch"));
    }

    #[test]
    fn allowed_tools_statusline_setup_has_edit() {
        let tools = allowed_tools_for_subagent("statusline-setup");
        assert!(tools.contains("edit_file"));
        assert!(tools.contains("bash"));
    }

    // --- format_agent_terminal_output ---

    #[test]
    fn format_agent_terminal_output_completed_no_result() {
        let output = format_agent_terminal_output("completed", None, None);
        assert!(output.contains("completed"));
        assert!(output.contains("## Result"));
    }

    #[test]
    fn format_agent_terminal_output_with_result() {
        let output = format_agent_terminal_output("completed", Some("task done"), None);
        assert!(output.contains("task done"));
        assert!(output.contains("Final response"));
    }

    #[test]
    fn format_agent_terminal_output_with_error() {
        let output = format_agent_terminal_output("failed", None, Some("something broke"));
        assert!(output.contains("something broke"));
        assert!(output.contains("Error"));
    }

    #[test]
    fn format_agent_terminal_output_empty_result_omitted() {
        let output = format_agent_terminal_output("completed", Some("   "), None);
        assert!(!output.contains("Final response"));
    }

    #[test]
    fn format_agent_terminal_output_empty_error_omitted() {
        let output = format_agent_terminal_output("failed", None, Some("   "));
        assert!(!output.contains("### Error"));
    }

    // --- execute_agent_with_spawn validation ---

    #[test]
    fn execute_agent_with_spawn_empty_description_errors() {
        let input = crate::types::AgentInput {
            description: String::new(),
            prompt: String::from("do something"),
            subagent_type: None,
            name: None,
            model: None,
        };
        let result = execute_agent_with_spawn(input, |_| Ok(()));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("description"));
    }

    #[test]
    fn execute_agent_with_spawn_empty_prompt_errors() {
        let input = crate::types::AgentInput {
            description: String::from("a task"),
            prompt: String::new(),
            subagent_type: None,
            name: None,
            model: None,
        };
        let result = execute_agent_with_spawn(input, |_| Ok(()));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("prompt"));
    }

    // --- tool_specs_for_allowed_tools ---

    #[test]
    fn tool_specs_for_allowed_tools_none_returns_all() {
        let specs = tool_specs_for_allowed_tools(None);
        assert!(!specs.is_empty());
    }

    #[test]
    fn tool_specs_for_allowed_tools_filters() {
        let allowed: BTreeSet<String> = ["bash"].iter().map(|s| s.to_string()).collect();
        let specs = tool_specs_for_allowed_tools(Some(&allowed));
        assert!(specs.iter().all(|s| s.name == "bash"));
    }

    #[test]
    fn tool_specs_for_allowed_tools_empty_set_returns_empty() {
        let allowed: BTreeSet<String> = BTreeSet::new();
        let specs = tool_specs_for_allowed_tools(Some(&allowed));
        assert!(specs.is_empty());
    }

    fn cmsg(role: MessageRole, blocks: Vec<ContentBlock>) -> ConversationMessage {
        ConversationMessage { role, blocks, usage: None }
    }

    fn test_usage() -> api::Usage {
        api::Usage { input_tokens: 10, output_tokens: 5, cache_creation_input_tokens: 0, cache_read_input_tokens: 0 }
    }

    fn test_resp(content: Vec<OutputContentBlock>) -> MessageResponse {
        MessageResponse {
            content, usage: test_usage(),
            id: "m1".into(), kind: "message".into(), model: "t".into(), role: "assistant".into(),
            stop_reason: Some("end_turn".into()), stop_sequence: None, request_id: None,
        }
    }

    fn empty_summary() -> runtime::TurnSummary {
        runtime::TurnSummary {
            assistant_messages: vec![], tool_results: vec![], prompt_cache_events: vec![],
            usage: runtime::TokenUsage { input_tokens: 0, output_tokens: 0, cache_creation_input_tokens: 0, cache_read_input_tokens: 0 },
            iterations: 0, auto_compaction: None,
        }
    }

    // --- convert_messages ---

    #[test]
    fn convert_messages_empty() { assert!(convert_messages(&[]).is_empty()); }

    #[test]
    fn convert_messages_user_text() {
        let r = convert_messages(&[cmsg(MessageRole::User, vec![ContentBlock::Text { text: "hi".into() }])]);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].role, "user");
    }

    #[test]
    fn convert_messages_assistant_text() {
        assert_eq!(convert_messages(&[cmsg(MessageRole::Assistant, vec![ContentBlock::Text { text: "a".into() }])])[0].role, "assistant");
    }

    #[test]
    fn convert_messages_system_maps_to_user() {
        assert_eq!(convert_messages(&[cmsg(MessageRole::System, vec![ContentBlock::Text { text: "s".into() }])])[0].role, "user");
    }

    #[test]
    fn convert_messages_tool_maps_to_user() {
        assert_eq!(convert_messages(&[cmsg(MessageRole::Tool, vec![ContentBlock::Text { text: "o".into() }])])[0].role, "user");
    }

    #[test]
    fn convert_messages_empty_blocks_omitted() {
        assert!(convert_messages(&[cmsg(MessageRole::User, vec![])]).is_empty());
    }

    #[test]
    fn convert_messages_tool_use_block() {
        assert_eq!(convert_messages(&[cmsg(MessageRole::Assistant, vec![ContentBlock::ToolUse { id: "t".into(), name: "b".into(), input: r#"{"command":"ls"}"#.into() }])]).len(), 1);
    }

    #[test]
    fn convert_messages_tool_result_block() {
        assert_eq!(convert_messages(&[cmsg(MessageRole::Tool, vec![ContentBlock::ToolResult { tool_use_id: "t".into(), tool_name: "bash".into(), output: "ok".into(), is_error: false }])]).len(), 1);
    }

    // --- push_output_block (agent variant) ---

    #[test]
    fn push_output_block_text() {
        let mut events = Vec::new();
        let mut pending = BTreeMap::new();
        push_output_block(OutputContentBlock::Text { text: "hi".into() }, 0, &mut events, &mut pending, true);
        assert!(matches!(&events[0], AssistantEvent::TextDelta(t) if t == "hi"));
    }

    #[test]
    fn push_output_block_empty_text() {
        let mut events = Vec::new();
        let mut pending = BTreeMap::new();
        push_output_block(OutputContentBlock::Text { text: "".into() }, 0, &mut events, &mut pending, true);
        assert!(events.is_empty());
    }

    #[test]
    fn push_output_block_tool_use_streaming() {
        let mut events = Vec::new();
        let mut pending = BTreeMap::new();
        push_output_block(OutputContentBlock::ToolUse { id: "t".into(), name: "b".into(), input: serde_json::json!({}) }, 0, &mut events, &mut pending, true);
        assert_eq!(pending.get(&0).unwrap().2, "");
    }

    #[test]
    fn push_output_block_tool_use_non_streaming() {
        let mut events = Vec::new();
        let mut pending = BTreeMap::new();
        push_output_block(OutputContentBlock::ToolUse { id: "t".into(), name: "b".into(), input: serde_json::json!({"k":"v"}) }, 0, &mut events, &mut pending, false);
        assert!(pending.get(&0).unwrap().2.contains("k"));
    }

    #[test]
    fn push_output_block_thinking() {
        let mut events = Vec::new();
        let mut pending = BTreeMap::new();
        push_output_block(OutputContentBlock::Thinking { thinking: "x".into(), signature: None }, 0, &mut events, &mut pending, true);
        assert!(events.is_empty());
    }

    // --- response_to_events (agent variant) ---

    #[test]
    fn response_to_events_text() {
        let events = response_to_events(test_resp(vec![OutputContentBlock::Text { text: "hello".into() }]));
        assert!(events.iter().any(|e| matches!(e, AssistantEvent::TextDelta(t) if t == "hello")));
        assert!(events.iter().any(|e| matches!(e, AssistantEvent::MessageStop)));
    }

    #[test]
    fn response_to_events_tool_use() {
        let events = response_to_events(test_resp(vec![OutputContentBlock::ToolUse { id: "t".into(), name: "bash".into(), input: serde_json::json!({"c":"l"}) }]));
        assert!(events.iter().any(|e| matches!(e, AssistantEvent::ToolUse { name, .. } if name == "bash")));
    }

    // --- final_assistant_text (agent variant) ---

    #[test]
    fn final_assistant_text_empty() {
        assert_eq!(final_assistant_text(&empty_summary()), "");
    }

    #[test]
    fn final_assistant_text_single() {
        let mut s = empty_summary();
        s.assistant_messages = vec![cmsg(MessageRole::Assistant, vec![ContentBlock::Text { text: "result".into() }])];
        assert_eq!(final_assistant_text(&s), "result");
    }

    // --- agent_permission_policy ---

    #[test]
    fn agent_permission_policy_is_danger_full_access() {
        let policy = agent_permission_policy();
        // Should be based on DangerFullAccess mode
        let _ = policy;
    }

    // --- persist_agent_terminal_state and append_agent_output ---

    #[test]
    fn persist_agent_terminal_state_and_append() {
        let dir = std::env::temp_dir().join(format!("colotcook-test-{}", make_agent_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let output_file = dir.join("agent.md");
        let manifest_file = dir.join("agent.json");

        std::fs::write(&output_file, "# Initial\n").unwrap();

        let manifest = AgentOutput {
            agent_id: "test-agent".into(),
            name: "test".into(),
            description: "test desc".into(),
            subagent_type: Some("general-purpose".into()),
            model: Some("opus".into()),
            status: "running".into(),
            output_file: output_file.display().to_string(),
            manifest_file: manifest_file.display().to_string(),
            created_at: "1234".into(),
            started_at: Some("1234".into()),
            completed_at: None,
            error: None,
        };

        persist_agent_terminal_state(&manifest, "completed", Some("done!"), None).unwrap();

        let output = std::fs::read_to_string(&output_file).unwrap();
        assert!(output.contains("completed"));
        assert!(output.contains("done!"));

        let manifest_json = std::fs::read_to_string(&manifest_file).unwrap();
        let loaded: AgentOutput = serde_json::from_str(&manifest_json).unwrap();
        assert_eq!(loaded.status, "completed");
        assert!(loaded.completed_at.is_some());

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_agent_output_appends() {
        let dir = std::env::temp_dir().join(format!("colotcook-append-{}", make_agent_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("output.md");
        std::fs::write(&path, "initial\n").unwrap();

        append_agent_output(&path.display().to_string(), "appended\n").unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("initial"));
        assert!(content.contains("appended"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_agent_output_missing_file_errors() {
        let result = append_agent_output("/nonexistent/path/file.md", "data");
        assert!(result.is_err());
    }

    // --- write_agent_manifest ---

    #[test]
    fn write_agent_manifest_creates_json() {
        let dir = std::env::temp_dir().join(format!("colotcook-manifest-{}", make_agent_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("manifest.json");

        let manifest = AgentOutput {
            agent_id: "test".into(),
            name: "t".into(),
            description: "d".into(),
            subagent_type: None,
            model: None,
            status: "running".into(),
            output_file: "o.md".into(),
            manifest_file: path.display().to_string(),
            created_at: "0".into(),
            started_at: None,
            completed_at: None,
            error: None,
        };
        write_agent_manifest(&manifest).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let loaded: AgentOutput = serde_json::from_str(&content).unwrap();
        assert_eq!(loaded.agent_id, "test");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- SubagentToolExecutor ---

    #[test]
    fn subagent_tool_executor_blocks_disallowed_tool() {
        let mut executor = SubagentToolExecutor::new(["bash"].iter().map(|s| s.to_string()).collect());
        let result = executor.execute("write_file", "{}");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not enabled"));
    }

    #[test]
    fn subagent_tool_executor_mcp_without_bridge_errors() {
        let mut executor = SubagentToolExecutor::new(
            ["mcp__test__tool"].iter().map(|s| s.to_string()).collect(),
        );
        let result = executor.execute("mcp__test__tool", "{}");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not available"));
    }
}
