//! Session management: create, resolve, list, and format managed sessions.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use colotcook_runtime::Session;

// ── Constants ───────────────────────────────────────────────────────────────

pub(crate) const PRIMARY_SESSION_EXTENSION: &str = "jsonl";
pub(crate) const LEGACY_SESSION_EXTENSION: &str = "json";
pub(crate) const LATEST_SESSION_REFERENCE: &str = "latest";
pub(crate) const SESSION_REFERENCE_ALIASES: &[&str] = &[LATEST_SESSION_REFERENCE, "last", "recent"];

// ── Types ───────────────────────────────────────────────────────────────────

/// Handle to a session identified by id and file path.
pub(crate) struct SessionHandle {
    pub id: String,
    pub path: PathBuf,
}

/// Summary of a managed session for listing/display.
#[derive(Debug, Clone)]
pub(crate) struct ManagedSessionSummary {
    pub id: String,
    pub path: PathBuf,
    pub modified_epoch_millis: u128,
    pub message_count: usize,
    pub parent_session_id: Option<String>,
    pub branch_name: Option<String>,
}

// ── Functions ───────────────────────────────────────────────────────────────

/// Return (and create if needed) the `.colotcook/sessions/` directory.
pub(crate) fn sessions_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    let path = cwd.join(".colotcook").join("sessions");
    fs::create_dir_all(&path)?;
    Ok(path)
}

/// Create a new session handle for the given id.
pub(crate) fn create_managed_session_handle(
    session_id: &str,
) -> Result<SessionHandle, Box<dyn std::error::Error>> {
    let id = session_id.to_string();
    let path = sessions_dir()?.join(format!("{id}.{PRIMARY_SESSION_EXTENSION}"));
    Ok(SessionHandle { id, path })
}

/// Resolve a CLI reference (alias, path, or session-id) into a `SessionHandle`.
pub(crate) fn resolve_session_reference(
    reference: &str,
) -> Result<SessionHandle, Box<dyn std::error::Error>> {
    if SESSION_REFERENCE_ALIASES
        .iter()
        .any(|alias| reference.eq_ignore_ascii_case(alias))
    {
        let latest = latest_managed_session()?;
        return Ok(SessionHandle {
            id: latest.id,
            path: latest.path,
        });
    }

    let direct = PathBuf::from(reference);
    let looks_like_path = direct.extension().is_some() || direct.components().count() > 1;
    let path = if direct.exists() {
        direct
    } else if looks_like_path {
        return Err(format_missing_session_reference(reference).into());
    } else {
        resolve_managed_session_path(reference)?
    };
    let id = path
        .file_name()
        .and_then(|value| value.to_str())
        .and_then(|name| {
            name.strip_suffix(&format!(".{PRIMARY_SESSION_EXTENSION}"))
                .or_else(|| name.strip_suffix(&format!(".{LEGACY_SESSION_EXTENSION}")))
        })
        .unwrap_or(reference)
        .to_string();
    Ok(SessionHandle { id, path })
}

/// Resolve a plain session-id to a file path inside the managed sessions directory.
pub(crate) fn resolve_managed_session_path(
    session_id: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let directory = sessions_dir()?;
    for extension in [PRIMARY_SESSION_EXTENSION, LEGACY_SESSION_EXTENSION] {
        let path = directory.join(format!("{session_id}.{extension}"));
        if path.exists() {
            return Ok(path);
        }
    }
    Err(format_missing_session_reference(session_id).into())
}

/// Check whether a path looks like a managed session file.
pub(crate) fn is_managed_session_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|extension| {
            extension == PRIMARY_SESSION_EXTENSION || extension == LEGACY_SESSION_EXTENSION
        })
}

