//! Live CLI session handler — owns the REPL runtime, session, and slash-command dispatch.

use std::env;
use std::fs;
use std::io::{self};
use std::path::Path;

use colotcook_commands::{
    handle_agents_slash_command, handle_plugins_slash_command, handle_skills_slash_command,
    SlashCommand,
};
use colotcook_runtime as runtime;
use colotcook_runtime::{
    resolve_sandbox_status, CompactionConfig, ConfigLoader, PermissionMode, Session,
};
use serde_json::json;

use crate::arg_parsing::{
    format_unknown_slash_command, permission_mode_from_label, resolve_model_alias,
    slash_command_completion_candidates_with_sessions, AllowedToolSet, CliOutputFormat,
};
use crate::render::{Spinner, TerminalRenderer};
use crate::reports::{
    format_auto_compaction_notice, format_bughunter_report, format_commit_preflight_report,
    format_commit_skipped_report, format_compact_report, format_cost_report, format_issue_report,
    format_model_report, format_model_switch_report, format_permissions_report,
    format_permissions_switch_report, format_pr_report, format_resume_report,
    format_sandbox_report, format_status_report, format_ultraplan_report,
    normalize_permission_mode, parse_git_status_branch, parse_git_workspace_summary,
    render_config_report, render_diff_report, render_export_text, render_last_tool_debug_report,
    render_memory_report, render_repl_help, render_resume_usage, render_teleport_report,
    render_version_report, resolve_export_path, run_init, status_context, validate_no_args,
    StatusUsage,
};
use crate::runtime_build::{
    build_plugin_manager, build_runtime, build_system_prompt, BuiltRuntime, CliPermissionPrompter,
    HookAbortMonitor,
};
use crate::session_management::{
    create_managed_session_handle, list_managed_sessions, render_session_list,
    resolve_session_reference, SessionHandle,
};
use crate::streaming::{
    collect_prompt_cache_events, collect_tool_results, collect_tool_uses, final_assistant_text,
    InternalPromptProgressReporter,
};
use crate::util::git_output;

/// Interactive CLI session that wraps the AI runtime and handles REPL commands.
pub(crate) struct LiveCli {
    /// Active model identifier.
    pub(crate) model: String,
    /// Optional restrict set of tools the session may call.
    pub(crate) allowed_tools: Option<AllowedToolSet>,
    /// Current permission mode for tool execution.
    pub(crate) permission_mode: PermissionMode,
    /// Rendered system-prompt sections passed to every turn.
    pub(crate) system_prompt: Vec<String>,
    /// Underlying AI runtime owning the current conversation state.
    pub(crate) runtime: BuiltRuntime,
    /// Filesystem-backed session handle (id + path).
    pub(crate) session: SessionHandle,
}

impl LiveCli {
    /// Create a new `LiveCli`, building a fresh session and persisting it immediately.
    pub(crate) fn new(
        model: String,
        enable_tools: bool,
        allowed_tools: Option<AllowedToolSet>,
        permission_mode: PermissionMode,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let system_prompt = build_system_prompt()?;
        let session_state = Session::new();
        let session = create_managed_session_handle(&session_state.session_id)?;
        let runtime = build_runtime(
            session_state.with_persistence_path(session.path.clone()),
            &session.id,
            model.clone(),
            system_prompt.clone(),
            enable_tools,
            true,
            allowed_tools.clone(),
            permission_mode,
            None,
        )?;
        let cli = Self {
            model,
            allowed_tools,
            permission_mode,
            system_prompt,
            runtime,
            session,
        };
        cli.persist_session()?;
        Ok(cli)
    }

    /// Return the startup banner string shown when the REPL first launches.
    pub(crate) fn startup_banner(&self) -> String {
        let cwd = env::current_dir().map_or_else(
            |_| "<unknown>".to_string(),
            |path| path.display().to_string(),
        );
        let status = status_context(None).ok();
        let git_branch = status
            .as_ref()
            .and_then(|context| context.git_branch.as_deref())
            .unwrap_or("unknown");
        let workspace = status.as_ref().map_or_else(
            || "unknown".to_string(),
            |context| context.git_summary.headline(),
        );
        let session_path = self.session.path.strip_prefix(Path::new(&cwd)).map_or_else(
            |_| self.session.path.display().to_string(),
            |path| path.display().to_string(),
        );
        format_startup_banner(&BannerParams {
            model: &self.model,
            permission_mode: self.permission_mode.as_str(),
            git_branch,
            workspace: &workspace,
            cwd: &cwd,
            session_id: &self.session.id,
            session_path: &session_path,
        })
    }

    /// Return the list of tab-completion candidates for the current REPL context.
    pub(crate) fn repl_completion_candidates(
        &self,
    ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        Ok(slash_command_completion_candidates_with_sessions(
            &self.model,
            Some(&self.session.id),
            list_managed_sessions()?
                .into_iter()
                .map(|session| session.id)
                .collect(),
        ))
    }

    /// Build a fresh runtime for the next user turn, paired with a hook-abort monitor.
    fn prepare_turn_runtime(
        &self,
        emit_output: bool,
    ) -> Result<(BuiltRuntime, HookAbortMonitor), Box<dyn std::error::Error>> {
        let hook_abort_signal = runtime::HookAbortSignal::new();
        let runtime = build_runtime(
            self.runtime.session().clone(),
            &self.session.id,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            emit_output,
            self.allowed_tools.clone(),
            self.permission_mode,
            None,
        )?
        .with_hook_abort_signal(hook_abort_signal.clone());
        let hook_abort_monitor = HookAbortMonitor::spawn(hook_abort_signal);

        Ok((runtime, hook_abort_monitor))
    }

