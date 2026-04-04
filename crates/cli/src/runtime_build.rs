//! Runtime construction, plugin wiring, tool executors, and hook monitoring.
use std::env;
use std::io::{self, Write};
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use colotcook_plugins::{PluginHooks, PluginManager, PluginManagerConfig, PluginRegistry};
use colotcook_runtime as runtime;
use colotcook_runtime::{
    load_system_prompt, ConfigLoader, ConversationRuntime, McpServerManager, PermissionMode,
    PermissionPolicy, Session, ToolError, ToolExecutor,
};
use colotcook_tools::{GlobalToolRegistry, McpBridge};

use crate::arg_parsing::{AllowedToolSet, DEFAULT_DATE};
use crate::render::TerminalRenderer;
use crate::streaming::{InternalPromptProgressReporter, MultiProviderRuntimeClient};
use crate::tool_display::format_tool_result;

/// Snapshot of plugin state used when constructing the runtime.
pub(crate) struct RuntimePluginState {
    pub(crate) feature_config: runtime::RuntimeFeatureConfig,
    pub(crate) tool_registry: GlobalToolRegistry,
    pub(crate) plugin_registry: PluginRegistry,
}

/// Fully-constructed conversation runtime with plugin registry.
pub(crate) struct BuiltRuntime {
    runtime: Option<ConversationRuntime<MultiProviderRuntimeClient, CliToolExecutor>>,
    pub(crate) plugin_registry: PluginRegistry,
    plugins_active: bool,
}

impl BuiltRuntime {
    /// Wrap a conversation runtime with its plugin registry.
    pub(crate) fn new(
        runtime: ConversationRuntime<MultiProviderRuntimeClient, CliToolExecutor>,
        plugin_registry: PluginRegistry,
    ) -> Self {
        Self {
            runtime: Some(runtime),
            plugin_registry,
            plugins_active: true,
        }
    }

    /// Attach a hook-abort signal so the runtime can be interrupted.
    pub(crate) fn with_hook_abort_signal(
        mut self,
        hook_abort_signal: runtime::HookAbortSignal,
    ) -> Self {
        let runtime = self
            .runtime
            .take()
            .expect("runtime should exist before installing hook abort signal");
        self.runtime = Some(runtime.with_hook_abort_signal(hook_abort_signal));
        self
    }

    /// Shut down all active plugins, idempotently.
    pub(crate) fn shutdown_plugins(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if self.plugins_active {
            self.plugin_registry.shutdown()?;
            self.plugins_active = false;
        }
        Ok(())
    }
}

impl Deref for BuiltRuntime {
    type Target = ConversationRuntime<MultiProviderRuntimeClient, CliToolExecutor>;

    fn deref(&self) -> &Self::Target {
        self.runtime
            .as_ref()
            .expect("runtime should exist while built runtime is alive")
    }
}

impl DerefMut for BuiltRuntime {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.runtime
            .as_mut()
            .expect("runtime should exist while built runtime is alive")
    }
}

impl Drop for BuiltRuntime {
    fn drop(&mut self) {
        let _ = self.shutdown_plugins();
    }
}

/// Background thread that watches for hook-abort signals and triggers process exit.
pub(crate) struct HookAbortMonitor {
    stop_tx: Option<Sender<()>>,
    join_handle: Option<JoinHandle<()>>,
}

impl HookAbortMonitor {
    /// Spawn a monitor that calls `std::process::exit` on abort.
    pub(crate) fn spawn(abort_signal: runtime::HookAbortSignal) -> Self {
        Self::spawn_with_waiter(abort_signal, move |stop_rx, abort_signal| {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };

            runtime.block_on(async move {
                let wait_for_stop = tokio::task::spawn_blocking(move || {
                    let _ = stop_rx.recv();
                });

                tokio::select! {
                    result = tokio::signal::ctrl_c() => {
                        if result.is_ok() {
                            abort_signal.abort();
                        }
                    }
                    _ = wait_for_stop => {}
                }
            });
        })
    }

    /// Spawn a monitor with a custom waiter function (useful in tests).
    pub(crate) fn spawn_with_waiter<F>(
        abort_signal: runtime::HookAbortSignal,
        wait_for_interrupt: F,
    ) -> Self
    where
        F: FnOnce(Receiver<()>, runtime::HookAbortSignal) + Send + 'static,
    {
        let (stop_tx, stop_rx) = mpsc::channel();
        let join_handle = thread::spawn(move || wait_for_interrupt(stop_rx, abort_signal));

        Self {
            stop_tx: Some(stop_tx),
            join_handle: Some(join_handle),
        }
    }

    /// Stop the monitor thread without triggering an abort.
    pub(crate) fn stop(mut self) {
        if let Some(stop_tx) = self.stop_tx.take() {
            let _ = stop_tx.send(());
        }
        if let Some(join_handle) = self.join_handle.take() {
            let _ = join_handle.join();
        }
    }
}