/// List all managed sessions sorted by most-recently-modified first.
pub(crate) fn list_managed_sessions(
) -> Result<Vec<ManagedSessionSummary>, Box<dyn std::error::Error>> {
    let mut sessions = Vec::new();
    for entry in fs::read_dir(sessions_dir()?)? {
        let entry = entry?;
        let path = entry.path();
        if !is_managed_session_file(&path) {
            continue;
        }
        let metadata = entry.metadata()?;
        let modified_epoch_millis = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis())
            .unwrap_or_default();
        let (id, message_count, parent_session_id, branch_name) =
            match Session::load_from_path(&path) {
                Ok(session) => {
                    let parent_session_id = session
                        .fork
                        .as_ref()
                        .map(|fork| fork.parent_session_id.clone());
                    let branch_name = session
                        .fork
                        .as_ref()
                        .and_then(|fork| fork.branch_name.clone());
                    (
                        session.session_id,
                        session.messages.len(),
                        parent_session_id,
                        branch_name,
                    )
                }
                Err(_) => (
                    path.file_stem()
                        .and_then(|value| value.to_str())
                        .unwrap_or("unknown")
                        .to_string(),
                    0,
                    None,
                    None,
                ),
            };
        sessions.push(ManagedSessionSummary {
            id,
            path,
            modified_epoch_millis,
            message_count,
            parent_session_id,
            branch_name,
        });
    }
    sessions.sort_by(|left, right| {
        right
            .modified_epoch_millis
            .cmp(&left.modified_epoch_millis)
            .then_with(|| right.id.cmp(&left.id))
    });
    Ok(sessions)
}

/// Return the most recently modified managed session.
pub(crate) fn latest_managed_session() -> Result<ManagedSessionSummary, Box<dyn std::error::Error>>
{
    list_managed_sessions()?
        .into_iter()
        .next()
        .ok_or_else(|| format_no_managed_sessions().into())
}

/// Error message for a session reference that could not be resolved.
pub(crate) fn format_missing_session_reference(reference: &str) -> String {
    format!(
        "session not found: {reference}\nHint: managed sessions live in .colotcook/sessions/. Try `{LATEST_SESSION_REFERENCE}` for the most recent session or `/session list` in the REPL."
    )
}

/// Error message when no managed sessions exist at all.
pub(crate) fn format_no_managed_sessions() -> String {
    format!(
        "no managed sessions found in .colotcook/sessions/\nStart `colotcook` to create a session, then rerun with `--resume {LATEST_SESSION_REFERENCE}`."
    )
}