    /// Shut down the existing runtime and replace it with `runtime`.
    fn replace_runtime(&mut self, runtime: BuiltRuntime) -> Result<(), Box<dyn std::error::Error>> {
        self.runtime.shutdown_plugins()?;
        self.runtime = runtime;
        Ok(())
    }

    /// Run one user turn with terminal spinner output.
    pub(crate) fn run_turn(&mut self, input: &str) -> Result<(), Box<dyn std::error::Error>> {
        let (mut runtime, hook_abort_monitor) = self.prepare_turn_runtime(true)?;
        let mut spinner = Spinner::new();
        let mut stdout = io::stdout();
        spinner.tick(
            "🦀 Thinking...",
            TerminalRenderer::new().color_theme(),
            &mut stdout,
        )?;
        let mut permission_prompter = CliPermissionPrompter::new(self.permission_mode);
        let result = runtime.run_turn(input, Some(&mut permission_prompter));
        hook_abort_monitor.stop();
        match result {
            Ok(summary) => {
                self.replace_runtime(runtime)?;
                spinner.finish(
                    "✨ Done",
                    TerminalRenderer::new().color_theme(),
                    &mut stdout,
                )?;
                println!();
                if let Some(event) = summary.auto_compaction {
                    println!(
                        "{}",
                        format_auto_compaction_notice(event.removed_message_count)
                    );
                }
                self.persist_session()?;
                Ok(())
            }
            Err(error) => {
                runtime.shutdown_plugins()?;
                spinner.fail(
                    "❌ Request failed",
                    TerminalRenderer::new().color_theme(),
                    &mut stdout,
                )?;
                Err(Box::new(error))
            }
        }
    }

    /// Run a user turn, selecting output mode from `output_format`.
    pub(crate) fn run_turn_with_output(
        &mut self,
        input: &str,
        output_format: CliOutputFormat,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match output_format {
            CliOutputFormat::Text => self.run_turn(input),
            CliOutputFormat::Json => self.run_prompt_json(input),
        }
    }

    /// Run a user turn and emit a JSON summary to stdout.
    fn run_prompt_json(&mut self, input: &str) -> Result<(), Box<dyn std::error::Error>> {
        let (mut runtime, hook_abort_monitor) = self.prepare_turn_runtime(false)?;
        let mut permission_prompter = CliPermissionPrompter::new(self.permission_mode);
        let result = runtime.run_turn(input, Some(&mut permission_prompter));
        hook_abort_monitor.stop();
        let summary = result?;
        self.replace_runtime(runtime)?;
        self.persist_session()?;
        let assistant_text = final_assistant_text(&summary);
        let tool_uses = collect_tool_uses(&summary);
        let tool_results = collect_tool_results(&summary);
        let prompt_cache_events = collect_prompt_cache_events(&summary);
        println!(
            "{}",
            build_prompt_json_value(&PromptJsonParams {
                assistant_text: &assistant_text,
                model: &self.model,
                iterations: summary.iterations,
                auto_compaction_removed: summary.auto_compaction.map(|e| e.removed_message_count),
                tool_uses: &tool_uses,
                tool_results: &tool_results,
                prompt_cache_events: &prompt_cache_events,
                usage_input: summary.usage.input_tokens,
                usage_output: summary.usage.output_tokens,
                usage_cache_creation: summary.usage.cache_creation_input_tokens,
                usage_cache_read: summary.usage.cache_read_input_tokens,
            })
        );
        Ok(())
    }

    /// Dispatch a slash command received in the REPL loop.
    ///
    /// Returns `true` if the session should be persisted after the command.
    pub(crate) fn handle_repl_command(
        &mut self,
        command: SlashCommand,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        Ok(match command {
            SlashCommand::Help => {
                println!("{}", render_repl_help());
                false
            }
            SlashCommand::Status => {
                self.print_status();
                false
            }
            SlashCommand::Bughunter { scope } => {
                self.run_bughunter(scope.as_deref())?;
                false
            }
            SlashCommand::Commit => {
                self.run_commit(None)?;
                false
            }
            SlashCommand::Pr { context } => {
                self.run_pr(context.as_deref())?;
                false
            }
            SlashCommand::Issue { context } => {
                self.run_issue(context.as_deref())?;
                false
            }
            SlashCommand::Ultraplan { task } => {
                self.run_ultraplan(task.as_deref())?;
                false
            }
            SlashCommand::Teleport { target } => {
                self.run_teleport(target.as_deref())?;
                false
            }
            SlashCommand::DebugToolCall => {
                self.run_debug_tool_call(None)?;
                false
            }
            SlashCommand::Sandbox => {
                Self::print_sandbox_status();
                false
            }
            SlashCommand::Compact => {
                self.compact()?;
                false
            }
            SlashCommand::Model { model } => self.set_model(model.as_deref())?,
            SlashCommand::Permissions { mode } => self.set_permissions(mode.as_deref())?,
            SlashCommand::Clear { confirm } => self.clear_session(confirm)?,
            SlashCommand::Cost => {
                self.print_cost();
                false
            }
            SlashCommand::Resume { session_path } => self.resume_session(session_path)?,
            SlashCommand::Config { section } => {
                Self::print_config(section.as_deref())?;
                false
            }
            SlashCommand::Memory => {
                Self::print_memory()?;
                false
            }
            SlashCommand::Init => {
                run_init()?;
                false
            }
            SlashCommand::Diff => {
                Self::print_diff()?;
                false
            }
            SlashCommand::Version => {
                Self::print_version();
                false
            }
            SlashCommand::Export { path } => {
                self.export_session(path.as_deref())?;
                false
            }
            SlashCommand::Session { action, target } => {
                self.handle_session_command(action.as_deref(), target.as_deref())?
            }
            SlashCommand::Plugins { action, target } => {
                self.handle_plugins_command(action.as_deref(), target.as_deref())?
            }
            SlashCommand::Agents { args } => {
                Self::print_agents(args.as_deref())?;
                false
            }
            SlashCommand::Skills { args } => {
                Self::print_skills(args.as_deref())?;
                false
            }
            SlashCommand::Unknown(name) => {
                eprintln!("{}", format_unknown_slash_command(&name));
                false
            }
        })
    }

