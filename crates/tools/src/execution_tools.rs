//! Execution tools: Notebook editing, `Sleep`, `Brief`/`SendUserMessage`,
//! `REPL`, `PowerShell`, and shell command helpers.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use colotcook_runtime as runtime;
use serde_json::{json, Value};

use crate::agent_tools::iso8601_now;
use crate::types::{
    BriefInput, BriefOutput, BriefStatus, NotebookCellType, NotebookEditInput, NotebookEditMode,
    NotebookEditOutput, PowerShellInput, ReplInput, ReplOutput, ResolvedAttachment, SleepInput,
    SleepOutput,
};

#[allow(clippy::too_many_lines)]
/// Edit a Jupyter notebook cell according to the given input.
pub(crate) fn execute_notebook_edit(
    input: NotebookEditInput,
) -> Result<NotebookEditOutput, String> {
    let path = std::path::PathBuf::from(&input.notebook_path);
    if path.extension().and_then(|ext| ext.to_str()) != Some("ipynb") {
        return Err(String::from(
            "File must be a Jupyter notebook (.ipynb file).",
        ));
    }

    let original_file = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
    let mut notebook: serde_json::Value =
        serde_json::from_str(&original_file).map_err(|error| error.to_string())?;
    let language = notebook
        .get("metadata")
        .and_then(|metadata| metadata.get("kernelspec"))
        .and_then(|kernelspec| kernelspec.get("language"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("python")
        .to_string();
    let cells = notebook
        .get_mut("cells")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| String::from("Notebook cells array not found"))?;

    let edit_mode = input.edit_mode.unwrap_or(NotebookEditMode::Replace);
    let target_index = match input.cell_id.as_deref() {
        Some(cell_id) => Some(resolve_cell_index(cells, Some(cell_id), edit_mode)?),
        None if matches!(
            edit_mode,
            NotebookEditMode::Replace | NotebookEditMode::Delete
        ) =>
        {
            Some(resolve_cell_index(cells, None, edit_mode)?)
        }
        None => None,
    };
    let resolved_cell_type = match edit_mode {
        NotebookEditMode::Delete => None,
        NotebookEditMode::Insert => Some(input.cell_type.unwrap_or(NotebookCellType::Code)),
        NotebookEditMode::Replace => Some(input.cell_type.unwrap_or_else(|| {
            target_index
                .and_then(|index| cells.get(index))
                .and_then(cell_kind)
                .unwrap_or(NotebookCellType::Code)
        })),
    };
    let new_source = require_notebook_source(input.new_source, edit_mode)?;

    let cell_id = match edit_mode {
        NotebookEditMode::Insert => {
            let resolved_cell_type = resolved_cell_type
                .ok_or_else(|| String::from("insert mode requires a cell type"))?;
            let new_id = make_cell_id(cells.len());
            let new_cell = build_notebook_cell(&new_id, resolved_cell_type, &new_source);
            let insert_at = target_index.map_or(cells.len(), |index| index + 1);
            cells.insert(insert_at, new_cell);
            cells
                .get(insert_at)
                .and_then(|cell| cell.get("id"))
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string)
        }
        NotebookEditMode::Delete => {
            let idx = target_index
                .ok_or_else(|| String::from("delete mode requires a target cell index"))?;
            let removed = cells.remove(idx);
            removed
                .get("id")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string)
        }
        NotebookEditMode::Replace => {
            let resolved_cell_type = resolved_cell_type
                .ok_or_else(|| String::from("replace mode requires a cell type"))?;
            let idx = target_index
                .ok_or_else(|| String::from("replace mode requires a target cell index"))?;
            let cell = cells
                .get_mut(idx)
                .ok_or_else(|| String::from("Cell index out of range"))?;
            cell["source"] = serde_json::Value::Array(source_lines(&new_source));
            cell["cell_type"] = serde_json::Value::String(match resolved_cell_type {
                NotebookCellType::Code => String::from("code"),
                NotebookCellType::Markdown => String::from("markdown"),
            });
            match resolved_cell_type {
                NotebookCellType::Code => {
                    if !cell.get("outputs").is_some_and(serde_json::Value::is_array) {
                        cell["outputs"] = json!([]);
                    }
                    if cell.get("execution_count").is_none() {
                        cell["execution_count"] = serde_json::Value::Null;
                    }
                }
                NotebookCellType::Markdown => {
                    if let Some(object) = cell.as_object_mut() {
                        object.remove("outputs");
                        object.remove("execution_count");
                    }
                }
            }
            cell.get("id")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string)
        }
    };

    let updated_file =
        serde_json::to_string_pretty(&notebook).map_err(|error| error.to_string())?;
    std::fs::write(&path, &updated_file).map_err(|error| error.to_string())?;

    Ok(NotebookEditOutput {
        new_source,
        cell_id,
        cell_type: resolved_cell_type,
        language,
        edit_mode: format_notebook_edit_mode(edit_mode),
        error: None,
        notebook_path: path.display().to_string(),
        original_file,
        updated_file,
    })
}

