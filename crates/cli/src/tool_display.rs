//! Tool call and result formatting for terminal display.

use crate::util::{
    extract_tool_path, first_visible_line, summarize_tool_payload, truncate_for_summary,
    truncate_output_for_display, READ_DISPLAY_MAX_CHARS, READ_DISPLAY_MAX_LINES,
    TOOL_OUTPUT_DISPLAY_MAX_CHARS, TOOL_OUTPUT_DISPLAY_MAX_LINES,
};

/// Render the start of a tool call (name + input summary).
pub(crate) fn format_tool_call_start(name: &str, input: &str) -> String {
    let parsed: serde_json::Value =
        serde_json::from_str(input).unwrap_or(serde_json::Value::String(input.to_string()));

    let detail = match name {
        "bash" | "Bash" => format_bash_call(&parsed),
        "read_file" | "Read" => {
            let path = extract_tool_path(&parsed);
            format!("\x1b[2m📄 Reading {path}…\x1b[0m")
        }
        "write_file" | "Write" => {
            let path = extract_tool_path(&parsed);
            let lines = parsed
                .get("content")
                .and_then(|value| value.as_str())
                .map_or(0, |content| content.lines().count());
            format!("\x1b[1;32m✏️ Writing {path}\x1b[0m \x1b[2m({lines} lines)\x1b[0m")
        }
        "edit_file" | "Edit" => {
            let path = extract_tool_path(&parsed);
            let old_value = parsed
                .get("old_string")
                .or_else(|| parsed.get("oldString"))
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let new_value = parsed
                .get("new_string")
                .or_else(|| parsed.get("newString"))
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            format!(
                "\x1b[1;33m📝 Editing {path}\x1b[0m{}",
                format_patch_preview(old_value, new_value)
                    .map(|preview| format!("\n{preview}"))
                    .unwrap_or_default()
            )
        }
        "glob_search" | "Glob" => format_search_start("🔎 Glob", &parsed),
        "grep_search" | "Grep" => format_search_start("🔎 Grep", &parsed),
        "web_search" | "WebSearch" => parsed
            .get("query")
            .and_then(|value| value.as_str())
            .unwrap_or("?")
            .to_string(),
        _ => summarize_tool_payload(input),
    };

    let border = "─".repeat(name.len() + 8);
    format!(
        "\x1b[38;5;245m╭─ \x1b[1;36m{name}\x1b[0;38;5;245m ─╮\x1b[0m\n\x1b[38;5;245m│\x1b[0m {detail}\n\x1b[38;5;245m╰{border}╯\x1b[0m"
    )
}

/// Render the result of a tool call with success/error indicator.
pub(crate) fn format_tool_result(name: &str, output: &str, is_error: bool) -> String {
    let icon = if is_error {
        "\x1b[1;31m✗\x1b[0m"
    } else {
        "\x1b[1;32m✓\x1b[0m"
    };
    if is_error {
        let summary = truncate_for_summary(output.trim(), 160);
        return if summary.is_empty() {
            format!("{icon} \x1b[38;5;245m{name}\x1b[0m")
        } else {
            format!("{icon} \x1b[38;5;245m{name}\x1b[0m\n\x1b[38;5;203m{summary}\x1b[0m")
        };
    }

    let parsed: serde_json::Value =
        serde_json::from_str(output).unwrap_or(serde_json::Value::String(output.to_string()));
    match name {
        "bash" | "Bash" => format_bash_result(icon, &parsed),
        "read_file" | "Read" => format_read_result(icon, &parsed),
        "write_file" | "Write" => format_write_result(icon, &parsed),
        "edit_file" | "Edit" => format_edit_result(icon, &parsed),
        "glob_search" | "Glob" => format_glob_result(icon, &parsed),
        "grep_search" | "Grep" => format_grep_result(icon, &parsed),
        _ => format_generic_tool_result(icon, name, &parsed),
    }
}

