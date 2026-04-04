//! Report formatting: status, model, permissions, cost, sandbox, git, export, etc.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use colotcook_commands::render_slash_command_help;
use colotcook_runtime as runtime;
use colotcook_runtime::{
    resolve_sandbox_status, ConfigLoader, ConfigSource, ContentBlock, MessageRole, ProjectContext,
    Session, TokenUsage,
};

use crate::arg_parsing::{BUILD_TARGET, DEFAULT_DATE, GIT_SHA, VERSION};
use crate::init::initialize_repo;
use crate::session_management::{LATEST_SESSION_REFERENCE, PRIMARY_SESSION_EXTENSION};
use crate::util::{indent_block, truncate_for_prompt};

// ── Types ───────────────────────────────────────────────────────────────────

/// Workspace status snapshot for the /status report.
pub(crate) struct StatusContext {
    pub cwd: PathBuf,
    pub session_path: Option<PathBuf>,
    pub loaded_config_files: usize,
    pub discovered_config_files: usize,
    pub memory_file_count: usize,
    pub project_root: Option<PathBuf>,
    pub git_branch: Option<String>,
    pub git_summary: GitWorkspaceSummary,
    pub sandbox_status: runtime::SandboxStatus,
}

/// Token usage snapshot for the /status report.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StatusUsage {
    pub message_count: usize,
    pub turns: u32,
    pub latest: TokenUsage,
    pub cumulative: TokenUsage,
    pub estimated_tokens: usize,
}

/// Summary of the git workspace state (staged, unstaged, etc.).
#[allow(clippy::struct_field_names)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct GitWorkspaceSummary {
    pub changed_files: usize,
    pub staged_files: usize,
    pub unstaged_files: usize,
    pub untracked_files: usize,
    pub conflicted_files: usize,
}

impl GitWorkspaceSummary {
    pub fn is_clean(self) -> bool {
        self.changed_files == 0
    }

    pub fn headline(self) -> String {
        if self.is_clean() {
            "clean".to_string()
        } else {
            let mut details = Vec::new();
            if self.staged_files > 0 {
                details.push(format!("{} staged", self.staged_files));
            }
            if self.unstaged_files > 0 {
                details.push(format!("{} unstaged", self.unstaged_files));
            }
            if self.untracked_files > 0 {
                details.push(format!("{} untracked", self.untracked_files));
            }
            if self.conflicted_files > 0 {
                details.push(format!("{} conflicted", self.conflicted_files));
            }
            format!(
                "dirty · {} files · {}",
                self.changed_files,
                details.join(", ")
            )
        }
    }
}

// ── Git helpers ─────────────────────────────────────────────────────────────

/// Parse the git status porcelain output into metadata (project root, branch).
pub(crate) fn parse_git_status_metadata(status: Option<&str>) -> (Option<PathBuf>, Option<String>) {
    parse_git_status_metadata_for(
        &env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        status,
    )
}

/// Parse the branch name from git status porcelain header.
pub(crate) fn parse_git_status_branch(status: Option<&str>) -> Option<String> {
    let status = status?;
    let first_line = status.lines().next()?;
    let line = first_line.strip_prefix("## ")?;
    if line.starts_with("HEAD") {
        return Some("detached HEAD".to_string());
    }
    let branch = line.split(['.', ' ']).next().unwrap_or_default().trim();
    if branch.is_empty() {
        None
    } else {
        Some(branch.to_string())
    }
}

/// Parse the workspace summary (staged/unstaged/untracked counts) from git status.
pub(crate) fn parse_git_workspace_summary(status: Option<&str>) -> GitWorkspaceSummary {
    let mut summary = GitWorkspaceSummary::default();
    let Some(status) = status else {
        return summary;
    };

    for line in status.lines() {
        if line.starts_with("## ") || line.trim().is_empty() {
            continue;
        }

        summary.changed_files += 1;
        let mut chars = line.chars();
        let index_status = chars.next().unwrap_or(' ');
        let worktree_status = chars.next().unwrap_or(' ');

        if index_status == '?' && worktree_status == '?' {
            summary.untracked_files += 1;
            continue;
        }

        if index_status != ' ' {
            summary.staged_files += 1;
        }
        if worktree_status != ' ' {
            summary.unstaged_files += 1;
        }
        if (matches!(index_status, 'U' | 'A') && matches!(worktree_status, 'U' | 'A'))
            || index_status == 'U'
            || worktree_status == 'U'
        {
            summary.conflicted_files += 1;
        }
    }

    summary
}

