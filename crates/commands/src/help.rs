//! Help rendering, suggestion, and slash command spec access.

use crate::types::{SlashCommandSpec, SLASH_COMMAND_SPECS};
use crate::validation::find_slash_command_spec;

/// Format the usage string for a slash command spec.
pub(crate) fn slash_command_usage(spec: &SlashCommandSpec) -> String {
    match spec.argument_hint {
        Some(argument_hint) => format!("/{} {argument_hint}", spec.name),
        None => format!("/{}", spec.name),
    }
}

/// Render detail lines for one slash command (used in `/help <cmd>`).
pub(crate) fn slash_command_detail_lines(spec: &SlashCommandSpec) -> Vec<String> {
    let mut lines = vec![format!("/{}", spec.name)];
    lines.push(format!("  Summary          {}", spec.summary));
    lines.push(format!("  Usage            {}", slash_command_usage(spec)));
    lines.push(format!(
        "  Category         {}",
        slash_command_category(spec.name)
    ));
    if !spec.aliases.is_empty() {
        lines.push(format!(
            "  Aliases          {}",
            spec.aliases
                .iter()
                .map(|alias| format!("/{alias}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if spec.resume_supported {
        lines.push("  Resume           Supported with --resume SESSION.jsonl".to_string());
    }
    lines
}

#[must_use]
/// Render the full detail block for a command by name.
pub fn render_slash_command_help_detail(name: &str) -> Option<String> {
    find_slash_command_spec(name).map(|spec| slash_command_detail_lines(spec).join("\n"))
}

#[must_use]
/// Return the full static slice of slash command specs.
pub fn slash_command_specs() -> &'static [SlashCommandSpec] {
    SLASH_COMMAND_SPECS
}

#[must_use]
/// Return specs for commands that work with `--resume`.
pub fn resume_supported_slash_commands() -> Vec<&'static SlashCommandSpec> {
    slash_command_specs()
        .iter()
        .filter(|spec| spec.resume_supported)
        .collect()
}

/// Return a human-readable category label for a command name.
pub(crate) fn slash_command_category(name: &str) -> &'static str {
    match name {
        "help" | "status" | "sandbox" | "model" | "permissions" | "cost" | "resume" | "session"
        | "version" => "Session & visibility",
        "compact" | "clear" | "config" | "memory" | "init" | "diff" | "commit" | "pr" | "issue"
        | "export" | "plugin" => "Workspace & git",
        "agents" | "skills" | "teleport" | "debug-tool-call" => "Discovery & debugging",
        "bughunter" | "ultraplan" => "Analysis & automation",
        _ => "Other",
    }
}

/// Format a single help-list line for a slash command.
pub(crate) fn format_slash_command_help_line(spec: &SlashCommandSpec) -> String {
    let name = slash_command_usage(spec);
    let alias_suffix = if spec.aliases.is_empty() {
        String::new()
    } else {
        format!(
            " (aliases: {})",
            spec.aliases
                .iter()
                .map(|alias| format!("/{alias}"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let resume = if spec.resume_supported {
        " [resume]"
    } else {
        ""
    };
    format!("  {name:<66} {}{alias_suffix}{resume}", spec.summary)
}

/// Compute the Levenshtein distance between two strings.
pub(crate) fn levenshtein_distance(left: &str, right: &str) -> usize {
    if left == right {
        return 0;
    }
    if left.is_empty() {
        return right.chars().count();
    }
    if right.is_empty() {
        return left.chars().count();
    }

    let right_chars = right.chars().collect::<Vec<_>>();
    let mut previous = (0..=right_chars.len()).collect::<Vec<_>>();
    let mut current = vec![0; right_chars.len() + 1];

    for (left_index, left_char) in left.chars().enumerate() {
        current[0] = left_index + 1;
        for (right_index, right_char) in right_chars.iter().enumerate() {
            let substitution_cost = usize::from(left_char != *right_char);
            current[right_index + 1] = (current[right_index] + 1)
                .min(previous[right_index + 1] + 1)
                .min(previous[right_index] + substitution_cost);
        }
        previous.clone_from(&current);
    }

    previous[right_chars.len()]
}

#[must_use]
/// Return suggestions for a mistyped slash command.
pub fn suggest_slash_commands(input: &str, limit: usize) -> Vec<String> {
    let query = input.trim().trim_start_matches('/').to_ascii_lowercase();
    if query.is_empty() || limit == 0 {
        return Vec::new();
    }

    let mut suggestions = slash_command_specs()
        .iter()
        .filter_map(|spec| {
            let best = std::iter::once(spec.name)
                .chain(spec.aliases.iter().copied())
                .map(str::to_ascii_lowercase)
                .map(|candidate| {
                    let prefix_rank =
                        if candidate.starts_with(&query) || query.starts_with(&candidate) {
                            0
                        } else if candidate.contains(&query) || query.contains(&candidate) {
                            1
                        } else {
                            2
                        };
                    let distance = levenshtein_distance(&candidate, &query);
                    (prefix_rank, distance)
                })
                .min();

            best.and_then(|(prefix_rank, distance)| {
                if prefix_rank <= 1 || distance <= 2 {
                    Some((prefix_rank, distance, spec.name.len(), spec.name))
                } else {
                    None
                }
            })
        })
        .collect::<Vec<_>>();

    suggestions.sort_unstable();
    suggestions
        .into_iter()
        .map(|(_, _, _, name)| format!("/{name}"))
        .take(limit)
        .collect()
}

#[must_use]
/// Render the full `/help` block listing all slash commands.
pub fn render_slash_command_help() -> String {
    let mut lines = vec![
        "Slash commands".to_string(),
        "  Start here        /status, /diff, /agents, /skills, /commit".to_string(),
        "  [resume]          also works with --resume SESSION.jsonl".to_string(),
        String::new(),
    ];

    let categories = [
        "Session & visibility",
        "Workspace & git",
        "Discovery & debugging",
        "Analysis & automation",
    ];

    for category in categories {
        lines.push(category.to_string());
        for spec in slash_command_specs()
            .iter()
            .filter(|spec| slash_command_category(spec.name) == category)
        {
            lines.push(format_slash_command_help_line(spec));
        }
        lines.push(String::new());
    }

    lines
        .into_iter()
        .rev()
        .skip_while(String::is_empty)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n")
}
