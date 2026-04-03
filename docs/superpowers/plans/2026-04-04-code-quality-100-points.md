# Code Quality 100 Points — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring ColotCook Rust workspace from 44/100 to 100/100 across clippy, formatting, tests, architecture, documentation, CI/CD, and security.

**Architecture:** Three-phase approach — Phase 1 establishes clean baseline (sequential), Phase 2 runs 3 parallel streams with strict file ownership (architecture modularization, error handling + tests, CI/CD + docs), Phase 3 polishes and verifies.

**Tech Stack:** Rust workspace (7 crates), cargo clippy/fmt, GitHub Actions, cargo-deny, cargo-llvm-cov

**Spec:** `docs/superpowers/specs/2026-04-04-code-quality-100-points-design.md`

---

## Phase 1: Foundation (Sequential)

### Task 1: Format entire workspace

**Files:**
- Modify: All `.rs` files across `crates/*/src/` and `crates/*/tests/`

- [ ] **Step 1: Run cargo fmt**

```bash
cargo fmt --all
```

- [ ] **Step 2: Verify formatting passes**

```bash
cargo fmt --check
```
Expected: No output, exit code 0.

- [ ] **Step 3: Verify compilation still works**

```bash
cargo check --workspace
```
Expected: No errors.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "style: apply cargo fmt to entire workspace"
```

---

### Task 2: Clippy auto-fix

**Files:**
- Modify: Multiple `.rs` files across all crates (auto-applied fixes)

- [ ] **Step 1: Run clippy auto-fix**

```bash
cargo clippy --fix --workspace --allow-dirty --allow-staged
```

This auto-fixes: `needless_borrow`, `format!` appended to String, `let...else`, `io::Error::other`, etc.

- [ ] **Step 2: Run cargo fmt again (clippy fix may break formatting)**

```bash
cargo fmt --all
```

- [ ] **Step 3: Verify clippy reduced warnings**

```bash
cargo clippy --workspace 2>&1 | grep "warning:" | grep -v "generated" | wc -l
```
Expected: Significantly fewer than 75.

- [ ] **Step 4: Verify tests still pass**

```bash
cargo test --workspace 2>&1 | grep "test result:"
```
Expected: Same pass/fail counts as before (103 passed, 1 failed in cli).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "fix: apply clippy auto-fixes across workspace"
```

---

### Task 3: Manual clippy fixes

**Files:**
- Modify: `crates/cli/src/main.rs` (doc_markdown, unused_self, needless_borrow, dead code)
- Modify: `crates/tools/src/lib.rs` (dead code, unused imports)
- Modify: `crates/runtime/src/*.rs` (various warnings)
- Modify: `crates/api/src/*.rs` (various warnings)
- Modify: `crates/commands/src/lib.rs` (various warnings)
- Modify: `crates/plugins/src/lib.rs` (various warnings)

- [ ] **Step 1: Get current clippy warnings list**

```bash
cargo clippy --workspace 2>&1 | grep "warning:" | grep -v "generated"
```

- [ ] **Step 2: Fix `doc_markdown` warnings — add backticks around type names in doc comments**

For each warning like `item in documentation is missing backticks`:
```rust
// Before
/// Execute an MCP tool call by delegating to the McpServerManager.
// After
/// Execute an MCP tool call by delegating to the `McpServerManager`.
```

- [ ] **Step 3: Fix `unused_self` warnings — convert methods to associated functions**

For each `unused self argument` warning, remove `&self` parameter and convert to associated function:
```rust
// Before
fn some_method(&self, input: &str) -> String { ... }
// After (if self is truly unused)
fn some_method(input: &str) -> String { ... }
```
Update all call sites from `self.some_method(...)` to `Self::some_method(...)`.

- [ ] **Step 4: Fix dead code warnings — remove unused structs and methods**

Remove these unused items identified by clippy:
- `SseEvent` struct (if unused outside tests)
- `InternalPromptProgressRun` struct (if unreachable — lines 3464-3469 in main.rs)
- Methods `run_internal_prompt_text_with_progress` and `run_internal_prompt_text` (if unused)

For each removal: check with grep that no production code references them. If only tests reference them, keep them but add `#[cfg(test)]` or `#[allow(dead_code)]` with a comment explaining why.

- [ ] **Step 5: Fix `match` → `if let` warnings**

```rust
// Before
match some_result {
    Ok(val) => { ... }
    _ => {}
}
// After
if let Ok(val) = some_result {
    ...
}
```

- [ ] **Step 6: Fix `this function's return value is unnecessary` warnings**

Change function signatures from `fn foo() -> ()` to `fn foo()` and remove `Ok(())` returns where the `?` operator isn't used.

- [ ] **Step 7: Fix remaining misc warnings**

- `struct_excessive_bools`: add `#[allow(clippy::struct_excessive_bools)]` with comment
- `too_many_lines`: add `#[allow(clippy::too_many_lines)]` with comment `// Will be split in Phase 2`
- `variables can be used directly in format!`: inline variables
- `these match arms have identical bodies`: merge arms with `|`

- [ ] **Step 8: Verify zero clippy warnings**

```bash
cargo clippy --workspace 2>&1 | grep "warning:" | grep -v "generated" | wc -l
```
Expected: 0

- [ ] **Step 9: Verify all tests still pass (except the known failing one)**

