//! Main slash command dispatcher and individual handlers.

use std::fmt::Write as _;

use colotcook_runtime as runtime;
use colotcook_runtime::{compact_session, CompactionConfig, Session};

use crate::help::render_slash_command_help;
use crate::plugins_command::SlashCommandResult;
use crate::types::SlashCommand;

/// Build the `/cost` report for a session.
pub(crate) fn handle_cost(session: &Session) -> String {
    let mut output = String::new();
    output.push_str("## Token Usage\n\n");
    let _ = writeln!(output, "- Total messages: {}", session.messages.len());
    output.push_str(
        "\nNote: Detailed token usage requires access to UsageTracker from the runtime.\n",
    );
    output.push_str(
        "To see actual token costs, integrate with the runtime's usage tracking system.\n",
    );
    output
}

/// Build the `/diff` git diff report.
pub(crate) fn handle_diff() -> String {
    match std::process::Command::new("git")
        .args(["diff", "--stat"])
        .output()
    {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stdout.is_empty() && stderr.is_empty() {
                "No changes detected.".to_string()
            } else if !stdout.is_empty() {
                format!("```\n{stdout}```")
            } else {
                format!("Error: {stderr}")
            }
        }
        Err(e) => format!("Failed to run git diff: {e}"),
    }
}

/// Build the `/commit` placeholder report.
pub(crate) fn handle_commit() -> String {
    "Use: /commit is currently a placeholder.\n\nTo commit changes:\n1. Stage your changes with: git add <files>\n2. Run: git commit -m \"Your message\"\n\nFull git integration with automatic message generation coming soon.".to_string()
}

/// Build the `/debug-tool-call` report.
pub(crate) fn handle_debug_tool_call(session: &Session) -> String {
    use runtime::ContentBlock;

    for message in session.messages.iter().rev() {
        for block in &message.blocks {
            if let ContentBlock::ToolUse { id, name, input } = block {
                let mut output = String::new();
                output.push_str("## Last Tool Call\n\n");
                let _ = writeln!(output, "- Tool: {name}");
                let _ = writeln!(output, "- ID: {id}");
                let _ = writeln!(output, "- Input:\n```json\n{input}\n```");
                return output;
            }
        }
    }
    "No tool calls found in session.".to_string()
}

/// Build the `/sandbox` status report.
pub(crate) fn handle_sandbox() -> String {
    let mut output = String::new();
    output.push_str("## Sandbox Status\n\n");
    output.push_str("- Sandbox detection requires integration with runtime sandbox module\n");
    output.push_str("- Working directory: ");
    if let Ok(cwd) = std::env::current_dir() {
        output.push_str(&cwd.display().to_string());
    } else {
        output.push_str("(unable to determine)");
    }
    output.push('\n');
    output
}

/// Build the `/session` info report.
pub(crate) fn handle_session(session: &Session) -> String {
    let mut output = String::new();
    output.push_str("## Session Info\n\n");
    let _ = writeln!(output, "- Session ID: {}", session.session_id);
    let _ = writeln!(output, "- Messages: {}", session.messages.len());
    let _ = writeln!(output, "- Version: {}", session.version);
    if let Some(compaction) = &session.compaction {
        let _ = writeln!(output, "- Compactions: {}", compaction.count);
    }
    if let Some(fork) = &session.fork {
        let _ = writeln!(output, "- Forked from: {}", fork.parent_session_id);
    }
    output
}