    /// Persist the current runtime session to its backing file.
    pub(crate) fn persist_session(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.runtime.session().save_to_path(&self.session.path)?;
        Ok(())
    }

    /// Print the current usage/status snapshot to stdout.
    fn print_status(&self) {
        let cumulative = self.runtime.usage().cumulative_usage();
        let latest = self.runtime.usage().current_turn_usage();
        println!(
            "{}",
            format_status_report(
                &self.model,
                StatusUsage {
                    message_count: self.runtime.session().messages.len(),
                    turns: self.runtime.usage().turns(),
                    latest,
                    cumulative,
                    estimated_tokens: self.runtime.estimated_tokens(),
                },
                self.permission_mode.as_str(),
                &status_context(Some(&self.session.path)).expect("status context should load"),
            )
        );
    }

    /// Print the sandbox isolation status to stdout.
    fn print_sandbox_status() {
        let cwd = env::current_dir().expect("current dir");
        let loader = ConfigLoader::default_for(&cwd);
        let runtime_config = loader
            .load()
            .unwrap_or_else(|_| runtime::RuntimeConfig::empty());
        println!(
            "{}",
            format_sandbox_report(&resolve_sandbox_status(runtime_config.sandbox(), &cwd))
        );
    }

    /// Switch to a new model, rebuilding the runtime.
    fn set_model(&mut self, model: Option<&str>) -> Result<bool, Box<dyn std::error::Error>> {
        match resolve_model_switch(&self.model, model) {
            ModelSwitchOutcome::ShowCurrent | ModelSwitchOutcome::AlreadyCurrent => {
                println!(
                    "{}",
                    format_model_report(
                        &self.model,
                        self.runtime.session().messages.len(),
                        self.runtime.usage().turns(),
                    )
                );
                Ok(false)
            }
            ModelSwitchOutcome::SwitchTo { resolved } => {
                let previous = self.model.clone();
                let session = self.runtime.session().clone();
                let message_count = session.messages.len();
                let runtime = build_runtime(
                    session,
                    &self.session.id,
                    resolved.clone(),
                    self.system_prompt.clone(),
                    true,
                    true,
                    self.allowed_tools.clone(),
                    self.permission_mode,
                    None,
                )?;
                self.replace_runtime(runtime)?;
                self.model.clone_from(&resolved);
                println!(
                    "{}",
                    format_model_switch_report(&previous, &resolved, message_count)
                );
                Ok(true)
            }
        }
    }

    /// Switch to a new permission mode, rebuilding the runtime.
    fn set_permissions(&mut self, mode: Option<&str>) -> Result<bool, Box<dyn std::error::Error>> {
        match resolve_permission_switch(self.permission_mode.as_str(), mode) {
            PermissionSwitchOutcome::ShowCurrent => {
                println!(
                    "{}",
                    format_permissions_report(self.permission_mode.as_str())
                );
                Ok(false)
            }
            PermissionSwitchOutcome::Invalid { input } => Err(format!(
                "unsupported permission mode '{input}'. Use read-only, workspace-write, or danger-full-access."
            )
            .into()),
            PermissionSwitchOutcome::AlreadyCurrent { normalized } => {
                println!("{}", format_permissions_report(normalized));
                Ok(false)
            }
            PermissionSwitchOutcome::SwitchTo { normalized } => {
                let previous = self.permission_mode.as_str().to_string();
                let session = self.runtime.session().clone();
                self.permission_mode = permission_mode_from_label(normalized);
                let runtime = build_runtime(
                    session,
                    &self.session.id,
                    self.model.clone(),
                    self.system_prompt.clone(),
                    true,
                    true,
                    self.allowed_tools.clone(),
                    self.permission_mode,
                    None,
                )?;
                self.replace_runtime(runtime)?;
                println!(
                    "{}",
                    format_permissions_switch_report(&previous, normalized)
                );
                Ok(true)
            }
        }
    }

    /// Clear the current session and start a fresh one.
    fn clear_session(&mut self, confirm: bool) -> Result<bool, Box<dyn std::error::Error>> {
        if !confirm {
            println!("{}", format_clear_confirmation_required());
            return Ok(false);
        }

        let session_state = Session::new();
        self.session = create_managed_session_handle(&session_state.session_id)?;
        let runtime = build_runtime(
            session_state.with_persistence_path(self.session.path.clone()),
            &self.session.id,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            true,
            self.allowed_tools.clone(),
            self.permission_mode,
            None,
        )?;
        self.replace_runtime(runtime)?;
        println!(
            "{}",
            format_clear_session_report(
                &self.model,
                self.permission_mode.as_str(),
                &self.session.id,
            )
        );
        Ok(true)
    }

    /// Print cumulative cost for this session.
    fn print_cost(&self) {
        let cumulative = self.runtime.usage().cumulative_usage();
        println!("{}", format_cost_report(cumulative));
    }

    /// Load and switch to a previously saved session by reference.
    fn resume_session(
        &mut self,
        session_path: Option<String>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let Some(session_ref) = session_path else {
            println!("{}", render_resume_usage());
            return Ok(false);
        };

        let handle = resolve_session_reference(&session_ref)?;
        let session = Session::load_from_path(&handle.path)?;
        let message_count = session.messages.len();
        let session_id = session.session_id.clone();
        let runtime = build_runtime(
            session,
            &handle.id,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            true,
            self.allowed_tools.clone(),
            self.permission_mode,
            None,
        )?;
        self.replace_runtime(runtime)?;
        self.session = SessionHandle {
            id: session_id,
            path: handle.path,
        };
        println!(
            "{}",
            format_resume_report(
                &self.session.path.display().to_string(),
                message_count,
                self.runtime.usage().turns(),
            )
        );
        Ok(true)
    }

