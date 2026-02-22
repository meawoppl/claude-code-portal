# Portal Setup — Local Development

## Overview

Two changes are needed to route VS Code Claude Code sessions through the portal:

1. A shim script that wraps the `claude` binary
2. A VS Code setting that points to the shim

## Changes Made

### 1. Shim Script

**File**: `~/.claude/claude-portal-shim.sh`

```bash
#!/bin/bash
exec claude-portal --shim --backend-url ws://localhost:3000 -- "$@"
```

This intercepts every VS Code Claude session and routes it through the portal proxy in `--shim` mode. Claude still works normally — the shim transparently tees output to the portal backend.

### 2. VS Code Setting

**File**: VS Code `settings.json` (global)

```json
"claudeCode.claudeProcessWrapper": "/path/to/.claude/claude-portal-shim.sh"
```

This tells the Claude Code extension to use the shim as a process wrapper when launching Claude. **Note**: the old setting name `claude-code.claudeBinaryPath` does not work — the correct setting is `claudeCode.claudeProcessWrapper`.

## Starting the Portal

```bash
./scripts/up.sh
```

This starts Docker Desktop (if needed), PostgreSQL, runs migrations, builds the frontend, and starts the backend. The portal is then available at http://localhost:3000/.

## How It Works

```
VS Code Extension
    → launches claude-portal-shim.sh (instead of claude)
    → claude-portal --shim spawns the real claude binary
    → stdin/stdout pass through transparently to VS Code
    → output is also forwarded to portal backend via WebSocket
```

If the portal backend is down, Claude still works normally in VS Code — the shim degrades gracefully.

## Undoing These Changes

1. Delete `~/.claude/claude-portal-shim.sh`
2. Remove the `claudeCode.claudeProcessWrapper` line from VS Code settings
