//! Integration tests that exercise the compiled `colotcook` binary.
//!
//! These tests cover CLI flag parsing, error reporting, and slash command
//! dispatch paths that are otherwise only reachable through the binary
//! entry point (main.rs → run()).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use colotcook_runtime::Session;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

// ── Helpers ────────────────────────────────────────────────────────────────

fn colotcook() -> Command {
    Command::new(env!("CARGO_BIN_EXE_colotcook"))
}

fn colotcook_in(cwd: &Path) -> Command {
    let mut cmd = colotcook();
    cmd.current_dir(cwd);
    cmd
}

fn unique_temp_dir(label: &str) -> PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_millis();
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "colotcook-integ-{label}-{}-{millis}-{counter}",
        std::process::id()
    ))
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "expected success.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_failure(output: &Output) {
    assert!(
        !output.status.success(),
        "expected failure.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout should be utf8")
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr should be utf8")
}

fn write_session(root: &Path, label: &str) -> PathBuf {
    let session_path = root.join(format!("{label}.jsonl"));
    let mut session = Session::new();
    session
        .push_user_text(format!("session fixture for {label}"))
        .expect("session write should succeed");
    session
        .save_to_path(&session_path)
        .expect("session should persist");
    session_path
}

// ── Help flag ──────────────────────────────────────────────────────────────

#[test]
fn help_flag_prints_usage_and_exits_zero() {
    let output = colotcook().arg("--help").output().unwrap();
    assert_success(&output);
    let stdout = stdout_of(&output);
    assert!(
        stdout.contains("Usage:"),
        "help should contain Usage section"
    );
    assert!(
        stdout.contains("--model"),
        "help should mention --model flag"
    );
    assert!(
        stdout.contains("--permission-mode"),
        "help should mention --permission-mode flag"
    );
    assert!(
        stdout.contains("Interactive slash commands:"),
        "help should list slash commands"
    );
}

#[test]
fn short_help_flag_also_works() {
    let output = colotcook().arg("-h").output().unwrap();
    assert_success(&output);
    assert!(stdout_of(&output).contains("Usage:"));
}

#[test]
fn help_subcommand_works() {
    let output = colotcook().arg("help").output().unwrap();
    assert_success(&output);
    assert!(stdout_of(&output).contains("Usage:"));
}

// ── Version flag ───────────────────────────────────────────────────────────

#[test]
fn version_flag_prints_version_and_exits_zero() {
    let output = colotcook().arg("--version").output().unwrap();
    assert_success(&output);
    let stdout = stdout_of(&output);
    assert!(
        stdout.contains("ColotCook"),
        "version output should contain product name"
    );
    assert!(
        stdout.contains("Version"),
        "version output should contain Version field"
    );
}

#[test]
fn short_version_flag_also_works() {
    let output = colotcook().arg("-V").output().unwrap();
    assert_success(&output);
    assert!(stdout_of(&output).contains("ColotCook"));
}

#[test]
fn version_subcommand_works() {
    let output = colotcook().arg("version").output().unwrap();
    assert_success(&output);
    assert!(stdout_of(&output).contains("ColotCook"));
}

// ── Unknown flag suggestions ───────────────────────────────────────────────

#[test]
fn unknown_flag_shows_error_and_suggests_closest() {
    let output = colotcook().arg("--modle").output().unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("unknown option: --modle"),
        "should report unknown option"
    );
    assert!(
        stderr.contains("--model"),
        "should suggest --model for --modle typo"
    );
}

#[test]
fn unknown_flag_resum_suggests_resume() {
    let output = colotcook().arg("--resum").output().unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(stderr.contains("--resume"));
}

#[test]
fn unknown_flag_hel_suggests_help() {
    let output = colotcook().arg("--hel").output().unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(stderr.contains("--help"));
}

#[test]
fn unknown_flag_includes_help_usage_hint() {
    let output = colotcook().arg("--foobar").output().unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("colotcook --help"),
        "error should mention colotcook --help"
    );
}

// ── Invalid permission mode ────────────────────────────────────────────────