    /// Print the rendered config report for the given section.
    fn print_config(section: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        println!("{}", render_config_report(section)?);
        Ok(())
    }

    /// Print the memory files report.
    fn print_memory() -> Result<(), Box<dyn std::error::Error>> {
        println!("{}", render_memory_report()?);
        Ok(())
    }

    /// Print the agents directory report.
    pub(crate) fn print_agents(args: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        let cwd = env::current_dir()?;
        println!("{}", handle_agents_slash_command(args, &cwd)?);
        Ok(())
    }

    /// Print the skills directory report.
    pub(crate) fn print_skills(args: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        let cwd = env::current_dir()?;
        println!("{}", handle_skills_slash_command(args, &cwd)?);
        Ok(())
    }

    /// Print the git diff report.
    fn print_diff() -> Result<(), Box<dyn std::error::Error>> {
        println!("{}", render_diff_report()?);
        Ok(())
    }

    /// Print the current version string.
    fn print_version() {
        println!("{}", render_version_report());
    }

    /// Export the session transcript to a file.
    fn export_session(
        &self,
        requested_path: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let export_path = resolve_export_path(requested_path, self.runtime.session())?;
        fs::write(&export_path, render_export_text(self.runtime.session()))?;
        println!(
            "{}",
            format_export_report(
                &export_path.display().to_string(),
                self.runtime.session().messages.len(),
            )
        );
        Ok(())
    }

    /// Handle `/session list|switch|fork` commands.
    fn handle_session_command(
        &mut self,
        action: Option<&str>,
        target: Option<&str>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        match action {
            None | Some("list") => {
                println!("{}", render_session_list(&self.session.id)?);
                Ok(false)
            }
            Some("switch") => {
                let Some(target) = target else {
                    println!("{}", format_session_switch_usage());
                    return Ok(false);
                };
                let handle = resolve_session_reference(target)?;
                let session = Session::load_from_path(&handle.path)?;
                let message_count = session.messages.len();
                let session_id = session.session_id.clone();
                let runtime = build_runtime(
                    session,
                    &handle.id,
                    self.model.clone(),
                    self.system_prompt.clone(),
                    true,
                    true,
                    self.allowed_tools.clone(),
                    self.permission_mode,
                    None,
                )?;
                self.replace_runtime(runtime)?;
                self.session = SessionHandle {
                    id: session_id,
                    path: handle.path,
                };
                println!(
                    "{}",
                    format_session_switch_report(
                        &self.session.id,
                        &self.session.path.display().to_string(),
                        message_count,
                    )
                );
                Ok(true)
            }
            Some("fork") => {
                let forked = self.runtime.fork_session(target.map(ToOwned::to_owned));
                let parent_session_id = self.session.id.clone();
                let handle = create_managed_session_handle(&forked.session_id)?;
                let branch_name = forked
                    .fork
                    .as_ref()
                    .and_then(|fork| fork.branch_name.clone());
                let forked = forked.with_persistence_path(handle.path.clone());
                let message_count = forked.messages.len();
                forked.save_to_path(&handle.path)?;
                let runtime = build_runtime(
                    forked,
                    &handle.id,
                    self.model.clone(),
                    self.system_prompt.clone(),
                    true,
                    true,
                    self.allowed_tools.clone(),
                    self.permission_mode,
                    None,
                )?;
                self.replace_runtime(runtime)?;
                self.session = handle;
                println!(
                    "{}",
                    format_session_fork_report(
                        &parent_session_id,
                        &self.session.id,
                        branch_name.as_deref(),
                        &self.session.path.display().to_string(),
                        message_count,
                    )
                );
                Ok(true)
            }
            Some(other) => {
                println!("{}", format_session_unknown_action(other));
                Ok(false)
            }
        }
    }

    /// Handle `/plugins` commands, reloading the runtime when the plugin set changes.
    fn handle_plugins_command(
        &mut self,
        action: Option<&str>,
        target: Option<&str>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let cwd = env::current_dir()?;
        let loader = ConfigLoader::default_for(&cwd);
        let runtime_config = loader.load()?;
        let mut manager = build_plugin_manager(&cwd, &loader, &runtime_config);
        let result = handle_plugins_slash_command(action, target, &mut manager)?;
        println!("{}", result.message);
        if result.reload_runtime {
            self.reload_runtime_features()?;
        }
        Ok(false)
    }

    /// Rebuild the runtime to pick up any newly installed plugins or config changes.
    fn reload_runtime_features(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let runtime = build_runtime(
            self.runtime.session().clone(),
            &self.session.id,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            true,
            self.allowed_tools.clone(),
            self.permission_mode,
            None,
        )?;
        self.replace_runtime(runtime)?;
        self.persist_session()
    }

    /// Compact the session history, removing old messages.
    fn compact(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let result = self.runtime.compact(CompactionConfig::default());
        let removed = result.removed_message_count;
        let kept = result.compacted_session.messages.len();
        let skipped = removed == 0;
        let runtime = build_runtime(
            result.compacted_session,
            &self.session.id,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            true,
            self.allowed_tools.clone(),
            self.permission_mode,
            None,
        )?;
        self.replace_runtime(runtime)?;
        self.persist_session()?;
        println!("{}", format_compact_report(removed, kept, skipped));
        Ok(())
    }

