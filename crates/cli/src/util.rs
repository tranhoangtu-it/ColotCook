//! Pure utility functions: string manipulation, suggestions, truncation, and shell helpers.

use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::Command;

use colotcook_runtime::{ContentBlock, MessageRole, Session};

// ── Display truncation constants ────────────────────────────────────────────

pub(crate) const DISPLAY_TRUNCATION_NOTICE: &str =
    "\x1b[2m… output truncated for display; full result preserved in session.\x1b[0m";
pub(crate) const READ_DISPLAY_MAX_LINES: usize = 80;
pub(crate) const READ_DISPLAY_MAX_CHARS: usize = 6_000;
pub(crate) const TOOL_OUTPUT_DISPLAY_MAX_LINES: usize = 60;
pub(crate) const TOOL_OUTPUT_DISPLAY_MAX_CHARS: usize = 4_000;

// ── Levenshtein & suggestion helpers ────────────────────────────────────────

/// Compute the Levenshtein edit-distance between two strings.
pub(crate) fn levenshtein_distance(left: &str, right: &str) -> usize {
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
            current[right_index + 1] = (previous[right_index + 1] + 1)
                .min(current[right_index] + 1)
                .min(previous[right_index] + substitution_cost);
        }
        previous.clone_from(&current);
    }

    previous[right_chars.len()]
}