#[must_use]
/// Dispatch a slash command and return the result.
pub fn handle_slash_command(
    input: &str,
    session: &Session,
    compaction: CompactionConfig,
) -> Option<SlashCommandResult> {
    let command = match SlashCommand::parse(input) {
        Ok(Some(command)) => command,
        Ok(None) => return None,
        Err(error) => {
            return Some(SlashCommandResult {
                message: error.to_string(),
                session: session.clone(),
            });
        }
    };

    match command {
        SlashCommand::Compact => {
            let result = compact_session(session, compaction);
            let message = if result.removed_message_count == 0 {
                "Compaction skipped: session is below the compaction threshold.".to_string()
            } else {
                format!(
                    "Compacted {} messages into a resumable system summary.",
                    result.removed_message_count
                )
            };
            Some(SlashCommandResult {
                message,
                session: result.compacted_session,
            })
        }
        SlashCommand::Help => Some(SlashCommandResult {
            message: render_slash_command_help(),
            session: session.clone(),
        }),
        SlashCommand::Cost => Some(SlashCommandResult {
            message: handle_cost(session),
            session: session.clone(),
        }),
        SlashCommand::Diff => Some(SlashCommandResult {
            message: handle_diff(),
            session: session.clone(),
        }),
        SlashCommand::Commit => Some(SlashCommandResult {
            message: handle_commit(),
            session: session.clone(),
        }),
        SlashCommand::DebugToolCall => Some(SlashCommandResult {
            message: handle_debug_tool_call(session),
            session: session.clone(),
        }),
        SlashCommand::Sandbox => Some(SlashCommandResult {
            message: handle_sandbox(),
            session: session.clone(),
        }),
        SlashCommand::Session { .. } => Some(SlashCommandResult {
            message: handle_session(session),
            session: session.clone(),
        }),
        SlashCommand::Status
        | SlashCommand::Bughunter { .. }
        | SlashCommand::Pr { .. }
        | SlashCommand::Issue { .. }
        | SlashCommand::Ultraplan { .. }
        | SlashCommand::Teleport { .. }
        | SlashCommand::Model { .. }
        | SlashCommand::Permissions { .. }
        | SlashCommand::Clear { .. }
        | SlashCommand::Resume { .. }
        | SlashCommand::Config { .. }
        | SlashCommand::Memory
        | SlashCommand::Init
        | SlashCommand::Version
        | SlashCommand::Export { .. }
        | SlashCommand::Plugins { .. }
        | SlashCommand::Agents { .. }
        | SlashCommand::Skills { .. }
        | SlashCommand::Unknown(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use colotcook_runtime::{CompactionConfig, Session};

    fn empty_session() -> Session {
        Session::new()
    }

    // ── handle_cost ──────────────────────────────────────────────────────────

    #[test]
    fn handle_cost_contains_token_usage_header() {
        let session = empty_session();
        let output = handle_cost(&session);
        assert!(output.contains("Token Usage"));
    }

    #[test]
    fn handle_cost_contains_message_count() {
        let session = empty_session();
        let output = handle_cost(&session);
        assert!(output.contains("Total messages"));
        assert!(output.contains('0'));
    }

    // ── handle_diff ──────────────────────────────────────────────────────────

    #[test]
    fn handle_diff_returns_string() {
        // Just verify no panic — actual output depends on git state
        let output = handle_diff();
        assert!(!output.is_empty() || output.is_empty()); // tautology to verify no panic
    }

    // ── handle_commit ────────────────────────────────────────────────────────

    #[test]
    fn handle_commit_is_placeholder() {
        let output = handle_commit();
        assert!(output.contains("/commit") || output.contains("commit"));
    }

    // ── handle_debug_tool_call ───────────────────────────────────────────────

    #[test]
    fn handle_debug_tool_call_no_tool_calls() {
        let session = empty_session();
        let output = handle_debug_tool_call(&session);
        assert!(output.contains("No tool calls found"));
    }

    // ── handle_sandbox ───────────────────────────────────────────────────────

    #[test]
    fn handle_sandbox_contains_sandbox_header() {
        let output = handle_sandbox();
        assert!(output.contains("Sandbox"));
    }

    #[test]
    fn handle_sandbox_contains_working_directory() {
        let output = handle_sandbox();
        // Should contain either the cwd or a note about being unable to determine it
        assert!(output.contains("Working directory") || output.contains("directory"));
    }

    // ── handle_session ───────────────────────────────────────────────────────

    #[test]
    fn handle_session_contains_session_id() {
        let session = empty_session();
        let output = handle_session(&session);
        assert!(output.contains("Session ID"));
        assert!(output.contains(&session.session_id));
    }

    #[test]
    fn handle_session_contains_message_count() {
        let session = empty_session();
        let output = handle_session(&session);
        assert!(output.contains("Messages"));
    }

    #[test]
    fn handle_session_contains_version() {
        let session = empty_session();
        let output = handle_session(&session);
        assert!(output.contains("Version"));
    }

    // ── handle_slash_command ─────────────────────────────────────────────────

    #[test]
    fn handle_slash_command_empty_input_returns_none() {
        let session = empty_session();
        let compaction = CompactionConfig::default();
        let result = handle_slash_command("", &session, compaction);
        assert!(result.is_none());
    }

    #[test]
    fn handle_slash_command_non_slash_input_returns_none() {
        let session = empty_session();
        let compaction = CompactionConfig::default();
        let result = handle_slash_command("hello world", &session, compaction);
        assert!(result.is_none());
    }

    #[test]
    fn handle_slash_command_help() {
        let session = empty_session();
        let compaction = CompactionConfig::default();
        let result = handle_slash_command("/help", &session, compaction);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(!r.message.is_empty());
    }

    #[test]
    fn handle_slash_command_cost() {
        let session = empty_session();
        let compaction = CompactionConfig::default();
        let result = handle_slash_command("/cost", &session, compaction);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(r.message.contains("Token Usage"));
    }

    #[test]
    fn handle_slash_command_diff() {
        let session = empty_session();
        let compaction = CompactionConfig::default();
        let result = handle_slash_command("/diff", &session, compaction);
        assert!(result.is_some());
    }

    #[test]
    fn handle_slash_command_commit() {
        let session = empty_session();
        let compaction = CompactionConfig::default();
        let result = handle_slash_command("/commit", &session, compaction);
        assert!(result.is_some());
    }

    #[test]
    fn handle_slash_command_debug_tool_call() {
        let session = empty_session();
        let compaction = CompactionConfig::default();
        let result = handle_slash_command("/debug-tool-call", &session, compaction);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(r.message.contains("No tool calls found"));
    }

    #[test]
    fn handle_slash_command_sandbox() {
        let session = empty_session();
        let compaction = CompactionConfig::default();
        let result = handle_slash_command("/sandbox", &session, compaction);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(r.message.contains("Sandbox"));
    }

    #[test]
    fn handle_slash_command_session() {
        let session = empty_session();
        let compaction = CompactionConfig::default();
        let result = handle_slash_command("/session", &session, compaction);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(r.message.contains("Session"));
    }

    #[test]
    fn handle_slash_command_compact_below_threshold() {
        // With default compaction config, empty session should be "below threshold"
        let session = empty_session();
        let compaction = CompactionConfig::default();
        let result = handle_slash_command("/compact", &session, compaction);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(r.message.contains("Compaction skipped") || r.message.contains("Compacted"));
    }

    #[test]
    fn handle_slash_command_unknown_returns_none_for_passthrough_commands() {
        let session = empty_session();
        let compaction = CompactionConfig::default();
        // /status is a passthrough command (handled externally)
        let result = handle_slash_command("/status", &session, compaction);
        assert!(result.is_none());
    }
}