    /// Run an internal (background) prompt, optionally reporting progress.
    #[allow(dead_code)] // Called dynamically via REPL command dispatch
    fn run_internal_prompt_text_with_progress(
        &self,
        prompt: &str,
        enable_tools: bool,
        progress: Option<InternalPromptProgressReporter>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let session = self.runtime.session().clone();
        let mut runtime = build_runtime(
            session,
            &self.session.id,
            self.model.clone(),
            self.system_prompt.clone(),
            enable_tools,
            false,
            self.allowed_tools.clone(),
            self.permission_mode,
            progress,
        )?;
        let mut permission_prompter = CliPermissionPrompter::new(self.permission_mode);
        let summary = runtime.run_turn(prompt, Some(&mut permission_prompter))?;
        let text = final_assistant_text(&summary).trim().to_string();
        runtime.shutdown_plugins()?;
        Ok(text)
    }

    /// Run an internal prompt without progress reporting.
    #[allow(dead_code)] // Called dynamically via REPL command dispatch
    fn run_internal_prompt_text(
        &self,
        prompt: &str,
        enable_tools: bool,
    ) -> Result<String, Box<dyn std::error::Error>> {
        self.run_internal_prompt_text_with_progress(prompt, enable_tools, None)
    }

    /// Handle `/bughunter` — print scope-specific bug-hunting instructions.
    #[allow(clippy::unused_self, clippy::unnecessary_wraps)]
    fn run_bughunter(&self, scope: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        println!("{}", format_bughunter_report(scope));
        Ok(())
    }

    /// Handle `/ultraplan` — print project-planning prompt scaffold.
    #[allow(clippy::unused_self, clippy::unnecessary_wraps)]
    fn run_ultraplan(&self, task: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        println!("{}", format_ultraplan_report(task));
        Ok(())
    }

    /// Handle `/teleport` — jump to a symbol or path in the editor.
    #[allow(clippy::unused_self)]
    fn run_teleport(&self, target: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        let Some(target) = validate_teleport_target(target) else {
            println!("{}", format_teleport_usage());
            return Ok(());
        };

        println!("{}", render_teleport_report(target)?);
        Ok(())
    }

    /// Handle `/debug-tool-call` — print the last raw tool call.
    #[allow(clippy::unused_self)]
    fn run_debug_tool_call(&self, args: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        validate_no_args("/debug-tool-call", args)?;
        println!("{}", render_last_tool_debug_report(self.runtime.session())?);
        Ok(())
    }

    /// Handle `/commit` — show a git commit preflight report.
    #[allow(clippy::unused_self)]
    fn run_commit(&mut self, args: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        validate_no_args("/commit", args)?;
        let status = git_output(&["status", "--short", "--branch"])?;
        match resolve_commit_preflight(&status) {
            None => println!("{}", format_commit_skipped_report()),
            Some(report) => println!("{report}"),
        }
        Ok(())
    }

    /// Handle `/pr` — print a pull-request context template.
    #[allow(clippy::unused_self)]
    fn run_pr(&self, context: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        let branch =
            resolve_git_branch_for(&env::current_dir()?).unwrap_or_else(|| "unknown".to_string());
        println!("{}", format_pr_report(&branch, context));
        Ok(())
    }

    /// Handle `/issue` — print a GitHub issue template.
    #[allow(clippy::unused_self, clippy::unnecessary_wraps)]
    fn run_issue(&self, context: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        println!("{}", format_issue_report(context));
        Ok(())
    }
}

// Bring `resolve_git_branch_for` into scope for `run_pr`.
use crate::reports::resolve_git_branch_for;

// ---------------------------------------------------------------------------
// Extracted pure functions — testable without runtime/terminal dependencies.
// ---------------------------------------------------------------------------

/// Parameters for rendering the startup banner (all pre-resolved strings).
pub(crate) struct BannerParams<'a> {
    /// Active model identifier.
    pub model: &'a str,
    /// Current permission mode label.
    pub permission_mode: &'a str,
    /// Git branch name (e.g. "main").
    pub git_branch: &'a str,
    /// One-line workspace summary (e.g. "3 staged, 1 modified").
    pub workspace: &'a str,
    /// Current working directory path.
    pub cwd: &'a str,
    /// Session identifier string.
    pub session_id: &'a str,
    /// Relative (or absolute) session file path.
    pub session_path: &'a str,
}

/// Render the startup banner from pre-resolved parameters (pure, no I/O).
pub(crate) fn format_startup_banner(params: &BannerParams<'_>) -> String {
    format!(
        "\x1b[38;5;208m\
 ██████╗ ██████╗ ██╗      ██████╗ ████████╗\n\
██╔════╝██╔═══██╗██║     ██╔═══██╗╚══██╔══╝\n\
██║     ██║   ██║██║     ██║   ██║   ██║   \n\
██║     ██║   ██║██║     ██║   ██║   ██║   \n\
╚██████╗╚██████╔╝███████╗╚██████╔╝   ██║   \n\
 ╚═════╝ ╚═════╝ ╚══════╝ ╚═════╝    ╚═╝   \x1b[0m\x1b[38;5;196mCook\x1b[0m 🍳\n\n\
  \x1b[2mModel\x1b[0m            {}\n\
  \x1b[2mPermissions\x1b[0m      {}\n\
  \x1b[2mBranch\x1b[0m           {}\n\
  \x1b[2mWorkspace\x1b[0m        {}\n\
  \x1b[2mDirectory\x1b[0m        {}\n\
  \x1b[2mSession\x1b[0m          {}\n\
  \x1b[2mAuto-save\x1b[0m        {}\n\n\
  Type \x1b[1m/help\x1b[0m for commands · \x1b[1m/status\x1b[0m for live context · \x1b[2m/resume latest\x1b[0m jumps back to the newest session · \x1b[1m/diff\x1b[0m then \x1b[1m/commit\x1b[0m to ship · \x1b[2mTab\x1b[0m for workflow completions · \x1b[2mShift+Enter\x1b[0m for newline",
        params.model,
        params.permission_mode,
        params.git_branch,
        params.workspace,
        params.cwd,
        params.session_id,
        params.session_path,
    )
}