```bash
cargo test --workspace 2>&1 | grep "test result:"
```

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "fix: resolve all clippy warnings (pedantic + all lints)"
```

---

### Task 4: Fix failing test

**Files:**
- Modify: `crates/cli/src/main.rs:6769` (test `build_runtime_runs_plugin_lifecycle_init_and_shutdown`)

- [ ] **Step 1: Read the failing test**

Read `crates/cli/src/main.rs` around line 6769 to understand the test structure.

- [ ] **Step 2: Add credential guard**

The test fails because `ANTHROPIC_API_KEY` is not set. Add an early return when credentials are missing:

```rust
#[test]
fn build_runtime_runs_plugin_lifecycle_init_and_shutdown() {
    // Skip in CI environments without API credentials
    if std::env::var("ANTHROPIC_API_KEY").is_err()
        && std::env::var("ANTHROPIC_AUTH_TOKEN").is_err()
    {
        eprintln!("Skipping: no Anthropic credentials available");
        return;
    }
    // ... rest of existing test
}
```

- [ ] **Step 3: Verify test passes (or skips gracefully)**

```bash
cargo test -p colotcook-cli -- build_runtime_runs_plugin_lifecycle 2>&1
```
Expected: Either "ok" (if key present) or the skip message with test passing.

- [ ] **Step 4: Verify full test suite**

```bash
cargo test --workspace 2>&1 | grep "test result:"
```
Expected: All lines show `ok`, zero failures.

- [ ] **Step 5: Commit**

```bash
git add crates/cli/src/main.rs
git commit -m "fix: skip credential-dependent test when API key unavailable"
```

---

## Phase 2: Parallel Streams

After Phase 1, create 3 branches from the Phase 1 checkpoint. Each stream has **strict file ownership** — no overlapping edits.

### Stream A: Architecture Modularization

**Branch:** `quality/stream-a-modularize`
**File ownership:** `crates/cli/src/`, `crates/tools/src/`, `crates/commands/src/`, `crates/plugins/src/`

---

### Task 5: Split `cli/src/main.rs` — Extract utility functions

**Files:**
- Create: `crates/cli/src/util.rs`
- Modify: `crates/cli/src/main.rs`

- [ ] **Step 1: Create `util.rs` with utility functions**

Extract these pure/utility functions from `main.rs` into `crates/cli/src/util.rs`:
- `levenshtein_distance` (lines 539-563)
- `ranked_suggestions` (lines 516-537)
- `suggest_closest_term` (lines 512-514)
- `render_suggestion_line` (lines 489-491)
- `indent_block` (lines 3102-3109)
- `truncate_for_prompt` (lines 3234-3241)
- `truncate_for_summary` (lines 4661-4669)
- `truncate_output_for_display` (lines 4671-4713)
- `first_visible_line` (lines 4418-4422)
- `sanitize_generated_message` (lines 3243-3245)
- `parse_titled_body` (lines 3247-3255)
- `write_temp_text_file` (lines 3197-3204)
- `recent_user_context` (lines 3206-3232)
- `extract_tool_path` (lines 4370-4378)
- `summarize_tool_payload` (lines 4653-4659)
- `command_exists` (lines 3189-3195)
- Display truncation constants (`DISPLAY_TRUNCATION_NOTICE`, `READ_DISPLAY_MAX_LINES`, etc.)

Add `pub(crate)` visibility to all items. Add doc comments to each function.

```rust
// crates/cli/src/util.rs

/// Compute the Levenshtein edit distance between two strings.
pub(crate) fn levenshtein_distance(a: &str, b: &str) -> usize {
    // ... exact code from main.rs lines 539-563
}