/// Validate that notebook source is non-empty.
pub(crate) fn require_notebook_source(
    source: Option<String>,
    edit_mode: NotebookEditMode,
) -> Result<String, String> {
    match edit_mode {
        NotebookEditMode::Delete => Ok(source.unwrap_or_default()),
        NotebookEditMode::Insert | NotebookEditMode::Replace => source
            .ok_or_else(|| String::from("new_source is required for insert and replace edits")),
    }
}

/// Build a JSON notebook cell object.
pub(crate) fn build_notebook_cell(
    cell_id: &str,
    cell_type: NotebookCellType,
    source: &str,
) -> Value {
    let mut cell = json!({
        "cell_type": match cell_type {
            NotebookCellType::Code => "code",
            NotebookCellType::Markdown => "markdown",
        },
        "id": cell_id,
        "metadata": {},
        "source": source_lines(source),
    });
    if let Some(object) = cell.as_object_mut() {
        match cell_type {
            NotebookCellType::Code => {
                object.insert(String::from("outputs"), json!([]));
                object.insert(String::from("execution_count"), Value::Null);
            }
            NotebookCellType::Markdown => {}
        }
    }
    cell
}

/// Detect the cell type of a JSON notebook cell.
pub(crate) fn cell_kind(cell: &serde_json::Value) -> Option<NotebookCellType> {
    cell.get("cell_type")
        .and_then(serde_json::Value::as_str)
        .map(|kind| {
            if kind == "markdown" {
                NotebookCellType::Markdown
            } else {
                NotebookCellType::Code
            }
        })
}

/// Maximum allowed sleep duration in milliseconds.
pub(crate) const MAX_SLEEP_DURATION_MS: u64 = 300_000;

#[allow(clippy::needless_pass_by_value)]
/// Sleep for the requested duration.
pub(crate) fn execute_sleep(input: SleepInput) -> Result<SleepOutput, String> {
    if input.duration_ms > MAX_SLEEP_DURATION_MS {
        return Err(format!(
            "duration_ms {} exceeds maximum allowed sleep of {MAX_SLEEP_DURATION_MS}ms",
            input.duration_ms,
        ));
    }
    std::thread::sleep(Duration::from_millis(input.duration_ms));
    Ok(SleepOutput {
        duration_ms: input.duration_ms,
        message: format!("Slept for {}ms", input.duration_ms),
    })
}

/// Emit a structured brief message to the user.
pub(crate) fn execute_brief(input: BriefInput) -> Result<BriefOutput, String> {
    if input.message.trim().is_empty() {
        return Err(String::from("message must not be empty"));
    }

    let attachments = input
        .attachments
        .as_ref()
        .map(|paths| {
            paths
                .iter()
                .map(|path| resolve_attachment(path))
                .collect::<Result<Vec<_>, String>>()
        })
        .transpose()?;

    let message = match input.status {
        BriefStatus::Normal | BriefStatus::Proactive => input.message,
    };

    Ok(BriefOutput {
        message,
        attachments,
        sent_at: iso8601_timestamp(),
    })
}

/// Resolve an attachment path to its content.
pub(crate) fn resolve_attachment(path: &str) -> Result<ResolvedAttachment, String> {
    let resolved = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
    let metadata = std::fs::metadata(&resolved).map_err(|error| error.to_string())?;
    Ok(ResolvedAttachment {
        path: resolved.display().to_string(),
        size: metadata.len(),
        is_image: is_image_path(&resolved),
    })
}

/// Return `true` if the path has an image extension.
pub(crate) fn is_image_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg")
    )
}