/// Load and render the system prompt sections for this session.
pub(crate) fn build_system_prompt() -> Result<Vec<String>, Box<dyn std::error::Error>> {
    Ok(load_system_prompt(
        env::current_dir()?,
        DEFAULT_DATE,
        env::consts::OS,
        "unknown",
    )?)
}

#[allow(dead_code)] // Entry point for plugin state initialization, called from session setup paths
/// Build the plugin state using the default config loader.
pub(crate) fn build_runtime_plugin_state() -> Result<RuntimePluginState, Box<dyn std::error::Error>>
{
    let cwd = env::current_dir()?;
    let loader = ConfigLoader::default_for(&cwd);
    let runtime_config = loader.load()?;
    build_runtime_plugin_state_with_loader(&cwd, &loader, &runtime_config)
}

/// Build the plugin state using the default config loader.
pub(crate) fn build_runtime_plugin_state_with_loader(
    cwd: &Path,
    loader: &ConfigLoader,
    runtime_config: &runtime::RuntimeConfig,
) -> Result<RuntimePluginState, Box<dyn std::error::Error>> {
    let plugin_manager = build_plugin_manager(cwd, loader, runtime_config);
    let plugin_registry = plugin_manager.plugin_registry()?;
    let plugin_hook_config =
        runtime_hook_config_from_plugin_hooks(plugin_registry.aggregated_hooks()?);
    let feature_config = runtime_config
        .feature_config()
        .clone()
        .with_hooks(runtime_config.hooks().merged(&plugin_hook_config));
    let tool_registry = GlobalToolRegistry::with_plugin_tools(plugin_registry.aggregated_tools()?)?;
    Ok(RuntimePluginState {
        feature_config,
        tool_registry,
        plugin_registry,
    })
}

/// Build a `PluginManager` from the loaded runtime configuration.
pub(crate) fn build_plugin_manager(
    cwd: &Path,
    loader: &ConfigLoader,
    runtime_config: &runtime::RuntimeConfig,
) -> PluginManager {
    let plugin_settings = runtime_config.plugins();
    let mut plugin_config = PluginManagerConfig::new(loader.config_home().to_path_buf());
    plugin_config.enabled_plugins = plugin_settings.enabled_plugins().clone();
    plugin_config.external_dirs = plugin_settings
        .external_directories()
        .iter()
        .map(|path| resolve_plugin_path(cwd, loader.config_home(), path))
        .collect();
    plugin_config.install_root = plugin_settings
        .install_root()
        .map(|path| resolve_plugin_path(cwd, loader.config_home(), path));
    plugin_config.registry_path = plugin_settings
        .registry_path()
        .map(|path| resolve_plugin_path(cwd, loader.config_home(), path));
    plugin_config.bundled_root = plugin_settings
        .bundled_root()
        .map(|path| resolve_plugin_path(cwd, loader.config_home(), path));
    PluginManager::new(plugin_config)
}

/// Resolve a plugin path relative to `cwd` or `config_home`.
pub(crate) fn resolve_plugin_path(cwd: &Path, config_home: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else if value.starts_with('.') {
        cwd.join(path)
    } else {
        config_home.join(path)
    }
}

/// Convert plugin hooks into runtime hook configuration entries.
pub(crate) fn runtime_hook_config_from_plugin_hooks(
    hooks: PluginHooks,
) -> runtime::RuntimeHookConfig {
    runtime::RuntimeHookConfig::new(
        hooks.pre_tool_use,
        hooks.post_tool_use,
        hooks.post_tool_use_failure,
    )
}