/// Format the start line for a search tool (Glob/Grep).
pub(crate) fn format_search_start(label: &str, parsed: &serde_json::Value) -> String {
    let pattern = parsed
        .get("pattern")
        .and_then(|value| value.as_str())
        .unwrap_or("?");
    let scope = parsed
        .get("path")
        .and_then(|value| value.as_str())
        .unwrap_or(".");
    format!("{label} {pattern}\n\x1b[2min {scope}\x1b[0m")
}

/// Format a diff preview for edit operations (old -> new).
pub(crate) fn format_patch_preview(old_value: &str, new_value: &str) -> Option<String> {
    if old_value.is_empty() && new_value.is_empty() {
        return None;
    }
    Some(format!(
        "\x1b[38;5;203m- {}\x1b[0m\n\x1b[38;5;70m+ {}\x1b[0m",
        truncate_for_summary(first_visible_line(old_value), 72),
        truncate_for_summary(first_visible_line(new_value), 72)
    ))
}

/// Format a bash command invocation.
fn format_bash_call(parsed: &serde_json::Value) -> String {
    let command = parsed
        .get("command")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if command.is_empty() {
        String::new()
    } else {
        format!(
            "\x1b[48;5;236;38;5;255m $ {} \x1b[0m",
            truncate_for_summary(command, 160)
        )
    }
}

/// Format a bash tool result with stdout/stderr.
fn format_bash_result(icon: &str, parsed: &serde_json::Value) -> String {
    use std::fmt::Write as _;

    let mut lines = vec![format!("{icon} \x1b[38;5;245mbash\x1b[0m")];
    if let Some(task_id) = parsed
        .get("backgroundTaskId")
        .and_then(|value| value.as_str())
    {
        write!(&mut lines[0], " backgrounded ({task_id})").expect("write to string");
    } else if let Some(status) = parsed
        .get("returnCodeInterpretation")
        .and_then(|value| value.as_str())
        .filter(|status| !status.is_empty())
    {
        write!(&mut lines[0], " {status}").expect("write to string");
    }

    if let Some(stdout) = parsed.get("stdout").and_then(|value| value.as_str()) {
        if !stdout.trim().is_empty() {
            lines.push(truncate_output_for_display(
                stdout,
                TOOL_OUTPUT_DISPLAY_MAX_LINES,
                TOOL_OUTPUT_DISPLAY_MAX_CHARS,
            ));
        }
    }
    if let Some(stderr) = parsed.get("stderr").and_then(|value| value.as_str()) {
        if !stderr.trim().is_empty() {
            lines.push(format!(
                "\x1b[38;5;203m{}\x1b[0m",
                truncate_output_for_display(
                    stderr,
                    TOOL_OUTPUT_DISPLAY_MAX_LINES,
                    TOOL_OUTPUT_DISPLAY_MAX_CHARS,
                )
            ));
        }
    }

    lines.join("\n\n")
}

/// Format a Read tool result.
fn format_read_result(icon: &str, parsed: &serde_json::Value) -> String {
    let file = parsed.get("file").unwrap_or(parsed);
    let path = extract_tool_path(file);
    let start_line = file
        .get("startLine")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(1);
    let num_lines = file
        .get("numLines")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let total_lines = file
        .get("totalLines")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(num_lines);
    let content = file
        .get("content")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let end_line = start_line.saturating_add(num_lines.saturating_sub(1));

    format!(
        "{icon} \x1b[2m📄 Read {path} (lines {}-{} of {})\x1b[0m\n{}",
        start_line,
        end_line.max(start_line),
        total_lines,
        truncate_output_for_display(content, READ_DISPLAY_MAX_LINES, READ_DISPLAY_MAX_CHARS)
    )
}

/// Format a Write tool result.
fn format_write_result(icon: &str, parsed: &serde_json::Value) -> String {
    let path = extract_tool_path(parsed);
    let kind = parsed
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("write");
    let line_count = parsed
        .get("content")
        .and_then(|value| value.as_str())
        .map_or(0, |content| content.lines().count());
    format!(
        "{icon} \x1b[1;32m✏️ {} {path}\x1b[0m \x1b[2m({line_count} lines)\x1b[0m",
        if kind == "create" { "Wrote" } else { "Updated" },
    )
}