/// Render a formatted list of all managed sessions.
pub(crate) fn render_session_list(
    active_session_id: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let sessions = list_managed_sessions()?;
    let mut lines = vec![
        "Sessions".to_string(),
        format!("  Directory         {}", sessions_dir()?.display()),
    ];
    if sessions.is_empty() {
        lines.push("  No managed sessions saved yet.".to_string());
        return Ok(lines.join("\n"));
    }
    for session in sessions {
        let marker = if session.id == active_session_id {
            "● current"
        } else {
            "○ saved"
        };
        let lineage = match (
            session.branch_name.as_deref(),
            session.parent_session_id.as_deref(),
        ) {
            (Some(branch_name), Some(parent_session_id)) => {
                format!(" branch={branch_name} from={parent_session_id}")
            }
            (None, Some(parent_session_id)) => format!(" from={parent_session_id}"),
            (Some(branch_name), None) => format!(" branch={branch_name}"),
            (None, None) => String::new(),
        };
        lines.push(format!(
            "  {id:<20} {marker:<10} msgs={msgs:<4} modified={modified}{lineage} path={path}",
            id = session.id,
            msgs = session.message_count,
            modified = format_session_modified_age(session.modified_epoch_millis),
            lineage = lineage,
            path = session.path.display(),
        ));
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    // ── is_managed_session_file ──────────────────────────────────────────────

    #[test]
    fn is_managed_session_file_jsonl_extension() {
        assert!(is_managed_session_file(Path::new("session.jsonl")));
    }

    #[test]
    fn is_managed_session_file_json_extension() {
        assert!(is_managed_session_file(Path::new("session.json")));
    }

    #[test]
    fn is_managed_session_file_txt_extension_false() {
        assert!(!is_managed_session_file(Path::new("session.txt")));
    }

    #[test]
    fn is_managed_session_file_no_extension_false() {
        assert!(!is_managed_session_file(Path::new("session")));
    }

    #[test]
    fn is_managed_session_file_rs_extension_false() {
        assert!(!is_managed_session_file(Path::new("main.rs")));
    }

    #[test]
    fn is_managed_session_file_full_path_jsonl() {
        assert!(is_managed_session_file(Path::new(
            "/home/user/.colotcook/sessions/abc123.jsonl"
        )));
    }

    // ── format_missing_session_reference ────────────────────────────────────

    #[test]
    fn format_missing_session_reference_contains_reference() {
        let msg = format_missing_session_reference("my-session-id");
        assert!(msg.contains("my-session-id"));
    }

    #[test]
    fn format_missing_session_reference_contains_hint() {
        let msg = format_missing_session_reference("x");
        assert!(msg.contains(".colotcook/sessions/"));
        assert!(msg.contains(LATEST_SESSION_REFERENCE));
    }

    // ── format_no_managed_sessions ──────────────────────────────────────────

    #[test]
    fn format_no_managed_sessions_contains_directory_hint() {
        let msg = format_no_managed_sessions();
        assert!(msg.contains(".colotcook/sessions/"));
    }

    #[test]
    fn format_no_managed_sessions_contains_resume_hint() {
        let msg = format_no_managed_sessions();
        assert!(msg.contains(LATEST_SESSION_REFERENCE));
    }

    // ── format_session_modified_age ──────────────────────────────────────────

    #[test]
    fn format_session_modified_age_just_now_for_zero_delta() {
        // Pass "now" epoch millis so delta is 0 seconds
        let now_millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let result = format_session_modified_age(now_millis);
        assert_eq!(result, "just-now");
    }

    #[test]
    fn format_session_modified_age_seconds_ago() {
        let now_millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        // 30 seconds ago
        let past = now_millis.saturating_sub(30_000);
        let result = format_session_modified_age(past);
        assert!(result.ends_with("s-ago"), "unexpected result: {result}");
    }

    #[test]
    fn format_session_modified_age_minutes_ago() {
        let now_millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        // 5 minutes ago
        let past = now_millis.saturating_sub(5 * 60 * 1_000);
        let result = format_session_modified_age(past);
        assert!(result.ends_with("m-ago"), "unexpected result: {result}");
    }

    #[test]
    fn format_session_modified_age_hours_ago() {
        let now_millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        // 2 hours ago
        let past = now_millis.saturating_sub(2 * 3_600 * 1_000);
        let result = format_session_modified_age(past);
        assert!(result.ends_with("h-ago"), "unexpected result: {result}");
    }

    #[test]
    fn format_session_modified_age_days_ago() {
        let now_millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        // 3 days ago
        let past = now_millis.saturating_sub(3 * 86_400 * 1_000);
        let result = format_session_modified_age(past);
        assert!(result.ends_with("d-ago"), "unexpected result: {result}");
    }

    #[test]
    fn format_session_modified_age_weeks_displayed_as_days() {
        let now_millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        // 2 weeks ago — displayed as Xd-ago
        let past = now_millis.saturating_sub(14 * 86_400 * 1_000);
        let result = format_session_modified_age(past);
        assert!(result.ends_with("d-ago"), "unexpected result: {result}");
    }

    // ── format_session_modified_age boundary cases ───────────────────────────

    #[test]
    fn format_session_modified_age_exactly_4_seconds_is_just_now() {
        let now_millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let past = now_millis.saturating_sub(4_000);
        let result = format_session_modified_age(past);
        assert_eq!(result, "just-now");
    }

    #[test]
    fn format_session_modified_age_exactly_5_seconds_is_seconds_ago() {
        let now_millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let past = now_millis.saturating_sub(5_000);
        let result = format_session_modified_age(past);
        assert_eq!(result, "5s-ago");
    }

    #[test]
    fn format_session_modified_age_59_seconds_is_seconds_ago() {
        let now_millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let past = now_millis.saturating_sub(59_000);
        let result = format_session_modified_age(past);
        assert_eq!(result, "59s-ago");
    }

    #[test]
    fn format_session_modified_age_exactly_1_minute_is_minutes_ago() {
        let now_millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let past = now_millis.saturating_sub(60_000);
        let result = format_session_modified_age(past);
        assert_eq!(result, "1m-ago");
    }

    #[test]
    fn format_session_modified_age_exactly_1_hour_is_hours_ago() {
        let now_millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let past = now_millis.saturating_sub(3_600_000);
        let result = format_session_modified_age(past);
        assert_eq!(result, "1h-ago");
    }

    #[test]
    fn format_session_modified_age_exactly_1_day_is_days_ago() {
        let now_millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let past = now_millis.saturating_sub(86_400_000);
        let result = format_session_modified_age(past);
        assert_eq!(result, "1d-ago");
    }

    // ── SESSION_REFERENCE_ALIASES ─────────────────────────────────────────────

    #[test]
    fn session_reference_aliases_contains_latest() {
        assert!(SESSION_REFERENCE_ALIASES.contains(&LATEST_SESSION_REFERENCE));
    }

    #[test]
    fn session_reference_aliases_contains_last() {
        assert!(SESSION_REFERENCE_ALIASES.contains(&"last"));
    }

    #[test]
    fn session_reference_aliases_contains_recent() {
        assert!(SESSION_REFERENCE_ALIASES.contains(&"recent"));
    }

    // ── is_managed_session_file with various paths ─────────────────────────

    #[test]
    fn is_managed_session_file_uppercase_extension_false() {
        // Extension matching is case-sensitive
        assert!(!is_managed_session_file(Path::new("session.JSONL")));
    }

    #[test]
    fn is_managed_session_file_double_extension_last_wins() {
        // Path::extension returns the last extension
        assert!(!is_managed_session_file(Path::new("session.jsonl.bak")));
    }

    // ── format_missing_session_reference variants ────────────────────────────

    #[test]
    fn format_missing_session_reference_contains_session_list_hint() {
        let msg = format_missing_session_reference("my-sess");
        assert!(msg.contains("/session list") || msg.contains("session list"));
    }

    // ── format_no_managed_sessions variants ─────────────────────────────────

    #[test]
    fn format_no_managed_sessions_contains_colotcook_command() {
        let msg = format_no_managed_sessions();
        assert!(msg.contains("colotcook"));
    }

    // ── sessions_dir integration ─────────────────────────────────────────────

    #[test]
    fn sessions_dir_creates_directory_if_not_exists() {
        // This just verifies sessions_dir() works without error
        let result = sessions_dir();
        assert!(result.is_ok());
        let path = result.unwrap();
        assert!(path.ends_with("sessions"));
    }

    // ── create_managed_session_handle ────────────────────────────────────────

    #[test]
    fn create_managed_session_handle_has_correct_id() {
        let handle = create_managed_session_handle("test-sess-001").expect("handle creation");
        assert_eq!(handle.id, "test-sess-001");
    }

    #[test]
    fn create_managed_session_handle_path_has_jsonl_extension() {
        let handle = create_managed_session_handle("test-sess-002").expect("handle creation");
        assert!(handle
            .path
            .to_string_lossy()
            .ends_with("test-sess-002.jsonl"));
    }

    // ── PRIMARY_SESSION_EXTENSION and LEGACY_SESSION_EXTENSION ──────────────

    #[test]
    fn primary_session_extension_is_jsonl() {
        assert_eq!(PRIMARY_SESSION_EXTENSION, "jsonl");
    }

    #[test]
    fn legacy_session_extension_is_json() {
        assert_eq!(LEGACY_SESSION_EXTENSION, "json");
    }
}

/// Format a human-readable age string from an epoch-millis timestamp.
pub(crate) fn format_session_modified_age(modified_epoch_millis: u128) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map_or(modified_epoch_millis, |duration| duration.as_millis());
    let delta_seconds = now
        .saturating_sub(modified_epoch_millis)
        .checked_div(1_000)
        .unwrap_or_default();
    match delta_seconds {
        0..=4 => "just-now".to_string(),
        5..=59 => format!("{delta_seconds}s-ago"),
        60..=3_599 => format!("{}m-ago", delta_seconds / 60),
        3_600..=86_399 => format!("{}h-ago", delta_seconds / 3_600),
        _ => format!("{}d-ago", delta_seconds / 86_400),
    }
}