/// Outcome of resolving a `/model` command (pure decision, no side-effects).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ModelSwitchOutcome {
    /// No model argument given — show the current model report.
    ShowCurrent,
    /// Requested model is already the active model.
    AlreadyCurrent,
    /// Model should be switched; `resolved` is the canonical model name.
    SwitchTo { resolved: String },
}

/// Decide what a `/model [name]` command should do (pure logic, no runtime).
pub(crate) fn resolve_model_switch(
    current_model: &str,
    requested: Option<&str>,
) -> ModelSwitchOutcome {
    let Some(requested) = requested else {
        return ModelSwitchOutcome::ShowCurrent;
    };
    let resolved = resolve_model_alias(requested).to_string();
    if resolved == current_model {
        ModelSwitchOutcome::AlreadyCurrent
    } else {
        ModelSwitchOutcome::SwitchTo { resolved }
    }
}

/// Outcome of resolving a `/permissions` command (pure decision, no side-effects).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PermissionSwitchOutcome {
    /// No mode argument given — show the current report.
    ShowCurrent,
    /// Requested mode is invalid.
    Invalid { input: String },
    /// Requested mode is already active.
    AlreadyCurrent { normalized: &'static str },
    /// Permission mode should be changed.
    SwitchTo { normalized: &'static str },
}

/// Decide what a `/permissions [mode]` command should do (pure logic, no runtime).
pub(crate) fn resolve_permission_switch(
    current_mode: &str,
    requested: Option<&str>,
) -> PermissionSwitchOutcome {
    let Some(requested) = requested else {
        return PermissionSwitchOutcome::ShowCurrent;
    };
    match normalize_permission_mode(requested) {
        None => PermissionSwitchOutcome::Invalid {
            input: requested.to_string(),
        },
        Some(normalized) if normalized == current_mode => {
            PermissionSwitchOutcome::AlreadyCurrent { normalized }
        }
        Some(normalized) => PermissionSwitchOutcome::SwitchTo { normalized },
    }
}

/// Format the `/clear` confirmation-required message.
pub(crate) fn format_clear_confirmation_required() -> &'static str {
    "clear: confirmation required; run /clear --confirm to start a fresh session."
}

/// Format the report shown after a session is successfully cleared.
pub(crate) fn format_clear_session_report(
    model: &str,
    permission_mode: &str,
    session_id: &str,
) -> String {
    format!(
        "Session cleared\n  Mode             fresh session\n  Preserved model  {model}\n  Permission mode  {permission_mode}\n  Session          {session_id}",
    )
}

/// Format the report shown after a successful `/export`.
pub(crate) fn format_export_report(export_path: &str, message_count: usize) -> String {
    format!(
        "Export\n  Result           wrote transcript\n  File             {export_path}\n  Messages         {message_count}",
    )
}

/// Format the report shown after a `/session switch`.
pub(crate) fn format_session_switch_report(
    session_id: &str,
    session_path: &str,
    message_count: usize,
) -> String {
    format!(
        "Session switched\n  Active session   {session_id}\n  File             {session_path}\n  Messages         {message_count}",
    )
}

/// Format the report shown after a `/session fork`.
pub(crate) fn format_session_fork_report(
    parent_session_id: &str,
    session_id: &str,
    branch_name: Option<&str>,
    session_path: &str,
    message_count: usize,
) -> String {
    format!(
        "Session forked\n  Parent session   {}\n  Active session   {}\n  Branch           {}\n  File             {}\n  Messages         {}",
        parent_session_id,
        session_id,
        branch_name.unwrap_or("(unnamed)"),
        session_path,
        message_count,
    )
}

/// Format the error message for an unknown `/session` action.
pub(crate) fn format_session_unknown_action(action: &str) -> String {
    format!(
        "Unknown /session action '{action}'. Use /session list, /session switch <session-id>, or /session fork [branch-name]."
    )
}

/// Format the usage hint shown when `/teleport` is called without arguments.
pub(crate) fn format_teleport_usage() -> &'static str {
    "Usage: /teleport <symbol-or-path>"
}

/// Format the usage hint shown when `/session switch` is called without a target.
pub(crate) fn format_session_switch_usage() -> &'static str {
    "Usage: /session switch <session-id>"
}

/// Validate a teleport target, trimming whitespace and rejecting empty input.
///
/// Returns `Some(trimmed)` if valid, `None` if empty or whitespace-only.
pub(crate) fn validate_teleport_target(target: Option<&str>) -> Option<&str> {
    target.map(str::trim).filter(|value| !value.is_empty())
}

/// Build the JSON value emitted by `run_prompt_json` (pure, no I/O).
///
/// The caller provides pre-collected data extracted from a `TurnSummary`.
pub(crate) fn build_prompt_json_value(params: &PromptJsonParams<'_>) -> serde_json::Value {
    json!({
        "message": params.assistant_text,
        "model": params.model,
        "iterations": params.iterations,
        "auto_compaction": params.auto_compaction_removed.map(|removed| json!({
            "removed_messages": removed,
            "notice": format_auto_compaction_notice(removed),
        })),
        "tool_uses": params.tool_uses,
        "tool_results": params.tool_results,
        "prompt_cache_events": params.prompt_cache_events,
        "usage": {
            "input_tokens": params.usage_input,
            "output_tokens": params.usage_output,
            "cache_creation_input_tokens": params.usage_cache_creation,
            "cache_read_input_tokens": params.usage_cache_read,
        }
    })
}