/// Resolve the current git branch name.
pub(crate) fn resolve_git_branch_for(cwd: &Path) -> Option<String> {
    let branch = run_git_capture_in(cwd, &["branch", "--show-current"])?;
    let branch = branch.trim();
    if !branch.is_empty() {
        return Some(branch.to_string());
    }

    let fallback = run_git_capture_in(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let fallback = fallback.trim();
    if fallback.is_empty() {
        None
    } else if fallback == "HEAD" {
        Some("detached HEAD".to_string())
    } else {
        Some(fallback.to_string())
    }
}

/// Run a git command and capture stdout.
pub(crate) fn run_git_capture_in(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// Find the git repository root for a given directory.
pub(crate) fn find_git_root_in(cwd: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .output()?;
    if !output.status.success() {
        return Err("not a git repository".into());
    }
    let path = String::from_utf8(output.stdout)?.trim().to_string();
    if path.is_empty() {
        return Err("empty git root".into());
    }
    Ok(PathBuf::from(path))
}

/// Combined metadata lookup (root + branch) for a given directory.
pub(crate) fn parse_git_status_metadata_for(
    cwd: &Path,
    status: Option<&str>,
) -> (Option<PathBuf>, Option<String>) {
    let branch = resolve_git_branch_for(cwd).or_else(|| parse_git_status_branch(status));
    let project_root = find_git_root_in(cwd).ok();
    (project_root, branch)
}

// ── Status context ──────────────────────────────────────────────────────────

/// Build the status context for the current workspace.
pub(crate) fn status_context(
    session_path: Option<&Path>,
) -> Result<StatusContext, Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    let loader = ConfigLoader::default_for(&cwd);
    let discovered_config_files = loader.discover().len();
    let runtime_config = loader.load()?;
    let project_context = ProjectContext::discover_with_git(&cwd, DEFAULT_DATE)?;
    let (project_root, git_branch) =
        parse_git_status_metadata(project_context.git_status.as_deref());
    let git_summary = parse_git_workspace_summary(project_context.git_status.as_deref());
    let sandbox_status = resolve_sandbox_status(runtime_config.sandbox(), &cwd);
    Ok(StatusContext {
        cwd,
        session_path: session_path.map(Path::to_path_buf),
        loaded_config_files: runtime_config.loaded_entries().len(),
        discovered_config_files,
        memory_file_count: project_context.instruction_files.len(),
        project_root,
        git_branch,
        git_summary,
        sandbox_status,
    })
}

// ── Format/render functions ─────────────────────────────────────────────────

/// Test-only helper for formatting unknown slash command messages.
#[cfg(test)]
pub(crate) fn format_unknown_slash_command_message(name: &str) -> String {
    let suggestions = crate::arg_parsing::suggest_slash_commands(name);
    if suggestions.is_empty() {
        format!("unknown slash command: /{name}. Use /help to list available commands.")
    } else {
        format!(
            "unknown slash command: /{name}. Did you mean {}? Use /help to list available commands.",
            suggestions.join(", ")
        )
    }
}

pub(crate) fn format_model_report(model: &str, message_count: usize, turns: u32) -> String {
    format!(
        "Model
  Current model    {model}
  Session messages {message_count}
  Session turns    {turns}

Usage
  Inspect current model with /model
  Switch models with /model <name>"
    )
}

pub(crate) fn format_model_switch_report(
    previous: &str,
    next: &str,
    message_count: usize,
) -> String {
    format!(
        "Model updated
  Previous         {previous}
  Current          {next}
  Preserved msgs   {message_count}"
    )
}

pub(crate) fn format_permissions_report(mode: &str) -> String {
    let modes = [
        ("read-only", "Read/search tools only", mode == "read-only"),
        (
            "workspace-write",
            "Edit files inside the workspace",
            mode == "workspace-write",
        ),
        (
            "danger-full-access",
            "Unrestricted tool access",
            mode == "danger-full-access",
        ),
    ]
    .into_iter()
    .map(|(name, description, is_current)| {
        let marker = if is_current {
            "● current"
        } else {
            "○ available"
        };
        format!("  {name:<18} {marker:<11} {description}")
    })
    .collect::<Vec<_>>()
    .join(
        "
",
    );

    format!(
        "Permissions
  Active mode      {mode}
  Mode status      live session default

Modes
{modes}

Usage
  Inspect current mode with /permissions
  Switch modes with /permissions <mode>"
    )
}

pub(crate) fn format_permissions_switch_report(previous: &str, next: &str) -> String {
    format!(
        "Permissions updated
  Result           mode switched
  Previous mode    {previous}
  Active mode      {next}
  Applies to       subsequent tool calls
  Usage            /permissions to inspect current mode"
    )
}

pub(crate) fn format_cost_report(usage: TokenUsage) -> String {
    format!(
        "Cost
  Input tokens     {}
  Output tokens    {}
  Cache create     {}
  Cache read       {}
  Total tokens     {}",
        usage.input_tokens,
        usage.output_tokens,
        usage.cache_creation_input_tokens,
        usage.cache_read_input_tokens,
        usage.total_tokens(),
    )
}

pub(crate) fn format_resume_report(session_path: &str, message_count: usize, turns: u32) -> String {
    format!(
        "Session resumed
  Session file     {session_path}
  Messages         {message_count}
  Turns            {turns}"
    )
}

pub(crate) fn render_resume_usage() -> String {
    format!(
        "Resume
  Usage            /resume <session-path|session-id|{LATEST_SESSION_REFERENCE}>
  Auto-save        .colotcook/sessions/<session-id>.{PRIMARY_SESSION_EXTENSION}
  Tip              use /session list to inspect saved sessions"
    )
}

pub(crate) fn format_compact_report(
    removed: usize,
    resulting_messages: usize,
    skipped: bool,
) -> String {
    if skipped {
        format!(
            "Compact
  Result           skipped
  Reason           session below compaction threshold
  Messages kept    {resulting_messages}"
        )
    } else {
        format!(
            "Compact
  Result           compacted
  Messages removed {removed}
  Messages kept    {resulting_messages}"
        )
    }
}

pub(crate) fn format_auto_compaction_notice(removed: usize) -> String {
    format!("[auto-compacted: removed {removed} messages]")
}

pub(crate) fn format_status_report(
    model: &str,
    usage: StatusUsage,
    permission_mode: &str,
    context: &StatusContext,
) -> String {
    [
        format!(
            "Status
  Model            {model}
  Permission mode  {permission_mode}
  Messages         {}
  Turns            {}
  Estimated tokens {}",
            usage.message_count, usage.turns, usage.estimated_tokens,
        ),
        format!(
            "Usage
  Latest total     {}
  Cumulative input {}
  Cumulative output {}
  Cumulative total {}",
            usage.latest.total_tokens(),
            usage.cumulative.input_tokens,
            usage.cumulative.output_tokens,
            usage.cumulative.total_tokens(),
        ),
        format!(
            "Workspace
  Cwd              {}
  Project root     {}
  Git branch       {}
  Git state        {}
  Changed files    {}
  Staged           {}
  Unstaged         {}
  Untracked        {}
  Session          {}
  Config files     loaded {}/{}
  Memory files     {}
  Suggested flow   /status → /diff → /commit",
            context.cwd.display(),
            context
                .project_root
                .as_ref()
                .map_or_else(|| "unknown".to_string(), |path| path.display().to_string()),
            context.git_branch.as_deref().unwrap_or("unknown"),
            context.git_summary.headline(),
            context.git_summary.changed_files,
            context.git_summary.staged_files,
            context.git_summary.unstaged_files,
            context.git_summary.untracked_files,
            context.session_path.as_ref().map_or_else(
                || "live-repl".to_string(),
                |path| path.display().to_string()
            ),
            context.loaded_config_files,
            context.discovered_config_files,
            context.memory_file_count,
        ),
        format_sandbox_report(&context.sandbox_status),
    ]
    .join(
        "

",
    )
}

pub(crate) fn format_sandbox_report(status: &runtime::SandboxStatus) -> String {
    format!(
        "Sandbox
  Enabled           {}
  Active            {}
  Supported         {}
  In container      {}
  Requested ns      {}
  Active ns         {}
  Requested net     {}
  Active net        {}
  Filesystem mode   {}
  Filesystem active {}
  Allowed mounts    {}
  Markers           {}
  Fallback reason   {}",
        status.enabled,
        status.active,
        status.supported,
        status.in_container,
        status.requested.namespace_restrictions,
        status.namespace_active,
        status.requested.network_isolation,
        status.network_active,
        status.filesystem_mode.as_str(),
        status.filesystem_active,
        if status.allowed_mounts.is_empty() {
            "<none>".to_string()
        } else {
            status.allowed_mounts.join(", ")
        },
        if status.container_markers.is_empty() {
            "<none>".to_string()
        } else {
            status.container_markers.join(", ")
        },
        status
            .fallback_reason
            .clone()
            .unwrap_or_else(|| "<none>".to_string()),
    )
}

pub(crate) fn format_commit_preflight_report(
    branch: Option<&str>,
    summary: GitWorkspaceSummary,
) -> String {
    format!(
        "Commit
  Result           ready
  Branch           {}
  Workspace        {}
  Changed files    {}
  Action           create a git commit from the current workspace changes",
        branch.unwrap_or("unknown"),
        summary.headline(),
        summary.changed_files,
    )
}

pub(crate) fn format_commit_skipped_report() -> String {
    "Commit
  Result           skipped
  Reason           no workspace changes
  Action           create a git commit from the current workspace changes
  Next             /status to inspect context · /diff to inspect repo changes"
        .to_string()
}

pub(crate) fn print_sandbox_status_snapshot() -> Result<(), Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    let loader = ConfigLoader::default_for(&cwd);
    let runtime_config = loader
        .load()
        .unwrap_or_else(|_| runtime::RuntimeConfig::empty());
    println!(
        "{}",
        format_sandbox_report(&resolve_sandbox_status(runtime_config.sandbox(), &cwd))
    );
    Ok(())
}