/// Return up to 3 candidates sorted by edit-distance from `input`.
pub(crate) fn ranked_suggestions<'a>(input: &str, candidates: &'a [&'a str]) -> Vec<&'a str> {
    let normalized_input = input.trim_start_matches('/').to_ascii_lowercase();
    let mut ranked = candidates
        .iter()
        .filter_map(|candidate| {
            let normalized_candidate = candidate.trim_start_matches('/').to_ascii_lowercase();
            let distance = levenshtein_distance(&normalized_input, &normalized_candidate);
            let prefix_bonus = usize::from(
                !(normalized_candidate.starts_with(&normalized_input)
                    || normalized_input.starts_with(&normalized_candidate)),
            );
            let score = distance + prefix_bonus;
            (score <= 4).then_some((score, *candidate))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| left.cmp(right).then_with(|| left.1.cmp(right.1)));
    ranked
        .into_iter()
        .map(|(_, candidate)| candidate)
        .take(3)
        .collect()
}

/// Return the single closest candidate, if any.
pub(crate) fn suggest_closest_term<'a>(input: &str, candidates: &'a [&'a str]) -> Option<&'a str> {
    ranked_suggestions(input, candidates).into_iter().next()
}

/// Format a "Did you mean …?" style suggestion line.
pub(crate) fn render_suggestion_line(label: &str, suggestions: &[String]) -> Option<String> {
    (!suggestions.is_empty()).then(|| format!("  {label:<16} {}", suggestions.join(", "),))
}

// ── Text manipulation ───────────────────────────────────────────────────────

/// Indent every line of `value` by `spaces` spaces.
pub(crate) fn indent_block(value: &str, spaces: usize) -> String {
    let indent = " ".repeat(spaces);
    value
        .lines()
        .map(|line| format!("{indent}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Truncate to `limit` chars for inclusion in prompts.
pub(crate) fn truncate_for_prompt(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        value.trim().to_string()
    } else {
        let truncated = value.chars().take(limit).collect::<String>();
        format!("{}\n…[truncated]", truncated.trim_end())
    }
}

/// Truncate to `limit` chars for short summaries.
pub(crate) fn truncate_for_summary(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

/// Truncate output for display with line + char limits.
pub(crate) fn truncate_output_for_display(
    content: &str,
    max_lines: usize,
    max_chars: usize,
) -> String {
    let original = content.trim_end_matches('\n');
    if original.is_empty() {
        return String::new();
    }

    let mut preview_lines = Vec::new();
    let mut used_chars = 0usize;
    let mut truncated = false;

    for (index, line) in original.lines().enumerate() {
        if index >= max_lines {
            truncated = true;
            break;
        }

        let newline_cost = usize::from(!preview_lines.is_empty());
        let available = max_chars.saturating_sub(used_chars + newline_cost);
        if available == 0 {
            truncated = true;
            break;
        }

        let line_chars = line.chars().count();
        if line_chars > available {
            preview_lines.push(line.chars().take(available).collect::<String>());
            truncated = true;
            break;
        }

        preview_lines.push(line.to_string());
        used_chars += newline_cost + line_chars;
    }

    let mut preview = preview_lines.join("\n");
    if truncated {
        if !preview.is_empty() {
            preview.push('\n');
        }
        preview.push_str(DISPLAY_TRUNCATION_NOTICE);
    }
    preview
}

/// Return the first non-blank line from `text`.
pub(crate) fn first_visible_line(text: &str) -> &str {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(text)
}

/// Strip fences and normalize newlines from AI-generated text.
#[allow(dead_code)] // Used by parse_titled_body for AI-generated content normalization
pub(crate) fn sanitize_generated_message(value: &str) -> String {
    value.trim().trim_matches('`').trim().replace("\r\n", "\n")
}

/// Parse "TITLE: …\nBODY: …" from AI-generated commit/PR text.
#[allow(dead_code)] // Used in AI-assisted commit/PR workflows, not all paths active
pub(crate) fn parse_titled_body(value: &str) -> Option<(String, String)> {
    let normalized = sanitize_generated_message(value);
    let title = normalized
        .lines()
        .find_map(|line| line.strip_prefix("TITLE:").map(str::trim))?;
    let body_start = normalized.find("BODY:")?;
    let body = normalized[body_start + "BODY:".len()..].trim();
    Some((title.to_string(), body.to_string()))
}

// ── File & process helpers ──────────────────────────────────────────────────

/// Write a temporary text file and return its path.
#[allow(dead_code)] // Used in prompt generation helpers, not all paths active
pub(crate) fn write_temp_text_file(
    filename: &str,
    contents: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let path = env::temp_dir().join(filename);
    fs::write(&path, contents)?;
    Ok(path)
}

/// Extract the last N user text messages from a session for context.
#[allow(dead_code)] // Used in prompt context building, not all code paths active
pub(crate) fn recent_user_context(session: &Session, limit: usize) -> String {
    let requests = session
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .filter_map(|message| {
            message.blocks.iter().find_map(|block| match block {
                ContentBlock::Text { text } => Some(text.trim().to_string()),
                _ => None,
            })
        })
        .rev()
        .take(limit)
        .collect::<Vec<_>>();

    if requests.is_empty() {
        "<no prior user messages>".to_string()
    } else {
        requests
            .into_iter()
            .rev()
            .enumerate()
            .map(|(index, text)| format!("{}. {}", index + 1, text))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Extract the file path from a tool-call JSON payload.
pub(crate) fn extract_tool_path(parsed: &serde_json::Value) -> String {
    parsed
        .get("file_path")
        .or_else(|| parsed.get("filePath"))
        .or_else(|| parsed.get("path"))
        .and_then(|value| value.as_str())
        .unwrap_or("?")
        .to_string()
}

/// Compact a JSON tool payload into a single-line summary.
pub(crate) fn summarize_tool_payload(payload: &str) -> String {
    let compact = match serde_json::from_str::<serde_json::Value>(payload) {
        Ok(value) => value.to_string(),
        Err(_) => payload.trim().to_string(),
    };
    truncate_for_summary(&compact, 96)
}

/// Check whether a CLI program exists on PATH.
#[allow(dead_code)] // Used in pre-flight checks that may not be active in all code paths
pub(crate) fn command_exists(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Run a git command in the current directory and return stdout.
pub(crate) fn git_output(args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(env::current_dir()?)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("git {} failed: {stderr}", args.join(" ")).into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

/// Run a git command and check it exits successfully.
#[allow(dead_code)] // Used in git workflow helpers, may not be called in all code paths
pub(crate) fn git_status_ok(args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(env::current_dir()?)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("git {} failed: {stderr}", args.join(" ")).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── levenshtein_distance ─────────────────────────────────────────────────

    #[test]
    fn levenshtein_same_string_is_zero() {
        assert_eq!(levenshtein_distance("hello", "hello"), 0);
    }

    #[test]
    fn levenshtein_empty_left_is_right_len() {
        assert_eq!(levenshtein_distance("", "abc"), 3);
    }

    #[test]
    fn levenshtein_empty_right_is_left_len() {
        assert_eq!(levenshtein_distance("abc", ""), 3);
    }

    #[test]
    fn levenshtein_both_empty_is_zero() {
        assert_eq!(levenshtein_distance("", ""), 0);
    }

    #[test]
    fn levenshtein_single_substitution() {
        assert_eq!(levenshtein_distance("cat", "bat"), 1);
    }

    #[test]
    fn levenshtein_insertion() {
        assert_eq!(levenshtein_distance("cat", "cats"), 1);
    }

    #[test]
    fn levenshtein_deletion() {
        assert_eq!(levenshtein_distance("cats", "cat"), 1);
    }

    #[test]
    fn levenshtein_completely_different() {
        assert_eq!(levenshtein_distance("abc", "xyz"), 3);
    }

    #[test]
    fn levenshtein_unicode_chars() {
        // Both strings have 3 chars, same content → distance 0
        assert_eq!(levenshtein_distance("日本語", "日本語"), 0);
    }

    // ── ranked_suggestions ──────────────────────────────────────────────────

    #[test]
    fn ranked_suggestions_exact_match_first() {
        let candidates = &["help", "hello", "heap"][..];
        let results = ranked_suggestions("help", candidates);
        assert_eq!(results[0], "help");
    }

    #[test]
    fn ranked_suggestions_empty_candidates() {
        let results = ranked_suggestions("help", &[]);
        assert!(results.is_empty());
    }

    #[test]
    fn ranked_suggestions_max_three_results() {
        let candidates = &["help", "heap", "heal", "heat", "hear"][..];
        let results = ranked_suggestions("help", candidates);
        assert!(results.len() <= 3);
    }

    #[test]
    fn ranked_suggestions_slash_prefix_normalized() {
        let candidates = &["help"][..];
        let results = ranked_suggestions("/help", candidates);
        assert!(!results.is_empty());
    }

    #[test]
    fn ranked_suggestions_case_insensitive() {
        let candidates = &["Help"][..];
        let results = ranked_suggestions("help", candidates);
        assert!(!results.is_empty());
    }

    #[test]
    fn ranked_suggestions_no_match_beyond_threshold() {
        // "zzz" is more than 4 edits away from "abc"
        let candidates = &["abc"][..];
        let results = ranked_suggestions("zzzzz", candidates);
        assert!(results.is_empty());
    }

    // ── suggest_closest_term ────────────────────────────────────────────────

    #[test]
    fn suggest_closest_term_returns_first_match() {
        let candidates = &["help", "heap"][..];
        let result = suggest_closest_term("help", candidates);
        assert_eq!(result, Some("help"));
    }

    #[test]
    fn suggest_closest_term_returns_none_when_no_match() {
        let candidates = &["abc"][..];
        let result = suggest_closest_term("zzzzz", candidates);
        assert!(result.is_none());
    }

    #[test]
    fn suggest_closest_term_empty_candidates_returns_none() {
        let result = suggest_closest_term("help", &[]);
        assert!(result.is_none());
    }

    // ── truncate_for_summary ────────────────────────────────────────────────

    #[test]
    fn truncate_for_summary_under_limit_unchanged() {
        assert_eq!(truncate_for_summary("hello", 10), "hello");
    }

    #[test]
    fn truncate_for_summary_at_limit_unchanged() {
        assert_eq!(truncate_for_summary("hello", 5), "hello");
    }

    #[test]
    fn truncate_for_summary_over_limit_adds_ellipsis() {
        let result = truncate_for_summary("hello world", 5);
        assert_eq!(result, "hello…");
    }

    #[test]
    fn truncate_for_summary_empty_string() {
        assert_eq!(truncate_for_summary("", 10), "");
    }

    #[test]
    fn truncate_for_summary_limit_zero() {
        let result = truncate_for_summary("hello", 0);
        // 0 chars taken, and there's still more → "…"
        assert_eq!(result, "…");
    }

    // ── truncate_for_prompt ─────────────────────────────────────────────────

    #[test]
    fn truncate_for_prompt_under_limit_trimmed() {
        assert_eq!(truncate_for_prompt("  hello  ", 20), "hello");
    }

    #[test]
    fn truncate_for_prompt_over_limit_adds_marker() {
        let result = truncate_for_prompt("hello world", 5);
        assert!(result.ends_with("…[truncated]"));
    }

    #[test]
    fn truncate_for_prompt_exact_limit() {
        assert_eq!(truncate_for_prompt("hello", 5), "hello");
    }

    // ── truncate_output_for_display ─────────────────────────────────────────

    #[test]
    fn truncate_output_for_display_empty_returns_empty() {
        assert_eq!(truncate_output_for_display("", 10, 100), "");
    }

    #[test]
    fn truncate_output_for_display_only_newlines_returns_empty() {
        assert_eq!(truncate_output_for_display("\n\n\n", 10, 100), "");
    }

    #[test]
    fn truncate_output_for_display_under_limits_unchanged() {
        let content = "line1\nline2\nline3";
        let result = truncate_output_for_display(content, 10, 1000);
        assert_eq!(result, "line1\nline2\nline3");
    }

    #[test]
    fn truncate_output_for_display_line_limit_triggers_truncation() {
        let content = "a\nb\nc\nd\ne";
        let result = truncate_output_for_display(content, 2, 1000);
        assert!(result.contains(DISPLAY_TRUNCATION_NOTICE));
        assert!(result.starts_with("a\nb"));
    }

    #[test]
    fn truncate_output_for_display_char_limit_triggers_truncation() {
        let content = "hello world";
        let result = truncate_output_for_display(content, 100, 5);
        assert!(result.contains(DISPLAY_TRUNCATION_NOTICE));
    }

    #[test]
    fn truncate_output_for_display_trailing_newlines_stripped() {
        let content = "hello\n\n\n";
        let result = truncate_output_for_display(content, 100, 1000);
        assert_eq!(result, "hello");
    }

    // ── first_visible_line ──────────────────────────────────────────────────

    #[test]
    fn first_visible_line_returns_first_non_blank() {
        assert_eq!(first_visible_line("\n  \nhello\nworld"), "hello");
    }

    #[test]
    fn first_visible_line_all_blank_returns_original() {
        // unwrap_or(text) path — text itself is returned
        let text = "   \n   ";
        assert_eq!(first_visible_line(text), text);
    }

    #[test]
    fn first_visible_line_single_line() {
        assert_eq!(first_visible_line("hello"), "hello");
    }

    #[test]
    fn first_visible_line_empty_string() {
        assert_eq!(first_visible_line(""), "");
    }

    // ── sanitize_generated_message ──────────────────────────────────────────

    #[test]
    fn sanitize_generated_message_trims_whitespace() {
        assert_eq!(sanitize_generated_message("  hello  "), "hello");
    }

    #[test]
    fn sanitize_generated_message_strips_backticks() {
        assert_eq!(sanitize_generated_message("`hello`"), "hello");
    }

    #[test]
    fn sanitize_generated_message_normalizes_crlf() {
        assert_eq!(sanitize_generated_message("a\r\nb"), "a\nb");
    }

    #[test]
    fn sanitize_generated_message_plain_text_unchanged() {
        assert_eq!(sanitize_generated_message("hello world"), "hello world");
    }

    // ── parse_titled_body ───────────────────────────────────────────────────

    #[test]
    fn parse_titled_body_valid_input() {
        let input = "TITLE: My Title\nBODY: My body text";
        let result = parse_titled_body(input);
        assert_eq!(
            result,
            Some(("My Title".to_string(), "My body text".to_string()))
        );
    }

    #[test]
    fn parse_titled_body_missing_title_returns_none() {
        let input = "BODY: Some body";
        assert!(parse_titled_body(input).is_none());
    }

    #[test]
    fn parse_titled_body_missing_body_returns_none() {
        let input = "TITLE: Some title";
        assert!(parse_titled_body(input).is_none());
    }

    #[test]
    fn parse_titled_body_multiline_body() {
        let input = "TITLE: T\nBODY: line1\nline2";
        let result = parse_titled_body(input);
        assert!(result.is_some());
        let (title, body) = result.unwrap();
        assert_eq!(title, "T");
        assert!(body.contains("line1"));
    }

    // ── extract_tool_path ───────────────────────────────────────────────────

    #[test]
    fn extract_tool_path_file_path_key() {
        let value = serde_json::json!({"file_path": "/foo/bar.rs"});
        assert_eq!(extract_tool_path(&value), "/foo/bar.rs");
    }

    #[test]
    fn extract_tool_path_camel_case_key() {
        let value = serde_json::json!({"filePath": "/foo/bar.rs"});
        assert_eq!(extract_tool_path(&value), "/foo/bar.rs");
    }

    #[test]
    fn extract_tool_path_path_key() {
        let value = serde_json::json!({"path": "/foo/bar.rs"});
        assert_eq!(extract_tool_path(&value), "/foo/bar.rs");
    }

    #[test]
    fn extract_tool_path_no_path_key_returns_question_mark() {
        let value = serde_json::json!({"command": "ls"});
        assert_eq!(extract_tool_path(&value), "?");
    }

    #[test]
    fn extract_tool_path_file_path_takes_priority_over_path() {
        let value = serde_json::json!({"file_path": "/primary", "path": "/secondary"});
        assert_eq!(extract_tool_path(&value), "/primary");
    }

    // ── summarize_tool_payload ──────────────────────────────────────────────

    #[test]
    fn summarize_tool_payload_valid_json_compacted() {
        let payload = r#"{"command": "ls -la"}"#;
        let result = summarize_tool_payload(payload);
        assert!(result.contains("ls -la"));
    }

    #[test]
    fn summarize_tool_payload_invalid_json_returned_trimmed() {
        let payload = "  not json  ";
        assert_eq!(summarize_tool_payload(payload), "not json");
    }

    #[test]
    fn summarize_tool_payload_long_payload_truncated() {
        let long = "x".repeat(200);
        let payload = format!(r#"{{"key": "{long}"}}"#);
        let result = summarize_tool_payload(&payload);
        assert!(result.ends_with('…'));
    }

    // ── command_exists ──────────────────────────────────────────────────────

    #[test]
    fn command_exists_known_command_returns_true() {
        assert!(command_exists("ls"));
    }

    #[test]
    fn command_exists_nonexistent_command_returns_false() {
        assert!(!command_exists("__nonexistent_command_xyz_123__"));
    }

    // ── render_suggestion_line ──────────────────────────────────────────────

    #[test]
    fn render_suggestion_line_empty_suggestions_returns_none() {
        let result = render_suggestion_line("Hint", &[]);
        assert!(result.is_none());
    }

    #[test]
    fn render_suggestion_line_one_suggestion() {
        let suggestions = vec!["help".to_string()];
        let result = render_suggestion_line("Did you mean", &suggestions);
        assert!(result.is_some());
        let line = result.unwrap();
        assert!(line.contains("help"));
        assert!(line.contains("Did you mean"));
    }

    #[test]
    fn render_suggestion_line_multiple_suggestions_joined() {
        let suggestions = vec!["help".to_string(), "hello".to_string(), "heap".to_string()];
        let result = render_suggestion_line("Suggestions", &suggestions);
        assert!(result.is_some());
        let line = result.unwrap();
        assert!(line.contains("help"));
        assert!(line.contains("hello"));
        assert!(line.contains("heap"));
    }

    // ── indent_block ────────────────────────────────────────────────────────

    #[test]
    fn indent_block_single_line() {
        let result = indent_block("hello", 4);
        assert_eq!(result, "    hello");
    }

    #[test]
    fn indent_block_multiple_lines() {
        let result = indent_block("line1\nline2\nline3", 2);
        assert_eq!(result, "  line1\n  line2\n  line3");
    }

    #[test]
    fn indent_block_zero_spaces() {
        let result = indent_block("hello", 0);
        assert_eq!(result, "hello");
    }

    #[test]
    fn indent_block_empty_string() {
        // Empty string has no lines, so nothing to indent
        let result = indent_block("", 4);
        assert_eq!(result, "");
    }

    // ── recent_user_context ────────────────────────────────────────────────

    #[test]
    fn recent_user_context_empty_session_returns_no_prior_messages() {
        use colotcook_runtime::Session;
        let session = Session::new();
        let result = recent_user_context(&session, 5);
        assert_eq!(result, "<no prior user messages>");
    }

    #[test]
    fn recent_user_context_with_messages_returns_numbered_list() {
        use colotcook_runtime::{ContentBlock, ConversationMessage, MessageRole, Session};
        let mut session = Session::new();
        session.messages.push(ConversationMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text {
                text: "Hello world".to_string(),
            }],
            usage: None,
        });
        let result = recent_user_context(&session, 5);
        assert!(result.contains("Hello world"));
        assert!(result.contains("1."));
    }

    #[test]
    fn recent_user_context_limits_to_count() {
        use colotcook_runtime::{ContentBlock, ConversationMessage, MessageRole, Session};
        let mut session = Session::new();
        for i in 1..=5 {
            session.messages.push(ConversationMessage {
                role: MessageRole::User,
                blocks: vec![ContentBlock::Text {
                    text: format!("Message {i}"),
                }],
                usage: None,
            });
        }
        let result = recent_user_context(&session, 2);
        // Should only have 2 most recent messages
        let lines: Vec<_> = result.lines().collect();
        assert_eq!(lines.len(), 2);
    }

    // ── write_temp_text_file ────────────────────────────────────────────────

    #[test]
    fn write_temp_text_file_creates_file_with_content() {
        let path =
            write_temp_text_file("test-temp-file.txt", "hello temp").expect("write temp file");
        let content = std::fs::read_to_string(&path).expect("read temp file");
        assert_eq!(content, "hello temp");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn write_temp_text_file_returns_temp_dir_path() {
        let path = write_temp_text_file("test-dir-check.txt", "data").expect("write temp file");
        let temp_dir = std::env::temp_dir();
        assert!(path.starts_with(temp_dir));
        let _ = std::fs::remove_file(&path);
    }

    // ── open_browser ────────────────────────────────────────────────────────

    #[test]
    fn open_browser_with_valid_url_does_not_panic() {
        // We just test that the function runs without panicking for a valid URL.
        // On CI without a display, it may fail with NotFound which is acceptable.
        let result = open_browser("https://example.com");
        // Either Ok or Err (browser not available in CI) — just not a panic
        let _ = result;
    }

    // ── git_output ──────────────────────────────────────────────────────────

    #[test]
    fn git_output_failing_command_returns_error() {
        let result = git_output(&["__nonexistent_git_subcommand_xyz__"]);
        assert!(result.is_err());
    }

    #[test]
    fn git_output_valid_command_returns_output() {
        // "git --version" is available on any dev machine
        let result = git_output(&["--version"]);
        assert!(result.is_ok());
        assert!(result.unwrap().contains("git"));
    }

    // ── git_status_ok ──────────────────────────────────────────────────────

    #[test]
    fn git_status_ok_valid_command_succeeds() {
        let result = git_status_ok(&["--version"]);
        assert!(result.is_ok());
    }

    #[test]
    fn git_status_ok_invalid_command_errors() {
        let result = git_status_ok(&["__nonexistent_subcommand_xyz__"]);
        assert!(result.is_err());
    }

    // ── levenshtein_distance additional ─────────────────────────────────────

    #[test]
    fn levenshtein_distance_transposition() {
        // "ab" → "ba" is distance 2 (not a transposition in classic Levenshtein)
        assert_eq!(levenshtein_distance("ab", "ba"), 2);
    }

    #[test]
    fn levenshtein_distance_long_strings() {
        let a = "the quick brown fox";
        let b = "the quick brown fox";
        assert_eq!(levenshtein_distance(a, b), 0);
    }

    // ── truncate_output_for_display additional ───────────────────────────────

    #[test]
    fn truncate_output_for_display_exactly_at_line_limit() {
        let content = "a\nb";
        let result = truncate_output_for_display(content, 2, 1000);
        // Exactly 2 lines, no truncation
        assert_eq!(result, "a\nb");
        assert!(!result.contains(DISPLAY_TRUNCATION_NOTICE));
    }

    #[test]
    fn truncate_output_for_display_long_single_line() {
        let content = "x".repeat(100);
        let result = truncate_output_for_display(&content, 100, 50);
        assert!(result.contains(DISPLAY_TRUNCATION_NOTICE));
    }
}

/// Open a URL in the platform default browser.
pub(crate) fn open_browser(url: &str) -> io::Result<()> {
    let commands = if cfg!(target_os = "macos") {
        vec![("open", vec![url])]
    } else if cfg!(target_os = "windows") {
        vec![("cmd", vec!["/C", "start", "", url])]
    } else {
        vec![("xdg-open", vec![url])]
    };
    for (program, args) in commands {
        match Command::new(program).args(args).spawn() {
            Ok(_) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no supported browser opener command found",
    ))
}
