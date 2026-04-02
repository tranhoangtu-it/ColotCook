# Security Policy

## Overview

ColotCook is an AI coding agent that executes LLM-generated commands on your system.
This document describes the security architecture, known limitations, and best practices.

## Threat Model

ColotCook operates with the permissions of the invoking user. The primary threats are:

1. **LLM-generated malicious commands** — The AI model may produce harmful shell commands
2. **Prompt injection** — Malicious content in files or web pages influencing the agent
3. **Credential theft** — Stored API keys and OAuth tokens being exfiltrated
4. **Resource exhaustion** — Runaway processes consuming CPU, memory, or disk

## Security Controls

### Sandbox Isolation

- **Linux namespace isolation** via `unshare` (user, mount, IPC, PID, UTS)
- **Network isolation** (optional, disabled by default)
- **Filesystem isolation** modes: `off`, `workspace-only`, `allow-list`
- **Resource limits** via `ulimit`: CPU time, memory, file descriptors, processes, file size
- **Container detection**: Automatic detection of Docker, Podman, Kubernetes environments

**Limitations:**
- Sandbox relies on Linux `unshare`; not available on macOS or Windows
- Filesystem isolation uses environment variables that subprocesses must voluntarily respect
- No seccomp or eBPF filtering (planned for future releases)
- Resource limits are advisory on some systems

### Permission System

- **Five permission modes**: `read-only`, `workspace-write`, `danger-full-access`, `prompt`, `allow`
- **Tool-level permissions** with configurable rules per tool name
- **Dangerous pattern detection** for common destructive commands
- **Path traversal validation** prevents access to sensitive system directories
- **Audit logging** available via `COLOTCOOK_AUDIT_LOG=1` environment variable

### Credential Storage

- Credentials stored in `~/.claw/credentials.json`
- **File permissions**: Automatically set to `0600` (owner read/write only) on Unix
- **Directory permissions**: Parent directory set to `0700` on Unix
- **File locking**: Advisory lock prevents concurrent write corruption
- **Permission auditing**: Warns and auto-fixes overly permissive file modes

**Recommendation:** For maximum security, consider using a secrets manager or system keyring instead of file-based storage.

### API Safety

- **Rate limiting**: Configurable via `COLOTCOOK_RATE_LIMIT_RPM` and `COLOTCOOK_RATE_LIMIT_TPM`
- **Conversation timeout**: Default 30 minutes, configurable
- **Iteration limit**: Default 200 iterations per conversation
- **Token budgets**: Auto-compaction at configurable token thresholds

## Configuration

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `COLOTCOOK_AUDIT_LOG` | Enable permission audit logging | disabled |
| `COLOTCOOK_LOG_LEVEL` | Minimum log level (debug/info/warn/error) | info |
| `COLOTCOOK_LOG_FORMAT` | Log format (text/json) | text |
| `COLOTCOOK_RATE_LIMIT_RPM` | Max API requests per minute | 60 |
| `COLOTCOOK_RATE_LIMIT_TPM` | Max tokens per minute | 1000000 |
| `COLOTCOOK_AUTO_COMPACT_INPUT_TOKENS` | Auto-compaction threshold | 100000 |
| `COLOTCOOK_SANDBOX_FILESYSTEM_MODE` | Sandbox filesystem mode | workspace-only |

## Reporting Vulnerabilities

If you discover a security vulnerability, please report it responsibly:

1. **Do NOT** create a public GitHub issue
2. Email: security@colotcook.dev
3. Include: description, steps to reproduce, impact assessment
4. We will respond within 48 hours

## Security Best Practices

1. **Use `workspace-write` permission mode** unless you specifically need full access
2. **Review generated commands** before allowing execution in interactive mode
3. **Enable audit logging** in production with `COLOTCOOK_AUDIT_LOG=1`
4. **Set rate limits** appropriate for your usage pattern
5. **Keep ColotCook updated** to get the latest security fixes
6. **Use a dedicated API key** with minimal required scopes
7. **Run in a container** for additional isolation when processing untrusted code
