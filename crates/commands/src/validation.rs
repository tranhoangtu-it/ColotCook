//! Slash command input validation and argument parsing.

use crate::agents_and_skills::normalize_optional_args;
use crate::help::{render_slash_command_help_detail, slash_command_specs};
use crate::types::{SlashCommand, SlashCommandParseError, SlashCommandSpec};

impl SlashCommand {
    /// Parse a raw input string into an optional slash command.
    pub fn parse(input: &str) -> Result<Option<Self>, SlashCommandParseError> {
        validate_slash_command_input(input)
    }
}

/// Validate and parse a slash command input string.
pub fn validate_slash_command_input(
    input: &str,
) -> Result<Option<SlashCommand>, SlashCommandParseError> {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return Ok(None);
    }

    let mut parts = trimmed.trim_start_matches('/').split_whitespace();
    let command = parts.next().unwrap_or_default();
    if command.is_empty() {
        return Err(SlashCommandParseError::new(
            "Slash command name is missing. Use /help to list available slash commands.",
        ));
    }

    let args = parts.collect::<Vec<_>>();
    let remainder = remainder_after_command(trimmed, command);

    Ok(Some(match command {
        "help" => {
            validate_no_args(command, &args)?;
            SlashCommand::Help
        }
        "status" => {
            validate_no_args(command, &args)?;
            SlashCommand::Status
        }
        "sandbox" => {
            validate_no_args(command, &args)?;
            SlashCommand::Sandbox
        }
        "compact" => {
            validate_no_args(command, &args)?;
            SlashCommand::Compact
        }
        "bughunter" => SlashCommand::Bughunter { scope: remainder },
        "commit" => {
            validate_no_args(command, &args)?;
            SlashCommand::Commit
        }
        "pr" => SlashCommand::Pr { context: remainder },
        "issue" => SlashCommand::Issue { context: remainder },
        "ultraplan" => SlashCommand::Ultraplan { task: remainder },
        "teleport" => SlashCommand::Teleport {
            target: Some(require_remainder(command, remainder, "<symbol-or-path>")?),
        },
        "debug-tool-call" => {
            validate_no_args(command, &args)?;
            SlashCommand::DebugToolCall
        }
        "model" => SlashCommand::Model {
            model: optional_single_arg(command, &args, "[model]")?,
        },
        "permissions" => SlashCommand::Permissions {
            mode: parse_permissions_mode(&args)?,
        },
        "clear" => SlashCommand::Clear {
            confirm: parse_clear_args(&args)?,
        },
        "cost" => {
            validate_no_args(command, &args)?;
            SlashCommand::Cost
        }
        "resume" => SlashCommand::Resume {
            session_path: Some(require_remainder(command, remainder, "<session-path>")?),
        },
        "config" => SlashCommand::Config {
            section: parse_config_section(&args)?,
        },
        "memory" => {
            validate_no_args(command, &args)?;
            SlashCommand::Memory
        }
        "init" => {
            validate_no_args(command, &args)?;
            SlashCommand::Init
        }
        "diff" => {
            validate_no_args(command, &args)?;
            SlashCommand::Diff
        }
        "version" => {
            validate_no_args(command, &args)?;
            SlashCommand::Version
        }
        "export" => SlashCommand::Export { path: remainder },
        "session" => parse_session_command(&args)?,
        "plugin" | "plugins" | "marketplace" => parse_plugin_command(&args)?,
        "agents" => SlashCommand::Agents {
            args: parse_list_or_help_args(command, remainder)?,
        },
        "skills" => SlashCommand::Skills {
            args: parse_skills_args(remainder.as_deref())?,
        },
        other => SlashCommand::Unknown(other.to_string()),
    }))
}
/// Return an error if any unexpected arguments are present.
pub(crate) fn validate_no_args(command: &str, args: &[&str]) -> Result<(), SlashCommandParseError> {
    if args.is_empty() {
        return Ok(());
    }

    Err(command_error(
        &format!("Unexpected arguments for /{command}."),
        command,
        &format!("/{command}"),
    ))
}

/// Extract an optional single-word argument.
pub(crate) fn optional_single_arg(
    command: &str,
    args: &[&str],
    argument_hint: &str,
) -> Result<Option<String>, SlashCommandParseError> {
    match args {
        [] => Ok(None),
        [value] => Ok(Some((*value).to_string())),
        _ => Err(usage_error(command, argument_hint)),
    }
}

