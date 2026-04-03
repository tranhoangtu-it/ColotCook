# Contributing to ColotCook

Thank you for your interest in contributing to ColotCook! This guide covers everything you need to get started.

## Development Setup

### Prerequisites

- Rust stable toolchain (1.75+): `rustup install stable`
- Additional components: `rustup component add rustfmt clippy`
- (Optional) cargo-deny for dependency auditing: `cargo install cargo-deny --locked`

### Building

```sh
# Build the workspace
cargo build --workspace

# Build the release binary
cargo build --release
```

## Workflow

Every change should pass all four CI gates before submitting a PR:

```sh
# 1. Format — must produce no diff
cargo fmt --check

# 2. Lint — all warnings are errors in CI
cargo clippy --workspace --all-targets

# 3. Tests — all tests must pass
cargo test --workspace

# 4. Dependency audit (requires cargo-deny)
cargo deny check
```

Running all gates at once:

```sh
cargo fmt --check && cargo clippy --workspace --all-targets && cargo test --workspace && cargo deny check
```

## Pull Request Guidelines

1. **Branch naming**: `feat/<description>`, `fix/<description>`, `docs/<description>`, `refactor/<description>`
2. **Commit messages**: Use [Conventional Commits](https://www.conventionalcommits.org/) — `feat:`, `fix:`, `docs:`, `chore:`, `refactor:`, `test:`
3. **Scope**: Keep each PR focused on a single concern; split large changes into a series of smaller PRs
4. **Tests**: Add or update tests for every behaviour change
5. **Docs**: Update `///` doc comments for any public API change; update `./docs/` for architectural changes
6. **Breaking changes**: Note in the PR description and prefix the commit footer with `BREAKING CHANGE:`

## File Ownership / Crate Layout

| Crate | Description |
|-------|-------------|
| `crates/api` | Provider HTTP clients, types, prompt caching |
| `crates/runtime` | Conversation loop, sessions, tools, MCP, sandbox, OAuth |
| `crates/telemetry` | Telemetry sinks and session tracing |
| `crates/tools` | Tool definitions (19 built-in tools) |
| `crates/commands` | Slash command implementations (15 commands) |
| `crates/plugins` | Plugin lifecycle and hook system |
| `crates/cli` | Binary entry point and terminal UI |

See `ARCHITECTURE.md` for a detailed description of each layer.

## Architecture References

- `ARCHITECTURE.md` — crate dependency graph and design decisions
- `docs/system-architecture.md` — component interactions and data flow
- `docs/code-standards.md` — naming conventions and coding style
- `SECURITY.md` — security model, permission modes, sandbox policy

## Security Considerations

- Never commit credentials, API keys, or `.env` files
- Sandbox behaviour is controlled by `SandboxConfig` in `crates/runtime/src/sandbox.rs`
- Permission prompts are enforced via `PermissionPolicy` in `crates/runtime/src/permissions.rs`
- OAuth tokens are stored under `~/.claude/` and protected by file-lock guards
- Report vulnerabilities privately per `SECURITY.md`