/// Return up to `max` suggestions from `candidates` ranked by edit distance.
pub(crate) fn ranked_suggestions(input: &str, candidates: &[&str], max: usize) -> Vec<String> {
    // ... exact code from main.rs lines 516-537
}
// ... etc for each function
```

- [ ] **Step 2: Add `mod util;` to `main.rs` and update imports**

In `main.rs` add:
```rust
mod util;
```

Replace all direct calls with `util::function_name(...)` or add `use util::*;` at top.

- [ ] **Step 3: Verify compilation**

```bash
cargo check -p colotcook-cli
```

- [ ] **Step 4: Verify tests pass**

```bash
cargo test -p colotcook-cli 2>&1 | grep "test result:"
```

- [ ] **Step 5: Commit**

```bash
git add crates/cli/src/util.rs crates/cli/src/main.rs
git commit -m "refactor(cli): extract utility functions to util.rs"
```

---

### Task 6: Split `cli/src/main.rs` — Extract OAuth flow

**Files:**
- Create: `crates/cli/src/oauth_flow.rs`
- Modify: `crates/cli/src/main.rs`

- [ ] **Step 1: Create `oauth_flow.rs`**

Extract these functions:
- `default_oauth_config` (lines 740-753)
- `run_login` (lines 755-805)
- `run_logout` (lines 807-811)
- `open_browser` (lines 813-832)
- `wait_for_oauth_callback` (lines 834-865)
- `resolve_cli_auth_source` (lines 3982-3990)
- Constants: `DEFAULT_OAUTH_CALLBACK_PORT`

Add necessary imports and `pub(crate)` visibility. Add doc comments.

- [ ] **Step 2: Add `mod oauth_flow;` and update imports in `main.rs`**

- [ ] **Step 3: Verify compilation and tests**

```bash
cargo check -p colotcook-cli && cargo test -p colotcook-cli 2>&1 | grep "test result:"
```

- [ ] **Step 4: Commit**

```bash
git add crates/cli/src/oauth_flow.rs crates/cli/src/main.rs
git commit -m "refactor(cli): extract OAuth flow to oauth_flow.rs"
```

---

### Task 7: Split `cli/src/main.rs` — Extract session management

**Files:**
- Create: `crates/cli/src/session_management.rs`
- Modify: `crates/cli/src/main.rs`

- [ ] **Step 1: Create `session_management.rs`**

Extract these functions:
- `sessions_dir` (lines 2401-2406)
- `create_managed_session_handle` (lines 2408-2414)
- `resolve_session_reference` (lines 2416-2447)
- `resolve_managed_session_path` (lines 2449-2458)
- `is_managed_session_file` (lines 2460-2466)
- `list_managed_sessions` (lines 2468-2527)
- `latest_managed_session` (lines 2529-2534)
- `format_missing_session_reference` (lines 2536-2540)
- `format_no_managed_sessions` (lines 2542-2546)
- `render_session_list` (lines 2548-2585)
- `format_session_modified_age` (lines 2587-2603)
- Structs: `SessionHandle` (1468-1472), `ManagedSessionSummary` (1474-1482)
- Constants: `PRIMARY_SESSION_EXTENSION`, `LEGACY_SESSION_EXTENSION`, `LATEST_SESSION_REFERENCE`, `SESSION_REFERENCE_ALIASES`

- [ ] **Step 2: Add `mod session_management;` and update imports in `main.rs`**

- [ ] **Step 3: Verify compilation and tests**

```bash
cargo check -p colotcook-cli && cargo test -p colotcook-cli 2>&1 | grep "test result:"
```

- [ ] **Step 4: Commit**

```bash
git add crates/cli/src/session_management.rs crates/cli/src/main.rs
git commit -m "refactor(cli): extract session management to session_management.rs"
```

---

### Task 8: Split `cli/src/main.rs` — Extract report formatters

**Files:**
- Create: `crates/cli/src/reports.rs`
- Modify: `crates/cli/src/main.rs`

- [ ] **Step 1: Create `reports.rs`**

Extract all `format_*` and `render_*` report functions:
- `format_model_report`, `format_model_switch_report`
- `format_permissions_report`, `format_permissions_switch_report`
- `format_cost_report`, `format_resume_report`, `render_resume_usage`
- `format_compact_report`, `format_auto_compaction_notice`
- `format_status_report`, `format_sandbox_report`
- `format_commit_preflight_report`, `format_commit_skipped_report`
- `render_config_report`, `render_memory_report`, `render_version_report`
- `render_export_text`, `default_export_filename`, `resolve_export_path`
- `render_teleport_report`, `render_last_tool_debug_report`
- `format_bughunter_report`, `format_ultraplan_report`
- `format_pr_report`, `format_issue_report`
- `render_repl_help`, `render_diff_report`, `render_diff_report_for`
- `print_help_to`, `print_help`
- Structs: `StatusContext`, `StatusUsage`, `GitWorkspaceSummary`
- Helper: `status_context`, `parse_git_status_metadata`, `parse_git_status_branch`, `parse_git_workspace_summary`, `resolve_git_branch_for`, `run_git_capture_in`, `find_git_root_in`, `parse_git_status_metadata_for`, `git_output`, `git_status_ok`, `run_git_diff_command_in`

- [ ] **Step 2: Add `mod reports;` and update imports in `main.rs`**

- [ ] **Step 3: Verify compilation and tests**

```bash
cargo check -p colotcook-cli && cargo test -p colotcook-cli 2>&1 | grep "test result:"
```

- [ ] **Step 4: Commit**

```bash
git add crates/cli/src/reports.rs crates/cli/src/main.rs
git commit -m "refactor(cli): extract report formatters to reports.rs"
```

---

### Task 9: Split `cli/src/main.rs` — Extract tool display, streaming, arg parsing

**Files:**
- Create: `crates/cli/src/tool_display.rs`
- Create: `crates/cli/src/streaming.rs`
- Create: `crates/cli/src/arg_parsing.rs`
- Modify: `crates/cli/src/main.rs`

- [ ] **Step 1: Create `tool_display.rs`**

Extract tool rendering functions:
- `format_tool_call_start`, `format_tool_result`
- `format_bash_call`, `format_bash_result`
- `format_read_result`, `format_write_result`, `format_edit_result`
- `format_glob_result`, `format_grep_result`, `format_generic_tool_result`
- `format_search_start`, `format_patch_preview`, `format_structured_patch_preview`

- [ ] **Step 2: Create `streaming.rs`**

Extract streaming/progress types and functions:
- `InternalPromptProgressState`, `InternalPromptProgressEvent`, `InternalPromptProgressShared`
- `InternalPromptProgressReporter` + all impl methods
- `InternalPromptProgressRun` + all impl methods
- `format_internal_prompt_progress_line`, `describe_tool_progress`
- `push_output_block`, `response_to_events`, `push_prompt_cache_record`, `prompt_cache_record_to_runtime_event`
- `final_assistant_text`, `collect_tool_uses`, `collect_tool_results`, `collect_prompt_cache_events`
- `MultiProviderRuntimeClient` struct + impl
- `convert_messages`, `filter_tool_specs`

- [ ] **Step 3: Create `arg_parsing.rs`**

Extract argument parsing functions:
- `parse_args`, `CliAction` enum, `CliOutputFormat` enum + impl
- `parse_single_word_command_alias`, `bare_slash_command_guidance`, `join_optional_args`
- `parse_direct_slash_cli_action`, `format_unknown_option`, `format_unknown_direct_slash_command`, `format_unknown_slash_command`
- `suggest_slash_commands` (cli version), `slash_command_completion_candidates_with_sessions`
- `resolve_model_alias`, `normalize_allowed_tools`, `current_tool_registry`
- `parse_permission_mode_arg`, `permission_mode_from_label`, `default_permission_mode`
- `normalize_permission_mode`
- `parse_system_prompt_args`, `parse_resume_args`, `resume_command_can_absorb_token`, `looks_like_slash_command_token`
- Constants: `DEFAULT_MODEL`, `DEFAULT_DATE`, `VERSION`, `BUILD_TARGET`, `GIT_SHA`, `CLI_OPTION_SUGGESTIONS`

- [ ] **Step 4: Add module declarations and update imports in `main.rs`**

```rust
mod arg_parsing;
mod streaming;
mod tool_display;
```

- [ ] **Step 5: Verify compilation and tests**

```bash
cargo check -p colotcook-cli && cargo test -p colotcook-cli 2>&1 | grep "test result:"
```

- [ ] **Step 6: Commit**

```bash
git add crates/cli/src/tool_display.rs crates/cli/src/streaming.rs crates/cli/src/arg_parsing.rs crates/cli/src/main.rs
git commit -m "refactor(cli): extract tool display, streaming, arg parsing modules"
```

---

### Task 10: Split `cli/src/main.rs` — Extract remaining (runtime build, permission, plugin lifecycle)

**Files:**
- Create: `crates/cli/src/runtime_build.rs`
- Modify: `crates/cli/src/main.rs`

- [ ] **Step 1: Create `runtime_build.rs`**

Extract runtime construction:
- `build_system_prompt`, `build_runtime_plugin_state`, `build_runtime_plugin_state_with_loader`
- `build_plugin_manager`, `resolve_plugin_path`, `runtime_hook_config_from_plugin_hooks`
- `build_runtime`, `build_runtime_with_plugin_state`
- `permission_policy`
- Structs: `RuntimePluginState`, `BuiltRuntime` + impls, `HookAbortMonitor` + impls
- `CliPermissionPrompter` + impls
- `CliHookProgressReporter` + impl
- `CliToolExecutor` + impls

- [ ] **Step 2: Verify `main.rs` is now primarily `LiveCli` struct + methods + `run()` + `main()`**

After all extractions, `main.rs` should contain:
- `main()`, `run()`
- `LiveCli` struct + all `impl LiveCli` methods
- `resume_session`, `run_resume_command`, `run_repl`
- `print_*` top-level functions that delegate to reports
- Module declarations

Target: `main.rs` ≤ 1500 lines.

- [ ] **Step 3: Verify compilation and tests**

```bash
cargo check -p colotcook-cli && cargo test -p colotcook-cli 2>&1 | grep "test result:"
```

- [ ] **Step 4: Verify no file exceeds 500 lines (except main.rs which should be ~1500 and will be further split if needed)**

```bash
find crates/cli/src -name "*.rs" -exec wc -l {} + | sort -rn
```

- [ ] **Step 5: Commit**

```bash
git add crates/cli/src/
git commit -m "refactor(cli): extract runtime build, permissions, plugin lifecycle"
```

---

### Task 11: Split `tools/src/lib.rs`

**Files:**
- Create: `crates/tools/src/types.rs`
- Create: `crates/tools/src/file_tools.rs`
- Create: `crates/tools/src/search_tools.rs`
- Create: `crates/tools/src/web_tools.rs`
- Create: `crates/tools/src/session_tools.rs`
- Create: `crates/tools/src/execution_tools.rs`
- Create: `crates/tools/src/agent_tools.rs`
- Create: `crates/tools/src/system_tools.rs`
- Modify: `crates/tools/src/lib.rs`

- [ ] **Step 1: Create `types.rs` — all Input/Output structs (lines 714-1068)**

Move all `*Input`, `*Output`, `*Item`, `*Status` structs and enums used as tool parameters/returns.

- [ ] **Step 2: Create `web_tools.rs` — web fetch/search (lines 1070-1451)**

Move: `execute_web_fetch`, `execute_web_search`, `build_http_client`, `normalize_fetch_url`, `build_search_url`, `normalize_fetched_content`, `summarize_web_fetch`, `extract_title`, `html_to_text`, `decode_html_entities`, `collapse_whitespace`, `preview_text`, `extract_search_hits`, `extract_search_hits_from_generic_links`, `extract_quoted_value`, `decode_duckduckgo_redirect`, `html_entity_decode_url`, `host_matches_list`, `normalize_domain_filter`, `dedupe_hits`

- [ ] **Step 3: Create `session_tools.rs` — todo write (lines 1453-1533)**

Move: `execute_todo_write`, `validate_todos`, `todo_store_path`

- [ ] **Step 4: Create `agent_tools.rs` — agent/subagent system (lines 1579-2659)**

Move: constants, `set_global_mcp_bridge`, `GLOBAL_MCP_BRIDGE`, `execute_agent`, `execute_agent_with_spawn`, `spawn_agent_job`, `run_agent_job`, `build_agent_runtime`, `build_agent_system_prompt`, `resolve_agent_model`, `allowed_tools_for_subagent`, `agent_permission_policy`, `AgentSpawnRequest`, `AgentResult`, `AgentOrchestrator`, `write_agent_manifest`, `persist_agent_terminal_state`, `append_agent_output`, `format_agent_terminal_output`, `agent_store_dir`, `make_agent_id`, `slugify_agent_name`, `normalize_subagent_type`, `iso8601_now`, `ProviderRuntimeClient`, `McpBridge`, `SubagentToolExecutor`, `tool_specs_for_allowed_tools`, `convert_messages`, `push_output_block`, `response_to_events`, `push_prompt_cache_record`, `prompt_cache_record_to_runtime_event`, `final_assistant_text`

- [ ] **Step 5: Create `search_tools.rs` — tool search (lines 2462-2596)**

Move: `execute_tool_search`, `deferred_tool_specs`, `search_tool_specs`, `normalize_tool_search_query`, `canonical_tool_token`

- [ ] **Step 6: Create `execution_tools.rs` — notebook, sleep, repl, powershell, shell (lines 2661-3651)**

Move: `execute_notebook_edit`, notebook helpers, `execute_sleep`, `execute_brief`, `execute_repl`, `execute_powershell`, `execute_shell_command`, `detect_powershell_shell`, `command_exists`, `resolve_repl_runtime`, `detect_first_command`, all related helpers.

- [ ] **Step 7: Create `system_tools.rs` — config, plan mode, structured output (lines 2893-3485)**

Move: `execute_config`, `execute_enter_plan_mode`, `execute_exit_plan_mode`, `execute_structured_output`, `supported_config_setting`, config helpers.

- [ ] **Step 8: Create `file_tools.rs` — file operation dispatchers (lines 605-641)**

Move: `run_read_file`, `run_write_file`, `run_edit_file`, `run_glob_search`, `run_grep_search`

- [ ] **Step 9: Update `lib.rs` to be a thin dispatcher**

`lib.rs` should contain:
- Module declarations
- Re-exports of public types (`pub use types::*;`, `pub use agent_tools::{AgentSpawnRequest, McpBridge, ...};`)
- `ToolManifestEntry`, `ToolSource`, `ToolRegistry`, `ToolSpec`, `GlobalToolRegistry` structs
- `mvp_tool_specs()` function
- `execute_tool()` dispatcher (which calls into modules)
- `normalize_tool_name`, `permission_mode_from_plugin` helpers

Target: `lib.rs` ≤ 400 lines.

- [ ] **Step 10: Verify compilation and tests**

```bash
cargo check --workspace && cargo test -p colotcook-tools 2>&1 | grep "test result:"
```

- [ ] **Step 11: Commit**

```bash
git add crates/tools/src/
git commit -m "refactor(tools): split monolithic lib.rs into category modules"
```

---

### Task 12: Split `plugins/src/lib.rs`

**Files:**
- Create: `crates/plugins/src/types.rs`
- Create: `crates/plugins/src/registry.rs`
- Create: `crates/plugins/src/discovery.rs`
- Create: `crates/plugins/src/lifecycle.rs`
- Modify: `crates/plugins/src/lib.rs`

- [ ] **Step 1: Create `types.rs`**

Move all type definitions:
- Enums: `PluginKind`, `PluginToolPermission`, `PluginInstallSource`, `PluginDefinition`, `PluginManifestValidationError`, `PluginError`
- Structs: `PluginMetadata`, `PluginHooks`, `PluginLifecycle`, `PluginManifest`, `PluginToolManifest`, `PluginToolDefinition`, `PluginCommandManifest`, `RawPluginManifest`, `RawPluginToolManifest`, `PluginTool`, `PluginSummary`, `PluginLoadFailure`, `InstalledPluginRecord`, `InstalledPluginRegistry`, `BuiltinPlugin`, `BundledPlugin`, `ExternalPlugin`, `PluginManagerConfig`, `InstallOutcome`, `UpdateOutcome`
- All their impl blocks
- Error trait impls and From impls

- [ ] **Step 2: Create `lifecycle.rs`**

Move:
- `Plugin` trait
- `impl Plugin for BuiltinPlugin/BundledPlugin/ExternalPlugin/PluginDefinition`
- `run_lifecycle_commands`, `resolve_lifecycle`, `validate_lifecycle_paths`

- [ ] **Step 3: Create `discovery.rs`**

Move:
- `PluginDiscovery` struct + impl
- `load_plugin_definition`, `load_plugin_from_directory`, `load_manifest_from_directory`, `load_manifest_from_path`
- `plugin_manifest_path`, `build_plugin_manifest`, `validate_required_manifest_field`
- `build_manifest_permissions`, `build_manifest_tools`, `build_manifest_commands`
- `validate_command_entries`, `validate_command_entry`
- `resolve_hooks`, `validate_hook_paths`, `resolve_hook_entry`, `is_literal_command`
- `resolve_tools`, `validate_tool_paths`, `validate_command_path`
- `resolve_local_source`, `parse_install_source`, `materialize_source`, `discover_plugin_dirs`
- Helper functions: `plugin_id`, `sanitize_plugin_id`, `copy_dir_all`, etc.

- [ ] **Step 4: Create `registry.rs`**

Move:
- `RegisteredPlugin` struct + impl
- `PluginRegistryReport` struct + impl
- `PluginRegistry` struct + impl
- `PluginManager` struct + impl
- `builtin_plugins` function

- [ ] **Step 5: Update `lib.rs` as thin re-export hub**

```rust
mod discovery;
pub mod hooks;
mod lifecycle;
mod registry;
mod types;