/// Extract the remainder after the command name as a required argument.
pub(crate) fn require_remainder(
    command: &str,
    remainder: Option<String>,
    argument_hint: &str,
) -> Result<String, SlashCommandParseError> {
    remainder.ok_or_else(|| usage_error(command, argument_hint))
}

/// Parse a `/permissions` command argument.
pub(crate) fn parse_permissions_mode(
    args: &[&str],
) -> Result<Option<String>, SlashCommandParseError> {
    let mode = optional_single_arg(
        "permissions",
        args,
        "[read-only|workspace-write|danger-full-access]",
    )?;
    if let Some(mode) = mode {
        if matches!(
            mode.as_str(),
            "read-only" | "workspace-write" | "danger-full-access"
        ) {
            return Ok(Some(mode));
        }
        return Err(command_error(
            &format!(
                "Unsupported /permissions mode '{mode}'. Use read-only, workspace-write, or danger-full-access."
            ),
            "permissions",
            "/permissions [read-only|workspace-write|danger-full-access]",
        ));
    }

    Ok(None)
}

/// Parse `/clear` arguments, returning whether `--confirm` was passed.
pub(crate) fn parse_clear_args(args: &[&str]) -> Result<bool, SlashCommandParseError> {
    match args {
        [] => Ok(false),
        ["--confirm"] => Ok(true),
        [unexpected] => Err(command_error(
            &format!("Unsupported /clear argument '{unexpected}'. Use /clear or /clear --confirm."),
            "clear",
            "/clear [--confirm]",
        )),
        _ => Err(usage_error("clear", "[--confirm]")),
    }
}

/// Parse an optional `/config` section argument.
pub(crate) fn parse_config_section(
    args: &[&str],
) -> Result<Option<String>, SlashCommandParseError> {
    let section = optional_single_arg("config", args, "[env|hooks|model|plugins]")?;
    if let Some(section) = section {
        if matches!(section.as_str(), "env" | "hooks" | "model" | "plugins") {
            return Ok(Some(section));
        }
        return Err(command_error(
            &format!("Unsupported /config section '{section}'. Use env, hooks, model, or plugins."),
            "config",
            "/config [env|hooks|model|plugins]",
        ));
    }

    Ok(None)
}

/// Parse `/session` sub-command and target arguments.
pub(crate) fn parse_session_command(args: &[&str]) -> Result<SlashCommand, SlashCommandParseError> {
    match args {
        [] => Ok(SlashCommand::Session {
            action: None,
            target: None,
        }),
        ["list"] => Ok(SlashCommand::Session {
            action: Some("list".to_string()),
            target: None,
        }),
        ["list", ..] => Err(usage_error("session", "[list|switch <session-id>|fork [branch-name]]")),
        ["switch"] => Err(usage_error("session switch", "<session-id>")),
        ["switch", target] => Ok(SlashCommand::Session {
            action: Some("switch".to_string()),
            target: Some((*target).to_string()),
        }),
        ["switch", ..] => Err(command_error(
            "Unexpected arguments for /session switch.",
            "session",
            "/session switch <session-id>",
        )),
        ["fork"] => Ok(SlashCommand::Session {
            action: Some("fork".to_string()),
            target: None,
        }),
        ["fork", target] => Ok(SlashCommand::Session {
            action: Some("fork".to_string()),
            target: Some((*target).to_string()),
        }),
        ["fork", ..] => Err(command_error(
            "Unexpected arguments for /session fork.",
            "session",
            "/session fork [branch-name]",
        )),
        [action, ..] => Err(command_error(
            &format!(
                "Unknown /session action '{action}'. Use list, switch <session-id>, or fork [branch-name]."
            ),
            "session",
            "/session [list|switch <session-id>|fork [branch-name]]",
        )),
    }
}

