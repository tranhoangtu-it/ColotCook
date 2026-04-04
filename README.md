# ColotCook

A production-ready AI coding agent written in pure Rust. ColotCook supports multiple AI providers and gives you a powerful CLI tool for AI-assisted development — right from your terminal.

[![CI](https://github.com/tranhoangtu-it/ColotCook/actions/workflows/ci.yml/badge.svg)](https://github.com/tranhoangtu-it/ColotCook/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2021+-orange.svg)](https://www.rust-lang.org/)

## Demo

```
$ colotcook "explain the architecture of this project"

╭──────────────────────────────────────────────────────╮
│  ColotCook v0.1.0 · claude-opus-4-6 · workspace-write │
╰──────────────────────────────────────────────────────╯

I'll analyze the project structure for you.

> Reading Cargo.toml...
> Reading crates/api/src/lib.rs...
> Reading crates/runtime/src/lib.rs...

This is a Rust workspace with 7 crates:

1. **colotcook-api** — Provider abstraction layer supporting Anthropic,
   OpenAI, Gemini, xAI, and Ollama with unified streaming.

2. **colotcook-cli** — Terminal UI with markdown rendering, spinners,
   and syntax highlighting.

3. **colotcook-runtime** — Core agent loop: conversations, sessions,
   sandbox isolation, and permission enforcement.

4. **colotcook-tools** — 19 built-in tools (file ops, search, bash,
   web fetch, MCP integration, agent orchestration).

5. **colotcook-commands** — 15 slash commands for session management.

6. **colotcook-plugins** — Plugin discovery, hooks, and lifecycle.

7. **colotcook-telemetry** — Analytics and request tracing.
```

```
$ colotcook --model ollama:llama3 "fix the bug in src/parser.rs"

╭──────────────────────────────────────────╮
│  ColotCook v0.1.0 · llama3 · read-only  │
╰──────────────────────────────────────────╯

> Reading src/parser.rs...
> Found issue at line 42: off-by-one in token boundary check

I found the bug. The `next_token()` function uses `<` instead of `<=`
when checking the buffer boundary, causing it to skip the last character.

> Editing src/parser.rs...

Applied fix:
```diff
-    if self.pos < self.input.len() {
+    if self.pos <= self.input.len() {
```

> Running cargo test...
  23 passed, 0 failed

The fix is applied and all tests pass.
```

```
$ colotcook --resume latest /status

Session          ~/.colotcook/sessions/session-abc123.jsonl
Messages         14
Model            claude-opus-4-6
Permission mode  workspace-write
Token usage      12,847 input · 3,291 output
Estimated cost   $0.42
```

## Features

- **Pure Rust** — no Python runtime, no Node.js, just a single compiled binary
- **Multi-provider support** — Anthropic Claude, OpenAI/GPT, Google Gemini, xAI Grok, and Ollama (local models)
- **19 built-in tools** — file operations, shell execution, search, web fetch, MCP integration, and more
- **15 slash commands** — `/export`, `/config`, `/status`, `/clear`, `/compact`, and others
- **Session persistence** — resume conversations with `--resume` or `--resume latest`
- **Plugin & hook system** — extend behavior with PreToolUse/PostToolUse hooks
- **MCP protocol** — connect to external tool servers via stdio, SSE, HTTP, and more
- **Permission modes** — `read-only`, `workspace-write`, `danger-full-access`, `prompt`, `allow`
- **Streaming responses** — real-time SSE streaming from all providers
- **Prompt caching** — reduce API costs with intelligent prompt cache management
- **Sandbox isolation** — Linux namespace-based sandboxing for safe code execution
- **OAuth authentication** — secure token-based auth with auto-refresh
- **~2000 tests** — 89.5% line coverage, 90.1% region coverage

## Architecture

ColotCook is organized as a Rust workspace with 7 crates:

```
ColotCook/
├── Cargo.toml              # Workspace root
├── Cargo.lock              # Dependency lock file
├── deny.toml               # License & vulnerability auditing
├── CONTRIBUTING.md          # Development guidelines
├── CHANGELOG.md             # Release history
├── SECURITY.md              # Threat model & security practices
├── ARCHITECTURE.md          # Detailed architecture documentation
└── crates/
    ├── api/                # colotcook-api: Provider abstraction & streaming
    ├── cli/                # colotcook-cli: Binary entry point & terminal UI
    ├── commands/           # colotcook-commands: 15 slash commands
    ├── plugins/            # colotcook-plugins: Plugin lifecycle & hooks
    │   └── bundled/        # Built-in plugins (colotcook-guard, example-bundled, sample-hooks)
    ├── runtime/            # colotcook-runtime: Agent loop, sessions, sandbox, permissions
    ├── telemetry/          # colotcook-telemetry: Analytics & tracing
    └── tools/              # colotcook-tools: 19 tool specifications
```

## Supported Providers

| Provider | Models | Auth |
|----------|--------|------|
| **Anthropic** | claude-opus-4-6, claude-sonnet-4-6, claude-haiku-4-5 | `ANTHROPIC_API_KEY` |
| **OpenAI** | gpt-4o, gpt-4-turbo, o1, o3 | `OPENAI_API_KEY` |
| **Google Gemini** | gemini-2.5-pro, gemini-2.5-flash | `GEMINI_API_KEY` or `GOOGLE_API_KEY` |
| **xAI** | grok-3 | `XAI_API_KEY` |
| **Ollama** | llama3, codellama, deepseek-coder, qwen2.5-coder, ... | No key needed (local) |

## Quick Start

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (edition 2021+)
- An API key for at least one supported provider (or Ollama for local models)

### Build from source

```bash
git clone https://github.com/tranhoangtu-it/ColotCook.git
cd ColotCook
cargo build --release
```

The binary will be at `target/release/colotcook`.

### Usage

```bash
# Start a new conversation (default: Anthropic Claude)
colotcook "explain this codebase"

# Use a specific model
colotcook --model gemini-2.5-pro "review this PR"

# Use Ollama (local)
colotcook --model ollama:llama3 "write tests for this function"

# Resume a previous session
colotcook --resume latest "continue where we left off"

# Permission modes
colotcook --permission-mode workspace-write "refactor this module"

# JSON output for scripting
colotcook --output-format json "list all TODO comments"

# Slash commands in a resumed session
colotcook --resume session.jsonl /status
colotcook --resume session.jsonl /export notes.txt
colotcook --resume session.jsonl /config model
```

### Environment Variables

```bash
# Required for Anthropic (default provider)
export ANTHROPIC_API_KEY="sk-ant-..."

# Optional: other providers
export OPENAI_API_KEY="sk-..."
export GEMINI_API_KEY="..."
export XAI_API_KEY="..."

# Optional: custom base URLs
export ANTHROPIC_BASE_URL="https://..."
export OPENAI_BASE_URL="https://..."
export GEMINI_BASE_URL="https://..."
export OLLAMA_BASE_URL="http://localhost:11434/v1"
```

## Configuration

ColotCook loads settings from multiple sources (highest priority first):

1. CLI flags (`--model`, `--permission-mode`)
2. Project-local config: `.colotcook/settings.local.json`
3. Project config: `.colotcook/settings.json`
4. User config: `~/.colotcook/settings.json`

Use `/config` to inspect the merged configuration.

## Built-in Tools

ColotCook provides 19 tools for the AI agent:

| Category | Tools |
|----------|-------|
| **File operations** | Read, Write, Edit, MultiEdit, NotebookEdit |
| **Search** | Glob, Grep, Search |
| **Execution** | Bash, BashBackground |
| **Web** | WebFetch, WebSearch |
| **Session** | TodoRead, TodoWrite |
| **Integration** | McpTool, UseMcpServer |
| **Agent** | Agent, Task |
| **System** | AskFollowupQuestion |

## Slash Commands

| Command | Description |
|---------|-------------|
| `/help` | Show available commands |
| `/status` | Session info, model, usage |
| `/compact` | Compress conversation history |
| `/clear` | Reset current session |
| `/config` | Inspect merged configuration |
| `/model` | Switch AI model |
| `/permissions` | Change permission mode |
| `/cost` | Show token usage and cost |
| `/export` | Export conversation to file |
| `/resume` | Load a previous session |
| `/session` | List and manage sessions |
| `/diff` | Show git workspace changes |
| `/version` | Print version info |
| `/plugins` | Manage plugins |
| `/agents` | List available agents |

## Bundled Plugins

ColotCook ships with built-in plugins in `crates/plugins/bundled/`:

- **colotcook-guard** — Pre-execution safety checks via `pre-guard.sh` hook
- **example-bundled** — Example plugin demonstrating pre/post hook lifecycle
- **sample-hooks** — Sample hook implementations for reference

## Development

```bash
# Run all tests (~2000 tests)
cargo test

# Run tests for a specific crate
cargo test -p colotcook-api
cargo test -p colotcook-runtime

# Check without building
cargo check

# Lint (zero warnings enforced)
cargo clippy --workspace -- -D warnings

# Format
cargo fmt --all

# Security audit
cargo deny check

# Coverage report
./scripts/coverage.sh
```

## Code Quality

| Metric | Value |
|--------|-------|
| Clippy warnings | 0 (pedantic + all lints) |
| Test count | ~2000 |
| Line coverage | 89.5% |
| Region coverage | 90.1% |
| Production `.expect()` | 0 |
| `unsafe` code | Forbidden workspace-wide |
| CI pipeline | Format, Clippy, Test, Deny |

## License

MIT
