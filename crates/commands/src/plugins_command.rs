//! Plugin slash command handler and report rendering.

use std::path::{Path, PathBuf};

use colotcook_plugins::{PluginError, PluginManager, PluginSummary};
use colotcook_runtime::Session;

use crate::agents_and_skills::{
    discover_definition_roots, discover_skill_roots, install_skill, load_agents_from_roots,
    load_skills_from_roots, normalize_optional_args, render_agents_report, render_agents_usage,
    render_skill_install_report, render_skills_report, render_skills_usage,
};

#[derive(Debug, Clone, PartialEq, Eq)]
/// Result of dispatching a slash command.
pub struct SlashCommandResult {
    pub message: String,
    pub session: Session,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Result of a `/plugins` command invocation.
pub struct PluginsCommandResult {
    pub message: String,
    pub reload_runtime: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
/// Source location of an agent or skill definition.
pub(crate) enum DefinitionSource {
    ProjectCodex,
    ProjectClaude,
    UserCodexHome,
    UserCodex,
    UserClaude,
}

impl DefinitionSource {
    /// Return a human-readable label for this source.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::ProjectCodex => "Project (.codex)",
            Self::ProjectClaude => "Project (.claude)",
            Self::UserCodexHome => "User ($CODEX_HOME)",
            Self::UserCodex => "User (~/.codex)",
            Self::UserClaude => "User (~/.claude)",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Summary of a discovered agent.
pub(crate) struct AgentSummary {
    pub name: String,
    pub description: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub source: DefinitionSource,
    pub shadowed_by: Option<DefinitionSource>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Summary of a discovered skill.
pub(crate) struct SkillSummary {
    pub name: String,
    pub description: Option<String>,
    pub source: DefinitionSource,
    pub shadowed_by: Option<DefinitionSource>,
    pub origin: SkillOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Origin type of a discovered skill.
pub(crate) enum SkillOrigin {
    SkillsDir,
    LegacyCommandsDir,
}

impl SkillOrigin {
    /// Return an optional detail label for this origin.
    pub(crate) fn detail_label(self) -> Option<&'static str> {
        match self {
            Self::SkillsDir => None,
            Self::LegacyCommandsDir => Some("legacy /commands"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// A root directory where skills are discovered.
pub(crate) struct SkillRoot {
    pub source: DefinitionSource,
    pub path: PathBuf,
    pub origin: SkillOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Metadata about a freshly installed skill.
pub(crate) struct InstalledSkill {
    pub invocation_name: String,
    pub display_name: Option<String>,
    pub source: PathBuf,
    pub registry_root: PathBuf,
    pub installed_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Source type for installing a skill.
pub(crate) enum SkillInstallSource {
    Directory { root: PathBuf, prompt_path: PathBuf },
    MarkdownFile { path: PathBuf },
}

#[allow(clippy::too_many_lines)]
/// Dispatch a `/plugins` command to install/enable/disable/uninstall.
pub fn handle_plugins_slash_command(
    action: Option<&str>,
    target: Option<&str>,
    manager: &mut PluginManager,
) -> Result<PluginsCommandResult, PluginError> {
    match action {
        None | Some("list") => Ok(PluginsCommandResult {
            message: render_plugins_report(&manager.list_installed_plugins()?),
            reload_runtime: false,
        }),
        Some("install") => {
            let Some(target) = target else {
                return Ok(PluginsCommandResult {
                    message: "Usage: /plugins install <path>".to_string(),
                    reload_runtime: false,
                });
            };
            let install = manager.install(target)?;
            let plugin = manager
                .list_installed_plugins()?
                .into_iter()
                .find(|plugin| plugin.metadata.id == install.plugin_id);
            Ok(PluginsCommandResult {
                message: render_plugin_install_report(&install.plugin_id, plugin.as_ref()),
                reload_runtime: true,
            })
        }
        Some("enable") => {
            let Some(target) = target else {
                return Ok(PluginsCommandResult {
                    message: "Usage: /plugins enable <name>".to_string(),
                    reload_runtime: false,
                });
            };
            let plugin = resolve_plugin_target(manager, target)?;
            manager.enable(&plugin.metadata.id)?;
            Ok(PluginsCommandResult {
                message: format!(
                    "Plugins\n  Result           enabled {}\n  Name             {}\n  Version          {}\n  Status           enabled",
                    plugin.metadata.id, plugin.metadata.name, plugin.metadata.version
                ),
                reload_runtime: true,
            })
        }
        Some("disable") => {
            let Some(target) = target else {
                return Ok(PluginsCommandResult {
                    message: "Usage: /plugins disable <name>".to_string(),
                    reload_runtime: false,
                });
            };
            let plugin = resolve_plugin_target(manager, target)?;
            manager.disable(&plugin.metadata.id)?;
            Ok(PluginsCommandResult {
                message: format!(
                    "Plugins\n  Result           disabled {}\n  Name             {}\n  Version          {}\n  Status           disabled",
                    plugin.metadata.id, plugin.metadata.name, plugin.metadata.version
                ),
                reload_runtime: true,
            })
        }
        Some("uninstall") => {
            let Some(target) = target else {
                return Ok(PluginsCommandResult {
                    message: "Usage: /plugins uninstall <plugin-id>".to_string(),
                    reload_runtime: false,
                });
            };
            manager.uninstall(target)?;
            Ok(PluginsCommandResult {
                message: format!("Plugins\n  Result           uninstalled {target}"),
                reload_runtime: true,
            })
        }
        Some("update") => {
            let Some(target) = target else {
                return Ok(PluginsCommandResult {
                    message: "Usage: /plugins update <plugin-id>".to_string(),
                    reload_runtime: false,
                });
            };
            let update = manager.update(target)?;
            let plugin = manager
                .list_installed_plugins()?
                .into_iter()
                .find(|plugin| plugin.metadata.id == update.plugin_id);
            Ok(PluginsCommandResult {
                message: format!(
                    "Plugins\n  Result           updated {}\n  Name             {}\n  Old version      {}\n  New version      {}\n  Status           {}",
                    update.plugin_id,
                    plugin
                        .as_ref()
                        .map_or_else(|| update.plugin_id.clone(), |plugin| plugin.metadata.name.clone()),
                    update.old_version,
                    update.new_version,
                    plugin
                        .as_ref()
                        .map_or("unknown", |plugin| if plugin.enabled { "enabled" } else { "disabled" }),
                ),
                reload_runtime: true,
            })
        }
        Some(other) => Ok(PluginsCommandResult {
            message: format!(
                "Unknown /plugins action '{other}'. Use list, install, enable, disable, uninstall, or update."
            ),
            reload_runtime: false,
        }),
    }
}

/// Dispatch an `/agents` command and return the rendered report.
pub fn handle_agents_slash_command(args: Option<&str>, cwd: &Path) -> std::io::Result<String> {
    match normalize_optional_args(args) {
        None | Some("list") => {
            let roots = discover_definition_roots(cwd, "agents");
            let agents = load_agents_from_roots(&roots)?;
            Ok(render_agents_report(&agents))
        }
        Some("-h" | "--help" | "help") => Ok(render_agents_usage(None)),
        Some(args) => Ok(render_agents_usage(Some(args))),
    }
}

/// Dispatch a `/skills` command and return the rendered report.
pub fn handle_skills_slash_command(args: Option<&str>, cwd: &Path) -> std::io::Result<String> {
    match normalize_optional_args(args) {
        None | Some("list") => {
            let roots = discover_skill_roots(cwd);
            let skills = load_skills_from_roots(&roots)?;
            Ok(render_skills_report(&skills))
        }
        Some("install") => Ok(render_skills_usage(Some("install"))),
        Some(args) if args.starts_with("install ") => {
            let target = args["install ".len()..].trim();
            if target.is_empty() {
                return Ok(render_skills_usage(Some("install")));
            }
            let install = install_skill(target, cwd)?;
            Ok(render_skill_install_report(&install))
        }
        Some("-h" | "--help" | "help") => Ok(render_skills_usage(None)),
        Some(args) => Ok(render_skills_usage(Some(args))),
    }
}

#[must_use]
/// Render the plugins listing report.
pub fn render_plugins_report(plugins: &[PluginSummary]) -> String {
    let mut lines = vec!["Plugins".to_string()];
    if plugins.is_empty() {
        lines.push("  No plugins installed.".to_string());
        return lines.join("\n");
    }
    for plugin in plugins {
        let enabled = if plugin.enabled {
            "enabled"
        } else {
            "disabled"
        };
        lines.push(format!(
            "  {name:<20} v{version:<10} {enabled}",
            name = plugin.metadata.name,
            version = plugin.metadata.version,
        ));
    }
    lines.join("\n")
}

/// Render a report after installing a plugin.
pub(crate) fn render_plugin_install_report(
    plugin_id: &str,
    plugin: Option<&PluginSummary>,
) -> String {
    let name = plugin.map_or(plugin_id, |plugin| plugin.metadata.name.as_str());
    let version = plugin.map_or("unknown", |plugin| plugin.metadata.version.as_str());
    let enabled = plugin.is_some_and(|plugin| plugin.enabled);
    format!(
        "Plugins\n  Result           installed {plugin_id}\n  Name             {name}\n  Version          {version}\n  Status           {}",
        if enabled { "enabled" } else { "disabled" }
    )
}

/// Resolve a plugin target to an install-ready path.
pub(crate) fn resolve_plugin_target(
    manager: &PluginManager,
    target: &str,
) -> Result<PluginSummary, PluginError> {
    let mut matches = manager
        .list_installed_plugins()?
        .into_iter()
        .filter(|plugin| plugin.metadata.id == target || plugin.metadata.name == target)
        .collect::<Vec<_>>();
    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => Err(PluginError::NotFound(format!(
            "plugin `{target}` is not installed or discoverable"
        ))),
        _ => Err(PluginError::InvalidManifest(format!(
            "plugin name `{target}` is ambiguous; use the full plugin id"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use colotcook_plugins::{PluginKind, PluginMetadata, PluginSummary};

    fn make_plugin_summary(id: &str, name: &str, version: &str, enabled: bool) -> PluginSummary {
        PluginSummary {
            metadata: PluginMetadata {
                id: id.to_string(),
                name: name.to_string(),
                version: version.to_string(),
                description: String::new(),
                kind: PluginKind::External,
                source: String::new(),
                default_enabled: true,
                root: None,
            },
            enabled,
        }
    }

    // ── render_plugins_report ────────────────────────────────────────────────

    #[test]
    fn render_plugins_report_empty_list() {
        let result = render_plugins_report(&[]);
        assert!(result.contains("No plugins installed."));
    }

    #[test]
    fn render_plugins_report_single_enabled_plugin() {
        let plugins = vec![make_plugin_summary("my-plugin", "My Plugin", "1.0.0", true)];
        let result = render_plugins_report(&plugins);
        assert!(result.contains("My Plugin"));
        assert!(result.contains("1.0.0"));
        assert!(result.contains("enabled"));
    }

    #[test]
    fn render_plugins_report_disabled_plugin() {
        let plugins = vec![make_plugin_summary("my-plugin", "My Plugin", "2.0.0", false)];
        let result = render_plugins_report(&plugins);
        assert!(result.contains("disabled"));
    }

    #[test]
    fn render_plugins_report_multiple_plugins() {
        let plugins = vec![
            make_plugin_summary("plugin-a", "Plugin A", "1.0", true),
            make_plugin_summary("plugin-b", "Plugin B", "2.0", false),
        ];
        let result = render_plugins_report(&plugins);
        assert!(result.contains("Plugin A"));
        assert!(result.contains("Plugin B"));
    }

    // ── render_plugin_install_report ─────────────────────────────────────────

    #[test]
    fn render_plugin_install_report_with_plugin() {
        let plugin = make_plugin_summary("my-id", "My Plugin", "1.0.0", true);
        let result = render_plugin_install_report("my-id", Some(&plugin));
        assert!(result.contains("installed my-id"));
        assert!(result.contains("My Plugin"));
        assert!(result.contains("1.0.0"));
        assert!(result.contains("enabled"));
    }

    #[test]
    fn render_plugin_install_report_without_plugin() {
        let result = render_plugin_install_report("unknown-id", None);
        assert!(result.contains("installed unknown-id"));
        assert!(result.contains("unknown-id")); // name fallback
        assert!(result.contains("unknown")); // version fallback
        assert!(result.contains("disabled"));
    }

    // ── DefinitionSource::label ──────────────────────────────────────────────

    #[test]
    fn definition_source_label_project_codex() {
        assert_eq!(DefinitionSource::ProjectCodex.label(), "Project (.codex)");
    }

    #[test]
    fn definition_source_label_user_claude() {
        assert_eq!(DefinitionSource::UserClaude.label(), "User (~/.claude)");
    }

    #[test]
    fn definition_source_label_user_codex_home() {
        assert!(DefinitionSource::UserCodexHome.label().contains("CODEX_HOME"));
    }

    // ── SkillOrigin::detail_label ────────────────────────────────────────────

    #[test]
    fn skill_origin_skills_dir_has_no_detail_label() {
        assert!(SkillOrigin::SkillsDir.detail_label().is_none());
    }

    #[test]
    fn skill_origin_legacy_commands_dir_has_detail_label() {
        assert_eq!(
            SkillOrigin::LegacyCommandsDir.detail_label(),
            Some("legacy /commands")
        );
    }
}