pub(crate) fn render_config_report(
    section: Option<&str>,
) -> Result<String, Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    let loader = ConfigLoader::default_for(&cwd);
    let discovered = loader.discover();
    let runtime_config = loader.load()?;

    let mut lines = vec![
        format!(
            "Config
  Working directory {}
  Loaded files      {}
  Merged keys       {}",
            cwd.display(),
            runtime_config.loaded_entries().len(),
            runtime_config.merged().len()
        ),
        "Discovered files".to_string(),
    ];
    for entry in discovered {
        let source = match entry.source {
            ConfigSource::User => "user",
            ConfigSource::Project => "project",
            ConfigSource::Local => "local",
        };
        let status = if runtime_config
            .loaded_entries()
            .iter()
            .any(|loaded_entry| loaded_entry.path == entry.path)
        {
            "loaded"
        } else {
            "missing"
        };
        lines.push(format!(
            "  {source:<7} {status:<7} {}",
            entry.path.display()
        ));
    }

    if let Some(section) = section {
        lines.push(format!("Merged section: {section}"));
        let value = match section {
            "env" => runtime_config.get("env"),
            "hooks" => runtime_config.get("hooks"),
            "model" => runtime_config.get("model"),
            "plugins" => runtime_config
                .get("plugins")
                .or_else(|| runtime_config.get("enabledPlugins")),
            other => {
                lines.push(format!(
                    "  Unsupported config section '{other}'. Use env, hooks, model, or plugins."
                ));
                return Ok(lines.join("\n"));
            }
        };
        lines.push(format!(
            "  {}",
            match value {
                Some(value) => value.render(),
                None => "<unset>".to_string(),
            }
        ));
        return Ok(lines.join("\n"));
    }

    lines.push("Merged JSON".to_string());
    lines.push(format!("  {}", runtime_config.as_json().render()));
    Ok(lines.join("\n"))
}

pub(crate) fn render_memory_report() -> Result<String, Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    let project_context = ProjectContext::discover(&cwd, DEFAULT_DATE)?;
    let mut lines = vec![format!(
        "Memory
  Working directory {}
  Instruction files {}",
        cwd.display(),
        project_context.instruction_files.len()
    )];
    if project_context.instruction_files.is_empty() {
        lines.push("Discovered files".to_string());
        lines.push(
            "  No CLAUDE instruction files discovered in the current directory ancestry."
                .to_string(),
        );
    } else {
        lines.push("Discovered files".to_string());
        for (index, file) in project_context.instruction_files.iter().enumerate() {
            let preview = file.content.lines().next().unwrap_or("").trim();
            let preview = if preview.is_empty() {
                "<empty>"
            } else {
                preview
            };
            lines.push(format!("  {}. {}", index + 1, file.path.display(),));
            lines.push(format!(
                "     lines={} preview={}",
                file.content.lines().count(),
                preview
            ));
        }
    }
    Ok(lines.join("\n"))
}

pub(crate) fn init_claude_md() -> Result<String, Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    Ok(initialize_repo(&cwd)?.render())
}

pub(crate) fn run_init() -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", init_claude_md()?);
    Ok(())
}

pub(crate) fn normalize_permission_mode(mode: &str) -> Option<&'static str> {
    match mode.trim() {
        "read-only" => Some("read-only"),
        "workspace-write" => Some("workspace-write"),
        "danger-full-access" => Some("danger-full-access"),
        _ => None,
    }
}

pub(crate) fn render_diff_report() -> Result<String, Box<dyn std::error::Error>> {
    render_diff_report_for(&env::current_dir()?)
}

pub(crate) fn render_diff_report_for(cwd: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let staged = run_git_diff_command_in(cwd, &["diff", "--cached"])?;
    let unstaged = run_git_diff_command_in(cwd, &["diff"])?;
    if staged.trim().is_empty() && unstaged.trim().is_empty() {
        return Ok(
            "Diff\n  Result           clean working tree\n  Detail           no current changes"
                .to_string(),
        );
    }

    let mut sections = Vec::new();
    if !staged.trim().is_empty() {
        sections.push(format!("Staged changes:\n{}", staged.trim_end()));
    }
    if !unstaged.trim().is_empty() {
        sections.push(format!("Unstaged changes:\n{}", unstaged.trim_end()));
    }

    Ok(format!("Diff\n\n{}", sections.join("\n\n")))
}

pub(crate) fn run_git_diff_command_in(
    cwd: &Path,
    args: &[&str],
) -> Result<String, Box<dyn std::error::Error>> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("git {} failed: {stderr}", args.join(" ")).into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

