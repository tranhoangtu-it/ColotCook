//! System configuration tools: `Config`, `EnterPlanMode`, `ExitPlanMode`,
//! and `StructuredOutput`.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::types::{
    ConfigInput, ConfigKind, ConfigOutput, ConfigScope, ConfigSettingSpec, ConfigValue,
    EnterPlanModeInput, ExitPlanModeInput, PlanModeOutput, PlanModeState, StructuredOutputInput,
    StructuredOutputResult,
};

/// Get or set a configuration value.
pub(crate) fn execute_config(input: ConfigInput) -> Result<ConfigOutput, String> {
    let setting = input.setting.trim();
    if setting.is_empty() {
        return Err(String::from("setting must not be empty"));
    }
    let Some(spec) = supported_config_setting(setting) else {
        return Ok(ConfigOutput {
            success: false,
            operation: None,
            setting: None,
            value: None,
            previous_value: None,
            new_value: None,
            error: Some(format!("Unknown setting: \"{setting}\"")),
        });
    };

    let path = config_file_for_scope(spec.scope)?;
    let mut document = read_json_object(&path)?;

    if let Some(value) = input.value {
        let normalized = normalize_config_value(spec, value)?;
        let previous_value = get_nested_value(&document, spec.path).cloned();
        set_nested_value(&mut document, spec.path, normalized.clone());
        write_json_object(&path, &document)?;
        Ok(ConfigOutput {
            success: true,
            operation: Some(String::from("set")),
            setting: Some(setting.to_string()),
            value: Some(normalized.clone()),
            previous_value,
            new_value: Some(normalized),
            error: None,
        })
    } else {
        Ok(ConfigOutput {
            success: true,
            operation: Some(String::from("get")),
            setting: Some(setting.to_string()),
            value: get_nested_value(&document, spec.path).cloned(),
            previous_value: None,
            new_value: None,
            error: None,
        })
    }
}

/// JSON path to the default permission mode setting.
pub(crate) const PERMISSION_DEFAULT_MODE_PATH: &[&str] = &["permissions", "defaultMode"];

/// Enter plan mode, storing state and returning the mode output.
pub(crate) fn execute_enter_plan_mode(
    _input: EnterPlanModeInput,
) -> Result<PlanModeOutput, String> {
    let settings_path = config_file_for_scope(ConfigScope::Settings)?;
    let state_path = plan_mode_state_file()?;
    let mut document = read_json_object(&settings_path)?;
    let current_local_mode = get_nested_value(&document, PERMISSION_DEFAULT_MODE_PATH).cloned();
    let current_is_plan =
        matches!(current_local_mode.as_ref(), Some(Value::String(value)) if value == "plan");

    if let Some(state) = read_plan_mode_state(&state_path)? {
        if current_is_plan {
            return Ok(PlanModeOutput {
                success: true,
                operation: String::from("enter"),
                changed: false,
                active: true,
                managed: true,
                message: String::from("Plan mode override is already active for this worktree."),
                settings_path: settings_path.display().to_string(),
                state_path: state_path.display().to_string(),
                previous_local_mode: state.previous_local_mode,
                current_local_mode,
            });
        }
        clear_plan_mode_state(&state_path)?;
    }

    if current_is_plan {
        return Ok(PlanModeOutput {
            success: true,
            operation: String::from("enter"),
            changed: false,
            active: true,
            managed: false,
            message: String::from(
                "Worktree-local plan mode is already enabled outside EnterPlanMode; leaving it unchanged.",
            ),
            settings_path: settings_path.display().to_string(),
            state_path: state_path.display().to_string(),
            previous_local_mode: None,
            current_local_mode,
        });
    }

    let state = PlanModeState {
        had_local_override: current_local_mode.is_some(),
        previous_local_mode: current_local_mode.clone(),
    };
    write_plan_mode_state(&state_path, &state)?;
    set_nested_value(
        &mut document,
        PERMISSION_DEFAULT_MODE_PATH,
        Value::String(String::from("plan")),
    );
    write_json_object(&settings_path, &document)?;

    Ok(PlanModeOutput {
        success: true,
        operation: String::from("enter"),
        changed: true,
        active: true,
        managed: true,
        message: String::from("Enabled worktree-local plan mode override."),
        settings_path: settings_path.display().to_string(),
        state_path: state_path.display().to_string(),
        previous_local_mode: state.previous_local_mode,
        current_local_mode: get_nested_value(&document, PERMISSION_DEFAULT_MODE_PATH).cloned(),
    })
}