/// Format a structured patch preview from diff hunks.
pub(crate) fn format_structured_patch_preview(parsed: &serde_json::Value) -> Option<String> {
    let hunks = parsed.get("structuredPatch")?.as_array()?;
    let mut preview = Vec::new();
    for hunk in hunks.iter().take(2) {
        let lines = hunk.get("lines")?.as_array()?;
        for line in lines.iter().filter_map(|value| value.as_str()).take(6) {
            match line.chars().next() {
                Some('+') => preview.push(format!("\x1b[38;5;70m{line}\x1b[0m")),
                Some('-') => preview.push(format!("\x1b[38;5;203m{line}\x1b[0m")),
                _ => preview.push(line.to_string()),
            }
        }
    }
    if preview.is_empty() {
        None
    } else {
        Some(preview.join("\n"))
    }
}

/// Format an Edit tool result with diff preview.
fn format_edit_result(icon: &str, parsed: &serde_json::Value) -> String {
    let path = extract_tool_path(parsed);
    let suffix = if parsed
        .get("replaceAll")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        " (replace all)"
    } else {
        ""
    };
    let preview = format_structured_patch_preview(parsed).or_else(|| {
        let old_value = parsed
            .get("oldString")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let new_value = parsed
            .get("newString")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        format_patch_preview(old_value, new_value)
    });

    match preview {
        Some(preview) => format!("{icon} \x1b[1;33m📝 Edited {path}{suffix}\x1b[0m\n{preview}"),
        None => format!("{icon} \x1b[1;33m📝 Edited {path}{suffix}\x1b[0m"),
    }
}