pub use discovery::load_plugin_from_directory;
pub use hooks::{HookEvent, HookRunResult, HookRunner};
pub use lifecycle::Plugin;
pub use registry::*;
pub use types::*;
```

Target: `lib.rs` ≤ 30 lines.

- [ ] **Step 6: Verify compilation and tests**

```bash
cargo check --workspace && cargo test -p colotcook-plugins 2>&1 | grep "test result:"
```

- [ ] **Step 7: Commit**

```bash
git add crates/plugins/src/
git commit -m "refactor(plugins): split lib.rs into types, registry, discovery, lifecycle"
```

---

### Task 13: Split `commands/src/lib.rs`

**Files:**
- Create: `crates/commands/src/types.rs`
- Create: `crates/commands/src/validation.rs`
- Create: `crates/commands/src/help.rs`
- Create: `crates/commands/src/agents_and_skills.rs`
- Create: `crates/commands/src/plugins_command.rs`
- Create: `crates/commands/src/handlers.rs`
- Modify: `crates/commands/src/lib.rs`

- [ ] **Step 1: Create `types.rs`**

Move: `CommandManifestEntry`, `CommandSource`, `CommandRegistry`, `SlashCommandSpec`, `SLASH_COMMAND_SPECS`, `SlashCommand`, `SlashCommandParseError`, `SlashCommandResult`, `PluginsCommandResult`

- [ ] **Step 2: Create `validation.rs`**

Move: `validate_slash_command_input`, `validate_no_args`, `optional_single_arg`, `require_remainder`, `parse_permissions_mode`, `parse_clear_args`, `parse_config_section`, `parse_session_command`, `parse_plugin_command`, `parse_list_or_help_args`, `parse_skills_args`, `usage_error`, `command_error`, `remainder_after_command`, `find_slash_command_spec`, `command_root_name`, `slash_command_usage`, `slash_command_detail_lines`, `normalize_optional_args`, `levenshtein_distance`, `suggest_slash_commands`

- [ ] **Step 3: Create `help.rs`**

Move: `render_slash_command_help_detail`, `slash_command_specs`, `resume_supported_slash_commands`, `slash_command_category`, `format_slash_command_help_line`, `render_slash_command_help`

- [ ] **Step 4: Create `agents_and_skills.rs`**

Move: `handle_agents_slash_command`, `handle_skills_slash_command`, `AgentSummary`, `SkillSummary`, `SkillOrigin`, `SkillInstallSource`, `InstalledSkill`, `SkillRoot`, `DefinitionSource`, all discovery/render functions for agents and skills (lines 917-1878)

- [ ] **Step 5: Create `plugins_command.rs`**

Move: `handle_plugins_slash_command`, `render_plugins_report`, `render_plugin_install_report`, `resolve_plugin_target`

- [ ] **Step 6: Create `handlers.rs`**

Move: `handle_slash_command`, `handle_cost`, `handle_diff`, `handle_commit`, `handle_debug_tool_call`, `handle_sandbox`, `handle_session`

- [ ] **Step 7: Update `lib.rs` as re-export hub**

Target: `lib.rs` ≤ 50 lines with module declarations and re-exports.

- [ ] **Step 8: Verify compilation and tests**

```bash
cargo check --workspace && cargo test -p colotcook-commands 2>&1 | grep "test result:"
```

- [ ] **Step 9: Commit**

```bash
git add crates/commands/src/
git commit -m "refactor(commands): split lib.rs into types, validation, help, handlers"
```

---

### Task 14: Stream A final — verify all file sizes and add doc comments

**Files:**
- Modify: All new modules created in Tasks 5-13

- [ ] **Step 1: Check no file exceeds 500 lines**

```bash
find crates/cli/src crates/tools/src crates/plugins/src crates/commands/src -name "*.rs" -exec wc -l {} + | sort -rn | head -20
```

If any file exceeds 500 lines, split further.

- [ ] **Step 2: Add doc comments to all `pub(crate)` and `pub` items in new modules**

Each public function/struct/enum must have at least a one-line `///` doc comment.