/// Exit plan mode and return the exit output.
pub(crate) fn execute_exit_plan_mode(_input: ExitPlanModeInput) -> Result<PlanModeOutput, String> {
    let settings_path = config_file_for_scope(ConfigScope::Settings)?;
    let state_path = plan_mode_state_file()?;
    let mut document = read_json_object(&settings_path)?;
    let current_local_mode = get_nested_value(&document, PERMISSION_DEFAULT_MODE_PATH).cloned();
    let current_is_plan =
        matches!(current_local_mode.as_ref(), Some(Value::String(value)) if value == "plan");

    let Some(state) = read_plan_mode_state(&state_path)? else {
        return Ok(PlanModeOutput {
            success: true,
            operation: String::from("exit"),
            changed: false,
            active: current_is_plan,
            managed: false,
            message: String::from("No EnterPlanMode override is active for this worktree."),
            settings_path: settings_path.display().to_string(),
            state_path: state_path.display().to_string(),
            previous_local_mode: None,
            current_local_mode,
        });
    };

    if !current_is_plan {
        clear_plan_mode_state(&state_path)?;
        return Ok(PlanModeOutput {
            success: true,
            operation: String::from("exit"),
            changed: false,
            active: false,
            managed: false,
            message: String::from(
                "Cleared stale EnterPlanMode state because plan mode was already changed outside the tool.",
            ),
            settings_path: settings_path.display().to_string(),
            state_path: state_path.display().to_string(),
            previous_local_mode: state.previous_local_mode,
            current_local_mode,
        });
    }

    if state.had_local_override {
        if let Some(previous_local_mode) = state.previous_local_mode.clone() {
            set_nested_value(
                &mut document,
                PERMISSION_DEFAULT_MODE_PATH,
                previous_local_mode,
            );
        } else {
            remove_nested_value(&mut document, PERMISSION_DEFAULT_MODE_PATH);
        }
    } else {
        remove_nested_value(&mut document, PERMISSION_DEFAULT_MODE_PATH);
    }
    write_json_object(&settings_path, &document)?;
    clear_plan_mode_state(&state_path)?;

    Ok(PlanModeOutput {
        success: true,
        operation: String::from("exit"),
        changed: true,
        active: false,
        managed: false,
        message: String::from("Restored the prior worktree-local plan mode setting."),
        settings_path: settings_path.display().to_string(),
        state_path: state_path.display().to_string(),
        previous_local_mode: state.previous_local_mode,
        current_local_mode: get_nested_value(&document, PERMISSION_DEFAULT_MODE_PATH).cloned(),
    })
}

/// Emit a structured output record.
pub(crate) fn execute_structured_output(
    input: StructuredOutputInput,
) -> Result<StructuredOutputResult, String> {
    if input.0.is_empty() {
        return Err(String::from("structured output payload must not be empty"));
    }
    Ok(StructuredOutputResult {
        data: String::from("Structured output provided successfully"),
        structured_output: input.0,
    })
}