pub(crate) fn render_teleport_report(target: &str) -> Result<String, Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;

    let file_list = Command::new("rg")
        .args(["--files"])
        .current_dir(&cwd)
        .output()?;
    let file_matches = if file_list.status.success() {
        String::from_utf8(file_list.stdout)?
            .lines()
            .filter(|line| line.contains(target))
            .take(10)
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    let content_output = Command::new("rg")
        .args(["-n", "-S", "--color", "never", target, "."])
        .current_dir(&cwd)
        .output()?;

    let mut lines = vec![
        "Teleport".to_string(),
        format!("  Target           {target}"),
        "  Action           search workspace files and content for the target".to_string(),
    ];
    if !file_matches.is_empty() {
        lines.push(String::new());
        lines.push("File matches".to_string());
        lines.extend(file_matches.into_iter().map(|path| format!("  {path}")));
    }

    if content_output.status.success() {
        let matches = String::from_utf8(content_output.stdout)?;
        if !matches.trim().is_empty() {
            lines.push(String::new());
            lines.push("Content matches".to_string());
            lines.push(truncate_for_prompt(&matches, 4_000));
        }
    }

    if lines.len() == 1 {
        lines.push("  Result           no matches found".to_string());
    }

    Ok(lines.join("\n"))
}

pub(crate) fn render_last_tool_debug_report(
    session: &Session,
) -> Result<String, Box<dyn std::error::Error>> {
    let last_tool_use = session
        .messages
        .iter()
        .rev()
        .find_map(|message| {
            message.blocks.iter().rev().find_map(|block| match block {
                ContentBlock::ToolUse { id, name, input } => {
                    Some((id.clone(), name.clone(), input.clone()))
                }
                _ => None,
            })
        })
        .ok_or_else(|| "no prior tool call found in session".to_string())?;

    let tool_result = session.messages.iter().rev().find_map(|message| {
        message.blocks.iter().rev().find_map(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id,
                tool_name,
                output,
                is_error,
            } if tool_use_id == &last_tool_use.0 => {
                Some((tool_name.clone(), output.clone(), *is_error))
            }
            _ => None,
        })
    });

    let mut lines = vec![
        "Debug tool call".to_string(),
        "  Action           inspect the last recorded tool call and its result".to_string(),
        format!("  Tool id          {}", last_tool_use.0),
        format!("  Tool name        {}", last_tool_use.1),
        "  Input".to_string(),
        indent_block(&last_tool_use.2, 4),
    ];

    match tool_result {
        Some((tool_name, output, is_error)) => {
            lines.push("  Result".to_string());
            lines.push(format!("    name           {tool_name}"));
            lines.push(format!(
                "    status         {}",
                if is_error { "error" } else { "ok" }
            ));
            lines.push(indent_block(&output, 4));
        }
        None => lines.push("  Result           missing tool result".to_string()),
    }

    Ok(lines.join("\n"))
}

pub(crate) fn validate_no_args(
    command_name: &str,
    args: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(args) = args.map(str::trim).filter(|value| !value.is_empty()) {
        return Err(format!(
            "{command_name} does not accept arguments. Received: {args}\nUsage: {command_name}"
        )
        .into());
    }
    Ok(())
}

pub(crate) fn format_bughunter_report(scope: Option<&str>) -> String {
    format!(
        "Bughunter
  Scope            {}
  Action           inspect the selected code for likely bugs and correctness issues
  Output           findings should include file paths, severity, and suggested fixes",
        scope.unwrap_or("the current repository")
    )
}

pub(crate) fn format_ultraplan_report(task: Option<&str>) -> String {
    format!(
        "Ultraplan
  Task             {}
  Action           break work into a multi-step execution plan
  Output           plan should cover goals, risks, sequencing, verification, and rollback",
        task.unwrap_or("the current repo work")
    )
}

pub(crate) fn format_pr_report(branch: &str, context: Option<&str>) -> String {
    format!(
        "PR
  Branch           {branch}
  Context          {}
  Action           draft or create a pull request for the current branch
  Output           title and markdown body suitable for GitHub",
        context.unwrap_or("none")
    )
}

pub(crate) fn format_issue_report(context: Option<&str>) -> String {
    format!(
        "Issue
  Context          {}
  Action           draft or create a GitHub issue from the current context
  Output           title and markdown body suitable for GitHub",
        context.unwrap_or("none")
    )
}

pub(crate) fn render_version_report() -> String {
    let git_sha = GIT_SHA.unwrap_or("unknown");
    let target = BUILD_TARGET.unwrap_or("unknown");
    format!(
        "ColotCook\n  Version          {VERSION}\n  Git SHA          {git_sha}\n  Target           {target}\n  Build date       {DEFAULT_DATE}"
    )
}

pub(crate) fn render_export_text(session: &Session) -> String {
    let mut lines = vec!["# Conversation Export".to_string(), String::new()];
    for (index, message) in session.messages.iter().enumerate() {
        let role = match message.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        };
        lines.push(format!("## {}. {role}", index + 1));
        for block in &message.blocks {
            match block {
                ContentBlock::Text { text } => lines.push(text.clone()),
                ContentBlock::ToolUse { id, name, input } => {
                    lines.push(format!("[tool_use id={id} name={name}] {input}"));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    tool_name,
                    output,
                    is_error,
                } => {
                    lines.push(format!(
                        "[tool_result id={tool_use_id} name={tool_name} error={is_error}] {output}"
                    ));
                }
            }
        }
        lines.push(String::new());
    }
    lines.join("\n")
}

pub(crate) fn default_export_filename(session: &Session) -> String {
    let stem = session
        .messages
        .iter()
        .find_map(|message| match message.role {
            MessageRole::User => message.blocks.iter().find_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            }),
            _ => None,
        })
        .map_or("conversation", |text| {
            text.lines().next().unwrap_or("conversation")
        })
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .take(8)
        .collect::<Vec<_>>()
        .join("-");
    let fallback = if stem.is_empty() {
        "conversation"
    } else {
        &stem
    };
    format!("{fallback}.txt")
}

pub(crate) fn resolve_export_path(
    requested_path: Option<&str>,
    session: &Session,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    let file_name =
        requested_path.map_or_else(|| default_export_filename(session), ToOwned::to_owned);
    let final_name = if Path::new(&file_name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("txt"))
    {
        file_name
    } else {
        format!("{file_name}.txt")
    };
    Ok(cwd.join(final_name))
}