#[allow(clippy::needless_pass_by_value)]
#[allow(clippy::too_many_arguments)]
/// Build a full conversation runtime with the given parameters.
pub(crate) fn build_runtime(
    session: Session,
    session_id: &str,
    model: String,
    system_prompt: Vec<String>,
    enable_tools: bool,
    emit_output: bool,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
    progress_reporter: Option<InternalPromptProgressReporter>,
) -> Result<BuiltRuntime, Box<dyn std::error::Error>> {
    // Load config once and reuse for both plugin state and MCP manager.
    let cwd = env::current_dir()?;
    let loader = ConfigLoader::default_for(&cwd);
    let runtime_config = loader.load()?;
    let runtime_plugin_state =
        build_runtime_plugin_state_with_loader(&cwd, &loader, &runtime_config)?;
    let mcp_manager = Arc::new(std::sync::Mutex::new(
        McpServerManager::from_runtime_config(&runtime_config),
    ));

    // Create a dedicated tokio runtime for MCP and register the global bridge
    // so that sub-agents inherit MCP tool access.
    let mcp_runtime = Arc::new(
        tokio::runtime::Runtime::new()
            .map_err(|e| Box::<dyn std::error::Error>::from(e.to_string()))?,
    );
    colotcook_tools::set_global_mcp_bridge(McpBridge::new(
        mcp_manager.clone(),
        mcp_runtime.clone(),
    ));

    build_runtime_with_plugin_state(
        session,
        session_id,
        model,
        system_prompt,
        enable_tools,
        emit_output,
        allowed_tools,
        permission_mode,
        progress_reporter,
        runtime_plugin_state,
        mcp_manager,
        mcp_runtime,
    )
}

#[allow(clippy::needless_pass_by_value)]
#[allow(clippy::too_many_arguments)]
/// Build a runtime from pre-built plugin state.
pub(crate) fn build_runtime_with_plugin_state(
    session: Session,
    session_id: &str,
    model: String,
    system_prompt: Vec<String>,
    enable_tools: bool,
    emit_output: bool,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
    progress_reporter: Option<InternalPromptProgressReporter>,
    runtime_plugin_state: RuntimePluginState,
    mcp_manager: Arc<std::sync::Mutex<McpServerManager>>,
    mcp_runtime: Arc<tokio::runtime::Runtime>,
) -> Result<BuiltRuntime, Box<dyn std::error::Error>> {
    let RuntimePluginState {
        feature_config,
        tool_registry,
        plugin_registry,
    } = runtime_plugin_state;
    plugin_registry.initialize()?;

    let mut runtime = ConversationRuntime::new_with_features(
        session,
        MultiProviderRuntimeClient::new(
            session_id,
            model,
            enable_tools,
            emit_output,
            allowed_tools.clone(),
            tool_registry.clone(),
            progress_reporter,
        )?,
        CliToolExecutor::new(
            allowed_tools.clone(),
            emit_output,
            tool_registry.clone(),
            mcp_manager,
            mcp_runtime,
        ),
        permission_policy(permission_mode, &feature_config, &tool_registry)
            .map_err(std::io::Error::other)?,
        system_prompt,
        &feature_config,
    );
    if emit_output {
        runtime = runtime.with_hook_progress_reporter(Box::new(CliHookProgressReporter));
    }
    Ok(BuiltRuntime::new(runtime, plugin_registry))
}

/// Terminal-printing hook progress reporter.
pub(crate) struct CliHookProgressReporter;

impl runtime::HookProgressReporter for CliHookProgressReporter {
    fn on_event(&mut self, event: &runtime::HookProgressEvent) {
        match event {
            runtime::HookProgressEvent::Started {
                event,
                tool_name,
                command,
            } => eprintln!(
                "[hook {event_name}] {tool_name}: {command}",
                event_name = event.as_str()
            ),
            runtime::HookProgressEvent::Completed {
                event,
                tool_name,
                command,
            } => eprintln!(
                "[hook done {event_name}] {tool_name}: {command}",
                event_name = event.as_str()
            ),
            runtime::HookProgressEvent::Cancelled {
                event,
                tool_name,
                command,
            } => eprintln!(
                "[hook cancelled {event_name}] {tool_name}: {command}",
                event_name = event.as_str()
            ),
        }
    }
}

