# Changelog

All notable changes to ColotCook are documented here.
Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning follows [Semantic Versioning](https://semver.org/).

---

## [0.1.0] — 2026-04-04

### Added

#### Multi-Provider AI Support
- Anthropic Claude (claude-opus-4-6, claude-sonnet-4-6, claude-haiku-4-5) via native Messages API
- OpenAI / GPT (gpt-4o, o1, o3) via OpenAI-compatible endpoint
- Google Gemini (gemini-2.5-pro, gemini-2.5-flash) via OpenAI-compat shim
- xAI Grok (grok-3, grok-3-mini) via OpenAI-compat shim
- Ollama local models (llama3, codellama, deepseek-coder, qwen2.5-coder, etc.)
- Model alias resolution (`sonnet` → `claude-sonnet-4-6`, `grok` → `grok-3`, etc.)

#### Built-in Tools (19)
- File operations: `read_file`, `write_file`, `edit_file`, `glob_search`
- Search: `grep_search` (ripgrep-style)
- Shell: `execute_bash`
- Web: `web_fetch`, `web_search`
- MCP: `mcp_call_tool`, `mcp_list_tools`, `mcp_list_resources`, `mcp_read_resource`
- Sessions & compaction: `compact_session`
- TODO: `todo_read`, `todo_write`
- Analysis: `dispatch_agent`
- Utilities: `exit_plan_mode`, `task_complete`

#### Slash Commands (15)
`/export`, `/config`, `/status`, `/clear`, `/compact`, `/resume`, `/model`,
`/permissions`, `/sandbox`, `/mcp`, `/plugin`, `/hook`, `/session`, `/help`, `/version`

#### Session Management
- Persistent conversation sessions stored under `~/.claude/sessions/`
- Resume with `--resume <id>` or `--resume latest`
- Automatic compaction when context approaches token limits
- Session forking support

#### Plugin & Hook System
- `PreToolUse` and `PostToolUse` hook lifecycle
- Bundled plugins: `colotcook-guard`, `example-bundled`, `sample-hooks`
- Plugin registry with `RuntimePluginConfig`

#### MCP Protocol Support
- Transports: stdio, SSE, HTTP streaming, managed proxy, WebSocket
- `McpServerManager` with per-server lifecycle and error handling
- OAuth-protected MCP servers via `McpOAuthConfig`
- Scoped MCP server configs with hash-based deduplication

#### Permission System
- Modes: `read-only`, `workspace-write`, `danger-full-access`, `prompt`, `allow`
- `PermissionPolicy` with per-path and per-tool overrides
- Interactive `PermissionPrompter` trait for terminal approval flows

#### Streaming
- Real-time SSE streaming from all providers
- Incremental SSE parser (`IncrementalSseParser`) and frame parser (`parse_frame`)
- `MessageStream` enum dispatching to provider-specific stream handles

#### Prompt Caching
- Disk-backed completion cache with FNV-1a fingerprinting
- Cache-break detection (expected vs unexpected invalidations)
- `PromptCacheStats` for cost-analysis and debugging
- Session-scoped TTL configuration

#### Sandbox
- Linux seccomp-based sandbox via `build_linux_sandbox_command`
- Container environment detection (`detect_container_environment`)
- `FilesystemIsolationMode` and `SandboxRequest` for per-tool policy

#### OAuth
- PKCE S256 code-challenge flow (`generate_pkce_pair`, `code_challenge_s256`)
- Token storage with cross-platform file-lock guards
- OAuth credential refresh and expiry detection
- Loopback redirect URI for CLI flows

#### Telemetry
- `TelemetrySink` trait with `MemoryTelemetrySink` (tests) and `JsonlTelemetrySink` (production)
- `SessionTracer` for structured HTTP lifecycle and analytics events
- `AnthropicRequestProfile` for version/beta header management

#### CI / Security
- GitHub Actions workflow: `fmt`, `clippy`, `test`, `deny` jobs
- `cargo-deny` configuration enforcing approved SPDX licenses and vulnerability checks
- `SECURITY.md` with responsible disclosure policy
- `CONTRIBUTING.md` with dev-setup and PR guidelines

[0.1.0]: https://github.com/colotcook/colotcook/releases/tag/v0.1.0