- [ ] **Step 3: Run clippy on modified crates**

```bash
cargo clippy -p colotcook-cli -p colotcook-tools -p colotcook-plugins -p colotcook-commands -- -D warnings
```

- [ ] **Step 4: Run all tests**

```bash
cargo test --workspace 2>&1 | grep "test result:"
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor: finalize modularization, add doc comments, verify file sizes"
```

---

### Stream B: Error Handling & Test Coverage

**Branch:** `quality/stream-b-error-handling`
**File ownership:** `crates/runtime/src/`, `crates/api/src/`, all `tests/` directories

---

### Task 15: Fix 5 production `.expect()` calls

**Files:**
- Modify: `crates/api/src/client.rs` (lines 112, 127)
- Modify: `crates/api/src/providers/anthropic.rs` (line 458)
- Modify: `crates/runtime/src/rate_limit.rs` (lines 78, 114)

- [ ] **Step 1: Fix `client.rs` — replace `.expect()` with explicit match arms**

Read `crates/api/src/client.rs` around lines 100-130. The two `.expect("non-Anthropic provider")` calls are on `openai_compat_client()` inside a `_ =>` catch-all arm.

Replace with explicit match on each provider variant:

```rust
// Before (inside match arm _ =>)
let client = self.openai_compat_client().expect("non-Anthropic provider");
// After — restructure the match to handle each variant explicitly
Self::OpenAi(client) | Self::Gemini(client) | Self::Grok(client) | Self::Ollama(client) => {
    client.send_message(request).await
}
```