/// Interactive terminal prompter for permission decisions.
pub(crate) struct CliPermissionPrompter {
    current_mode: PermissionMode,
}

impl CliPermissionPrompter {
    /// Wrap a conversation runtime with its plugin registry.
    pub(crate) fn new(current_mode: PermissionMode) -> Self {
        Self { current_mode }
    }
}

impl runtime::PermissionPrompter for CliPermissionPrompter {
    fn decide(
        &mut self,
        request: &runtime::PermissionRequest,
    ) -> runtime::PermissionPromptDecision {
        println!();
        println!("Permission approval required");
        println!("  Tool             {}", request.tool_name);
        println!("  Current mode     {}", self.current_mode.as_str());
        println!("  Required mode    {}", request.required_mode.as_str());
        if let Some(reason) = &request.reason {
            println!("  Reason           {reason}");
        }
        println!("  Input            {}", request.input);
        print!("Approve this tool call? [y/N]: ");
        let _ = io::stdout().flush();

        let mut response = String::new();
        match io::stdin().read_line(&mut response) {
            Ok(_) => {
                let normalized = response.trim().to_ascii_lowercase();
                if matches!(normalized.as_str(), "y" | "yes") {
                    runtime::PermissionPromptDecision::Allow
                } else {
                    runtime::PermissionPromptDecision::Deny {
                        reason: format!(
                            "tool '{}' denied by user approval prompt",
                            request.tool_name
                        ),
                    }
                }
            }
            Err(error) => runtime::PermissionPromptDecision::Deny {
                reason: format!("permission approval failed: {error}"),
            },
        }
    }
}

/// Tool executor that runs tools and streams output to the terminal.
pub(crate) struct CliToolExecutor {
    renderer: TerminalRenderer,
    emit_output: bool,
    allowed_tools: Option<AllowedToolSet>,
    tool_registry: GlobalToolRegistry,
    mcp_manager: Arc<std::sync::Mutex<McpServerManager>>,
    /// Dedicated tokio runtime for async MCP operations.
    /// Separate from the API client runtime to avoid re-entrancy deadlocks.
    mcp_runtime: Arc<tokio::runtime::Runtime>,
}

impl CliToolExecutor {
    /// Wrap a conversation runtime with its plugin registry.
    pub(crate) fn new(
        allowed_tools: Option<AllowedToolSet>,
        emit_output: bool,
        tool_registry: GlobalToolRegistry,
        mcp_manager: Arc<std::sync::Mutex<McpServerManager>>,
        mcp_runtime: Arc<tokio::runtime::Runtime>,
    ) -> Self {
        Self {
            renderer: TerminalRenderer::new(),
            emit_output,
            allowed_tools,
            tool_registry,
            mcp_manager,
            mcp_runtime,
        }
    }
}

impl ToolExecutor for CliToolExecutor {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        if self
            .allowed_tools
            .as_ref()
            .is_some_and(|allowed| !allowed.contains(tool_name))
        {
            return Err(ToolError::new(format!(
                "tool `{tool_name}` is not enabled by the current --allowedTools setting"
            )));
        }

        // Route MCP tool calls (prefixed with "mcp__") to the MCP server manager.
        if tool_name.starts_with("mcp__") {
            return self.execute_mcp_tool(tool_name, input);
        }

        let value = serde_json::from_str(input)
            .map_err(|error| ToolError::new(format!("invalid tool input JSON: {error}")))?;
        match self.tool_registry.execute(tool_name, &value) {
            Ok(output) => {
                if self.emit_output {
                    let markdown = format_tool_result(tool_name, &output, false);
                    self.renderer
                        .stream_markdown(&markdown, &mut io::stdout())
                        .map_err(|error| ToolError::new(error.to_string()))?;
                }
                Ok(output)
            }
            Err(error) => {
                if self.emit_output {
                    let markdown = format_tool_result(tool_name, &error, true);
                    self.renderer
                        .stream_markdown(&markdown, &mut io::stdout())
                        .map_err(|stream_error| ToolError::new(stream_error.to_string()))?;
                }
                Err(ToolError::new(error))
            }
        }
    }
}

