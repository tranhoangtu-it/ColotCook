#!/bin/sh
# ColotCook Guard - Pre-tool-use hook
# Blocks dangerous shell commands before they execute.
# Exit code 0 = allow, exit code 2 = deny

# Only check Bash tool calls
if [ "$HOOK_TOOL_NAME" != "Bash" ] && [ "$HOOK_TOOL_NAME" != "bash" ]; then
    exit 0
fi

INPUT="$HOOK_TOOL_INPUT"

# Dangerous patterns to block
check_pattern() {
    pattern="$1"
    reason="$2"
    if printf '%s' "$INPUT" | grep -qiE "$pattern"; then
        printf '{"decision":"block","reason":"%s"}\n' "$reason"
        exit 2
    fi
}

# Block destructive recursive removal of root or home
check_pattern 'rm\s+(-[a-zA-Z]*f[a-zA-Z]*\s+)?/' "Blocked: recursive file deletion at root path"
check_pattern 'rm\s+-[a-zA-Z]*r[a-zA-Z]*\s+(-[a-zA-Z]*\s+)*/\s*$' "Blocked: rm -rf /"
check_pattern 'rm\s+-[a-zA-Z]*r[a-zA-Z]*\s+(-[a-zA-Z]*\s+)*~' "Blocked: rm -rf ~ (home directory)"

# Block sudo usage
check_pattern '^\s*sudo\s' "Blocked: sudo is not allowed in agent mode"
check_pattern ';\s*sudo\s' "Blocked: sudo is not allowed in agent mode"
check_pattern '\|\s*sudo\s' "Blocked: sudo is not allowed in agent mode"

# Block format/disk operations
check_pattern 'mkfs\.' "Blocked: filesystem format commands are not allowed"
check_pattern 'dd\s+.*of=/dev/' "Blocked: direct disk write (dd) is not allowed"

# Block network exfiltration of sensitive files
check_pattern 'curl.*(/etc/passwd|/etc/shadow|\.ssh/|\.env)' "Blocked: potential data exfiltration"
check_pattern 'wget.*(/etc/passwd|/etc/shadow|\.ssh/|\.env)' "Blocked: potential data exfiltration"

# Block fork bombs
check_pattern ':\(\)\{' "Blocked: fork bomb detected"
check_pattern ':\s*\(\s*\)\s*\{' "Blocked: fork bomb detected"

# If none matched, allow
exit 0
