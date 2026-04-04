//! Input and output types for all built-in tool implementations.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize)]
/// Input for the `read_file` tool.
pub(crate) struct ReadFileInput {
    pub path: String,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
/// Input for the `write_file` tool.
pub(crate) struct WriteFileInput {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
/// Input for the `edit_file` tool.
pub(crate) struct EditFileInput {
    pub path: String,
    pub old_string: String,
    pub new_string: String,
    pub replace_all: Option<bool>,
}

#[derive(Debug, Deserialize)]
/// Input for the `glob_search` tool.
pub(crate) struct GlobSearchInputValue {
    pub pattern: String,
    pub path: Option<String>,
}

#[derive(Debug, Deserialize)]
/// Input for the `web_fetch` tool.
pub(crate) struct WebFetchInput {
    pub url: String,
    pub prompt: String,
}

#[derive(Debug, Deserialize)]
/// Input for the `web_search` tool.
pub(crate) struct WebSearchInput {
    pub query: String,
    pub allowed_domains: Option<Vec<String>>,
    pub blocked_domains: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
/// Input for the `todo_write` tool.
pub(crate) struct TodoWriteInput {
    pub todos: Vec<TodoItem>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
/// A single to-do item.
pub(crate) struct TodoItem {
    pub content: String,
    #[serde(rename = "activeForm")]
    pub active_form: String,
    pub status: TodoStatus,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
/// Status of a to-do item.
pub(crate) enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Deserialize)]
/// Input for the `skill` tool.
pub(crate) struct SkillInput {
    pub skill: String,
    pub args: Option<String>,
}

#[derive(Debug, Deserialize)]
/// Input for the `agent` tool.
pub(crate) struct AgentInput {
    pub description: String,
    pub prompt: String,
    pub subagent_type: Option<String>,
    pub name: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Deserialize)]
/// Input for the `tool_search` tool.
pub(crate) struct ToolSearchInput {
    pub query: String,
    pub max_results: Option<usize>,
}

#[derive(Debug, Deserialize)]
/// Input for the `notebook_edit` tool.
pub(crate) struct NotebookEditInput {
    pub notebook_path: String,
    pub cell_id: Option<String>,
    pub new_source: Option<String>,
    pub cell_type: Option<NotebookCellType>,
    pub edit_mode: Option<NotebookEditMode>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
/// Type of a Jupyter notebook cell.
pub(crate) enum NotebookCellType {
    Code,
    Markdown,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
/// Mode for a notebook cell edit operation.
pub(crate) enum NotebookEditMode {
    Replace,
    Insert,
    Delete,
}

#[derive(Debug, Deserialize)]
/// Input for the `sleep` tool.
pub(crate) struct SleepInput {
    pub duration_ms: u64,
}

#[derive(Debug, Deserialize)]
/// Input for the `brief` tool.
pub(crate) struct BriefInput {
    pub message: String,
    pub attachments: Option<Vec<String>>,
    pub status: BriefStatus,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
/// Status code for a brief message.
pub(crate) enum BriefStatus {
    Normal,
    Proactive,
}

#[derive(Debug, Deserialize)]
/// Input for the `config` tool.
pub(crate) struct ConfigInput {
    pub setting: String,
    pub value: Option<ConfigValue>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
/// Input for the `enter_plan_mode` tool (no fields).
pub(crate) struct EnterPlanModeInput {}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
/// Input for the `exit_plan_mode` tool (no fields).
pub(crate) struct ExitPlanModeInput {}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
/// A typed configuration value.
pub(crate) enum ConfigValue {
    String(String),
    Bool(bool),
    Number(f64),
}

#[derive(Debug, Deserialize)]
#[serde(transparent)]
/// Input for the `structured_output` tool.
pub(crate) struct StructuredOutputInput(pub BTreeMap<String, Value>);

#[derive(Debug, Deserialize)]
/// Input for the `repl` tool.
pub(crate) struct ReplInput {
    pub code: String,
    pub language: String,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
/// Input for the `powershell` tool.
pub(crate) struct PowerShellInput {
    pub command: String,
    pub timeout: Option<u64>,
    pub description: Option<String>,
    pub run_in_background: Option<bool>,
}

#[derive(Debug, Serialize)]
/// Output of the `web_fetch` tool.
pub(crate) struct WebFetchOutput {
    pub bytes: usize,
    pub code: u16,
    #[serde(rename = "codeText")]
    pub code_text: String,
    pub result: String,
    #[serde(rename = "durationMs")]
    pub duration_ms: u128,
    pub url: String,
}

#[derive(Debug, Serialize)]
/// Output of the `web_search` tool.
pub(crate) struct WebSearchOutput {
    pub query: String,
    pub results: Vec<WebSearchResultItem>,
    #[serde(rename = "durationSeconds")]
    pub duration_seconds: f64,
}

#[derive(Debug, Serialize)]
/// Output of the `todo_write` tool.
pub(crate) struct TodoWriteOutput {
    #[serde(rename = "oldTodos")]
    pub old_todos: Vec<TodoItem>,
    #[serde(rename = "newTodos")]
    pub new_todos: Vec<TodoItem>,
    #[serde(rename = "verificationNudgeNeeded")]
    pub verification_nudge_needed: Option<bool>,
}

#[derive(Debug, Serialize)]
/// Output of the `skill` tool.
pub(crate) struct SkillOutput {
    pub skill: String,
    pub path: String,
    pub args: Option<String>,
    pub description: Option<String>,
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Output of the `agent` tool.
pub(crate) struct AgentOutput {
    #[serde(rename = "agentId")]
    pub agent_id: String,
    pub name: String,
    pub description: String,
    #[serde(rename = "subagentType")]
    pub subagent_type: Option<String>,
    pub model: Option<String>,
    pub status: String,
    #[serde(rename = "outputFile")]
    pub output_file: String,
    #[serde(rename = "manifestFile")]
    pub manifest_file: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "startedAt", skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(rename = "completedAt", skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
/// Output of the `tool_search` tool.
pub(crate) struct ToolSearchOutput {
    pub matches: Vec<String>,
    pub query: String,
    pub normalized_query: String,
    #[serde(rename = "total_deferred_tools")]
    pub total_deferred_tools: usize,
    #[serde(rename = "pending_mcp_servers")]
    pub pending_mcp_servers: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
/// Output of the `notebook_edit` tool.
pub(crate) struct NotebookEditOutput {
    pub new_source: String,
    pub cell_id: Option<String>,
    pub cell_type: Option<NotebookCellType>,
    pub language: String,
    pub edit_mode: String,
    pub error: Option<String>,
    pub notebook_path: String,
    pub original_file: String,
    pub updated_file: String,
}

#[derive(Debug, Serialize)]
/// Output of the `sleep` tool.
pub(crate) struct SleepOutput {
    pub duration_ms: u64,
    pub message: String,
}

#[derive(Debug, Serialize)]
/// Output of the `brief` tool.
pub(crate) struct BriefOutput {
    pub message: String,
    pub attachments: Option<Vec<ResolvedAttachment>>,
    #[serde(rename = "sentAt")]
    pub sent_at: String,
}

#[derive(Debug, Serialize)]
/// A resolved file attachment with path and content.
pub(crate) struct ResolvedAttachment {
    pub path: String,
    pub size: u64,
    #[serde(rename = "isImage")]
    pub is_image: bool,
}

#[derive(Debug, Serialize)]
/// Output of the `config` tool.
pub(crate) struct ConfigOutput {
    pub success: bool,
    pub operation: Option<String>,
    pub setting: Option<String>,
    pub value: Option<Value>,
    #[serde(rename = "previousValue")]
    pub previous_value: Option<Value>,
    #[serde(rename = "newValue")]
    pub new_value: Option<Value>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Persisted state for plan mode.
pub(crate) struct PlanModeState {
    #[serde(rename = "hadLocalOverride")]
    pub had_local_override: bool,
    #[serde(rename = "previousLocalMode")]
    pub previous_local_mode: Option<Value>,
}

#[derive(Debug, Serialize)]
#[allow(clippy::struct_excessive_bools)] // Required fields for JSON API contract
/// Output of a plan-mode transition tool.
pub(crate) struct PlanModeOutput {
    pub success: bool,
    pub operation: String,
    pub changed: bool,
    pub active: bool,
    pub managed: bool,
    pub message: String,
    #[serde(rename = "settingsPath")]
    pub settings_path: String,
    #[serde(rename = "statePath")]
    pub state_path: String,
    #[serde(rename = "previousLocalMode")]
    pub previous_local_mode: Option<Value>,
    #[serde(rename = "currentLocalMode")]
    pub current_local_mode: Option<Value>,
}

#[derive(Debug, Serialize)]
/// Result of a `structured_output` call.
pub(crate) struct StructuredOutputResult {
    pub data: String,
    pub structured_output: BTreeMap<String, Value>,
}

#[derive(Debug, Serialize)]
/// Output of the `repl` tool.
pub(crate) struct ReplOutput {
    pub language: String,
    pub stdout: String,
    pub stderr: String,
    #[serde(rename = "exitCode")]
    pub exit_code: i32,
    #[serde(rename = "durationMs")]
    pub duration_ms: u128,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
/// A single web search result item.
pub(crate) enum WebSearchResultItem {
    SearchResult {
        tool_use_id: String,
        content: Vec<SearchHit>,
    },
    Commentary(String),
}

#[derive(Debug, Serialize)]
/// An extracted search result hit.
pub(crate) struct SearchHit {
    pub title: String,
    pub url: String,
}

#[derive(Clone, Copy)]
/// Scope at which a config setting is stored.
pub(crate) enum ConfigScope {
    Global,
    Settings,
}

#[derive(Clone, Copy)]
/// Specification for a supported config setting.
pub(crate) struct ConfigSettingSpec {
    pub scope: ConfigScope,
    pub kind: ConfigKind,
    pub path: &'static [&'static str],
    pub options: Option<&'static [&'static str]>,
}

#[derive(Clone, Copy)]
/// Type of a config setting value.
pub(crate) enum ConfigKind {
    Boolean,
    String,
}