impl CliToolExecutor {
    /// Execute an MCP tool call by delegating to the `McpServerManager`.
    ///
    /// Uses a dedicated tokio runtime (`mcp_runtime`) to avoid re-entrancy
    /// deadlocks with the API client's runtime.
    fn execute_mcp_tool(&self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        let arguments: Option<serde_json::Value> = if input.is_empty() || input == "{}" {
            None
        } else {
            Some(
                serde_json::from_str(input)
                    .map_err(|e| ToolError::new(format!("invalid MCP tool input JSON: {e}")))?,
            )
        };

        let tool_name_owned = tool_name.to_string();

        // Lock the manager synchronously (std::sync::Mutex), then run the
        // async call_tool on the dedicated MCP runtime.
        let result = {
            let mut manager = self
                .mcp_manager
                .lock()
                .map_err(|e| ToolError::new(format!("MCP manager lock poisoned: {e}")))?;
            self.mcp_runtime
                .block_on(manager.call_tool(&tool_name_owned, arguments))
        };

        match result {
            Ok(response) => {
                if let Some(ref error) = response.error {
                    return Err(ToolError::new(format!(
                        "MCP JSON-RPC error {}: {}",
                        error.code, error.message
                    )));
                }
                match response.result {
                    Some(call_result) => {
                        let output = serde_json::to_string(&call_result)
                            .unwrap_or_else(|_| "{}".to_string());
                        if self.emit_output {
                            let markdown = format_tool_result(tool_name, &output, false);
                            self.renderer
                                .stream_markdown(&markdown, &mut io::stdout())
                                .map_err(|e| ToolError::new(e.to_string()))?;
                        }
                        Ok(output)
                    }
                    None => Err(ToolError::new("MCP tool call returned no result")),
                }
            }
            Err(error) => Err(ToolError::new(format!("MCP tool call failed: {error}"))),
        }
    }
}