/// Flat parameter bag for [`build_prompt_json_value`].
pub(crate) struct PromptJsonParams<'a> {
    /// Final assistant text extracted from the turn summary.
    pub assistant_text: &'a str,
    /// Model identifier.
    pub model: &'a str,
    /// Number of agentic iterations in the turn.
    pub iterations: usize,
    /// If auto-compaction occurred, the number of removed messages.
    pub auto_compaction_removed: Option<usize>,
    /// Serialised tool-use entries.
    pub tool_uses: &'a [serde_json::Value],
    /// Serialised tool-result entries.
    pub tool_results: &'a [serde_json::Value],
    /// Serialised prompt-cache events.
    pub prompt_cache_events: &'a [serde_json::Value],
    /// Token usage counters.
    pub usage_input: u32,
    pub usage_output: u32,
    pub usage_cache_creation: u32,
    pub usage_cache_read: u32,
}

/// Determine whether a `/commit` preflight should skip (workspace clean) or proceed.
///
/// Returns `None` when the workspace is clean, or `Some(report)` with the preflight report.
pub(crate) fn resolve_commit_preflight(git_status_output: &str) -> Option<String> {
    let summary = parse_git_workspace_summary(Some(git_status_output));
    let branch = parse_git_status_branch(Some(git_status_output));
    if summary.is_clean() {
        return None;
    }
    Some(format_commit_preflight_report(branch.as_deref(), summary))
}