- [ ] **Step 2: Fix `anthropic.rs` — refactor retry loop to carry error**

Read `crates/api/src/providers/anthropic.rs` around line 458.

```rust
// Before
last_error.expect("retry loop must capture an error")
// After — initialize with a sentinel or restructure loop
// Option A: Use loop variable that's proven non-None
let mut last_error: Option<ApiError> = None;
// ... in loop body: last_error = Some(e);
// After loop:
last_error.unwrap_or_else(|| unreachable!("retry loop always sets last_error"))
```

Or better — restructure to use a concrete error type:

```rust
let mut last_error = ApiError::RetriesExhausted {
    attempts: 0,
    last_error: Box::new(ApiError::Io(io::Error::new(io::ErrorKind::Other, "no attempts"))),
};
```

- [ ] **Step 3: Fix `rate_limit.rs` — handle mutex poison gracefully**

```rust
// Before
let guard = self.inner.lock().expect("rate limiter lock");
// After
let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
```

This recovers from poison by taking the inner guard, which is the standard pattern for non-critical mutexes.

- [ ] **Step 4: Verify zero `.expect()` in production code**

```bash
# Search for .expect in src/ files, excluding test blocks
grep -rn '\.expect(' crates/*/src/**/*.rs | grep -v '#\[cfg(test)\]' | grep -v 'mod tests' | grep -v '// test'
```

Review each remaining hit — should be zero or all inside `#[cfg(test)]` blocks.

- [ ] **Step 5: Verify compilation and tests**

```bash
cargo check --workspace && cargo test --workspace 2>&1 | grep "test result:"
```

- [ ] **Step 6: Commit**

```bash
git add crates/api/ crates/runtime/
git commit -m "fix: eliminate all .expect() calls from production code paths"
```

---

### Task 16: Setup coverage measurement

**Files:**
- Create: `scripts/coverage.sh`
- Modify: `README.md` (add coverage badge section)

- [ ] **Step 1: Create coverage script**

```bash
#!/usr/bin/env bash
set -euo pipefail

# Install cargo-llvm-cov if not present
if ! command -v cargo-llvm-cov &> /dev/null; then
    cargo install cargo-llvm-cov
fi

# Run coverage
cargo llvm-cov --workspace --html --output-dir target/coverage

echo "Coverage report: target/coverage/html/index.html"
echo ""
cargo llvm-cov --workspace --summary-only
```

- [ ] **Step 2: Make script executable**

```bash
chmod +x scripts/coverage.sh
```

- [ ] **Step 3: Run coverage and check current percentage**

```bash
cargo install cargo-llvm-cov 2>/dev/null || true
cargo llvm-cov --workspace --summary-only 2>&1
```

- [ ] **Step 4: Commit**

```bash
git add scripts/coverage.sh
git commit -m "chore: add coverage measurement script (cargo-llvm-cov)"
```

---

### Task 17: Add edge-case tests for error types

**Files:**
- Modify: `crates/api/tests/client_integration.rs`
- Modify: `crates/runtime/src/session.rs` (test module)
- Modify: `crates/runtime/src/config.rs` (test module)

- [ ] **Step 1: Add ApiError Display/From tests**

In `crates/api/tests/client_integration.rs`, add:

```rust
#[test]
fn api_error_display_covers_all_variants() {
    use colotcook_api::error::ApiError;
    use std::io;

    let errors = vec![
        ApiError::MissingCredentials {
            provider: "Test",
            env_vars: &["TEST_KEY"],
        },
        ApiError::ExpiredOAuthToken,
        ApiError::Auth("auth failed".into()),
        ApiError::Http(reqwest::get("http://[::1]:1").unwrap_err()),
        ApiError::Io(io::Error::new(io::ErrorKind::NotFound, "test")),
        ApiError::InvalidSseFrame("bad frame"),
    ];

    for err in &errors {
        let msg = format!("{err}");
        assert!(!msg.is_empty(), "Display should produce non-empty string for {err:?}");
    }
}

#[test]
fn api_error_from_io_error() {
    let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "test");
    let api_err: colotcook_api::error::ApiError = io_err.into();
    assert!(matches!(api_err, colotcook_api::error::ApiError::Io(_)));
}
```

- [ ] **Step 2: Add SessionError edge-case tests**

In the `#[cfg(test)]` block of `crates/runtime/src/session.rs`:

```rust
#[test]
fn session_error_from_io_converts_correctly() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing file");
    let session_err: SessionError = io_err.into();
    assert!(matches!(session_err, SessionError::Io(_)));
    let msg = format!("{session_err}");
    assert!(msg.contains("missing file"));
}
```

- [ ] **Step 3: Add ConfigError edge-case tests**

In the `#[cfg(test)]` block of `crates/runtime/src/config.rs`:

```rust
#[test]
fn config_error_display_shows_useful_message() {
    let err = ConfigError::Parse("invalid toml".into());
    assert_eq!(format!("{err}"), "invalid toml");

    let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "no access");
    let err: ConfigError = io_err.into();
    let msg = format!("{err}");
    assert!(msg.contains("no access"));
}
```

- [ ] **Step 4: Verify new tests pass**

```bash
cargo test --workspace 2>&1 | grep "test result:"
```

- [ ] **Step 5: Commit**

```bash
git add crates/api/tests/ crates/runtime/src/session.rs crates/runtime/src/config.rs
git commit -m "test: add edge-case tests for error types across api and runtime"
```

---

### Stream C: CI/CD + Security + Docs

**Branch:** `quality/stream-c-cicd-docs`
**File ownership:** `.github/`, root config files, doc comments in `crates/runtime/src/`, `crates/api/src/`, `crates/telemetry/src/`

---

### Task 18: GitHub Actions CI workflow

**Files:**
- Create: `.github/workflows/ci.yml`

- [ ] **Step 1: Create CI workflow**

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

env:
  CARGO_TERM_COLOR: always
  RUSTFLAGS: -D warnings

jobs:
  fmt:
    name: Format
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt
      - run: cargo fmt --check

  clippy:
    name: Clippy
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo clippy --workspace --all-targets

  test:
    name: Test
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo test --workspace

  deny:
    name: Dependency audit
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: EmbarkStudios/cargo-deny-action@v2
```

- [ ] **Step 2: Verify YAML syntax**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))" 2>&1 || echo "Install pyyaml: pip install pyyaml"
```

- [ ] **Step 3: Commit**

```bash
mkdir -p .github/workflows
git add .github/workflows/ci.yml
git commit -m "ci: add GitHub Actions workflow (fmt, clippy, test, deny)"
```

---

### Task 19: cargo-deny configuration

**Files:**
- Create: `deny.toml`

- [ ] **Step 1: Create `deny.toml`**

```toml
[advisories]
vulnerability = "deny"
unmaintained = "warn"
yanked = "warn"
notice = "warn"

[licenses]
unlicensed = "deny"
allow = [
    "MIT",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Unicode-3.0",
    "Unicode-DFS-2016",
    "Zlib",
    "OpenSSL",
    "Ring",
]
copyleft = "deny"

[bans]
multiple-versions = "warn"
wildcards = "allow"

[sources]
unknown-registry = "warn"
unknown-git = "warn"
allow-registry = ["https://github.com/rust-lang/crates.io-index"]
allow-git = []
```

- [ ] **Step 2: Test cargo-deny locally**

```bash
cargo install cargo-deny 2>/dev/null || true
cargo deny check 2>&1 | tail -20
```

If license issues arise, add the missing license to the allow list.

- [ ] **Step 3: Iterate until `cargo deny check` passes**

Fix any license or advisory issues that come up.

- [ ] **Step 4: Commit**

```bash
git add deny.toml
git commit -m "chore: add cargo-deny configuration for license and vulnerability auditing"
```

---

### Task 20: Add doc comments to runtime, api, telemetry public items

**Files:**
- Modify: `crates/api/src/types.rs`
- Modify: `crates/api/src/error.rs`
- Modify: `crates/api/src/client.rs`
- Modify: `crates/api/src/lib.rs`
- Modify: `crates/api/src/prompt_cache.rs`
- Modify: `crates/api/src/providers/mod.rs`
- Modify: `crates/runtime/src/lib.rs`
- Modify: `crates/runtime/src/conversation.rs`
- Modify: `crates/runtime/src/config.rs`
- Modify: `crates/runtime/src/session.rs`
- Modify: `crates/runtime/src/permissions.rs`
- Modify: `crates/runtime/src/mcp_stdio.rs`
- Modify: `crates/runtime/src/sandbox.rs`
- Modify: `crates/runtime/src/prompt.rs`
- Modify: `crates/telemetry/src/lib.rs`

- [ ] **Step 1: Add module-level doc comments (`//!`)**

For each crate's `lib.rs`:
```rust
//! Provider abstraction and streaming for ColotCook.
//!
//! This crate handles communication with AI providers (Anthropic, OpenAI, Gemini, xAI, Ollama)
//! and provides a unified streaming interface.
```

- [ ] **Step 2: Add `///` to all `pub` structs, enums, traits**

Use concise single-line docs:
```rust
/// Error returned by API provider operations.
pub enum ApiError { ... }

/// Cached prompt state for reducing API costs.
pub struct PromptCache { ... }
```