/// Parse `/plugins` sub-command and target arguments.
pub(crate) fn parse_plugin_command(args: &[&str]) -> Result<SlashCommand, SlashCommandParseError> {
    match args {
        [] => Ok(SlashCommand::Plugins {
            action: None,
            target: None,
        }),
        ["list"] => Ok(SlashCommand::Plugins {
            action: Some("list".to_string()),
            target: None,
        }),
        ["list", ..] => Err(usage_error("plugin list", "")),
        ["install"] => Err(usage_error("plugin install", "<path>")),
        ["install", target @ ..] => Ok(SlashCommand::Plugins {
            action: Some("install".to_string()),
            target: Some(target.join(" ")),
        }),
        ["enable"] => Err(usage_error("plugin enable", "<name>")),
        ["enable", target] => Ok(SlashCommand::Plugins {
            action: Some("enable".to_string()),
            target: Some((*target).to_string()),
        }),
        ["enable", ..] => Err(command_error(
            "Unexpected arguments for /plugin enable.",
            "plugin",
            "/plugin enable <name>",
        )),
        ["disable"] => Err(usage_error("plugin disable", "<name>")),
        ["disable", target] => Ok(SlashCommand::Plugins {
            action: Some("disable".to_string()),
            target: Some((*target).to_string()),
        }),
        ["disable", ..] => Err(command_error(
            "Unexpected arguments for /plugin disable.",
            "plugin",
            "/plugin disable <name>",
        )),
        ["uninstall"] => Err(usage_error("plugin uninstall", "<id>")),
        ["uninstall", target] => Ok(SlashCommand::Plugins {
            action: Some("uninstall".to_string()),
            target: Some((*target).to_string()),
        }),
        ["uninstall", ..] => Err(command_error(
            "Unexpected arguments for /plugin uninstall.",
            "plugin",
            "/plugin uninstall <id>",
        )),
        ["update"] => Err(usage_error("plugin update", "<id>")),
        ["update", target] => Ok(SlashCommand::Plugins {
            action: Some("update".to_string()),
            target: Some((*target).to_string()),
        }),
        ["update", ..] => Err(command_error(
            "Unexpected arguments for /plugin update.",
            "plugin",
            "/plugin update <id>",
        )),
        [action, ..] => Err(command_error(
            &format!(
                "Unknown /plugin action '{action}'. Use list, install <path>, enable <name>, disable <name>, uninstall <id>, or update <id>."
            ),
            "plugin",
            "/plugin [list|install <path>|enable <name>|disable <name>|uninstall <id>|update <id>]",
        )),
    }
}

/// Parse args for commands that accept `list` or `help`.
pub(crate) fn parse_list_or_help_args(
    command: &str,
    args: Option<String>,
) -> Result<Option<String>, SlashCommandParseError> {
    match normalize_optional_args(args.as_deref()) {
        None | Some("list" | "help" | "-h" | "--help") => Ok(args),
        Some(unexpected) => Err(command_error(
            &format!(
                "Unexpected arguments for /{command}: {unexpected}. Use /{command}, /{command} list, or /{command} help."
            ),
            command,
            &format!("/{command} [list|help]"),
        )),
    }
}

/// Parse args for the `/skills` command.
pub(crate) fn parse_skills_args(
    args: Option<&str>,
) -> Result<Option<String>, SlashCommandParseError> {
    let Some(args) = normalize_optional_args(args) else {
        return Ok(None);
    };

    if matches!(args, "list" | "help" | "-h" | "--help") {
        return Ok(Some(args.to_string()));
    }

    if args == "install" {
        return Err(command_error(
            "Usage: /skills install <path>",
            "skills",
            "/skills install <path>",
        ));
    }

    if let Some(target) = args.strip_prefix("install").map(str::trim) {
        if !target.is_empty() {
            return Ok(Some(format!("install {target}")));
        }
    }

    Err(command_error(
        &format!(
            "Unexpected arguments for /skills: {args}. Use /skills, /skills list, /skills install <path>, or /skills help."
        ),
        "skills",
        "/skills [list|install <path>|help]",
    ))
}

/// Build a usage error for a command.
pub(crate) fn usage_error(command: &str, argument_hint: &str) -> SlashCommandParseError {
    let usage = format!("/{command} {argument_hint}");
    let usage = usage.trim_end().to_string();
    command_error(
        &format!("Usage: {usage}"),
        command_root_name(command),
        &usage,
    )
}

/// Build a generic command error with usage hint.
pub(crate) fn command_error(message: &str, command: &str, usage: &str) -> SlashCommandParseError {
    let detail = render_slash_command_help_detail(command)
        .map(|detail| format!("\n\n{detail}"))
        .unwrap_or_default();
    SlashCommandParseError::new(format!("{message}\n  Usage            {usage}{detail}"))
}