/// Return the spec for a supported config setting name.
pub(crate) fn supported_config_setting(setting: &str) -> Option<ConfigSettingSpec> {
    Some(match setting {
        "theme" => ConfigSettingSpec {
            scope: ConfigScope::Global,
            kind: ConfigKind::String,
            path: &["theme"],
            options: None,
        },
        "editorMode" => ConfigSettingSpec {
            scope: ConfigScope::Global,
            kind: ConfigKind::String,
            path: &["editorMode"],
            options: Some(&["default", "vim", "emacs"]),
        },
        "verbose" => ConfigSettingSpec {
            scope: ConfigScope::Global,
            kind: ConfigKind::Boolean,
            path: &["verbose"],
            options: None,
        },
        "preferredNotifChannel" => ConfigSettingSpec {
            scope: ConfigScope::Global,
            kind: ConfigKind::String,
            path: &["preferredNotifChannel"],
            options: None,
        },
        "autoCompactEnabled" => ConfigSettingSpec {
            scope: ConfigScope::Global,
            kind: ConfigKind::Boolean,
            path: &["autoCompactEnabled"],
            options: None,
        },
        "autoMemoryEnabled" => ConfigSettingSpec {
            scope: ConfigScope::Settings,
            kind: ConfigKind::Boolean,
            path: &["autoMemoryEnabled"],
            options: None,
        },
        "autoDreamEnabled" => ConfigSettingSpec {
            scope: ConfigScope::Settings,
            kind: ConfigKind::Boolean,
            path: &["autoDreamEnabled"],
            options: None,
        },
        "fileCheckpointingEnabled" => ConfigSettingSpec {
            scope: ConfigScope::Global,
            kind: ConfigKind::Boolean,
            path: &["fileCheckpointingEnabled"],
            options: None,
        },
        "showTurnDuration" => ConfigSettingSpec {
            scope: ConfigScope::Global,
            kind: ConfigKind::Boolean,
            path: &["showTurnDuration"],
            options: None,
        },
        "terminalProgressBarEnabled" => ConfigSettingSpec {
            scope: ConfigScope::Global,
            kind: ConfigKind::Boolean,
            path: &["terminalProgressBarEnabled"],
            options: None,
        },
        "todoFeatureEnabled" => ConfigSettingSpec {
            scope: ConfigScope::Global,
            kind: ConfigKind::Boolean,
            path: &["todoFeatureEnabled"],
            options: None,
        },
        "model" => ConfigSettingSpec {
            scope: ConfigScope::Settings,
            kind: ConfigKind::String,
            path: &["model"],
            options: None,
        },
        "alwaysThinkingEnabled" => ConfigSettingSpec {
            scope: ConfigScope::Settings,
            kind: ConfigKind::Boolean,
            path: &["alwaysThinkingEnabled"],
            options: None,
        },
        "permissions.defaultMode" => ConfigSettingSpec {
            scope: ConfigScope::Settings,
            kind: ConfigKind::String,
            path: &["permissions", "defaultMode"],
            options: Some(&["default", "plan", "acceptEdits", "dontAsk", "auto"]),
        },
        "language" => ConfigSettingSpec {
            scope: ConfigScope::Settings,
            kind: ConfigKind::String,
            path: &["language"],
            options: None,
        },
        "teammateMode" => ConfigSettingSpec {
            scope: ConfigScope::Global,
            kind: ConfigKind::String,
            path: &["teammateMode"],
            options: Some(&["tmux", "in-process", "auto"]),
        },
        _ => return None,
    })
}

/// Validate and normalize a config value for a setting spec.
pub(crate) fn normalize_config_value(
    spec: ConfigSettingSpec,
    value: ConfigValue,
) -> Result<Value, String> {
    let normalized = match (spec.kind, value) {
        (ConfigKind::Boolean, ConfigValue::Bool(value)) => Value::Bool(value),
        (ConfigKind::Boolean, ConfigValue::String(value)) => {
            match value.trim().to_ascii_lowercase().as_str() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                _ => return Err(String::from("setting requires true or false")),
            }
        }
        (ConfigKind::Boolean, ConfigValue::Number(_)) => {
            return Err(String::from("setting requires true or false"))
        }
        (ConfigKind::String, ConfigValue::String(value)) => Value::String(value),
        (ConfigKind::String, ConfigValue::Bool(value)) => Value::String(value.to_string()),
        (ConfigKind::String, ConfigValue::Number(value)) => json!(value),
    };

    if let Some(options) = spec.options {
        let Some(as_str) = normalized.as_str() else {
            return Err(String::from("setting requires a string value"));
        };
        if !options.iter().any(|option| option == &as_str) {
            return Err(format!(
                "Invalid value \"{as_str}\". Options: {}",
                options.join(", ")
            ));
        }
    }

    Ok(normalized)
}

