/// Main slash command dispatcher and individual handlers.

use std::fmt::Write as _;

use colotcook_runtime as runtime;
use colotcook_runtime::{compact_session, CompactionConfig, Session};

use crate::types::*;
use crate::plugins_command::SlashCommandResult;
use crate::help::render_slash_command_help;


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

pub(crate) fn handle_commit() -> String {
    "Use: /commit is currently a placeholder.\n\nTo commit changes:\n1. Stage your changes with: git add <files>\n2. Run: git commit -m \"Your message\"\n\nFull git integration with automatic message generation coming soon.".to_string()
}

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