/// Execute a REPL command in the appropriate language runtime.
pub(crate) fn execute_repl(input: ReplInput) -> Result<ReplOutput, String> {
    if input.code.trim().is_empty() {
        return Err(String::from("code must not be empty"));
    }
    let runtime = resolve_repl_runtime(&input.language)?;
    let started = Instant::now();
    let mut process = Command::new(runtime.program);
    process
        .args(runtime.args)
        .arg(&input.code)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let output = if let Some(timeout_ms) = input.timeout_ms {
        let mut child = process.spawn().map_err(|error| error.to_string())?;
        loop {
            if child
                .try_wait()
                .map_err(|error| error.to_string())?
                .is_some()
            {
                break child
                    .wait_with_output()
                    .map_err(|error| error.to_string())?;
            }
            if started.elapsed() >= Duration::from_millis(timeout_ms) {
                child.kill().map_err(|error| error.to_string())?;
                child
                    .wait_with_output()
                    .map_err(|error| error.to_string())?;
                return Err(format!(
                    "REPL execution exceeded timeout of {timeout_ms} ms"
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    } else {
        process
            .spawn()
            .map_err(|error| error.to_string())?
            .wait_with_output()
            .map_err(|error| error.to_string())?
    };

    Ok(ReplOutput {
        language: input.language,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_code: output.status.code().unwrap_or(1),
        duration_ms: started.elapsed().as_millis(),
    })
}

/// Descriptor for a language REPL runtime.
pub(crate) struct ReplRuntime {
    program: &'static str,
    args: &'static [&'static str],
}

/// Resolve the REPL runtime for a given language.
pub(crate) fn resolve_repl_runtime(language: &str) -> Result<ReplRuntime, String> {
    match language.trim().to_ascii_lowercase().as_str() {
        "python" | "py" => Ok(ReplRuntime {
            program: detect_first_command(&["python3", "python"])
                .ok_or_else(|| String::from("python runtime not found"))?,
            args: &["-c"],
        }),
        "javascript" | "js" | "node" => Ok(ReplRuntime {
            program: detect_first_command(&["node"])
                .ok_or_else(|| String::from("node runtime not found"))?,
            args: &["-e"],
        }),
        "sh" | "shell" | "bash" => Ok(ReplRuntime {
            program: detect_first_command(&["bash", "sh"])
                .ok_or_else(|| String::from("shell runtime not found"))?,
            args: &["-lc"],
        }),
        other => Err(format!("unsupported REPL language: {other}")),
    }
}

/// Return the first available command from `commands`.
pub(crate) fn detect_first_command(commands: &[&'static str]) -> Option<&'static str> {
    commands
        .iter()
        .copied()
        .find(|command| command_exists(command))
}

/// Return the current UTC time as an ISO 8601 string.
pub(crate) fn iso8601_timestamp() -> String {
    if let Ok(output) = Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
    {
        if output.status.success() {
            return String::from_utf8_lossy(&output.stdout).trim().to_string();
        }
    }
    iso8601_now()
}

#[allow(clippy::needless_pass_by_value)]
/// Execute a `PowerShell` script block.
pub(crate) fn execute_powershell(
    input: PowerShellInput,
) -> std::io::Result<runtime::BashCommandOutput> {
    let _ = &input.description;
    let shell = detect_powershell_shell()?;
    execute_shell_command(
        shell,
        &input.command,
        input.timeout,
        input.run_in_background,
    )
}

/// Detect the available `PowerShell` executable.
pub(crate) fn detect_powershell_shell() -> std::io::Result<&'static str> {
    if command_exists("pwsh") {
        Ok("pwsh")
    } else if command_exists("powershell") {
        Ok("powershell")
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "PowerShell executable not found (expected `pwsh` or `powershell` in PATH)",
        ))
    }
}

/// Check whether a command exists on `PATH`.
pub(crate) fn command_exists(command: &str) -> bool {
    std::process::Command::new("sh")
        .arg("-lc")
        .arg(format!("command -v {command} >/dev/null 2>&1"))
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[allow(clippy::too_many_lines)]
/// Execute a shell (bash/sh) command and capture output.
pub(crate) fn execute_shell_command(
    shell: &str,
    command: &str,
    timeout: Option<u64>,
    run_in_background: Option<bool>,
) -> std::io::Result<runtime::BashCommandOutput> {
    if run_in_background.unwrap_or(false) {
        let child = std::process::Command::new(shell)
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-Command")
            .arg(command)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        return Ok(runtime::BashCommandOutput {
            stdout: String::new(),
            stderr: String::new(),
            raw_output_path: None,
            interrupted: false,
            is_image: None,
            background_task_id: Some(child.id().to_string()),
            backgrounded_by_user: Some(true),
            assistant_auto_backgrounded: Some(false),
            dangerously_disable_sandbox: None,
            return_code_interpretation: None,
            no_output_expected: Some(true),
            structured_content: None,
            persisted_output_path: None,
            persisted_output_size: None,
            sandbox_status: None,
        });
    }

    let mut process = std::process::Command::new(shell);
    process
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-Command")
        .arg(command);
    process
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    if let Some(timeout_ms) = timeout {
        let mut child = process.spawn()?;
        let started = Instant::now();
        loop {
            if let Some(status) = child.try_wait()? {
                let output = child.wait_with_output()?;
                return Ok(runtime::BashCommandOutput {
                    stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                    raw_output_path: None,
                    interrupted: false,
                    is_image: None,
                    background_task_id: None,
                    backgrounded_by_user: None,
                    assistant_auto_backgrounded: None,
                    dangerously_disable_sandbox: None,
                    return_code_interpretation: status
                        .code()
                        .filter(|code| *code != 0)
                        .map(|code| format!("exit_code:{code}")),
                    no_output_expected: Some(output.stdout.is_empty() && output.stderr.is_empty()),
                    structured_content: None,
                    persisted_output_path: None,
                    persisted_output_size: None,
                    sandbox_status: None,
                });
            }
            if started.elapsed() >= Duration::from_millis(timeout_ms) {
                let _ = child.kill();
                let output = child.wait_with_output()?;
                let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
                let stderr = if stderr.trim().is_empty() {
                    format!("Command exceeded timeout of {timeout_ms} ms")
                } else {
                    format!(
                        "{}
Command exceeded timeout of {timeout_ms} ms",
                        stderr.trim_end()
                    )
                };
                return Ok(runtime::BashCommandOutput {
                    stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                    stderr,
                    raw_output_path: None,
                    interrupted: true,
                    is_image: None,
                    background_task_id: None,
                    backgrounded_by_user: None,
                    assistant_auto_backgrounded: None,
                    dangerously_disable_sandbox: None,
                    return_code_interpretation: Some(String::from("timeout")),
                    no_output_expected: Some(false),
                    structured_content: None,
                    persisted_output_path: None,
                    persisted_output_size: None,
                    sandbox_status: None,
                });
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    let output = process.output()?;
    Ok(runtime::BashCommandOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        raw_output_path: None,
        interrupted: false,
        is_image: None,
        background_task_id: None,
        backgrounded_by_user: None,
        assistant_auto_backgrounded: None,
        dangerously_disable_sandbox: None,
        return_code_interpretation: output
            .status
            .code()
            .filter(|code| *code != 0)
            .map(|code| format!("exit_code:{code}")),
        no_output_expected: Some(output.stdout.is_empty() && output.stderr.is_empty()),
        structured_content: None,
        persisted_output_path: None,
        persisted_output_size: None,
        sandbox_status: None,
    })
}

/// Find the cell index for the given cell ID.
pub(crate) fn resolve_cell_index(
    cells: &[serde_json::Value],
    cell_id: Option<&str>,
    edit_mode: NotebookEditMode,
) -> Result<usize, String> {
    if cells.is_empty()
        && matches!(
            edit_mode,
            NotebookEditMode::Replace | NotebookEditMode::Delete
        )
    {
        return Err(String::from("Notebook has no cells to edit"));
    }
    if let Some(cell_id) = cell_id {
        cells
            .iter()
            .position(|cell| cell.get("id").and_then(serde_json::Value::as_str) == Some(cell_id))
            .ok_or_else(|| format!("Cell id not found: {cell_id}"))
    } else {
        Ok(cells.len().saturating_sub(1))
    }
}

/// Convert a source string into a JSON array of lines.
pub(crate) fn source_lines(source: &str) -> Vec<serde_json::Value> {
    if source.is_empty() {
        return vec![serde_json::Value::String(String::new())];
    }
    source
        .split_inclusive('\n')
        .map(|line| serde_json::Value::String(line.to_string()))
        .collect()
}

/// Return a human-readable label for a `NotebookEditMode`.
pub(crate) fn format_notebook_edit_mode(mode: NotebookEditMode) -> String {
    match mode {
        NotebookEditMode::Replace => String::from("replace"),
        NotebookEditMode::Insert => String::from("insert"),
        NotebookEditMode::Delete => String::from("delete"),
    }
}

/// Generate a cell ID string for the given index.
pub(crate) fn make_cell_id(index: usize) -> String {
    format!("cell-{}", index + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- make_cell_id ---

    #[test]
    fn make_cell_id_zero() {
        assert_eq!(make_cell_id(0), "cell-1");
    }

    #[test]
    fn make_cell_id_nonzero() {
        assert_eq!(make_cell_id(4), "cell-5");
    }

    // --- format_notebook_edit_mode ---

    #[test]
    fn format_notebook_edit_mode_replace() {
        assert_eq!(
            format_notebook_edit_mode(NotebookEditMode::Replace),
            "replace"
        );
    }

    #[test]
    fn format_notebook_edit_mode_insert() {
        assert_eq!(
            format_notebook_edit_mode(NotebookEditMode::Insert),
            "insert"
        );
    }

    #[test]
    fn format_notebook_edit_mode_delete() {
        assert_eq!(
            format_notebook_edit_mode(NotebookEditMode::Delete),
            "delete"
        );
    }

    // --- source_lines ---

    #[test]
    fn source_lines_empty_source() {
        let lines = source_lines("");
        assert_eq!(lines, vec![json!("")]);
    }

    #[test]
    fn source_lines_single_line() {
        let lines = source_lines("print('hello')");
        assert_eq!(lines, vec![json!("print('hello')")]);
    }

    #[test]
    fn source_lines_multiple_lines() {
        let lines = source_lines("line1\nline2\n");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], json!("line1\n"));
        assert_eq!(lines[1], json!("line2\n"));
    }

    // --- cell_kind ---

    #[test]
    fn cell_kind_code() {
        let cell = json!({"cell_type": "code"});
        assert_eq!(cell_kind(&cell), Some(NotebookCellType::Code));
    }

    #[test]
    fn cell_kind_markdown() {
        let cell = json!({"cell_type": "markdown"});
        assert_eq!(cell_kind(&cell), Some(NotebookCellType::Markdown));
    }

    #[test]
    fn cell_kind_unknown_defaults_to_code() {
        let cell = json!({"cell_type": "raw"});
        assert_eq!(cell_kind(&cell), Some(NotebookCellType::Code));
    }

    #[test]
    fn cell_kind_missing_field() {
        let cell = json!({"source": []});
        assert_eq!(cell_kind(&cell), None);
    }

    // --- build_notebook_cell ---

    #[test]
    fn build_notebook_cell_code() {
        let cell = build_notebook_cell("abc", NotebookCellType::Code, "x = 1");
        assert_eq!(cell["cell_type"], "code");
        assert_eq!(cell["id"], "abc");
        assert!(cell["outputs"].is_array());
        assert!(cell["execution_count"].is_null());
    }

    #[test]
    fn build_notebook_cell_markdown() {
        let cell = build_notebook_cell("md1", NotebookCellType::Markdown, "# Title");
        assert_eq!(cell["cell_type"], "markdown");
        assert!(cell.get("outputs").is_none());
        assert!(cell.get("execution_count").is_none());
    }

    // --- resolve_cell_index ---

    #[test]
    fn resolve_cell_index_empty_cells_replace_errors() {
        let cells: Vec<serde_json::Value> = vec![];
        let result = resolve_cell_index(&cells, None, NotebookEditMode::Replace);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no cells"));
    }

    #[test]
    fn resolve_cell_index_empty_cells_delete_errors() {
        let cells: Vec<serde_json::Value> = vec![];
        let result = resolve_cell_index(&cells, None, NotebookEditMode::Delete);
        assert!(result.is_err());
    }

    #[test]
    fn resolve_cell_index_empty_cells_insert_returns_zero() {
        let cells: Vec<serde_json::Value> = vec![];
        let result = resolve_cell_index(&cells, None, NotebookEditMode::Insert).unwrap();
        assert_eq!(result, 0);
    }

    #[test]
    fn resolve_cell_index_by_id() {
        let cells = vec![
            json!({"id": "cell-1", "cell_type": "code"}),
            json!({"id": "cell-2", "cell_type": "markdown"}),
        ];
        let idx = resolve_cell_index(&cells, Some("cell-2"), NotebookEditMode::Replace).unwrap();
        assert_eq!(idx, 1);
    }

    #[test]
    fn resolve_cell_index_id_not_found_errors() {
        let cells = vec![json!({"id": "cell-1", "cell_type": "code"})];
        let result = resolve_cell_index(&cells, Some("cell-99"), NotebookEditMode::Replace);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Cell id not found"));
    }

    #[test]
    fn resolve_cell_index_no_id_returns_last() {
        let cells = vec![
            json!({"id": "cell-1"}),
            json!({"id": "cell-2"}),
            json!({"id": "cell-3"}),
        ];
        let idx = resolve_cell_index(&cells, None, NotebookEditMode::Replace).unwrap();
        assert_eq!(idx, 2);
    }

    // --- require_notebook_source ---

    #[test]
    fn require_notebook_source_delete_mode_none_ok() {
        let result = require_notebook_source(None, NotebookEditMode::Delete).unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn require_notebook_source_insert_mode_none_errors() {
        let result = require_notebook_source(None, NotebookEditMode::Insert);
        assert!(result.is_err());
    }

    #[test]
    fn require_notebook_source_replace_mode_none_errors() {
        let result = require_notebook_source(None, NotebookEditMode::Replace);
        assert!(result.is_err());
    }

    #[test]
    fn require_notebook_source_insert_with_source_ok() {
        let result =
            require_notebook_source(Some(String::from("code")), NotebookEditMode::Insert).unwrap();
        assert_eq!(result, "code");
    }

    // --- detect_first_command ---

    #[test]
    fn detect_first_command_true_exists() {
        // `true` is universally available on Unix
        let result = detect_first_command(&["true"]);
        assert_eq!(result, Some("true"));
    }

    #[test]
    fn detect_first_command_nonexistent_returns_none() {
        let result = detect_first_command(&["__nonexistent_cmd_12345__"]);
        assert!(result.is_none());
    }

    #[test]
    fn detect_first_command_picks_first_available() {
        let result = detect_first_command(&["__bad__", "true"]);
        assert_eq!(result, Some("true"));
    }

    // --- command_exists ---

    #[test]
    fn command_exists_true_cmd() {
        assert!(command_exists("true"));
    }

    #[test]
    fn command_exists_nonexistent_cmd() {
        assert!(!command_exists("__colotcook_test_nonexistent__"));
    }

    // --- iso8601_timestamp ---

    #[test]
    fn iso8601_timestamp_nonempty() {
        let ts = iso8601_timestamp();
        assert!(!ts.is_empty());
    }

    // --- is_image_path ---

    #[test]
    fn is_image_path_png() {
        assert!(is_image_path(std::path::Path::new("photo.png")));
    }

    #[test]
    fn is_image_path_jpg() {
        assert!(is_image_path(std::path::Path::new("img.jpg")));
    }

    #[test]
    fn is_image_path_svg() {
        assert!(is_image_path(std::path::Path::new("icon.svg")));
    }

    #[test]
    fn is_image_path_txt_not_image() {
        assert!(!is_image_path(std::path::Path::new("readme.txt")));
    }

    #[test]
    fn is_image_path_no_extension() {
        assert!(!is_image_path(std::path::Path::new("Makefile")));
    }

    // --- execute_sleep validation ---

    #[test]
    fn execute_sleep_exceeds_max_errors() {
        let input = SleepInput {
            duration_ms: MAX_SLEEP_DURATION_MS + 1,
        };
        let result = execute_sleep(input);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("exceeds maximum"));
    }

    #[test]
    fn execute_sleep_zero_ok() {
        let input = SleepInput { duration_ms: 0 };
        let result = execute_sleep(input).unwrap();
        assert_eq!(result.duration_ms, 0);
    }

    // --- resolve_repl_runtime ---

    #[test]
    fn resolve_repl_runtime_unsupported_errors() {
        let result = resolve_repl_runtime("cobol");
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.contains("unsupported"), "error was: {err}");
    }

    #[test]
    fn resolve_repl_runtime_python_variants() {
        // py alias — may or may not have python installed, just check no panic
        let _ = resolve_repl_runtime("py");
    }

    #[test]
    fn resolve_repl_runtime_javascript_variants() {
        // js alias — may or may not be installed
        let _ = resolve_repl_runtime("js");
    }

    #[test]
    fn resolve_repl_runtime_bash_variant() {
        // shell alias — bash or sh should be available
        let result = resolve_repl_runtime("shell");
        assert!(
            result.is_ok(),
            "expected bash/sh to be available, got error"
        );
    }
}