/// Return the config file path for the given scope.
pub(crate) fn config_file_for_scope(scope: ConfigScope) -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    Ok(match scope {
        ConfigScope::Global => config_home_dir()?.join("settings.json"),
        ConfigScope::Settings => cwd.join(".claw").join("settings.local.json"),
    })
}

/// Return the `~/.claude` config home directory.
pub(crate) fn config_home_dir() -> Result<PathBuf, String> {
    if let Ok(path) = std::env::var("CLAW_CONFIG_HOME") {
        return Ok(PathBuf::from(path));
    }
    let home = std::env::var("HOME").map_err(|_| String::from("HOME is not set"))?;
    Ok(PathBuf::from(home).join(".claw"))
}

/// Read a JSON object from disk, returning empty object if file missing.
pub(crate) fn read_json_object(path: &Path) -> Result<serde_json::Map<String, Value>, String> {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            if contents.trim().is_empty() {
                return Ok(serde_json::Map::new());
            }
            serde_json::from_str::<Value>(&contents)
                .map_err(|error| error.to_string())?
                .as_object()
                .cloned()
                .ok_or_else(|| String::from("config file must contain a JSON object"))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::Map::new()),
        Err(error) => Err(error.to_string()),
    }
}

/// Write a JSON object to disk with pretty-printing.
pub(crate) fn write_json_object(
    path: &Path,
    value: &serde_json::Map<String, Value>,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(
        path,
        serde_json::to_string_pretty(value).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

/// Get a nested value by dot-separated path.
pub(crate) fn get_nested_value<'a>(
    value: &'a serde_json::Map<String, Value>,
    path: &[&str],
) -> Option<&'a Value> {
    let (first, rest) = path.split_first()?;
    let mut current = value.get(*first)?;
    for key in rest {
        current = current.as_object()?.get(*key)?;
    }
    Some(current)
}

/// Set a nested value by dot-separated path, creating objects as needed.
pub(crate) fn set_nested_value(
    root: &mut serde_json::Map<String, Value>,
    path: &[&str],
    new_value: Value,
) {
    let (first, rest) = path.split_first().expect("config path must not be empty");
    if rest.is_empty() {
        root.insert((*first).to_string(), new_value);
        return;
    }

    let entry = root
        .entry((*first).to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(serde_json::Map::new());
    }
    let map = entry.as_object_mut().expect("object inserted");
    set_nested_value(map, rest, new_value);
}

/// Remove a nested value by dot-separated path.
pub(crate) fn remove_nested_value(
    root: &mut serde_json::Map<String, Value>,
    path: &[&str],
) -> bool {
    let Some((first, rest)) = path.split_first() else {
        return false;
    };
    if rest.is_empty() {
        return root.remove(*first).is_some();
    }

    let mut should_remove_parent = false;
    let removed = root.get_mut(*first).is_some_and(|entry| {
        entry.as_object_mut().is_some_and(|map| {
            let removed = remove_nested_value(map, rest);
            should_remove_parent = removed && map.is_empty();
            removed
        })
    });

    if should_remove_parent {
        root.remove(*first);
    }

    removed
}