- [ ] **Step 3: Add `///` to all `pub` functions with non-obvious signatures**

```rust
/// Load the system prompt from the project context and merge with defaults.
pub fn load_system_prompt(context: &ProjectContext) -> Result<String, PromptBuildError> { ... }
```

- [ ] **Step 4: Verify clippy doc_markdown lint passes**

```bash
cargo clippy -p colotcook-api -p colotcook-runtime -p colotcook-telemetry -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add crates/api/ crates/runtime/ crates/telemetry/
git commit -m "docs: add doc comments to all public items in api, runtime, telemetry"
```

---

### Task 21: Add CONTRIBUTING.md and CHANGELOG.md

**Files:**
- Create: `CONTRIBUTING.md`
- Create: `CHANGELOG.md`

- [ ] **Step 1: Create `CONTRIBUTING.md`**

```markdown
# Contributing to ColotCook

## Development Setup

1. Install [Rust](https://www.rust-lang.org/tools/install) (edition 2021+)
2. Clone the repository
3. Run `cargo build` to verify setup

## Development Workflow

```bash
# Format code
cargo fmt --all

# Lint
cargo clippy --workspace -- -D warnings

# Run tests
cargo test --workspace

# Check dependencies
cargo deny check
```

## Pull Request Guidelines

- All PRs must pass CI (format, clippy, tests, deny)
- Use conventional commit messages: `feat:`, `fix:`, `refactor:`, `docs:`, `test:`, `chore:`
- Keep changes focused — one concern per PR
- Add tests for new functionality
- Add doc comments for new public items

## Architecture

See `ARCHITECTURE.md` for crate structure and dependency graph.

## Security

See `SECURITY.md` for the threat model and reporting vulnerabilities.
```

- [ ] **Step 2: Create `CHANGELOG.md`**

```markdown
# Changelog

All notable changes to ColotCook will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).

## [0.1.0] — 2026-04-04

### Added
- Multi-provider support: Anthropic Claude, OpenAI/GPT, Google Gemini, xAI Grok, Ollama
- 19 built-in tools: file ops, search, execution, web, session, integration, agent, system
- 15 slash commands: /export, /config, /status, /clear, /compact, and more
- Session persistence with --resume support
- Plugin and hook system (PreToolUse/PostToolUse)
- MCP protocol support (stdio, SSE, HTTP)
- 5 permission modes: read-only, workspace-write, danger-full-access, prompt, allow
- Streaming responses via SSE from all providers
- Prompt caching for API cost reduction
- Sandbox isolation (Linux namespaces)
- OAuth authentication flow
- GitHub Actions CI pipeline
- cargo-deny for license and vulnerability auditing
```

- [ ] **Step 3: Commit**

```bash
git add CONTRIBUTING.md CHANGELOG.md
git commit -m "docs: add CONTRIBUTING.md and CHANGELOG.md"
```

---

## Phase 2 Merge

### Task 22: Merge streams in order

- [ ] **Step 1: Merge Stream C into main**

```bash
git checkout main
git merge quality/stream-c-cicd-docs --no-ff -m "merge: Stream C — CI/CD, security, docs"
```

- [ ] **Step 2: Merge Stream B into main**

```bash
git merge quality/stream-b-error-handling --no-ff -m "merge: Stream B — error handling, tests, coverage"
```

Resolve any conflicts in `crates/runtime/src/` or `crates/api/src/` (doc comments from C + code changes from B).

- [ ] **Step 3: Merge Stream A into main**

```bash
git merge quality/stream-a-modularize --no-ff -m "merge: Stream A — architecture modularization"
```

This is the largest merge. Conflicts are expected only if Streams B/C added doc comments to files that Stream A moved. Resolve by applying doc comments to the new module locations.

- [ ] **Step 4: Verify post-merge**

```bash
cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo test --workspace
```

- [ ] **Step 5: Commit any merge fixups**

```bash
git add -A
git commit -m "fix: resolve merge conflicts from parallel streams"
```

---

## Phase 3: Polish

### Task 23: Final clippy sweep

**Files:**
- Modify: Any files with new warnings from merges

- [ ] **Step 1: Run clippy with deny**

```bash
cargo clippy --workspace -- -D warnings 2>&1
```

- [ ] **Step 2: Fix any remaining warnings**

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "fix: resolve clippy warnings after stream merges"
```

---

### Task 24: Final test sweep and coverage

- [ ] **Step 1: Run full test suite**

```bash
cargo test --workspace
```
Expected: 100% pass.

- [ ] **Step 2: Run coverage**

```bash
cargo llvm-cov --workspace --summary-only
```

- [ ] **Step 3: If coverage < 80%, identify uncovered modules**

```bash
cargo llvm-cov --workspace --html --output-dir target/coverage
open target/coverage/html/index.html
```

Add tests for uncovered critical paths.

- [ ] **Step 4: Commit any new tests**

```bash
git add -A
git commit -m "test: add tests to reach ≥80% coverage target"
```

---

### Task 25: Final verification — all success criteria

- [ ] **Step 1: Verify all success criteria**

```bash
# 1. Formatting
cargo fmt --check
echo "✓ Formatting"

# 2. Clippy
cargo clippy --workspace -- -D warnings
echo "✓ Clippy"

# 3. Tests
cargo test --workspace
echo "✓ Tests"

# 4. Deny
cargo deny check
echo "✓ Deny"

# 5. File sizes
echo "--- File sizes (should all be ≤500 lines) ---"
find crates -name "*.rs" -not -path "*/tests/*" -exec wc -l {} + | sort -rn | head -20

# 6. No .expect in production
echo "--- Production .expect() calls ---"
grep -rn '\.expect(' crates/*/src/ --include="*.rs" | grep -v 'mod tests' | grep -v '#\[cfg(test)\]' | grep -v '#\[test\]'
```

- [ ] **Step 2: Final commit if any changes needed**

```bash
git add -A
git commit -m "chore: final quality polish — 100/100 target achieved"
```