pub(crate) fn render_repl_help() -> String {
    [
        "REPL".to_string(),
        "  /exit                Quit the REPL".to_string(),
        "  /quit                Quit the REPL".to_string(),
        "  Up/Down              Navigate prompt history".to_string(),
        "  Tab                  Complete commands, modes, and recent sessions".to_string(),
        "  Ctrl-C               Clear input (or exit on empty prompt)".to_string(),
        "  Shift+Enter/Ctrl+J   Insert a newline".to_string(),
        "  Auto-save            .colotcook/sessions/<session-id>.jsonl".to_string(),
        "  Resume latest        /resume latest".to_string(),
        "  Browse sessions      /session list".to_string(),
        String::new(),
        render_slash_command_help(),
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use colotcook_runtime::{SandboxStatus, TokenUsage};

    // ── format_model_report ──────────────────────────────────────────────────

    #[test]
    fn format_model_report_contains_model_name() {
        let result = format_model_report("claude-3-opus", 10, 5);
        assert!(result.contains("claude-3-opus"));
        assert!(result.contains("10"));
        assert!(result.contains("5"));
    }

    #[test]
    fn format_model_report_contains_usage_section() {
        let result = format_model_report("model-x", 0, 0);
        assert!(result.contains("Usage"));
        assert!(result.contains("/model"));
    }

    // ── format_model_switch_report ───────────────────────────────────────────

    #[test]
    fn format_model_switch_report_contains_previous_and_next() {
        let result = format_model_switch_report("old-model", "new-model", 5);
        assert!(result.contains("old-model"));
        assert!(result.contains("new-model"));
        assert!(result.contains("5"));
    }

    #[test]
    fn format_model_switch_report_has_update_header() {
        let result = format_model_switch_report("a", "b", 0);
        assert!(result.starts_with("Model updated"));
    }

    // ── format_permissions_report ────────────────────────────────────────────

    #[test]
    fn format_permissions_report_marks_current_mode() {
        let result = format_permissions_report("read-only");
        assert!(result.contains("● current"));
        assert!(result.contains("read-only"));
    }

    #[test]
    fn format_permissions_report_workspace_write_current() {
        let result = format_permissions_report("workspace-write");
        assert!(result.contains("workspace-write"));
        assert!(result.contains("● current"));
    }

    #[test]
    fn format_permissions_report_all_modes_listed() {
        let result = format_permissions_report("read-only");
        assert!(result.contains("read-only"));
        assert!(result.contains("workspace-write"));
        assert!(result.contains("danger-full-access"));
    }

    // ── format_permissions_switch_report ────────────────────────────────────

    #[test]
    fn format_permissions_switch_report_shows_prev_and_next() {
        let result = format_permissions_switch_report("read-only", "workspace-write");
        assert!(result.contains("read-only"));
        assert!(result.contains("workspace-write"));
    }

    #[test]
    fn format_permissions_switch_report_has_updated_header() {
        let result = format_permissions_switch_report("a", "b");
        assert!(result.starts_with("Permissions updated"));
    }

    // ── format_cost_report ───────────────────────────────────────────────────

    #[test]
    fn format_cost_report_shows_token_counts() {
        let usage = TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: 10,
            cache_read_input_tokens: 5,
        };
        let result = format_cost_report(usage);
        assert!(result.contains("100"));
        assert!(result.contains("50"));
        assert!(result.contains("10"));
        assert!(result.contains("5"));
    }

    #[test]
    fn format_cost_report_shows_total() {
        let usage = TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };
        let result = format_cost_report(usage);
        // total = 150
        assert!(result.contains("150"));
    }

    #[test]
    fn format_cost_report_zero_usage() {
        let usage = TokenUsage::default();
        let result = format_cost_report(usage);
        assert!(result.contains("Cost"));
    }

    // ── format_compact_report ────────────────────────────────────────────────

    #[test]
    fn format_compact_report_compacted_variant() {
        let result = format_compact_report(5, 10, false);
        assert!(result.contains("compacted"));
        assert!(result.contains("5"));
        assert!(result.contains("10"));
    }

    #[test]
    fn format_compact_report_skipped_variant() {
        let result = format_compact_report(0, 8, true);
        assert!(result.contains("skipped"));
        assert!(result.contains("8"));
    }

    // ── format_sandbox_report ────────────────────────────────────────────────

    #[test]
    fn format_sandbox_report_default_status_renders() {
        let status = SandboxStatus::default();
        let result = format_sandbox_report(&status);
        assert!(result.starts_with("Sandbox"));
        assert!(result.contains("<none>"));
    }

    #[test]
    fn format_sandbox_report_with_fallback_reason() {
        let mut status = SandboxStatus::default();
        status.fallback_reason = Some("unsupported platform".to_string());
        let result = format_sandbox_report(&status);
        assert!(result.contains("unsupported platform"));
    }

    #[test]
    fn format_sandbox_report_with_allowed_mounts() {
        let mut status = SandboxStatus::default();
        status.allowed_mounts = vec!["/tmp".to_string(), "/home".to_string()];
        let result = format_sandbox_report(&status);
        assert!(result.contains("/tmp"));
        assert!(result.contains("/home"));
    }

    // ── render_version_report ────────────────────────────────────────────────

    #[test]
    fn render_version_report_contains_colotcook_header() {
        let result = render_version_report();
        assert!(result.starts_with("ColotCook"));
    }

    #[test]
    fn render_version_report_contains_version_field() {
        let result = render_version_report();
        assert!(result.contains("Version"));
    }

    #[test]
    fn render_version_report_contains_build_date() {
        let result = render_version_report();
        assert!(result.contains("Build date"));
    }

    // ── parse_git_status_branch ──────────────────────────────────────────────

    #[test]
    fn parse_git_status_branch_normal_branch() {
        let status = "## main...origin/main\nM  src/main.rs";
        assert_eq!(
            parse_git_status_branch(Some(status)),
            Some("main".to_string())
        );
    }

    #[test]
    fn parse_git_status_branch_detached_head() {
        let status = "## HEAD (no branch)";
        let result = parse_git_status_branch(Some(status));
        assert_eq!(result, Some("detached HEAD".to_string()));
    }

    #[test]
    fn parse_git_status_branch_none_input() {
        assert!(parse_git_status_branch(None).is_none());
    }

    #[test]
    fn parse_git_status_branch_feature_branch() {
        let status = "## feature/my-feature...origin/feature/my-feature";
        let result = parse_git_status_branch(Some(status));
        assert_eq!(result, Some("feature/my-feature".to_string()));
    }

    // ── parse_git_workspace_summary ──────────────────────────────────────────

    #[test]
    fn parse_git_workspace_summary_none_returns_default() {
        let summary = parse_git_workspace_summary(None);
        assert_eq!(summary, GitWorkspaceSummary::default());
    }

    #[test]
    fn parse_git_workspace_summary_untracked_file() {
        let status = "## main\n?? new-file.txt";
        let summary = parse_git_workspace_summary(Some(status));
        assert_eq!(summary.untracked_files, 1);
        assert_eq!(summary.changed_files, 1);
    }

    #[test]
    fn parse_git_workspace_summary_staged_and_unstaged() {
        let status = "## main\nMM src/main.rs\nA  new.rs";
        let summary = parse_git_workspace_summary(Some(status));
        assert_eq!(summary.changed_files, 2);
        assert!(summary.staged_files > 0);
        assert!(summary.unstaged_files > 0);
    }

    #[test]
    fn parse_git_workspace_summary_clean_status() {
        let status = "## main...origin/main";
        let summary = parse_git_workspace_summary(Some(status));
        assert!(summary.is_clean());
    }

    #[test]
    fn git_workspace_summary_headline_clean() {
        let summary = GitWorkspaceSummary::default();
        assert_eq!(summary.headline(), "clean");
    }

    #[test]
    fn git_workspace_summary_headline_dirty() {
        let summary = GitWorkspaceSummary {
            changed_files: 3,
            staged_files: 1,
            unstaged_files: 2,
            untracked_files: 0,
            conflicted_files: 0,
        };
        let headline = summary.headline();
        assert!(headline.contains("dirty"));
        assert!(headline.contains("3 files"));
        assert!(headline.contains("1 staged"));
        assert!(headline.contains("2 unstaged"));
    }

    // ── render_export_text ───────────────────────────────────────────────────

    #[test]
    fn render_export_text_empty_session_has_header() {
        use colotcook_runtime::Session;
        let session = Session::default();
        let result = render_export_text(&session);
        assert!(result.starts_with("# Conversation Export"));
    }

    // ── default_export_filename ──────────────────────────────────────────────

    #[test]
    fn default_export_filename_empty_session_uses_fallback() {
        use colotcook_runtime::Session;
        let session = Session::default();
        let result = default_export_filename(&session);
        assert!(result.ends_with(".txt"));
    }

    #[test]
    fn default_export_filename_with_user_message() {
        use colotcook_runtime::{ContentBlock, ConversationMessage, MessageRole, Session};
        let mut session = Session::default();
        session.messages.push(ConversationMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text {
                text: "Hello World test".to_string(),
            }],
            usage: None,
        });
        let result = default_export_filename(&session);
        assert!(result.ends_with(".txt"));
        assert!(result.contains("hello"));
    }

    // ── normalize_permission_mode ────────────────────────────────────────────

    #[test]
    fn normalize_permission_mode_read_only() {
        assert_eq!(normalize_permission_mode("read-only"), Some("read-only"));
    }

    #[test]
    fn normalize_permission_mode_workspace_write() {
        assert_eq!(
            normalize_permission_mode("workspace-write"),
            Some("workspace-write")
        );
    }

    #[test]
    fn normalize_permission_mode_danger_full_access() {
        assert_eq!(
            normalize_permission_mode("danger-full-access"),
            Some("danger-full-access")
        );
    }

    #[test]
    fn normalize_permission_mode_unknown_returns_none() {
        assert!(normalize_permission_mode("unknown-mode").is_none());
    }

    #[test]
    fn normalize_permission_mode_trims_whitespace() {
        assert_eq!(
            normalize_permission_mode("  read-only  "),
            Some("read-only")
        );
    }

    // ── format_status_report ─────────────────────────────────────────────────

    fn test_status_context() -> StatusContext {
        StatusContext {
            cwd: PathBuf::from("/home/user/project"),
            session_path: Some(PathBuf::from("/tmp/session.jsonl")),
            loaded_config_files: 2,
            discovered_config_files: 3,
            memory_file_count: 1,
            project_root: Some(PathBuf::from("/home/user/project")),
            git_branch: Some("main".to_string()),
            git_summary: GitWorkspaceSummary {
                changed_files: 5,
                staged_files: 2,
                unstaged_files: 3,
                untracked_files: 0,
                conflicted_files: 0,
            },
            sandbox_status: SandboxStatus::default(),
        }
    }

    fn test_usage() -> StatusUsage {
        StatusUsage {
            message_count: 10,
            turns: 3,
            latest: TokenUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            },
            cumulative: TokenUsage {
                input_tokens: 500,
                output_tokens: 200,
                cache_creation_input_tokens: 10,
                cache_read_input_tokens: 5,
            },
            estimated_tokens: 700,
        }
    }

    #[test]
    fn format_status_report_contains_model() {
        let result =
            format_status_report("opus", test_usage(), "read-only", &test_status_context());
        assert!(result.contains("opus"));
    }

    #[test]
    fn format_status_report_contains_permission_mode() {
        let result = format_status_report(
            "opus",
            test_usage(),
            "workspace-write",
            &test_status_context(),
        );
        assert!(result.contains("workspace-write"));
    }

    #[test]
    fn format_status_report_contains_workspace_info() {
        let result = format_status_report("m", test_usage(), "read-only", &test_status_context());
        assert!(result.contains("/home/user/project"));
        assert!(result.contains("main"));
    }

    #[test]
    fn format_status_report_contains_usage_info() {
        let result = format_status_report("m", test_usage(), "read-only", &test_status_context());
        assert!(result.contains("10")); // message_count
        assert!(result.contains("700")); // estimated_tokens
    }

    #[test]
    fn format_status_report_no_project_root() {
        let mut ctx = test_status_context();
        ctx.project_root = None;
        let result = format_status_report("m", test_usage(), "read-only", &ctx);
        assert!(result.contains("unknown"));
    }

    #[test]
    fn format_status_report_no_git_branch() {
        let mut ctx = test_status_context();
        ctx.git_branch = None;
        let result = format_status_report("m", test_usage(), "read-only", &ctx);
        assert!(result.contains("unknown"));
    }

    #[test]
    fn format_status_report_no_session_path() {
        let mut ctx = test_status_context();
        ctx.session_path = None;
        let result = format_status_report("m", test_usage(), "read-only", &ctx);
        assert!(result.contains("live-repl"));
    }

    // ── format_commit_preflight_report ────────────────────────────────────

    #[test]
    fn format_commit_preflight_report_with_branch() {
        let summary = GitWorkspaceSummary {
            changed_files: 3,
            staged_files: 2,
            unstaged_files: 1,
            untracked_files: 0,
            conflicted_files: 0,
        };
        let result = format_commit_preflight_report(Some("main"), summary);
        assert!(result.contains("main"));
        assert!(result.contains("ready"));
        assert!(result.contains("3"));
    }

    #[test]
    fn format_commit_preflight_report_no_branch() {
        let summary = GitWorkspaceSummary::default();
        let result = format_commit_preflight_report(None, summary);
        assert!(result.contains("unknown"));
    }

    // ── format_commit_skipped_report ──────────────────────────────────────

    #[test]
    fn format_commit_skipped_report_contains_skipped() {
        let result = format_commit_skipped_report();
        assert!(result.contains("skipped"));
        assert!(result.contains("no workspace changes"));
    }

    // ── format_bughunter_report ──────────────────────────────────────────

    #[test]
    fn format_bughunter_report_with_scope() {
        let result = format_bughunter_report(Some("src/main.rs"));
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("Bughunter"));
    }

    #[test]
    fn format_bughunter_report_no_scope() {
        let result = format_bughunter_report(None);
        assert!(result.contains("the current repository"));
    }

    // ── format_ultraplan_report ──────────────────────────────────────────

    #[test]
    fn format_ultraplan_report_with_task() {
        let result = format_ultraplan_report(Some("implement auth"));
        assert!(result.contains("implement auth"));
        assert!(result.contains("Ultraplan"));
    }

    #[test]
    fn format_ultraplan_report_no_task() {
        let result = format_ultraplan_report(None);
        assert!(result.contains("the current repo work"));
    }

    // ── format_pr_report ─────────────────────────────────────────────────

    #[test]
    fn format_pr_report_with_context() {
        let result = format_pr_report("feature/auth", Some("add oauth support"));
        assert!(result.contains("feature/auth"));
        assert!(result.contains("add oauth support"));
    }

    #[test]
    fn format_pr_report_no_context() {
        let result = format_pr_report("main", None);
        assert!(result.contains("main"));
        assert!(result.contains("none"));
    }

    // ── format_issue_report ──────────────────────────────────────────────

    #[test]
    fn format_issue_report_with_context() {
        let result = format_issue_report(Some("login button broken"));
        assert!(result.contains("login button broken"));
    }

    #[test]
    fn format_issue_report_no_context() {
        let result = format_issue_report(None);
        assert!(result.contains("none"));
    }

    // ── format_resume_report ─────────────────────────────────────────────

    #[test]
    fn format_resume_report_contains_session_path() {
        let result = format_resume_report("/tmp/session.jsonl", 5, 2);
        assert!(result.contains("/tmp/session.jsonl"));
        assert!(result.contains("5"));
        assert!(result.contains("2"));
    }

    // ── render_resume_usage ──────────────────────────────────────────────

    #[test]
    fn render_resume_usage_contains_usage_section() {
        let result = render_resume_usage();
        assert!(result.contains("Resume"));
        assert!(result.contains("/resume"));
    }

    // ── format_auto_compaction_notice ─────────────────────────────────────

    #[test]
    fn format_auto_compaction_notice_contains_count() {
        let result = format_auto_compaction_notice(42);
        assert!(result.contains("42"));
        assert!(result.contains("auto-compacted"));
    }

    // ── validate_no_args ──────────────────────────────────────────────────

    #[test]
    fn validate_no_args_none_ok() {
        assert!(validate_no_args("/status", None).is_ok());
    }

    #[test]
    fn validate_no_args_empty_ok() {
        assert!(validate_no_args("/status", Some("")).is_ok());
    }

    #[test]
    fn validate_no_args_whitespace_ok() {
        assert!(validate_no_args("/status", Some("   ")).is_ok());
    }

    #[test]
    fn validate_no_args_with_args_errors() {
        let result = validate_no_args("/status", Some("extra"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("/status"));
    }

    // ── render_last_tool_debug_report ─────────────────────────────────────

    #[test]
    fn render_last_tool_debug_report_no_tool_errors() {
        use colotcook_runtime::Session;
        let session = Session::default();
        let result = render_last_tool_debug_report(&session);
        assert!(result.is_err());
    }

    #[test]
    fn render_last_tool_debug_report_with_tool_use() {
        use colotcook_runtime::{ContentBlock, ConversationMessage, MessageRole, Session};
        let mut session = Session::default();
        session.messages.push(ConversationMessage {
            role: MessageRole::Assistant,
            blocks: vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "bash".into(),
                input: r#"{"command":"ls"}"#.into(),
            }],
            usage: None,
        });
        let result = render_last_tool_debug_report(&session).unwrap();
        assert!(result.contains("bash"));
        assert!(result.contains("t1"));
        assert!(result.contains("missing tool result"));
    }

    #[test]
    fn render_last_tool_debug_report_with_tool_result() {
        use colotcook_runtime::{ContentBlock, ConversationMessage, MessageRole, Session};
        let mut session = Session::default();
        session.messages.push(ConversationMessage {
            role: MessageRole::Assistant,
            blocks: vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "bash".into(),
                input: r#"{"command":"ls"}"#.into(),
            }],
            usage: None,
        });
        session.messages.push(ConversationMessage {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                tool_use_id: "t1".into(),
                tool_name: "bash".into(),
                output: "file1.txt\nfile2.txt".into(),
                is_error: false,
            }],
            usage: None,
        });
        let result = render_last_tool_debug_report(&session).unwrap();
        assert!(result.contains("bash"));
        assert!(result.contains("ok"));
        assert!(result.contains("file1.txt"));
    }

    #[test]
    fn render_last_tool_debug_report_with_error_result() {
        use colotcook_runtime::{ContentBlock, ConversationMessage, MessageRole, Session};
        let mut session = Session::default();
        session.messages.push(ConversationMessage {
            role: MessageRole::Assistant,
            blocks: vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "bash".into(),
                input: "{}".into(),
            }],
            usage: None,
        });
        session.messages.push(ConversationMessage {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                tool_use_id: "t1".into(),
                tool_name: "bash".into(),
                output: "command not found".into(),
                is_error: true,
            }],
            usage: None,
        });
        let result = render_last_tool_debug_report(&session).unwrap();
        assert!(result.contains("error"));
    }

    // ── render_export_text ────────────────────────────────────────────────

    #[test]
    fn render_export_text_with_messages() {
        use colotcook_runtime::{ContentBlock, ConversationMessage, MessageRole, Session};
        let mut session = Session::default();
        session.messages.push(ConversationMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text {
                text: "Hello".into(),
            }],
            usage: None,
        });
        session.messages.push(ConversationMessage {
            role: MessageRole::Assistant,
            blocks: vec![ContentBlock::Text {
                text: "Hi there!".into(),
            }],
            usage: None,
        });
        let result = render_export_text(&session);
        assert!(result.contains("user"));
        assert!(result.contains("assistant"));
        assert!(result.contains("Hello"));
        assert!(result.contains("Hi there!"));
    }

    #[test]
    fn render_export_text_with_tool_blocks() {
        use colotcook_runtime::{ContentBlock, ConversationMessage, MessageRole, Session};
        let mut session = Session::default();
        session.messages.push(ConversationMessage {
            role: MessageRole::Assistant,
            blocks: vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "bash".into(),
                input: r#"{"command":"ls"}"#.into(),
            }],
            usage: None,
        });
        session.messages.push(ConversationMessage {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                tool_use_id: "t1".into(),
                tool_name: "bash".into(),
                output: "file.txt".into(),
                is_error: false,
            }],
            usage: None,
        });
        let result = render_export_text(&session);
        assert!(result.contains("[tool_use"));
        assert!(result.contains("[tool_result"));
    }

    // ── default_export_filename ───────────────────────────────────────────

    #[test]
    fn default_export_filename_special_chars() {
        use colotcook_runtime::{ContentBlock, ConversationMessage, MessageRole, Session};
        let mut session = Session::default();
        session.messages.push(ConversationMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text {
                text: "Fix the @#$ bug!!!".into(),
            }],
            usage: None,
        });
        let result = default_export_filename(&session);
        assert!(result.ends_with(".txt"));
        // Should not contain special chars
        assert!(!result.contains('@'));
        assert!(!result.contains('#'));
    }

    // ── format_unknown_slash_command_message ──────────────────────────────

    #[test]
    fn format_unknown_slash_command_no_suggestions() {
        let result = format_unknown_slash_command_message("xyznonexistent");
        assert!(result.contains("unknown slash command"));
        assert!(result.contains("/help"));
    }

    #[test]
    fn format_unknown_slash_command_with_suggestions() {
        // "statu" is close to "status"
        let result = format_unknown_slash_command_message("statu");
        assert!(result.contains("unknown slash command"));
    }

    // ── GitWorkspaceSummary ──────────────────────────────────────────────

    #[test]
    fn git_workspace_summary_headline_with_untracked() {
        let summary = GitWorkspaceSummary {
            changed_files: 2,
            staged_files: 0,
            unstaged_files: 0,
            untracked_files: 2,
            conflicted_files: 0,
        };
        let headline = summary.headline();
        assert!(headline.contains("2 untracked"));
    }

    #[test]
    fn git_workspace_summary_headline_with_conflicts() {
        let summary = GitWorkspaceSummary {
            changed_files: 1,
            staged_files: 0,
            unstaged_files: 0,
            untracked_files: 0,
            conflicted_files: 1,
        };
        let headline = summary.headline();
        assert!(headline.contains("1 conflicted"));
    }

    // ── parse_git_workspace_summary additional ───────────────────────────

    #[test]
    fn parse_git_workspace_summary_conflicted_files() {
        let status = "## main\nUU conflicted.rs";
        let summary = parse_git_workspace_summary(Some(status));
        assert_eq!(summary.conflicted_files, 1);
    }

    #[test]
    fn parse_git_workspace_summary_empty_lines_skipped() {
        let status = "## main\n\nM  file.rs\n\n";
        let summary = parse_git_workspace_summary(Some(status));
        assert_eq!(summary.changed_files, 1);
    }

    #[test]
    fn parse_git_workspace_summary_staged_only() {
        let status = "## main\nA  new.rs";
        let summary = parse_git_workspace_summary(Some(status));
        assert_eq!(summary.staged_files, 1);
        assert_eq!(summary.unstaged_files, 0);
    }

    #[test]
    fn parse_git_workspace_summary_unstaged_only() {
        let status = "## main\n M modified.rs";
        let summary = parse_git_workspace_summary(Some(status));
        assert_eq!(summary.staged_files, 0);
        assert_eq!(summary.unstaged_files, 1);
    }

    // ── parse_git_status_branch additional ───────────────────────────────

    #[test]
    fn parse_git_status_branch_empty_branch() {
        let status = "## ";
        assert!(parse_git_status_branch(Some(status)).is_none());
    }

    #[test]
    fn parse_git_status_branch_no_header() {
        let status = "M  src/main.rs";
        assert!(parse_git_status_branch(Some(status)).is_none());
    }

    // ── format_sandbox_report additional ─────────────────────────────────

    #[test]
    fn format_sandbox_report_with_container_markers() {
        let mut status = SandboxStatus::default();
        status.container_markers = vec!["docker".to_string(), "k8s".to_string()];
        let result = format_sandbox_report(&status);
        assert!(result.contains("docker"));
        assert!(result.contains("k8s"));
    }

    #[test]
    fn format_sandbox_report_enabled_and_active() {
        let mut status = SandboxStatus::default();
        status.enabled = true;
        status.active = true;
        let result = format_sandbox_report(&status);
        assert!(result.contains("true"));
    }

    // ── format_permissions_report additional ──────────────────────────────

    #[test]
    fn format_permissions_report_danger_mode() {
        let result = format_permissions_report("danger-full-access");
        assert!(result.contains("● current"));
        assert!(result.contains("danger-full-access"));
    }

    // ── format_cost_report additional ────────────────────────────────────

    #[test]
    fn format_cost_report_large_numbers() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 500_000,
            cache_creation_input_tokens: 100_000,
            cache_read_input_tokens: 50_000,
        };
        let result = format_cost_report(usage);
        assert!(result.contains("1000000"));
        assert!(result.contains("500000"));
    }

    // ── render_repl_help ─────────────────────────────────────────────────

    #[test]
    fn render_repl_help_contains_exit() {
        let result = render_repl_help();
        assert!(result.contains("/exit"));
        assert!(result.contains("/quit"));
    }

    #[test]
    fn render_repl_help_contains_keyboard_shortcuts() {
        let result = render_repl_help();
        assert!(result.contains("Ctrl-C"));
        assert!(result.contains("Tab"));
    }

    // ── render_diff_report_for ───────────────────────────────────────────

    #[test]
    fn render_diff_report_for_temp_dir_clean() {
        let dir = std::env::temp_dir().join("colotcook-diff-test-clean");
        let _ = std::fs::create_dir_all(&dir);
        // Init a git repo
        let _ = std::process::Command::new("git")
            .args(["init"])
            .current_dir(&dir)
            .output();
        let _ = std::process::Command::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(&dir)
            .output();
        let result = render_diff_report_for(&dir);
        if let Ok(report) = result {
            assert!(report.contains("clean") || report.contains("Diff"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Print a /status snapshot (used in non-REPL mode).
pub(crate) fn print_status_snapshot(
    model: &str,
    permission_mode: colotcook_runtime::PermissionMode,
) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "{}",
        format_status_report(
            model,
            StatusUsage {
                message_count: 0,
                turns: 0,
                latest: TokenUsage::default(),
                cumulative: TokenUsage::default(),
                estimated_tokens: 0,
            },
            permission_mode.as_str(),
            &status_context(None)?,
        )
    );
    Ok(())
}