/// Return the path to the plan-mode state file.
pub(crate) fn plan_mode_state_file() -> Result<PathBuf, String> {
    Ok(config_file_for_scope(ConfigScope::Settings)?
        .parent()
        .ok_or_else(|| String::from("settings.local.json has no parent directory"))?
        .join("tool-state")
        .join("plan-mode.json"))
}

/// Read the plan-mode state from disk.
pub(crate) fn read_plan_mode_state(path: &Path) -> Result<Option<PlanModeState>, String> {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            if contents.trim().is_empty() {
                return Ok(None);
            }
            serde_json::from_str(&contents)
                .map(Some)
                .map_err(|error| error.to_string())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

/// Write the plan-mode state to disk.
pub(crate) fn write_plan_mode_state(path: &Path, state: &PlanModeState) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(
        path,
        serde_json::to_string_pretty(state).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

/// Delete the plan-mode state file.
pub(crate) fn clear_plan_mode_state(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- iso8601_timestamp (re-exported from system_tools via execute_config) ---

    // --- supported_config_setting ---

    #[test]
    fn supported_config_setting_known_returns_some() {
        assert!(supported_config_setting("theme").is_some());
        assert!(supported_config_setting("verbose").is_some());
        assert!(supported_config_setting("model").is_some());
        assert!(supported_config_setting("permissions.defaultMode").is_some());
        assert!(supported_config_setting("editorMode").is_some());
        assert!(supported_config_setting("language").is_some());
        assert!(supported_config_setting("teammateMode").is_some());
    }

    #[test]
    fn supported_config_setting_unknown_returns_none() {
        assert!(supported_config_setting("nonExistentSetting").is_none());
        assert!(supported_config_setting("").is_none());
        assert!(supported_config_setting("THEME").is_none());
    }

    // --- normalize_config_value ---

    #[test]
    fn normalize_config_value_bool_true() {
        let spec = supported_config_setting("verbose").unwrap();
        let result = normalize_config_value(spec, ConfigValue::Bool(true)).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn normalize_config_value_bool_string_true() {
        let spec = supported_config_setting("verbose").unwrap();
        let result =
            normalize_config_value(spec, ConfigValue::String(String::from("true"))).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn normalize_config_value_bool_string_false() {
        let spec = supported_config_setting("verbose").unwrap();
        let result =
            normalize_config_value(spec, ConfigValue::String(String::from("false"))).unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn normalize_config_value_bool_string_invalid_errors() {
        let spec = supported_config_setting("verbose").unwrap();
        let result = normalize_config_value(spec, ConfigValue::String(String::from("yes")));
        assert!(result.is_err());
    }

    #[test]
    fn normalize_config_value_bool_number_errors() {
        let spec = supported_config_setting("verbose").unwrap();
        let result = normalize_config_value(spec, ConfigValue::Number(1.0));
        assert!(result.is_err());
    }

    #[test]
    fn normalize_config_value_string_kind_string_value() {
        let spec = supported_config_setting("theme").unwrap();
        let result =
            normalize_config_value(spec, ConfigValue::String(String::from("dark"))).unwrap();
        assert_eq!(result, Value::String(String::from("dark")));
    }

    #[test]
    fn normalize_config_value_string_kind_bool_value_converts() {
        let spec = supported_config_setting("theme").unwrap();
        let result = normalize_config_value(spec, ConfigValue::Bool(true)).unwrap();
        assert_eq!(result, Value::String(String::from("true")));
    }

    #[test]
    fn normalize_config_value_options_invalid_errors() {
        let spec = supported_config_setting("editorMode").unwrap();
        let result =
            normalize_config_value(spec, ConfigValue::String(String::from("invalid-mode")));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid value"));
    }

    #[test]
    fn normalize_config_value_options_valid_accepted() {
        let spec = supported_config_setting("editorMode").unwrap();
        let result =
            normalize_config_value(spec, ConfigValue::String(String::from("vim"))).unwrap();
        assert_eq!(result, Value::String(String::from("vim")));
    }

    #[test]
    fn normalize_config_value_permissions_default_mode_options() {
        let spec = supported_config_setting("permissions.defaultMode").unwrap();
        let result =
            normalize_config_value(spec, ConfigValue::String(String::from("plan"))).unwrap();
        assert_eq!(result, Value::String(String::from("plan")));
    }

    // --- config_file_for_scope ---

    #[test]
    fn config_file_for_scope_global_returns_settings_json() {
        let path = config_file_for_scope(ConfigScope::Global).unwrap();
        assert!(
            path.to_string_lossy().contains("settings.json"),
            "path: {}",
            path.display()
        );
    }

    #[test]
    fn config_file_for_scope_settings_returns_local_json() {
        let path = config_file_for_scope(ConfigScope::Settings).unwrap();
        assert!(
            path.to_string_lossy().contains("settings.local.json"),
            "path: {}",
            path.display()
        );
    }

    #[test]
    fn config_file_for_scope_global_uses_claw_config_home_env() {
        std::env::set_var("CLAW_CONFIG_HOME", "/tmp/test-claw-cfg");
        let path = config_file_for_scope(ConfigScope::Global).unwrap();
        assert_eq!(
            path,
            std::path::PathBuf::from("/tmp/test-claw-cfg/settings.json")
        );
        std::env::remove_var("CLAW_CONFIG_HOME");
    }

    // --- get_nested_value / set_nested_value / remove_nested_value ---

    #[test]
    fn get_nested_value_top_level() {
        let mut map = serde_json::Map::new();
        map.insert(String::from("key"), json!("value"));
        let result = get_nested_value(&map, &["key"]);
        assert_eq!(result, Some(&json!("value")));
    }

    #[test]
    fn get_nested_value_nested() {
        let mut map = serde_json::Map::new();
        map.insert(String::from("outer"), json!({"inner": 42}));
        let result = get_nested_value(&map, &["outer", "inner"]);
        assert_eq!(result, Some(&json!(42)));
    }

    #[test]
    fn get_nested_value_missing_key() {
        let map = serde_json::Map::new();
        assert_eq!(get_nested_value(&map, &["missing"]), None);
    }

    #[test]
    fn set_nested_value_top_level() {
        let mut map = serde_json::Map::new();
        set_nested_value(&mut map, &["key"], json!("hello"));
        assert_eq!(map.get("key"), Some(&json!("hello")));
    }

    #[test]
    fn set_nested_value_nested_creates_objects() {
        let mut map = serde_json::Map::new();
        set_nested_value(&mut map, &["outer", "inner"], json!(99));
        let result = get_nested_value(&map, &["outer", "inner"]);
        assert_eq!(result, Some(&json!(99)));
    }

    #[test]
    fn set_nested_value_overwrites_non_object() {
        let mut map = serde_json::Map::new();
        map.insert(String::from("outer"), json!("not-an-object"));
        set_nested_value(&mut map, &["outer", "inner"], json!(1));
        let result = get_nested_value(&map, &["outer", "inner"]);
        assert_eq!(result, Some(&json!(1)));
    }

    #[test]
    fn remove_nested_value_top_level() {
        let mut map = serde_json::Map::new();
        map.insert(String::from("key"), json!("value"));
        let removed = remove_nested_value(&mut map, &["key"]);
        assert!(removed);
        assert!(map.get("key").is_none());
    }

    #[test]
    fn remove_nested_value_nested_removes_empty_parent() {
        let mut map = serde_json::Map::new();
        set_nested_value(&mut map, &["outer", "inner"], json!(1));
        let removed = remove_nested_value(&mut map, &["outer", "inner"]);
        assert!(removed);
        // Parent "outer" should be removed since it became empty
        assert!(map.get("outer").is_none());
    }

    #[test]
    fn remove_nested_value_missing_returns_false() {
        let mut map = serde_json::Map::new();
        let removed = remove_nested_value(&mut map, &["missing"]);
        assert!(!removed);
    }

    // --- read_json_object ---

    #[test]
    fn read_json_object_missing_file_returns_empty() {
        let path = std::path::Path::new("/tmp/colotcook-test-nonexistent-99999.json");
        let result = read_json_object(path).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn read_json_object_valid_json() {
        let path = std::env::temp_dir().join("colotcook-test-rjo.json");
        std::fs::write(&path, r#"{"key":"val"}"#).unwrap();
        let result = read_json_object(&path).unwrap();
        assert_eq!(result.get("key"), Some(&json!("val")));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_json_object_empty_file_returns_empty() {
        let path = std::env::temp_dir().join("colotcook-test-rjo-empty.json");
        std::fs::write(&path, "").unwrap();
        let result = read_json_object(&path).unwrap();
        assert!(result.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_json_object_whitespace_only_returns_empty() {
        let path = std::env::temp_dir().join("colotcook-test-rjo-ws.json");
        std::fs::write(&path, "   \n\t  ").unwrap();
        let result = read_json_object(&path).unwrap();
        assert!(result.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    // --- write_json_object ---

    #[test]
    fn write_json_object_creates_file() {
        let path = std::env::temp_dir().join("colotcook-test-wjo.json");
        let mut map = serde_json::Map::new();
        map.insert(String::from("written"), json!(true));
        write_json_object(&path, &map).unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("written"));
        let _ = std::fs::remove_file(&path);
    }

    // --- read/write/clear plan_mode_state ---

    #[test]
    fn read_plan_mode_state_missing_returns_none() {
        let path = std::path::Path::new("/tmp/colotcook-test-plan-state-missing.json");
        let result = read_plan_mode_state(path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn write_and_read_plan_mode_state_roundtrip() {
        let path =
            std::env::temp_dir().join("colotcook-test-plan-state.json");
        let state = PlanModeState {
            had_local_override: true,
            previous_local_mode: Some(json!("default")),
        };
        write_plan_mode_state(&path, &state).unwrap();
        let read_back = read_plan_mode_state(&path).unwrap().unwrap();
        assert_eq!(read_back.had_local_override, true);
        assert_eq!(read_back.previous_local_mode, Some(json!("default")));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn clear_plan_mode_state_removes_file() {
        let path = std::env::temp_dir().join("colotcook-test-plan-clear.json");
        std::fs::write(&path, "{}").unwrap();
        clear_plan_mode_state(&path).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn clear_plan_mode_state_missing_file_ok() {
        let path =
            std::path::Path::new("/tmp/colotcook-test-plan-clear-nonexistent.json");
        let result = clear_plan_mode_state(path);
        assert!(result.is_ok());
    }

    // --- execute_structured_output ---

    #[test]
    fn execute_structured_output_empty_map_errors() {
        use crate::types::StructuredOutputInput;
        use std::collections::BTreeMap;
        let input = StructuredOutputInput(BTreeMap::new());
        let result = execute_structured_output(input);
        assert!(result.is_err());
    }

    #[test]
    fn execute_structured_output_non_empty_ok() {
        use crate::types::StructuredOutputInput;
        use std::collections::BTreeMap;
        let mut map = BTreeMap::new();
        map.insert(String::from("status"), json!("done"));
        let input = StructuredOutputInput(map);
        let result = execute_structured_output(input).unwrap();
        assert!(result.structured_output.contains_key("status"));
    }

    // --- execute_config ---

    #[test]
    fn execute_config_empty_setting_errors() {
        let input = ConfigInput {
            setting: String::new(),
            value: None,
        };
        let result = execute_config(input);
        assert!(result.is_err());
    }

    #[test]
    fn execute_config_unknown_setting_returns_error_output() {
        let input = ConfigInput {
            setting: String::from("unknownXyz"),
            value: None,
        };
        let output = execute_config(input).unwrap();
        assert!(!output.success);
        assert!(output.error.is_some());
    }
}