/// Return the trimmed remainder of `input` after the command word.
pub(crate) fn remainder_after_command(input: &str, command: &str) -> Option<String> {
    input
        .trim()
        .strip_prefix(&format!("/{command}"))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

/// Find a slash command spec by name or alias.
pub(crate) fn find_slash_command_spec(name: &str) -> Option<&'static SlashCommandSpec> {
    slash_command_specs().iter().find(|spec| {
        spec.name.eq_ignore_ascii_case(name)
            || spec
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(name))
    })
}

/// Return the root command name, stripping any leading slash.
pub(crate) fn command_root_name(command: &str) -> &str {
    command.split_whitespace().next().unwrap_or(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SlashCommand;

    // --- SlashCommand::parse ---

    #[test]
    fn parse_non_slash_returns_none() {
        assert_eq!(SlashCommand::parse("hello").unwrap(), None);
    }

    #[test]
    fn parse_empty_command_after_slash() {
        assert!(SlashCommand::parse("/").is_err());
    }

    #[test]
    fn parse_help() {
        assert_eq!(
            SlashCommand::parse("/help").unwrap(),
            Some(SlashCommand::Help)
        );
    }

    #[test]
    fn parse_status() {
        assert_eq!(
            SlashCommand::parse("/status").unwrap(),
            Some(SlashCommand::Status)
        );
    }

    #[test]
    fn parse_sandbox() {
        assert_eq!(
            SlashCommand::parse("/sandbox").unwrap(),
            Some(SlashCommand::Sandbox)
        );
    }

    #[test]
    fn parse_compact() {
        assert_eq!(
            SlashCommand::parse("/compact").unwrap(),
            Some(SlashCommand::Compact)
        );
    }

    #[test]
    fn parse_commit() {
        assert_eq!(
            SlashCommand::parse("/commit").unwrap(),
            Some(SlashCommand::Commit)
        );
    }

    #[test]
    fn parse_cost() {
        assert_eq!(
            SlashCommand::parse("/cost").unwrap(),
            Some(SlashCommand::Cost)
        );
    }

    #[test]
    fn parse_memory() {
        assert_eq!(
            SlashCommand::parse("/memory").unwrap(),
            Some(SlashCommand::Memory)
        );
    }

    #[test]
    fn parse_init() {
        assert_eq!(
            SlashCommand::parse("/init").unwrap(),
            Some(SlashCommand::Init)
        );
    }

    #[test]
    fn parse_diff() {
        assert_eq!(
            SlashCommand::parse("/diff").unwrap(),
            Some(SlashCommand::Diff)
        );
    }

    #[test]
    fn parse_version() {
        assert_eq!(
            SlashCommand::parse("/version").unwrap(),
            Some(SlashCommand::Version)
        );
    }

    #[test]
    fn parse_debug_tool_call() {
        assert_eq!(
            SlashCommand::parse("/debug-tool-call").unwrap(),
            Some(SlashCommand::DebugToolCall)
        );
    }

    #[test]
    fn parse_unknown_command() {
        assert_eq!(
            SlashCommand::parse("/nonexistent").unwrap(),
            Some(SlashCommand::Unknown("nonexistent".to_string()))
        );
    }

    #[test]
    fn parse_help_with_args_errors() {
        assert!(SlashCommand::parse("/help extra").is_err());
    }

    #[test]
    fn parse_model_no_args() {
        assert_eq!(
            SlashCommand::parse("/model").unwrap(),
            Some(SlashCommand::Model { model: None })
        );
    }

    #[test]
    fn parse_model_with_arg() {
        assert_eq!(
            SlashCommand::parse("/model opus").unwrap(),
            Some(SlashCommand::Model {
                model: Some("opus".to_string())
            })
        );
    }

    #[test]
    fn parse_model_too_many_args_errors() {
        assert!(SlashCommand::parse("/model a b").is_err());
    }

    #[test]
    fn parse_export_with_path() {
        assert_eq!(
            SlashCommand::parse("/export /tmp/file.md").unwrap(),
            Some(SlashCommand::Export {
                path: Some("/tmp/file.md".to_string())
            })
        );
    }

    #[test]
    fn parse_export_no_path() {
        assert_eq!(
            SlashCommand::parse("/export").unwrap(),
            Some(SlashCommand::Export { path: None })
        );
    }

    #[test]
    fn parse_bughunter() {
        let result = SlashCommand::parse("/bughunter src/").unwrap();
        assert_eq!(
            result,
            Some(SlashCommand::Bughunter {
                scope: Some("src/".to_string())
            })
        );
    }

    #[test]
    fn parse_pr_with_context() {
        let result = SlashCommand::parse("/pr fix auth bug").unwrap();
        assert_eq!(
            result,
            Some(SlashCommand::Pr {
                context: Some("fix auth bug".to_string())
            })
        );
    }

    #[test]
    fn parse_issue_no_context() {
        let result = SlashCommand::parse("/issue").unwrap();
        assert_eq!(result, Some(SlashCommand::Issue { context: None }));
    }

    // --- validate_no_args ---

    #[test]
    fn validate_no_args_empty() {
        assert!(validate_no_args("help", &[]).is_ok());
    }

    #[test]
    fn validate_no_args_with_args() {
        assert!(validate_no_args("help", &["extra"]).is_err());
    }

    // --- optional_single_arg ---

    #[test]
    fn optional_single_arg_none() {
        assert_eq!(optional_single_arg("model", &[], "[model]").unwrap(), None);
    }

    #[test]
    fn optional_single_arg_one() {
        assert_eq!(
            optional_single_arg("model", &["opus"], "[model]").unwrap(),
            Some("opus".to_string())
        );
    }

    #[test]
    fn optional_single_arg_too_many() {
        assert!(optional_single_arg("model", &["a", "b"], "[model]").is_err());
    }

    // --- require_remainder ---

    #[test]
    fn require_remainder_present() {
        assert_eq!(
            require_remainder("teleport", Some("target".to_string()), "<target>").unwrap(),
            "target"
        );
    }

    #[test]
    fn require_remainder_missing() {
        assert!(require_remainder("teleport", None, "<target>").is_err());
    }

    // --- parse_permissions_mode ---

    #[test]
    fn parse_permissions_mode_none() {
        assert_eq!(parse_permissions_mode(&[]).unwrap(), None);
    }

    #[test]
    fn parse_permissions_mode_valid() {
        assert_eq!(
            parse_permissions_mode(&["read-only"]).unwrap(),
            Some("read-only".to_string())
        );
        assert_eq!(
            parse_permissions_mode(&["workspace-write"]).unwrap(),
            Some("workspace-write".to_string())
        );
        assert_eq!(
            parse_permissions_mode(&["danger-full-access"]).unwrap(),
            Some("danger-full-access".to_string())
        );
    }

    #[test]
    fn parse_permissions_mode_invalid() {
        assert!(parse_permissions_mode(&["admin"]).is_err());
    }

    // --- parse_clear_args ---

    #[test]
    fn parse_clear_args_empty() {
        assert!(!parse_clear_args(&[]).unwrap());
    }

    #[test]
    fn parse_clear_args_confirm() {
        assert!(parse_clear_args(&["--confirm"]).unwrap());
    }

    #[test]
    fn parse_clear_args_invalid() {
        assert!(parse_clear_args(&["--force"]).is_err());
    }

    #[test]
    fn parse_clear_args_too_many() {
        assert!(parse_clear_args(&["--confirm", "extra"]).is_err());
    }

    // --- parse_config_section ---

    #[test]
    fn parse_config_section_none() {
        assert_eq!(parse_config_section(&[]).unwrap(), None);
    }

    #[test]
    fn parse_config_section_valid() {
        assert_eq!(
            parse_config_section(&["env"]).unwrap(),
            Some("env".to_string())
        );
        assert_eq!(
            parse_config_section(&["hooks"]).unwrap(),
            Some("hooks".to_string())
        );
        assert_eq!(
            parse_config_section(&["model"]).unwrap(),
            Some("model".to_string())
        );
        assert_eq!(
            parse_config_section(&["plugins"]).unwrap(),
            Some("plugins".to_string())
        );
    }

    #[test]
    fn parse_config_section_invalid() {
        assert!(parse_config_section(&["database"]).is_err());
    }

    // --- parse_session_command ---

    #[test]
    fn parse_session_no_args() {
        let cmd = parse_session_command(&[]).unwrap();
        assert_eq!(
            cmd,
            SlashCommand::Session {
                action: None,
                target: None
            }
        );
    }

    #[test]
    fn parse_session_list() {
        let cmd = parse_session_command(&["list"]).unwrap();
        assert_eq!(
            cmd,
            SlashCommand::Session {
                action: Some("list".to_string()),
                target: None
            }
        );
    }

    #[test]
    fn parse_session_switch_with_target() {
        let cmd = parse_session_command(&["switch", "abc"]).unwrap();
        assert_eq!(
            cmd,
            SlashCommand::Session {
                action: Some("switch".to_string()),
                target: Some("abc".to_string())
            }
        );
    }

    #[test]
    fn parse_session_switch_no_target() {
        assert!(parse_session_command(&["switch"]).is_err());
    }

    #[test]
    fn parse_session_switch_too_many() {
        assert!(parse_session_command(&["switch", "a", "b"]).is_err());
    }

    #[test]
    fn parse_session_fork_no_target() {
        let cmd = parse_session_command(&["fork"]).unwrap();
        assert_eq!(
            cmd,
            SlashCommand::Session {
                action: Some("fork".to_string()),
                target: None
            }
        );
    }

    #[test]
    fn parse_session_fork_with_target() {
        let cmd = parse_session_command(&["fork", "branch"]).unwrap();
        assert_eq!(
            cmd,
            SlashCommand::Session {
                action: Some("fork".to_string()),
                target: Some("branch".to_string())
            }
        );
    }

    #[test]
    fn parse_session_fork_too_many() {
        assert!(parse_session_command(&["fork", "a", "b"]).is_err());
    }

    #[test]
    fn parse_session_unknown_action() {
        assert!(parse_session_command(&["delete"]).is_err());
    }

    #[test]
    fn parse_session_list_with_extra() {
        assert!(parse_session_command(&["list", "extra"]).is_err());
    }

    // --- parse_plugin_command ---

    #[test]
    fn parse_plugin_no_args() {
        let cmd = parse_plugin_command(&[]).unwrap();
        assert_eq!(
            cmd,
            SlashCommand::Plugins {
                action: None,
                target: None
            }
        );
    }

    #[test]
    fn parse_plugin_list() {
        let cmd = parse_plugin_command(&["list"]).unwrap();
        assert_eq!(
            cmd,
            SlashCommand::Plugins {
                action: Some("list".to_string()),
                target: None
            }
        );
    }

    #[test]
    fn parse_plugin_install_no_target() {
        assert!(parse_plugin_command(&["install"]).is_err());
    }

    #[test]
    fn parse_plugin_install_with_target() {
        let cmd = parse_plugin_command(&["install", "/path/to/plugin"]).unwrap();
        assert_eq!(
            cmd,
            SlashCommand::Plugins {
                action: Some("install".to_string()),
                target: Some("/path/to/plugin".to_string())
            }
        );
    }

    #[test]
    fn parse_plugin_enable_no_target() {
        assert!(parse_plugin_command(&["enable"]).is_err());
    }

    #[test]
    fn parse_plugin_enable_with_target() {
        let cmd = parse_plugin_command(&["enable", "my-plugin"]).unwrap();
        assert_eq!(
            cmd,
            SlashCommand::Plugins {
                action: Some("enable".to_string()),
                target: Some("my-plugin".to_string())
            }
        );
    }

    #[test]
    fn parse_plugin_disable_no_target() {
        assert!(parse_plugin_command(&["disable"]).is_err());
    }

    #[test]
    fn parse_plugin_disable_with_target() {
        let cmd = parse_plugin_command(&["disable", "my-plugin"]).unwrap();
        assert_eq!(
            cmd,
            SlashCommand::Plugins {
                action: Some("disable".to_string()),
                target: Some("my-plugin".to_string())
            }
        );
    }

    #[test]
    fn parse_plugin_uninstall_no_target() {
        assert!(parse_plugin_command(&["uninstall"]).is_err());
    }

    #[test]
    fn parse_plugin_uninstall_with_target() {
        let cmd = parse_plugin_command(&["uninstall", "my-plugin"]).unwrap();
        assert_eq!(
            cmd,
            SlashCommand::Plugins {
                action: Some("uninstall".to_string()),
                target: Some("my-plugin".to_string())
            }
        );
    }

    #[test]
    fn parse_plugin_update_no_target() {
        assert!(parse_plugin_command(&["update"]).is_err());
    }

    #[test]
    fn parse_plugin_update_with_target() {
        let cmd = parse_plugin_command(&["update", "my-plugin"]).unwrap();
        assert_eq!(
            cmd,
            SlashCommand::Plugins {
                action: Some("update".to_string()),
                target: Some("my-plugin".to_string())
            }
        );
    }

    #[test]
    fn parse_plugin_unknown_action() {
        assert!(parse_plugin_command(&["remove"]).is_err());
    }

    #[test]
    fn parse_plugin_enable_too_many() {
        assert!(parse_plugin_command(&["enable", "a", "b"]).is_err());
    }

    #[test]
    fn parse_plugin_disable_too_many() {
        assert!(parse_plugin_command(&["disable", "a", "b"]).is_err());
    }

    #[test]
    fn parse_plugin_uninstall_too_many() {
        assert!(parse_plugin_command(&["uninstall", "a", "b"]).is_err());
    }

    #[test]
    fn parse_plugin_update_too_many() {
        assert!(parse_plugin_command(&["update", "a", "b"]).is_err());
    }

    #[test]
    fn parse_plugin_list_with_extra() {
        assert!(parse_plugin_command(&["list", "extra"]).is_err());
    }

    // --- remainder_after_command ---

    #[test]
    fn remainder_after_command_present() {
        assert_eq!(
            remainder_after_command("/export /tmp/file.md", "export"),
            Some("/tmp/file.md".to_string())
        );
    }

    #[test]
    fn remainder_after_command_empty() {
        assert_eq!(remainder_after_command("/export", "export"), None);
    }

    #[test]
    fn remainder_after_command_whitespace_only() {
        assert_eq!(remainder_after_command("/export   ", "export"), None);
    }

    // --- command_root_name ---

    #[test]
    fn command_root_name_single() {
        assert_eq!(command_root_name("help"), "help");
    }

    #[test]
    fn command_root_name_multi() {
        assert_eq!(command_root_name("session switch"), "session");
    }

    // --- parse_skills_args ---

    #[test]
    fn parse_skills_args_none() {
        assert_eq!(parse_skills_args(None).unwrap(), None);
    }

    #[test]
    fn parse_skills_args_list() {
        assert_eq!(
            parse_skills_args(Some("list")).unwrap(),
            Some("list".to_string())
        );
    }

    #[test]
    fn parse_skills_args_help() {
        assert_eq!(
            parse_skills_args(Some("help")).unwrap(),
            Some("help".to_string())
        );
    }

    #[test]
    fn parse_skills_args_install_no_target() {
        assert!(parse_skills_args(Some("install")).is_err());
    }

    #[test]
    fn parse_skills_args_install_with_target() {
        assert_eq!(
            parse_skills_args(Some("install /path")).unwrap(),
            Some("install /path".to_string())
        );
    }

    #[test]
    fn parse_skills_args_invalid() {
        assert!(parse_skills_args(Some("remove")).is_err());
    }

    // --- parse_list_or_help_args ---

    #[test]
    fn parse_list_or_help_args_none() {
        assert!(parse_list_or_help_args("agents", None).is_ok());
    }

    #[test]
    fn parse_list_or_help_args_list() {
        assert!(parse_list_or_help_args("agents", Some("list".to_string())).is_ok());
    }

    #[test]
    fn parse_list_or_help_args_help() {
        assert!(parse_list_or_help_args("agents", Some("help".to_string())).is_ok());
    }

    #[test]
    fn parse_list_or_help_args_invalid() {
        assert!(parse_list_or_help_args("agents", Some("delete".to_string())).is_err());
    }

    // --- parse from /plugin aliases ---

    #[test]
    fn parse_plugins_alias() {
        let result = SlashCommand::parse("/plugins").unwrap();
        assert!(matches!(
            result,
            Some(SlashCommand::Plugins { action: None, .. })
        ));
    }

    #[test]
    fn parse_marketplace_alias() {
        let result = SlashCommand::parse("/marketplace").unwrap();
        assert!(matches!(
            result,
            Some(SlashCommand::Plugins { action: None, .. })
        ));
    }

    // --- whitespace handling ---

    #[test]
    fn parse_with_leading_trailing_whitespace() {
        assert_eq!(
            SlashCommand::parse("  /help  ").unwrap(),
            Some(SlashCommand::Help)
        );
    }
}