// ---------------------------------------------------------------------------
// Tests for extracted pure functions.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- format_startup_banner ------------------------------------------------

    #[test]
    fn startup_banner_contains_all_fields() {
        let banner = format_startup_banner(&BannerParams {
            model: "claude-opus-4-6",
            permission_mode: "danger-full-access",
            git_branch: "main",
            workspace: "3 staged",
            cwd: "/tmp/project",
            session_id: "sess-abc",
            session_path: ".colotcook/sessions/sess-abc.json",
        });
        assert!(banner.contains("claude-opus-4-6"));
        assert!(banner.contains("danger-full-access"));
        assert!(banner.contains("main"));
        assert!(banner.contains("3 staged"));
        assert!(banner.contains("/tmp/project"));
        assert!(banner.contains("sess-abc"));
        assert!(banner.contains(".colotcook/sessions/sess-abc.json"));
        assert!(banner.contains("Cook"));
        assert!(banner.contains("/help"));
    }

    #[test]
    fn startup_banner_renders_unknown_branch() {
        let banner = format_startup_banner(&BannerParams {
            model: "gpt-4o",
            permission_mode: "read-only",
            git_branch: "unknown",
            workspace: "unknown",
            cwd: "/home/user",
            session_id: "s1",
            session_path: "s1.json",
        });
        assert!(banner.contains("unknown"));
        assert!(banner.contains("gpt-4o"));
    }

    // -- resolve_model_switch -------------------------------------------------

    #[test]
    fn model_switch_show_current_when_none() {
        assert_eq!(
            resolve_model_switch("claude-opus-4-6", None),
            ModelSwitchOutcome::ShowCurrent
        );
    }

    #[test]
    fn model_switch_already_current_same_name() {
        assert_eq!(
            resolve_model_switch("claude-opus-4-6", Some("claude-opus-4-6")),
            ModelSwitchOutcome::AlreadyCurrent
        );
    }

    #[test]
    fn model_switch_already_current_via_alias() {
        // "opus" resolves to "claude-opus-4-6"
        assert_eq!(
            resolve_model_switch("claude-opus-4-6", Some("opus")),
            ModelSwitchOutcome::AlreadyCurrent
        );
    }

    #[test]
    fn model_switch_to_different_model() {
        assert_eq!(
            resolve_model_switch("claude-opus-4-6", Some("sonnet")),
            ModelSwitchOutcome::SwitchTo {
                resolved: "claude-sonnet-4-6".to_string()
            }
        );
    }

    #[test]
    fn model_switch_unrecognised_alias_passed_through() {
        assert_eq!(
            resolve_model_switch("claude-opus-4-6", Some("my-custom-model")),
            ModelSwitchOutcome::SwitchTo {
                resolved: "my-custom-model".to_string()
            }
        );
    }

    // -- resolve_permission_switch --------------------------------------------

    #[test]
    fn permission_switch_show_current_when_none() {
        assert_eq!(
            resolve_permission_switch("danger-full-access", None),
            PermissionSwitchOutcome::ShowCurrent
        );
    }

    #[test]
    fn permission_switch_invalid_mode() {
        assert_eq!(
            resolve_permission_switch("read-only", Some("banana")),
            PermissionSwitchOutcome::Invalid {
                input: "banana".to_string()
            }
        );
    }

    #[test]
    fn permission_switch_already_current() {
        assert_eq!(
            resolve_permission_switch("read-only", Some("read-only")),
            PermissionSwitchOutcome::AlreadyCurrent {
                normalized: "read-only"
            }
        );
    }

    #[test]
    fn permission_switch_to_different_mode() {
        assert_eq!(
            resolve_permission_switch("read-only", Some("workspace-write")),
            PermissionSwitchOutcome::SwitchTo {
                normalized: "workspace-write"
            }
        );
    }

    #[test]
    fn permission_switch_normalises_alias() {
        // "danger-full-access" can also be written as "danger" depending on normalize_permission_mode
        let result = resolve_permission_switch("read-only", Some("danger-full-access"));
        assert_eq!(
            result,
            PermissionSwitchOutcome::SwitchTo {
                normalized: "danger-full-access"
            }
        );
    }

    // -- format_clear_confirmation_required ------------------------------------

    #[test]
    fn clear_confirmation_message_mentions_confirm_flag() {
        let msg = format_clear_confirmation_required();
        assert!(msg.contains("--confirm"));
        assert!(msg.contains("/clear"));
    }

    // -- format_clear_session_report ------------------------------------------

    #[test]
    fn clear_session_report_contains_all_fields() {
        let report = format_clear_session_report("gpt-4o", "read-only", "sess-xyz");
        assert!(report.contains("Session cleared"));
        assert!(report.contains("gpt-4o"));
        assert!(report.contains("read-only"));
        assert!(report.contains("sess-xyz"));
        assert!(report.contains("fresh session"));
    }

    // -- format_export_report -------------------------------------------------

    #[test]
    fn export_report_contains_path_and_count() {
        let report = format_export_report("/tmp/export.md", 42);
        assert!(report.contains("Export"));
        assert!(report.contains("/tmp/export.md"));
        assert!(report.contains("42"));
        assert!(report.contains("wrote transcript"));
    }

    // -- format_session_switch_report -----------------------------------------

    #[test]
    fn session_switch_report_contains_all_fields() {
        let report = format_session_switch_report("s-123", "/tmp/s-123.json", 10);
        assert!(report.contains("Session switched"));
        assert!(report.contains("s-123"));
        assert!(report.contains("/tmp/s-123.json"));
        assert!(report.contains("10"));
    }

    // -- format_session_fork_report -------------------------------------------

    #[test]
    fn session_fork_report_with_branch() {
        let report = format_session_fork_report(
            "parent-1",
            "child-2",
            Some("feature/foo"),
            "/tmp/child.json",
            5,
        );
        assert!(report.contains("Session forked"));
        assert!(report.contains("parent-1"));
        assert!(report.contains("child-2"));
        assert!(report.contains("feature/foo"));
        assert!(report.contains("/tmp/child.json"));
        assert!(report.contains("5"));
    }

    #[test]
    fn session_fork_report_without_branch() {
        let report = format_session_fork_report("p", "c", None, "/f.json", 0);
        assert!(report.contains("(unnamed)"));
    }

    // -- format_session_unknown_action ----------------------------------------

    #[test]
    fn session_unknown_action_includes_action_name() {
        let msg = format_session_unknown_action("destroy");
        assert!(msg.contains("destroy"));
        assert!(msg.contains("Unknown /session action"));
    }

    // -- format_teleport_usage ------------------------------------------------

    #[test]
    fn teleport_usage_mentions_symbol_or_path() {
        assert!(format_teleport_usage().contains("/teleport"));
    }

    // -- format_session_switch_usage ------------------------------------------

    #[test]
    fn session_switch_usage_mentions_session_id() {
        assert!(format_session_switch_usage().contains("session-id"));
    }

    // -- validate_teleport_target ---------------------------------------------

    #[test]
    fn validate_teleport_target_none() {
        assert_eq!(validate_teleport_target(None), None);
    }

    #[test]
    fn validate_teleport_target_empty() {
        assert_eq!(validate_teleport_target(Some("")), None);
    }

    #[test]
    fn validate_teleport_target_whitespace() {
        assert_eq!(validate_teleport_target(Some("   ")), None);
    }

    #[test]
    fn validate_teleport_target_valid() {
        assert_eq!(
            validate_teleport_target(Some("  foo::bar  ")),
            Some("foo::bar")
        );
    }

    // -- build_prompt_json_value ----------------------------------------------

    #[test]
    fn prompt_json_contains_expected_keys() {
        let value = build_prompt_json_value(&PromptJsonParams {
            assistant_text: "Hello world",
            model: "claude-opus-4-6",
            iterations: 3,
            auto_compaction_removed: None,
            tool_uses: &[],
            tool_results: &[],
            prompt_cache_events: &[],
            usage_input: 100,
            usage_output: 50,
            usage_cache_creation: 10,
            usage_cache_read: 5,
        });
        assert_eq!(value["message"], "Hello world");
        assert_eq!(value["model"], "claude-opus-4-6");
        assert_eq!(value["iterations"], 3);
        assert!(value["auto_compaction"].is_null());
        assert_eq!(value["usage"]["input_tokens"], 100);
        assert_eq!(value["usage"]["output_tokens"], 50);
        assert_eq!(value["usage"]["cache_creation_input_tokens"], 10);
        assert_eq!(value["usage"]["cache_read_input_tokens"], 5);
    }

    #[test]
    fn prompt_json_includes_auto_compaction_when_present() {
        let value = build_prompt_json_value(&PromptJsonParams {
            assistant_text: "ok",
            model: "m",
            iterations: 1,
            auto_compaction_removed: Some(7),
            tool_uses: &[],
            tool_results: &[],
            prompt_cache_events: &[],
            usage_input: 0,
            usage_output: 0,
            usage_cache_creation: 0,
            usage_cache_read: 0,
        });
        assert_eq!(value["auto_compaction"]["removed_messages"], 7);
        assert!(value["auto_compaction"]["notice"]
            .as_str()
            .unwrap()
            .contains('7'));
    }

    #[test]
    fn prompt_json_includes_tool_uses() {
        let tool_use = serde_json::json!({"id": "t1", "name": "read_file"});
        let value = build_prompt_json_value(&PromptJsonParams {
            assistant_text: "",
            model: "m",
            iterations: 1,
            auto_compaction_removed: None,
            tool_uses: &[tool_use.clone()],
            tool_results: &[],
            prompt_cache_events: &[],
            usage_input: 0,
            usage_output: 0,
            usage_cache_creation: 0,
            usage_cache_read: 0,
        });
        assert_eq!(value["tool_uses"][0]["name"], "read_file");
    }

    // -- resolve_commit_preflight ---------------------------------------------

    #[test]
    fn commit_preflight_clean_workspace_returns_none() {
        // A clean `git status --short --branch` output has only the branch line.
        let status = "## main...origin/main\n";
        assert!(resolve_commit_preflight(status).is_none());
    }

    #[test]
    fn commit_preflight_dirty_workspace_returns_report() {
        let status = "## main...origin/main\n M src/lib.rs\n?? new_file.txt\n";
        let report = resolve_commit_preflight(status).expect("should be Some");
        assert!(report.contains("Commit"));
    }
}