/// Format a Glob tool result.
pub(crate) fn format_glob_result(icon: &str, parsed: &serde_json::Value) -> String {
    let num_files = parsed
        .get("numFiles")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let filenames = parsed
        .get("filenames")
        .and_then(|value| value.as_array())
        .map(|files| {
            files
                .iter()
                .filter_map(|value| value.as_str())
                .take(8)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    if filenames.is_empty() {
        format!("{icon} \x1b[38;5;245mglob_search\x1b[0m matched {num_files} files")
    } else {
        format!("{icon} \x1b[38;5;245mglob_search\x1b[0m matched {num_files} files\n{filenames}")
    }
}

/// Format a Grep tool result.
pub(crate) fn format_grep_result(icon: &str, parsed: &serde_json::Value) -> String {
    let num_matches = parsed
        .get("numMatches")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let num_files = parsed
        .get("numFiles")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let content = parsed
        .get("content")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let filenames = parsed
        .get("filenames")
        .and_then(|value| value.as_array())
        .map(|files| {
            files
                .iter()
                .filter_map(|value| value.as_str())
                .take(8)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let summary = format!(
        "{icon} \x1b[38;5;245mgrep_search\x1b[0m {num_matches} matches across {num_files} files"
    );
    if !content.trim().is_empty() {
        format!(
            "{summary}\n{}",
            truncate_output_for_display(
                content,
                TOOL_OUTPUT_DISPLAY_MAX_LINES,
                TOOL_OUTPUT_DISPLAY_MAX_CHARS,
            )
        )
    } else if !filenames.is_empty() {
        format!("{summary}\n{filenames}")
    } else {
        summary
    }
}

/// Format a generic (non-specialized) tool result.
pub(crate) fn format_generic_tool_result(
    icon: &str,
    name: &str,
    parsed: &serde_json::Value,
) -> String {
    let rendered_output = match parsed {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Null => String::new(),
        serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
            serde_json::to_string_pretty(parsed).unwrap_or_else(|_| parsed.to_string())
        }
        _ => parsed.to_string(),
    };
    let preview = truncate_output_for_display(
        &rendered_output,
        TOOL_OUTPUT_DISPLAY_MAX_LINES,
        TOOL_OUTPUT_DISPLAY_MAX_CHARS,
    );

    if preview.is_empty() {
        format!("{icon} \x1b[38;5;245m{name}\x1b[0m")
    } else if preview.contains('\n') {
        format!("{icon} \x1b[38;5;245m{name}\x1b[0m\n{preview}")
    } else {
        format!("{icon} \x1b[38;5;245m{name}:\x1b[0m {preview}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── format_search_start ──────────────────────────────────────────────────

    #[test]
    fn format_search_start_includes_pattern_and_scope() {
        let parsed = serde_json::json!({"pattern": "*.rs", "path": "src/"});
        let result = format_search_start("🔎 Glob", &parsed);
        assert!(result.contains("*.rs"));
        assert!(result.contains("src/"));
    }

    #[test]
    fn format_search_start_defaults_when_keys_absent() {
        let parsed = serde_json::json!({});
        let result = format_search_start("🔎 Grep", &parsed);
        assert!(result.contains("?"));
        assert!(result.contains("."));
    }

    // ── format_patch_preview ─────────────────────────────────────────────────

    #[test]
    fn format_patch_preview_both_empty_returns_none() {
        assert!(format_patch_preview("", "").is_none());
    }

    #[test]
    fn format_patch_preview_non_empty_returns_some() {
        let result = format_patch_preview("old line", "new line");
        assert!(result.is_some());
        let s = result.unwrap();
        assert!(s.contains("old line") || s.contains("new line"));
    }

    #[test]
    fn format_patch_preview_contains_minus_and_plus_colored() {
        let result = format_patch_preview("foo", "bar").unwrap();
        // ANSI colored lines with - and +
        assert!(result.contains("foo"));
        assert!(result.contains("bar"));
    }

    #[test]
    fn format_patch_preview_old_only() {
        let result = format_patch_preview("removed", "");
        assert!(result.is_some());
        assert!(result.unwrap().contains("removed"));
    }

    // ── format_bash_call (via format_tool_call_start) ────────────────────────

    #[test]
    fn format_tool_call_start_bash_includes_command() {
        let input = r#"{"command": "ls -la"}"#;
        let result = format_tool_call_start("bash", input);
        assert!(result.contains("ls -la"));
    }

    #[test]
    fn format_tool_call_start_bash_empty_command() {
        let input = r#"{"command": ""}"#;
        let result = format_tool_call_start("bash", input);
        // Should still render border with bash
        assert!(result.contains("bash"));
    }

    #[test]
    fn format_tool_call_start_read_includes_path() {
        let input = r#"{"file_path": "/src/main.rs"}"#;
        let result = format_tool_call_start("read_file", input);
        assert!(result.contains("/src/main.rs"));
    }

    #[test]
    fn format_tool_call_start_write_includes_path_and_lines() {
        let input = r#"{"file_path": "/out.txt", "content": "line1\nline2\nline3"}"#;
        let result = format_tool_call_start("write_file", input);
        assert!(result.contains("/out.txt"));
        assert!(result.contains("3 lines"));
    }

    #[test]
    fn format_tool_call_start_glob_includes_pattern() {
        let input = r#"{"pattern": "**/*.rs", "path": "."}"#;
        let result = format_tool_call_start("glob_search", input);
        assert!(result.contains("**/*.rs"));
    }

    #[test]
    fn format_tool_call_start_grep_includes_pattern() {
        let input = r#"{"pattern": "fn main", "path": "src/"}"#;
        let result = format_tool_call_start("grep_search", input);
        assert!(result.contains("fn main"));
    }

    #[test]
    fn format_tool_call_start_web_search_shows_query() {
        let input = r#"{"query": "rust async"}"#;
        let result = format_tool_call_start("web_search", input);
        assert!(result.contains("rust async"));
    }

    // ── format_tool_result (bash) ────────────────────────────────────────────

    #[test]
    fn format_tool_result_bash_success_shows_stdout() {
        let output =
            r#"{"stdout": "hello world", "stderr": "", "returnCodeInterpretation": "success"}"#;
        let result = format_tool_result("bash", output, false);
        assert!(result.contains("hello world"));
    }

    #[test]
    fn format_tool_result_bash_error_shows_summary() {
        let result = format_tool_result("bash", "command failed: no such file", true);
        assert!(result.contains("command failed"));
    }

    #[test]
    fn format_tool_result_bash_backgrounded() {
        let output = r#"{"backgroundTaskId": "task-123"}"#;
        let result = format_tool_result("bash", output, false);
        assert!(result.contains("task-123"));
    }

    // ── format_read_result ───────────────────────────────────────────────────

    #[test]
    fn format_tool_result_read_shows_path_and_content() {
        let output = r#"{"file_path": "/src/lib.rs", "startLine": 1, "numLines": 2, "totalLines": 100, "content": "fn foo() {}"}"#;
        let result = format_tool_result("read_file", output, false);
        assert!(result.contains("/src/lib.rs"));
        assert!(result.contains("fn foo() {}"));
    }

    // ── format_write_result ──────────────────────────────────────────────────

    #[test]
    fn format_tool_result_write_shows_path() {
        let output = r#"{"file_path": "/out.txt", "type": "create", "content": "a\nb\nc"}"#;
        let result = format_tool_result("write_file", output, false);
        assert!(result.contains("/out.txt"));
        assert!(result.contains("3 lines"));
    }

    #[test]
    fn format_tool_result_write_updated_type() {
        let output = r#"{"file_path": "/file.rs", "type": "update", "content": "x"}"#;
        let result = format_tool_result("write_file", output, false);
        assert!(result.contains("Updated"));
    }

    // ── format_glob_result ───────────────────────────────────────────────────

    #[test]
    fn format_glob_result_no_files() {
        let parsed = serde_json::json!({"numFiles": 0});
        let result = format_glob_result("✓", &parsed);
        assert!(result.contains("0 files"));
    }

    #[test]
    fn format_glob_result_with_filenames() {
        let parsed = serde_json::json!({
            "numFiles": 2,
            "filenames": ["src/a.rs", "src/b.rs"]
        });
        let result = format_glob_result("✓", &parsed);
        assert!(result.contains("src/a.rs"));
        assert!(result.contains("src/b.rs"));
        assert!(result.contains("2 files"));
    }

    // ── format_grep_result ───────────────────────────────────────────────────

    #[test]
    fn format_grep_result_shows_match_counts() {
        let parsed = serde_json::json!({
            "numMatches": 3,
            "numFiles": 2,
            "content": ""
        });
        let result = format_grep_result("✓", &parsed);
        assert!(result.contains("3 matches"));
        assert!(result.contains("2 files"));
    }

    #[test]
    fn format_grep_result_with_content() {
        let parsed = serde_json::json!({
            "numMatches": 1,
            "numFiles": 1,
            "content": "src/main.rs:10: fn main() {}"
        });
        let result = format_grep_result("✓", &parsed);
        assert!(result.contains("fn main()"));
    }

    #[test]
    fn format_grep_result_with_filenames_no_content() {
        let parsed = serde_json::json!({
            "numMatches": 1,
            "numFiles": 1,
            "filenames": ["src/main.rs"]
        });
        let result = format_grep_result("✓", &parsed);
        assert!(result.contains("src/main.rs"));
    }

    // ── format_generic_tool_result ───────────────────────────────────────────

    #[test]
    fn format_generic_tool_result_string_output() {
        let parsed = serde_json::Value::String("hello".to_string());
        let result = format_generic_tool_result("✓", "my_tool", &parsed);
        assert!(result.contains("my_tool"));
        assert!(result.contains("hello"));
    }

    #[test]
    fn format_generic_tool_result_null_output() {
        let parsed = serde_json::Value::Null;
        let result = format_generic_tool_result("✓", "my_tool", &parsed);
        assert!(result.contains("my_tool"));
    }

    #[test]
    fn format_generic_tool_result_object_output() {
        let parsed = serde_json::json!({"key": "value"});
        let result = format_generic_tool_result("✓", "my_tool", &parsed);
        assert!(result.contains("my_tool"));
        assert!(result.contains("key"));
    }

    #[test]
    fn format_generic_tool_result_multiline_output() {
        let parsed = serde_json::Value::String("line1\nline2".to_string());
        let result = format_generic_tool_result("✓", "my_tool", &parsed);
        // multiline → newline between name and content
        assert!(result.contains('\n'));
    }

    // -- Tests migrated from main.rs ------------------------------------------

    #[test]
    fn tool_rendering_helpers_compact_output() {
        let start = format_tool_call_start("read_file", r#"{"path":"src/main.rs"}"#);
        assert!(start.contains("read_file"));
        assert!(start.contains("src/main.rs"));

        let done = format_tool_result(
            "read_file",
            r#"{"file":{"filePath":"src/main.rs","content":"hello","numLines":1,"startLine":1,"totalLines":1}}"#,
            false,
        );
        assert!(done.contains("Read src/main.rs"));
        assert!(done.contains("hello"));
    }

    #[test]
    fn tool_rendering_truncates_large_read_output_for_display_only() {
        let content = (0..200)
            .map(|index| format!("line {index:03}"))
            .collect::<Vec<_>>()
            .join("\n");
        let output = serde_json::json!({
            "file": {
                "filePath": "src/main.rs",
                "content": content,
                "numLines": 200,
                "startLine": 1,
                "totalLines": 200
            }
        })
        .to_string();

        let rendered = format_tool_result("read_file", &output, false);

        assert!(rendered.contains("line 000"));
        assert!(rendered.contains("line 079"));
        assert!(!rendered.contains("line 199"));
        assert!(rendered.contains("full result preserved in session"));
        assert!(output.contains("line 199"));
    }

    #[test]
    fn tool_rendering_truncates_large_bash_output_for_display_only() {
        let stdout = (0..120)
            .map(|index| format!("stdout {index:03}"))
            .collect::<Vec<_>>()
            .join("\n");
        let output = serde_json::json!({
            "stdout": stdout,
            "stderr": "",
            "returnCodeInterpretation": "completed successfully"
        })
        .to_string();

        let rendered = format_tool_result("bash", &output, false);

        assert!(rendered.contains("stdout 000"));
        assert!(rendered.contains("stdout 059"));
        assert!(!rendered.contains("stdout 119"));
        assert!(rendered.contains("full result preserved in session"));
        assert!(output.contains("stdout 119"));
    }

    #[test]
    fn tool_rendering_truncates_generic_long_output_for_display_only() {
        let items = (0..120)
            .map(|index| format!("payload {index:03}"))
            .collect::<Vec<_>>();
        let output = serde_json::json!({
            "summary": "plugin payload",
            "items": items,
        })
        .to_string();

        let rendered = format_tool_result("plugin_echo", &output, false);

        assert!(rendered.contains("plugin_echo"));
        assert!(rendered.contains("payload 000"));
        assert!(rendered.contains("payload 040"));
        assert!(!rendered.contains("payload 080"));
        assert!(!rendered.contains("payload 119"));
        assert!(rendered.contains("full result preserved in session"));
        assert!(output.contains("payload 119"));
    }

    #[test]
    fn tool_rendering_truncates_raw_generic_output_for_display_only() {
        let output = (0..120)
            .map(|index| format!("raw {index:03}"))
            .collect::<Vec<_>>()
            .join("\n");

        let rendered = format_tool_result("plugin_echo", &output, false);

        assert!(rendered.contains("plugin_echo"));
        assert!(rendered.contains("raw 000"));
        assert!(rendered.contains("raw 059"));
        assert!(!rendered.contains("raw 119"));
        assert!(rendered.contains("full result preserved in session"));
        assert!(output.contains("raw 119"));
    }
}
