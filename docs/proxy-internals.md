# Proxy Internals

This document describes the internal architecture and message flow of the `claude-proxy` binary.

## Overview

The proxy acts as a bridge between:
1. **Claude CLI** - The local Claude Code command-line tool
2. **Backend WebSocket** - The cc-proxy backend server
3. **Web Interface** - Users interacting via browser

```
┌─────────────┐     JSON Lines      ┌─────────────┐     WebSocket      ┌─────────────┐
│  Claude CLI │ ←────────────────→  │    Proxy    │ ←────────────────→ │   Backend   │
│  (stdin/out)│                     │             │                     │             │
└─────────────┘                     └─────────────┘                     └─────────────┘
                                                                              ↕
                                                                        ┌─────────────┐
                                                                        │  Frontend   │
                                                                        │  (Browser)  │
                                                                        └─────────────┘
```

## Message Types

### Claude CLI Protocol (via `claude-codes` crate)

The proxy communicates with Claude CLI using the JSON Lines protocol defined in the `claude-codes` crate:

**Input Messages (`ClaudeInput`)**:
```rust
ClaudeInput::User(UserMessage {
    message: MessageContent {
        role: "user",
        content: vec![ContentBlock::Text(TextBlock { text: "..." })]
    },
    session_id: Some("session-uuid")
})
```

**Output Messages (`ClaudeOutput`)**:
- `ClaudeOutput::System` - Initialization and metadata
- `ClaudeOutput::User` - Echoed user messages
- `ClaudeOutput::Assistant` - Claude's responses with content blocks:
  - `TextBlock` - Plain text responses
  - `ThinkingBlock` - Claude's reasoning (with signature)
  - `ToolUseBlock` - Tool invocations (name, id, input)
  - `ToolResultBlock` - Results from tool execution
- `ClaudeOutput::Result` - Session completion with cost/timing info

### WebSocket Protocol (`ProxyMessage`)

The proxy communicates with the backend using `shared::ProxyMessage`:

```rust
enum ProxyMessage {
    Register { session_name, auth_token, working_directory },
    ClaudeOutput { content: serde_json::Value },  // Raw ClaudeOutput JSON
    ClaudeInput { content: serde_json::Value },   // Text to send to Claude
    Heartbeat,
    Error { message },
    SessionStatus { status },
}
```

## Message Flow

### Startup Sequence

1. **Parse Configuration**
   - Load saved auth from `~/.config/cc-proxy/config.json`
   - Parse CLI arguments (`--init`, `--backend-url`, etc.)

2. **Authentication**
   - If `--init` provided: Extract JWT token and save to config
   - If `--dev` mode: Skip authentication entirely
   - Otherwise: Use cached token or run device flow OAuth

3. **Connect to Backend**
   ```
   Proxy → Backend: WebSocket connect to /ws/session
   Proxy → Backend: ProxyMessage::Register { session_name, auth_token, cwd }
   ```

4. **Spawn Claude CLI**
   - Uses `ClaudeCliBuilder` with flags:
     - `--print` - Non-interactive mode
     - `--output-format stream-json` - JSON output
     - `--input-format stream-json` - JSON input
     - `--verbose` - Required for streaming
     - `--session-id <uuid>` - Unique session identifier

### Message Forwarding

**User Input (Frontend → Claude)**:
```
Frontend → Backend: ProxyMessage::ClaudeInput { content: "hello" }
Backend → Proxy: (via WebSocket)
Proxy: Constructs ClaudeInput::user_message("hello", session_id)
Proxy → Claude: JSON line to stdin
```

**Claude Response (Claude → Frontend)**:
```
Claude → Proxy: JSON line from stdout (ClaudeOutput)
Proxy: Parses as ClaudeOutput, logs message type
Proxy → Backend: ProxyMessage::ClaudeOutput { content: raw_json }
Backend → Frontend: (via WebSocket broadcast)
```

## Async Task Structure

The proxy uses `claude_codes::AsyncClient` for type-safe communication with Claude CLI.
It spawns concurrent tasks coordinated via channels:

```rust
// Create the Claude client
let builder = ClaudeCliBuilder::new().session_id(&session_id);
let mut claude_client = AsyncClient::from_builder(builder).await?;

// Channels for coordination
let (output_tx, output_rx) = mpsc::unbounded_channel::<ClaudeOutput>();
let (input_tx, input_rx) = mpsc::unbounded_channel::<String>();

// Task 1: Forward Claude outputs to Backend
tokio::spawn(async {
    while let Some(output) = output_rx.recv().await {
        let content = serde_json::to_value(&output)?;
        ws_write.send(ProxyMessage::ClaudeOutput { content }).await;
    }
});

// Task 2: Read WebSocket → Send to input channel
tokio::spawn(async {
    while let msg = ws_read.next().await {
        if let ProxyMessage::ClaudeInput { content } = msg {
            input_tx.send(text);
        }
    }
});

// Task 3: Read Claude stderr → Log warnings
tokio::spawn(async {
    let mut stderr = claude_client.take_stderr();
    while let line = stderr.read_line().await {
        warn!("Claude stderr: {}", line);
    }
});

// Main loop: Coordinate sends and receives
loop {
    tokio::select! {
        Some(text) = input_rx.recv() => {
            let input = ClaudeInput::user_message(&text, &session_id);
            claude_client.send(&input).await?;
        }
        result = claude_client.receive() => {
            output_tx.send(result?);
        }
    }
}
```

## Configuration Storage

Config is stored in `~/.config/cc-proxy/config.json`:

```json
{
  "session_auths": {
    "/path/to/project": {
      "user_id": "uuid",
      "auth_token": "jwt-token",
      "user_email": "user@example.com",
      "last_used": "2024-01-01T00:00:00Z",
      "backend_url": "wss://server.com",
      "session_prefix": "my-prefix"
    }
  }
}
```

Auth is keyed by working directory, allowing different credentials per project.

## Error Handling

- **Connection failures**: Logged and cause graceful shutdown
- **Parse errors**: Logged as warnings, raw content still forwarded
- **Claude CLI crashes**: Detected via stdout EOF, triggers cleanup
- **WebSocket close**: Triggers session status update in backend

## Rich Content Support

The `ClaudeOutput` types preserve rich content for frontend rendering:

- **Thinking blocks**: Can be shown/hidden in UI
- **Tool use**: Display tool name, inputs, and results
- **Cost tracking**: `ResultMessage` includes `total_cost_usd`
- **Model info**: `AssistantMessage` includes model name
- **Usage stats**: Input/output token counts

The backend stores these as raw JSON, allowing the frontend to render appropriate UI for each content type.
