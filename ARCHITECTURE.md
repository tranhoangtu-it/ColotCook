# ColotCook Architecture

## Overview

ColotCook is a multi-provider AI coding agent built in Rust. It follows a modular
workspace architecture with strict crate boundaries.

## Workspace Structure

```
colotcook/
├── crates/
│   ├── api/        # AI provider abstraction layer
│   ├── cli/        # Terminal UI and user interaction
│   ├── commands/   # Slash command definitions and handlers
│   ├── plugins/    # Plugin system (discovery, lifecycle, hooks)
│   ├── runtime/    # Core agent loop, sessions, sandbox, permissions
│   ├── telemetry/  # Analytics and session recording
│   └── tools/      # Tool definitions and execution
├── Cargo.toml      # Workspace root
├── SECURITY.md     # Security policy and controls
└── ARCHITECTURE.md # This file
```

## Crate Dependency Graph

```
cli
├── commands
│   ├── runtime
│   └── plugins
├── runtime
│   └── (no internal deps)
├── api
│   └── runtime (for OAuth types)
├── tools
│   └── api
├── plugins
│   └── (no internal deps)
└── telemetry
    └── (no internal deps)
```

## Key Design Decisions

### 1. Provider Abstraction (crates/api/)

All AI providers implement a common `Provider` trait with `send_message()`.
Provider selection happens at runtime via model name prefix detection:
- `claude-*` → Anthropic
- `gpt-*`, `o1-*` → OpenAI
- `gemini-*` → Google
- `grok-*` → xAI
- No prefix match → Ollama (local)

SSE streaming uses a state-machine parser for efficient memory usage.

### 2. Permission System (crates/runtime/permissions.rs)

Five permission modes from most restrictive to least:
1. **ReadOnly** — No file writes, no shell commands
2. **WorkspaceWrite** — Write only within project directory
3. **Prompt** — Ask user for each tool invocation
4. **Allow** — Allow all with audit logging
5. **DangerFullAccess** — No restrictions (development only)

Permission rules can be configured per-tool in settings.json.

### 3. Conversation Loop (crates/runtime/conversation.rs)

The `ConversationRuntime` struct owns the conversation lifecycle:
1. Accept user input
2. Build API request with system prompt + message history
3. Stream response from provider
4. Parse tool use requests
5. Check permissions for each tool
6. Execute tools and collect results
7. Loop until no more tool calls or limits exceeded

Safety controls: iteration limit (200), conversation timeout (30min),
auto-compaction, graceful shutdown signal.

### 4. Session Persistence (crates/runtime/session.rs)

Sessions are stored as JSONL (JSON Lines) with:
- Atomic writes via temp file + rename
- Advisory file locking for concurrent access safety
- Automatic rotation at 256KB with max 3 rotated files
- Fork support for branching conversations

### 5. Sandbox (crates/runtime/sandbox.rs)

Linux-specific isolation using namespace separation:
- User, mount, IPC, PID, UTS namespace isolation via `unshare`
- Optional network isolation
- Filesystem isolation modes
- Resource limits via `ulimit` (CPU, memory, FDs, processes, file size)
- Container environment detection

### 6. Plugin System (crates/plugins/)

Plugins are directory-based bundles containing:
- `plugin.json` manifest (name, version, description)
- Optional hook scripts (Pre/PostToolUse lifecycle)
- Optional tool definitions
- Optional slash commands

Plugin lifecycle: Install → Init → (Pre/PostToolUse hooks) → Shutdown

### 7. MCP Protocol (crates/runtime/mcp_*.rs)

Model Context Protocol support with multiple transports:
- **stdio** — Spawn subprocess, communicate via stdin/stdout
- **HTTP** — POST JSON-RPC requests to remote server
- **SSE** — Server-Sent Events for streaming (parser implemented)
- **WebSocket** — Planned

OAuth 2.0 with PKCE (S256) for authenticated MCP servers.
Per-server token management with automatic refresh.

## Error Handling Strategy

Each crate defines its own error types (no anyhow/thiserror):
- `ApiError` — Provider communication errors with retry classification
- `RuntimeError` — Categorized enum (API, Tool, Permission, Session, Limit, Config, Hook)
- `SessionError` — IO and format errors for persistence
- `PluginError` — Plugin lifecycle errors

## Testing Strategy

- **Unit tests**: Inline in each module, test individual functions
- **Integration tests**: In `tests/` directories, test crate-level behavior
- **Security tests**: Validate sandbox boundaries, permission enforcement
- **Concurrency tests**: Verify file locking under parallel access

## Configuration

Layered configuration loading (highest priority first):
1. CLI flags
2. Project-local: `.colotcook/settings.local.json`
3. Project: `.colotcook/settings.json`
4. User: `~/.colotcook/settings.json`