#[test]
fn invalid_permission_mode_shows_error() {
    let output = colotcook()
        .args(["--permission-mode", "invalid"])
        .output()
        .unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("unsupported permission mode"),
        "should reject invalid permission mode"
    );
}

#[test]
fn permission_mode_read_only_accepted_in_status() {
    let dir = unique_temp_dir("perm-ro");
    fs::create_dir_all(&dir).unwrap();
    let output = colotcook_in(&dir)
        .args(["--permission-mode", "read-only", "status"])
        .output()
        .unwrap();
    assert_success(&output);
    assert!(stdout_of(&output).contains("read-only"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn permission_mode_workspace_write_accepted_in_status() {
    let dir = unique_temp_dir("perm-ws");
    fs::create_dir_all(&dir).unwrap();
    let output = colotcook_in(&dir)
        .args(["--permission-mode", "workspace-write", "status"])
        .output()
        .unwrap();
    assert_success(&output);
    assert!(stdout_of(&output).contains("workspace-write"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn permission_mode_danger_full_access_accepted_in_status() {
    let dir = unique_temp_dir("perm-dfa");
    fs::create_dir_all(&dir).unwrap();
    let output = colotcook_in(&dir)
        .args(["--permission-mode", "danger-full-access", "status"])
        .output()
        .unwrap();
    assert_success(&output);
    assert!(stdout_of(&output).contains("danger-full-access"));
    let _ = fs::remove_dir_all(dir);
}

// ── Model flag ─────────────────────────────────────────────────────────────

#[test]
fn model_flag_reflected_in_status() {
    let dir = unique_temp_dir("model-flag");
    fs::create_dir_all(&dir).unwrap();
    let output = colotcook_in(&dir)
        .args(["--model", "sonnet", "status"])
        .output()
        .unwrap();
    assert_success(&output);
    assert!(stdout_of(&output).contains("claude-sonnet-4-6"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn model_equals_syntax_reflected_in_status() {
    let dir = unique_temp_dir("model-eq");
    fs::create_dir_all(&dir).unwrap();
    let output = colotcook_in(&dir)
        .args(["--model=haiku", "status"])
        .output()
        .unwrap();
    assert_success(&output);
    assert!(stdout_of(&output).contains("claude-haiku"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn custom_model_name_reflected_in_status() {
    let dir = unique_temp_dir("model-custom");
    fs::create_dir_all(&dir).unwrap();
    let output = colotcook_in(&dir)
        .args(["--model", "gpt-4o", "status"])
        .output()
        .unwrap();
    assert_success(&output);
    assert!(stdout_of(&output).contains("gpt-4o"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn missing_model_value_shows_error() {
    let output = colotcook().arg("--model").output().unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(stderr.contains("missing value for --model"));
}

// ── Output format flag ─────────────────────────────────────────────────────

#[test]
fn invalid_output_format_shows_error() {
    let output = colotcook()
        .args(["--output-format", "yaml", "prompt", "hello"])
        .output()
        .unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(stderr.contains("unsupported value for --output-format"));
}

#[test]
fn output_format_equals_syntax_invalid_shows_error() {
    let output = colotcook()
        .args(["--output-format=ndjson", "prompt", "hello"])
        .output()
        .unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(stderr.contains("unsupported value for --output-format"));
}

#[test]
fn missing_output_format_value_shows_error() {
    let output = colotcook().arg("--output-format").output().unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(stderr.contains("missing value for --output-format"));
}

// ── Permission mode missing value ──────────────────────────────────────────

#[test]
fn missing_permission_mode_value_shows_error() {
    let output = colotcook().arg("--permission-mode").output().unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(stderr.contains("missing value for --permission-mode"));
}

// ── Dangerously skip permissions ───────────────────────────────────────────

#[test]
fn dangerously_skip_permissions_sets_danger_full_access() {
    let dir = unique_temp_dir("skip-perms");
    fs::create_dir_all(&dir).unwrap();
    let output = colotcook_in(&dir)
        .args(["--dangerously-skip-permissions", "status"])
        .output()
        .unwrap();
    assert_success(&output);
    assert!(stdout_of(&output).contains("danger-full-access"));
    let _ = fs::remove_dir_all(dir);
}

// ── Slash commands via binary ──────────────────────────────────────────────

#[test]
fn slash_help_command_prints_help() {
    let output = colotcook().arg("/help").output().unwrap();
    assert_success(&output);
    let stdout = stdout_of(&output);
    assert!(stdout.contains("Usage:"));
}

#[test]
fn slash_agents_command_runs_successfully() {
    let dir = unique_temp_dir("agents-cmd");
    fs::create_dir_all(&dir).unwrap();
    let output = colotcook_in(&dir).arg("/agents").output().unwrap();
    // agents command may succeed or fail depending on config, but should not panic
    let combined = format!("{}{}", stdout_of(&output), stderr_of(&output));
    assert!(
        !combined.contains("panicked"),
        "agents command should not panic"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn slash_skills_command_runs_successfully() {
    let dir = unique_temp_dir("skills-cmd");
    fs::create_dir_all(&dir).unwrap();
    let output = colotcook_in(&dir).arg("/skills").output().unwrap();
    let combined = format!("{}{}", stdout_of(&output), stderr_of(&output));
    assert!(
        !combined.contains("panicked"),
        "skills command should not panic"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn unknown_slash_command_outside_repl_shows_error_and_suggests() {
    let output = colotcook().arg("/stats").output().unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(stderr.contains("unknown slash command outside the REPL: /stats"));
    assert!(stderr.contains("Did you mean"));
    assert!(stderr.contains("/status"));
}

#[test]
fn unknown_slash_command_cler_suggests_clear() {
    let output = colotcook().arg("/cler").output().unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(stderr.contains("/clear"));
}

#[test]
fn interactive_only_slash_command_shows_guidance() {
    let output = colotcook().arg("/status").output().unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("interactive-only"),
        "should explain /status is interactive-only"
    );
}

#[test]
fn interactive_only_slash_compact_shows_guidance() {
    let output = colotcook().arg("/compact").output().unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("interactive-only"),
        "should explain /compact is interactive-only"
    );
}

// ── Subcommand aliases ─────────────────────────────────────────────────────

#[test]
fn status_subcommand_works() {
    let dir = unique_temp_dir("status-sub");
    fs::create_dir_all(&dir).unwrap();
    let output = colotcook_in(&dir).arg("status").output().unwrap();
    assert_success(&output);
    assert!(stdout_of(&output).contains("Status"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn sandbox_subcommand_works() {
    let dir = unique_temp_dir("sandbox-sub");
    fs::create_dir_all(&dir).unwrap();
    let output = colotcook_in(&dir).arg("sandbox").output().unwrap();
    assert_success(&output);
    assert!(stdout_of(&output).contains("Sandbox"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn bootstrap_plan_subcommand_prints_phases() {
    let output = colotcook().arg("bootstrap-plan").output().unwrap();
    assert_success(&output);
    // bootstrap-plan prints phase descriptions
    let stdout = stdout_of(&output);
    assert!(!stdout.is_empty(), "bootstrap-plan should produce output");
}

#[test]
fn system_prompt_subcommand_generates_prompt() {
    let dir = unique_temp_dir("sys-prompt");
    fs::create_dir_all(&dir).unwrap();
    let output = colotcook()
        .args([
            "system-prompt",
            "--cwd",
            dir.to_str().unwrap(),
            "--date",
            "2026-01-01",
        ])
        .output()
        .unwrap();
    assert_success(&output);
    let stdout = stdout_of(&output);
    assert!(!stdout.is_empty(), "system-prompt should produce output");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn system_prompt_missing_cwd_value_shows_error() {
    let output = colotcook()
        .args(["system-prompt", "--cwd"])
        .output()
        .unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(stderr.contains("missing value for --cwd"));
}

#[test]
fn system_prompt_missing_date_value_shows_error() {
    let output = colotcook()
        .args(["system-prompt", "--date"])
        .output()
        .unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(stderr.contains("missing value for --date"));
}

#[test]
fn system_prompt_unknown_option_shows_error() {
    let output = colotcook()
        .args(["system-prompt", "--verbose"])
        .output()
        .unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(stderr.contains("unknown system-prompt option"));
}

// ── Dump manifests ─────────────────────────────────────────────────────────

#[test]
fn dump_manifests_exits_with_error() {
    let output = colotcook().arg("dump-manifests").output().unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(stderr.contains("dump-manifests is not available"));
}

// ── Prompt subcommand ──────────────────────────────────────────────────────

#[test]
fn prompt_subcommand_with_empty_prompt_shows_error() {
    let output = colotcook().arg("prompt").output().unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(stderr.contains("prompt subcommand requires a prompt string"));
}

#[test]
fn short_print_flag_with_empty_prompt_shows_error() {
    let output = colotcook().arg("-p").output().unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(stderr.contains("-p requires a prompt string"));
}

// ── Single-word slash command guidance ──────────────────────────────────────

#[test]
fn bare_cost_shows_slash_command_guidance() {
    let output = colotcook().arg("cost").output().unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(stderr.contains("slash command"));
    assert!(stderr.contains("/cost"));
}

#[test]
fn bare_compact_shows_slash_command_guidance() {
    let output = colotcook().arg("compact").output().unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(stderr.contains("slash command"));
}

// ── Resume with slash commands ─────────────────────────────────────────────

#[test]
fn resume_with_status_shows_session_info() {
    let dir = unique_temp_dir("resume-status-integ");
    fs::create_dir_all(&dir).unwrap();
    let session_path = write_session(&dir, "test-session");
    let output = colotcook_in(&dir)
        .args(["--resume", session_path.to_str().unwrap(), "/status"])
        .output()
        .unwrap();
    assert_success(&output);
    let stdout = stdout_of(&output);
    assert!(stdout.contains("Status"));
    assert!(stdout.contains("Messages         1"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn resume_with_version_shows_version_info() {
    let dir = unique_temp_dir("resume-version");
    fs::create_dir_all(&dir).unwrap();
    let session_path = write_session(&dir, "ver-session");
    let output = colotcook_in(&dir)
        .args(["--resume", session_path.to_str().unwrap(), "/version"])
        .output()
        .unwrap();
    assert_success(&output);
    assert!(stdout_of(&output).contains("ColotCook"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn resume_with_cost_shows_cost_report() {
    let dir = unique_temp_dir("resume-cost");
    fs::create_dir_all(&dir).unwrap();
    let session_path = write_session(&dir, "cost-session");
    let output = colotcook_in(&dir)
        .args(["--resume", session_path.to_str().unwrap(), "/cost"])
        .output()
        .unwrap();
    assert_success(&output);
    let stdout = stdout_of(&output);
    assert!(
        stdout.contains("Cost") || stdout.contains("cost") || stdout.contains("$"),
        "cost report should contain cost information"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn resume_with_help_shows_help() {
    let dir = unique_temp_dir("resume-help");
    fs::create_dir_all(&dir).unwrap();
    let session_path = write_session(&dir, "help-session");
    let output = colotcook_in(&dir)
        .args(["--resume", session_path.to_str().unwrap(), "/help"])
        .output()
        .unwrap();
    assert_success(&output);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn resume_with_compact_runs_compaction() {
    let dir = unique_temp_dir("resume-compact");
    fs::create_dir_all(&dir).unwrap();
    let session_path = write_session(&dir, "compact-session");
    let output = colotcook_in(&dir)
        .args(["--resume", session_path.to_str().unwrap(), "/compact"])
        .output()
        .unwrap();
    assert_success(&output);
    let stdout = stdout_of(&output);
    assert!(
        stdout.contains("Compact") || stdout.contains("compact"),
        "compact report should be present"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn resume_with_clear_requires_confirm() {
    let dir = unique_temp_dir("resume-clear-noconfirm");
    fs::create_dir_all(&dir).unwrap();
    let session_path = write_session(&dir, "clear-session");
    let output = colotcook_in(&dir)
        .args(["--resume", session_path.to_str().unwrap(), "/clear"])
        .output()
        .unwrap();
    assert_success(&output);
    let stdout = stdout_of(&output);
    assert!(
        stdout.contains("confirmation required"),
        "clear without --confirm should require confirmation"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn resume_with_clear_confirm_clears_session() {
    let dir = unique_temp_dir("resume-clear-confirm");
    fs::create_dir_all(&dir).unwrap();
    let session_path = write_session(&dir, "clearme");
    let output = colotcook_in(&dir)
        .args([
            "--resume",
            session_path.to_str().unwrap(),
            "/clear",
            "--confirm",
        ])
        .output()
        .unwrap();
    assert_success(&output);
    let stdout = stdout_of(&output);
    assert!(stdout.contains("Cleared"));
    // Verify session was actually cleared
    let restored = Session::load_from_path(&session_path).unwrap();
    assert!(restored.messages.is_empty());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn resume_with_export_writes_transcript() {
    let dir = unique_temp_dir("resume-export");
    fs::create_dir_all(&dir).unwrap();
    let session_path = write_session(&dir, "export-session");
    let export_path = dir.join("exported.txt");
    let output = colotcook_in(&dir)
        .args([
            "--resume",
            session_path.to_str().unwrap(),
            "/export",
            export_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_success(&output);
    let stdout = stdout_of(&output);
    assert!(stdout.contains("Export"));
    assert!(stdout.contains("wrote transcript"));
    assert!(export_path.exists(), "export file should be created");
    let content = fs::read_to_string(&export_path).unwrap();
    assert!(content.contains("# Conversation Export"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn resume_with_multiple_commands_chains() {
    let dir = unique_temp_dir("resume-multi");
    fs::create_dir_all(&dir).unwrap();
    let session_path = write_session(&dir, "multi-session");
    let output = colotcook_in(&dir)
        .args([
            "--resume",
            session_path.to_str().unwrap(),
            "/status",
            "/version",
            "/cost",
        ])
        .output()
        .unwrap();
    assert_success(&output);
    let stdout = stdout_of(&output);
    assert!(stdout.contains("Status"));
    assert!(stdout.contains("colotcook"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn resume_unknown_slash_command_shows_error() {
    let dir = unique_temp_dir("resume-unknown");
    fs::create_dir_all(&dir).unwrap();
    let session_path = write_session(&dir, "unknown-session");
    let output = colotcook_in(&dir)
        .args(["--resume", session_path.to_str().unwrap(), "/foobar"])
        .output()
        .unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(stderr.contains("Unknown slash command") || stderr.contains("unknown"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn resume_without_path_defaults_to_latest() {
    // --resume without a path should try to find latest session
    let dir = unique_temp_dir("resume-no-path");
    let sessions_dir = dir.join(".colotcook").join("sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    let session_path = sessions_dir.join("only-session.jsonl");
    let mut session = Session::new().with_persistence_path(&session_path);
    session.push_user_text("the only session").unwrap();
    session.save_to_path(&session_path).unwrap();

    let output = colotcook_in(&dir)
        .args(["--resume", "/status"])
        .output()
        .unwrap();
    // Should succeed because it finds the latest session
    assert_success(&output);
    let stdout = stdout_of(&output);
    assert!(stdout.contains("Status"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn resume_nonexistent_session_shows_error() {
    let dir = unique_temp_dir("resume-missing");
    fs::create_dir_all(&dir).unwrap();
    let output = colotcook_in(&dir)
        .args(["--resume", "nonexistent-session-id-xyz", "/status"])
        .output()
        .unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(stderr.contains("failed to restore session"));
    let _ = fs::remove_dir_all(dir);
}

// ── Resume with trailing non-slash arguments ───────────────────────────────

#[test]
fn resume_non_slash_trailing_args_shows_error() {
    let output = colotcook()
        .args(["--resume", "session.jsonl", "not-a-command"])
        .output()
        .unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(stderr.contains("trailing arguments must be slash commands"));
}

// ── Allowed tools flag ─────────────────────────────────────────────────────

#[test]
fn allowed_tools_unknown_tool_shows_error() {
    let output = colotcook()
        .args(["--allowedTools", "teleport"])
        .output()
        .unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(stderr.contains("unsupported tool in --allowedTools"));
}

#[test]
fn allowed_tools_equals_syntax_unknown_tool_shows_error() {
    let output = colotcook()
        .args(["--allowed-tools=teleport"])
        .output()
        .unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(stderr.contains("unsupported tool in --allowedTools"));
}

// ── Combined flags ─────────────────────────────────────────────────────────

#[test]
fn model_and_permission_mode_combined_in_status() {
    let dir = unique_temp_dir("combined-flags");
    fs::create_dir_all(&dir).unwrap();
    let output = colotcook_in(&dir)
        .args([
            "--model",
            "opus",
            "--permission-mode",
            "read-only",
            "status",
        ])
        .output()
        .unwrap();
    assert_success(&output);
    let stdout = stdout_of(&output);
    assert!(stdout.contains("claude-opus-4-6"));
    assert!(stdout.contains("read-only"));
    let _ = fs::remove_dir_all(dir);
}

// ── Permission mode with equals syntax ────────────────────────────────────

#[test]
fn permission_mode_equals_syntax_in_status() {
    let dir = unique_temp_dir("perm-eq-syntax");
    fs::create_dir_all(&dir).unwrap();
    let output = colotcook_in(&dir)
        .args(["--permission-mode=workspace-write", "status"])
        .output()
        .unwrap();
    assert_success(&output);
    assert!(stdout_of(&output).contains("workspace-write"));
    let _ = fs::remove_dir_all(dir);
}

// ── Login/logout ───────────────────────────────────────────────────────────
// Note: login and logout tests are omitted because login opens a browser
// and may block indefinitely in a headless test environment.

// ── Init subcommand ────────────────────────────────────────────────────────

#[test]
fn init_subcommand_runs_without_panic() {
    let dir = unique_temp_dir("init-sub");
    fs::create_dir_all(&dir).unwrap();
    let output = colotcook_in(&dir).arg("init").output().unwrap();
    let combined = format!("{}{}", stdout_of(&output), stderr_of(&output));
    assert!(!combined.contains("panicked"), "init should not panic");
    let _ = fs::remove_dir_all(dir);
}

// ── Resume memory command ──────────────────────────────────────────────────

#[test]
fn resume_memory_command_runs() {
    let dir = unique_temp_dir("resume-memory");
    fs::create_dir_all(&dir).unwrap();
    let session_path = write_session(&dir, "mem-session");
    let output = colotcook_in(&dir)
        .args(["--resume", session_path.to_str().unwrap(), "/memory"])
        .output()
        .unwrap();
    // memory command should succeed (possibly with no memory files)
    assert_success(&output);
    let _ = fs::remove_dir_all(dir);
}

// ── Resume config command ──────────────────────────────────────────────────

#[test]
fn resume_config_command_runs() {
    let dir = unique_temp_dir("resume-config-integ");
    fs::create_dir_all(&dir).unwrap();
    let session_path = write_session(&dir, "config-session");
    let output = colotcook_in(&dir)
        .args(["--resume", session_path.to_str().unwrap(), "/config"])
        .output()
        .unwrap();
    assert_success(&output);
    assert!(stdout_of(&output).contains("Config"));
    let _ = fs::remove_dir_all(dir);
}

// ── Resume sandbox command ─────────────────────────────────────────────────

#[test]
fn resume_sandbox_command_runs() {
    let dir = unique_temp_dir("resume-sandbox");
    fs::create_dir_all(&dir).unwrap();
    let session_path = write_session(&dir, "sandbox-session");
    let output = colotcook_in(&dir)
        .args(["--resume", session_path.to_str().unwrap(), "/sandbox"])
        .output()
        .unwrap();
    assert_success(&output);
    assert!(stdout_of(&output).contains("Sandbox"));
    let _ = fs::remove_dir_all(dir);
}

// ── Print flag (-p) ────────────────────────────────────────────────────────

#[test]
fn missing_allowed_tools_value_shows_error() {
    let output = colotcook().arg("--allowedTools").output().unwrap();
    assert_failure(&output);
    let stderr = stderr_of(&output);
    assert!(stderr.contains("missing value for --allowedTools"));
}

// ── Resume diff command in a git repo ──────────────────────────────────────

#[test]
fn resume_diff_command_in_git_repo() {
    let dir = unique_temp_dir("resume-diff-git");
    fs::create_dir_all(&dir).unwrap();
    // Set up a minimal git repo
    Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&dir)
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(&dir)
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&dir)
        .status()
        .unwrap();
    fs::write(dir.join("file.txt"), "hello\n").unwrap();
    Command::new("git")
        .args(["add", "file.txt"])
        .current_dir(&dir)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "init", "--quiet"])
        .current_dir(&dir)
        .status()
        .unwrap();
    // Create a change
    fs::write(dir.join("file.txt"), "hello\nworld\n").unwrap();

    let session_path = write_session(&dir, "diff-session");
    let output = colotcook_in(&dir)
        .args(["--resume", session_path.to_str().unwrap(), "/diff"])
        .output()
        .unwrap();
    assert_success(&output);
    let stdout = stdout_of(&output);
    assert!(stdout.contains("file.txt"));
    let _ = fs::remove_dir_all(dir);
}

// ── Resume init command ────────────────────────────────────────────────────

#[test]
fn resume_init_command_creates_claude_md() {
    let dir = unique_temp_dir("resume-init");
    fs::create_dir_all(&dir).unwrap();
    let session_path = write_session(&dir, "init-session");
    let output = colotcook_in(&dir)
        .args(["--resume", session_path.to_str().unwrap(), "/init"])
        .output()
        .unwrap();
    // init may succeed or show guidance
    assert_success(&output);
    let _ = fs::remove_dir_all(dir);
}

// ── Agents with args ───────────────────────────────────────────────────────

#[test]
fn agents_with_help_arg() {
    let dir = unique_temp_dir("agents-help");
    fs::create_dir_all(&dir).unwrap();
    let output = colotcook_in(&dir)
        .args(["agents", "--help"])
        .output()
        .unwrap();
    // Should run without panic
    let combined = format!("{}{}", stdout_of(&output), stderr_of(&output));
    assert!(!combined.contains("panicked"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn skills_with_help_arg() {
    let dir = unique_temp_dir("skills-help");
    fs::create_dir_all(&dir).unwrap();
    let output = colotcook_in(&dir)
        .args(["skills", "--help"])
        .output()
        .unwrap();
    let combined = format!("{}{}", stdout_of(&output), stderr_of(&output));
    assert!(!combined.contains("panicked"));
    let _ = fs::remove_dir_all(dir);
}

// ── Resume agents/skills commands ──────────────────────────────────────────

#[test]
fn resume_agents_command_runs() {
    let dir = unique_temp_dir("resume-agents");
    fs::create_dir_all(&dir).unwrap();
    let session_path = write_session(&dir, "agents-session");
    let output = colotcook_in(&dir)
        .args(["--resume", session_path.to_str().unwrap(), "/agents"])
        .output()
        .unwrap();
    // agents may succeed or fail depending on config, but shouldn't panic
    let combined = format!("{}{}", stdout_of(&output), stderr_of(&output));
    assert!(!combined.contains("panicked"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn resume_skills_command_runs() {
    let dir = unique_temp_dir("resume-skills");
    fs::create_dir_all(&dir).unwrap();
    let session_path = write_session(&dir, "skills-session");
    let output = colotcook_in(&dir)
        .args(["--resume", session_path.to_str().unwrap(), "/skills"])
        .output()
        .unwrap();
    let combined = format!("{}{}", stdout_of(&output), stderr_of(&output));
    assert!(!combined.contains("panicked"));
    let _ = fs::remove_dir_all(dir);
}

// ── Resume=path syntax ─────────────────────────────────────────────────────

#[test]
fn resume_equals_syntax_works() {
    let dir = unique_temp_dir("resume-eq");
    fs::create_dir_all(&dir).unwrap();
    let session_path = write_session(&dir, "eq-session");
    let flag = format!("--resume={}", session_path.to_str().unwrap());
    let output = colotcook_in(&dir)
        .args([&flag, "/status"])
        .output()
        .unwrap();
    assert_success(&output);
    assert!(stdout_of(&output).contains("Status"));
    let _ = fs::remove_dir_all(dir);
}