/// Build a `PermissionPolicy` from the active permission mode.
pub(crate) fn permission_policy(
    mode: PermissionMode,
    feature_config: &runtime::RuntimeFeatureConfig,
    tool_registry: &GlobalToolRegistry,
) -> Result<PermissionPolicy, String> {
    Ok(tool_registry.permission_specs(None)?.into_iter().fold(
        PermissionPolicy::new(mode).with_permission_rules(feature_config.permission_rules()),
        |policy, (name, required_permission)| {
            policy.with_tool_requirement(name, required_permission)
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use colotcook_runtime::HookProgressReporter;
    use std::path::PathBuf;

    // --- resolve_plugin_path ---

    #[test]
    fn resolve_plugin_path_absolute_returned_as_is() {
        let cwd = PathBuf::from("/home/user/project");
        let config_home = PathBuf::from("/home/user/.config");
        let result = resolve_plugin_path(&cwd, &config_home, "/usr/local/plugin");
        assert_eq!(result, PathBuf::from("/usr/local/plugin"));
    }

    #[test]
    fn resolve_plugin_path_dot_relative_uses_cwd() {
        let cwd = PathBuf::from("/home/user/project");
        let config_home = PathBuf::from("/home/user/.config");
        let result = resolve_plugin_path(&cwd, &config_home, "./my-plugin");
        assert_eq!(result, PathBuf::from("/home/user/project/my-plugin"));
    }

    #[test]
    fn resolve_plugin_path_dot_dot_relative_uses_cwd() {
        let cwd = PathBuf::from("/home/user/project");
        let config_home = PathBuf::from("/home/user/.config");
        let result = resolve_plugin_path(&cwd, &config_home, "../shared-plugin");
        assert_eq!(result, PathBuf::from("/home/user/project/../shared-plugin"));
    }

    #[test]
    fn resolve_plugin_path_bare_name_uses_config_home() {
        let cwd = PathBuf::from("/home/user/project");
        let config_home = PathBuf::from("/home/user/.config");
        let result = resolve_plugin_path(&cwd, &config_home, "plugins/my-plugin");
        assert_eq!(
            result,
            PathBuf::from("/home/user/.config/plugins/my-plugin")
        );
    }

    #[test]
    fn resolve_plugin_path_bare_single_name() {
        let cwd = PathBuf::from("/home/user/project");
        let config_home = PathBuf::from("/home/user/.config");
        let result = resolve_plugin_path(&cwd, &config_home, "my-plugin");
        assert_eq!(result, PathBuf::from("/home/user/.config/my-plugin"));
    }

    // --- HookAbortMonitor ---

    #[test]
    fn hook_abort_monitor_spawn_and_stop() {
        let signal = runtime::HookAbortSignal::new();
        let monitor = HookAbortMonitor::spawn_with_waiter(signal, |stop_rx, _signal| {
            let _ = stop_rx.recv();
        });
        monitor.stop();
        // Should not hang or panic
    }

    #[test]
    fn hook_abort_monitor_custom_waiter() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let signal = runtime::HookAbortSignal::new();
        let waiter_ran = Arc::new(AtomicBool::new(false));
        let waiter_ran_clone = waiter_ran.clone();

        let monitor = HookAbortMonitor::spawn_with_waiter(signal, move |stop_rx, _signal| {
            waiter_ran_clone.store(true, Ordering::SeqCst);
            let _ = stop_rx.recv();
        });
        monitor.stop();
        assert!(waiter_ran.load(Ordering::SeqCst));
    }

    // --- runtime_hook_config_from_plugin_hooks ---

    #[test]
    fn runtime_hook_config_from_empty_plugin_hooks() {
        let hooks = PluginHooks {
            pre_tool_use: vec![],
            post_tool_use: vec![],
            post_tool_use_failure: vec![],
        };
        let config = runtime_hook_config_from_plugin_hooks(hooks);
        // Should produce a valid config with empty hook lists
        let _ = config;
    }

    // --- CliPermissionPrompter ---

    #[test]
    fn cli_permission_prompter_new() {
        let prompter = CliPermissionPrompter::new(PermissionMode::ReadOnly);
        assert_eq!(prompter.current_mode, PermissionMode::ReadOnly);
    }

    // --- CliHookProgressReporter ---

    #[test]
    fn cli_hook_progress_reporter_started_event() {
        let mut reporter = CliHookProgressReporter;
        // Just verify it doesn't panic on any event variant
        reporter.on_event(&runtime::HookProgressEvent::Started {
            event: runtime::HookEvent::PreToolUse,
            tool_name: "bash".to_string(),
            command: "echo test".to_string(),
        });
        reporter.on_event(&runtime::HookProgressEvent::Completed {
            event: runtime::HookEvent::PostToolUse,
            tool_name: "bash".to_string(),
            command: "echo done".to_string(),
        });
        reporter.on_event(&runtime::HookProgressEvent::Cancelled {
            event: runtime::HookEvent::PreToolUse,
            tool_name: "bash".to_string(),
            command: "echo cancel".to_string(),
        });
    }

    // -- Tests migrated from main.rs ------------------------------------------

    #[test]
    fn hook_abort_monitor_stops_without_aborting() {
        use std::sync::mpsc;
        use std::time::Duration;

        let abort_signal = runtime::HookAbortSignal::new();
        let (ready_tx, ready_rx) = mpsc::channel();
        let monitor = HookAbortMonitor::spawn_with_waiter(
            abort_signal.clone(),
            move |stop_rx, abort_signal| {
                ready_tx.send(()).expect("ready signal");
                let _ = stop_rx.recv();
                assert!(!abort_signal.is_aborted());
            },
        );

        ready_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("waiter should be ready");
        monitor.stop();

        assert!(!abort_signal.is_aborted());
    }

    #[test]
    fn hook_abort_monitor_propagates_interrupt() {
        use std::sync::mpsc;
        use std::time::Duration;

        let abort_signal = runtime::HookAbortSignal::new();
        let (done_tx, done_rx) = mpsc::channel();
        let monitor = HookAbortMonitor::spawn_with_waiter(
            abort_signal.clone(),
            move |_stop_rx, abort_signal| {
                abort_signal.abort();
                done_tx.send(()).expect("done signal");
            },
        );

        done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("interrupt should complete");
        monitor.stop();

        assert!(abort_signal.is_aborted());
    }
}
