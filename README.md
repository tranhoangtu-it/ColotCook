# ColotCook

A production-ready AI coding agent written in pure Rust. ColotCook supports multiple AI providers and gives you a powerful CLI tool for AI-assisted development — right from your terminal.

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

## Architecture

ColotCook is organized as a Rust workspace with 7 crates:

```
ColotCook/
├── Cargo.toml              # Workspace root
├── Cargo.lock              # Dependency lock file
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

## Bundled Plugins

ColotCook ships with built-in plugins in `crates/plugins/bundled/`:

- **colotcook-guard** — Pre-execution safety checks via `pre-guard.sh` hook
- **example-bundled** — Example plugin demonstrating pre/post hook lifecycle
- **sample-hooks** — Sample hook implementations for reference

## Development

```bash
# Run all tests
cargo test

# Run tests for a specific crate
cargo test -p colotcook-api
cargo test -p colotcook-runtime

# Check without building
cargo check

# Lint
cargo clippy

# Format
cargo fmt
```

## License

MIT
