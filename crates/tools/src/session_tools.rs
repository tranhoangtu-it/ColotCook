//! Session task management tools (`TodoWrite`) and `Skill` loading.

use crate::types::{
    SkillInput, SkillOutput, TodoItem, TodoStatus, TodoWriteInput, TodoWriteOutput,
};

pub(crate) fn execute_todo_write(input: TodoWriteInput) -> Result<TodoWriteOutput, String> {
    validate_todos(&input.todos)?;
    let store_path = todo_store_path()?;
    let old_todos = if store_path.exists() {
        serde_json::from_str::<Vec<TodoItem>>(
            &std::fs::read_to_string(&store_path).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?
    } else {
        Vec::new()
    };

    let all_done = input
        .todos
        .iter()
        .all(|todo| matches!(todo.status, TodoStatus::Completed));
    let persisted = if all_done {
        Vec::new()
    } else {
        input.todos.clone()
    };

    if let Some(parent) = store_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(
        &store_path,
        serde_json::to_string_pretty(&persisted).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;

    let verification_nudge_needed = (all_done
        && input.todos.len() >= 3
        && !input
            .todos
            .iter()
            .any(|todo| todo.content.to_lowercase().contains("verif")))
    .then_some(true);

    Ok(TodoWriteOutput {
        old_todos,
        new_todos: input.todos,
        verification_nudge_needed,
    })
}

pub(crate) fn execute_skill(input: SkillInput) -> Result<SkillOutput, String> {
    let skill_path = resolve_skill_path(&input.skill)?;
    let prompt = std::fs::read_to_string(&skill_path).map_err(|error| error.to_string())?;
    let description = parse_skill_description(&prompt);

    Ok(SkillOutput {
        skill: input.skill,
        path: skill_path.display().to_string(),
        args: input.args,
        description,
        prompt,
    })
}

pub(crate) fn validate_todos(todos: &[TodoItem]) -> Result<(), String> {
    if todos.is_empty() {
        return Err(String::from("todos must not be empty"));
    }
    // Allow multiple in_progress items for parallel workflows
    if todos.iter().any(|todo| todo.content.trim().is_empty()) {
        return Err(String::from("todo content must not be empty"));
    }
    if todos.iter().any(|todo| todo.active_form.trim().is_empty()) {
        return Err(String::from("todo activeForm must not be empty"));
    }
    Ok(())
}

pub(crate) fn todo_store_path() -> Result<std::path::PathBuf, String> {
    if let Ok(path) = std::env::var("COLOTCOOK_TODO_STORE") {
        return Ok(std::path::PathBuf::from(path));
    }
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    Ok(cwd.join(".colotcook-todos.json"))
}

pub(crate) fn resolve_skill_path(skill: &str) -> Result<std::path::PathBuf, String> {
    let requested = skill.trim().trim_start_matches('/').trim_start_matches('$');
    if requested.is_empty() {
        return Err(String::from("skill must not be empty"));
    }

    let mut candidates = Vec::new();
    if let Ok(codex_home) = std::env::var("CODEX_HOME") {
        candidates.push(std::path::PathBuf::from(codex_home).join("skills"));
    }
    if let Ok(home) = std::env::var("HOME") {
        let home = std::path::PathBuf::from(home);
        candidates.push(home.join(".agents").join("skills"));
        candidates.push(home.join(".config").join("opencode").join("skills"));
        candidates.push(home.join(".codex").join("skills"));
    }
    candidates.push(std::path::PathBuf::from("/home/bellman/.codex/skills"));

    for root in candidates {
        let direct = root.join(requested).join("SKILL.md");
        if direct.exists() {
            return Ok(direct);
        }

        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.flatten() {
                let path = entry.path().join("SKILL.md");
                if !path.exists() {
                    continue;
                }
                if entry
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(requested)
                {
                    return Ok(path);
                }
            }
        }
    }

    Err(format!("unknown skill: {requested}"))
}

pub(crate) fn parse_skill_description(contents: &str) -> Option<String> {
    for line in contents.lines() {
        if let Some(value) = line.strip_prefix("description:") {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{TodoItem, TodoStatus};

    fn make_todo(content: &str, active_form: &str, status: TodoStatus) -> TodoItem {
        TodoItem {
            content: content.to_string(),
            active_form: active_form.to_string(),
            status,
        }
    }

    // --- validate_todos ---

    #[test]
    fn validate_todos_empty_list_errors() {
        let result = validate_todos(&[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    #[test]
    fn validate_todos_valid_list_ok() {
        let todos = vec![make_todo("Fix bug", "Fixing the bug", TodoStatus::Pending)];
        assert!(validate_todos(&todos).is_ok());
    }

    #[test]
    fn validate_todos_empty_content_errors() {
        let todos = vec![make_todo("", "Do something", TodoStatus::Pending)];
        let result = validate_todos(&todos);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("content"));
    }

    #[test]
    fn validate_todos_whitespace_only_content_errors() {
        let todos = vec![make_todo("   ", "Do something", TodoStatus::Pending)];
        let result = validate_todos(&todos);
        assert!(result.is_err());
    }

    #[test]
    fn validate_todos_empty_active_form_errors() {
        let todos = vec![make_todo("Fix bug", "", TodoStatus::Pending)];
        let result = validate_todos(&todos);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("activeForm"));
    }

    #[test]
    fn validate_todos_whitespace_active_form_errors() {
        let todos = vec![make_todo("Fix bug", "   ", TodoStatus::Pending)];
        let result = validate_todos(&todos);
        assert!(result.is_err());
    }

    #[test]
    fn validate_todos_multiple_in_progress_allowed() {
        let todos = vec![
            make_todo("Task 1", "Doing task 1", TodoStatus::InProgress),
            make_todo("Task 2", "Doing task 2", TodoStatus::InProgress),
        ];
        assert!(validate_todos(&todos).is_ok());
    }

    #[test]
    fn validate_todos_all_completed_allowed() {
        let todos = vec![
            make_todo("Task 1", "Done 1", TodoStatus::Completed),
            make_todo("Task 2", "Done 2", TodoStatus::Completed),
        ];
        assert!(validate_todos(&todos).is_ok());
    }

    // --- todo_store_path ---

    #[test]
    fn todo_store_path_env_override() {
        std::env::set_var("COLOTCOOK_TODO_STORE", "/tmp/my-todos.json");
        let path = todo_store_path().unwrap();
        assert_eq!(path, std::path::PathBuf::from("/tmp/my-todos.json"));
        std::env::remove_var("COLOTCOOK_TODO_STORE");
    }

    #[test]
    fn todo_store_path_default_uses_cwd() {
        std::env::remove_var("COLOTCOOK_TODO_STORE");
        let path = todo_store_path().unwrap();
        assert!(
            path.to_string_lossy().contains(".colotcook-todos.json"),
            "path was: {}",
            path.display()
        );
    }

    // --- parse_skill_description ---

    #[test]
    fn parse_skill_description_found() {
        let contents = "name: my-skill\ndescription: Does cool things\nother: value";
        let desc = parse_skill_description(contents);
        assert_eq!(desc, Some(String::from("Does cool things")));
    }

    #[test]
    fn parse_skill_description_not_found() {
        let contents = "name: my-skill\nother: value";
        assert_eq!(parse_skill_description(contents), None);
    }

    #[test]
    fn parse_skill_description_empty_value_skipped() {
        let contents = "description:   \nname: my-skill";
        assert_eq!(parse_skill_description(contents), None);
    }

    #[test]
    fn parse_skill_description_trims_whitespace() {
        let contents = "description:   Trimmed value  ";
        let desc = parse_skill_description(contents).unwrap();
        assert_eq!(desc, "Trimmed value");
    }

    // --- execute_todo_write with temp store ---

    fn temp_todo_path(suffix: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("colotcook-test-todos-{suffix}.json"))
    }

    #[test]
    fn execute_todo_write_all_completed_clears_store() {
        let path = temp_todo_path("completed");
        let path_str = path.to_string_lossy().to_string();
        std::env::set_var("COLOTCOOK_TODO_STORE", &path_str);

        let todos = vec![
            make_todo("Task 1", "Done 1", TodoStatus::Completed),
            make_todo("Task 2", "Done 2", TodoStatus::Completed),
            make_todo("Task 3", "Done 3", TodoStatus::Completed),
        ];
        let input = crate::types::TodoWriteInput { todos };
        let result = execute_todo_write(input).unwrap();

        // When all completed, persisted list should be empty
        let written = std::fs::read_to_string(&path).unwrap();
        let persisted: Vec<TodoItem> = serde_json::from_str(&written).unwrap();
        assert!(persisted.is_empty());
        assert_eq!(result.new_todos.len(), 3);

        std::env::remove_var("COLOTCOOK_TODO_STORE");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn execute_todo_write_pending_todos_persisted() {
        let path = temp_todo_path("pending");
        let path_str = path.to_string_lossy().to_string();
        std::env::set_var("COLOTCOOK_TODO_STORE", &path_str);

        let todos = vec![
            make_todo("Task 1", "Do task 1", TodoStatus::Pending),
            make_todo("Task 2", "Do task 2", TodoStatus::InProgress),
        ];
        let input = crate::types::TodoWriteInput {
            todos: todos.clone(),
        };
        let result = execute_todo_write(input).unwrap();

        let written = std::fs::read_to_string(&path).unwrap();
        let persisted: Vec<TodoItem> = serde_json::from_str(&written).unwrap();
        assert_eq!(persisted.len(), 2);
        assert!(result.verification_nudge_needed.is_none());

        std::env::remove_var("COLOTCOOK_TODO_STORE");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn execute_todo_write_empty_todos_errors() {
        let result = execute_todo_write(crate::types::TodoWriteInput { todos: vec![] });
        assert!(result.is_err());
    }

    #[test]
    fn execute_todo_write_verification_nudge_when_all_done_no_verify_word() {
        let path = temp_todo_path("nudge");
        let path_str = path.to_string_lossy().to_string();
        std::env::set_var("COLOTCOOK_TODO_STORE", &path_str);

        let todos = vec![
            make_todo("Task 1", "Done 1", TodoStatus::Completed),
            make_todo("Task 2", "Done 2", TodoStatus::Completed),
            make_todo("Task 3", "Done 3", TodoStatus::Completed),
        ];
        let input = crate::types::TodoWriteInput { todos };
        let result = execute_todo_write(input).unwrap();

        // Should trigger nudge since all 3+ completed and no "verif" word
        assert_eq!(result.verification_nudge_needed, Some(true));

        std::env::remove_var("COLOTCOOK_TODO_STORE");
        let _ = std::fs::remove_file(&path);
    }
}
