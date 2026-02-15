# Launcher Roadmap

## Current State

The launcher daemon can spawn proxy processes and track their PIDs, but it
treats them as fire-and-forget subprocesses. Stdout/stderr are piped but
**never read** — all proxy output is silently discarded. There is no way to
see proxy logs from the web UI, no session-correlated logging, and no
structured process lifecycle management.

## Goal

Turn the launcher into a proper process supervisor that wraps each proxy
child, captures its output, tags log lines with the session UUID, and
forwards structured logs to the backend for storage and display.

---

## Task DAG

```
                    ┌────────────────────────────┐
                    │ Stage 0: Foundation        │
                    │                            │
                    │  0a. Refactor spawn to     │
                    │      wrap child I/O        │
                    │                            │
                    │  0b. Add ProxyLog message  │
                    │      to shared protocol    │
                    └──────┬────────┬────────────┘
                           │        │
              ┌────────────┘        └─────────────┐
              ▼                                    ▼
 ┌─────────────────────────┐        ┌──────────────────────────┐
 │ Stage 1a: Launcher      │        │ Stage 1b: Proxy          │
 │  stdout/stderr capture  │        │  session-tagged logging  │
 │                         │        │                          │
 │  - Async line readers   │        │  - --session-id CLI flag │
 │    on child stdout/err  │        │  - tracing span with     │
 │  - Tag each line with   │        │    session_id field      │
 │    session UUID + level │        │  - JSON log format when  │
 │  - Local ring buffer    │        │    launched (not TTY)    │
 │    per session          │        │                          │
 └──────────┬──────────────┘        └──────────┬───────────────┘
            │                                   │
            └──────────┬────────────────────────┘
                       ▼
         ┌──────────────────────────────┐
         │ Stage 2: Log Forwarding      │
         │                              │
         │  - Launcher sends ProxyLog   │
         │    messages over WebSocket   │
         │  - Rate limiting / batching  │
         │  - Backend handler stores    │
         │    logs in DB                │
         │  - REST endpoint to query    │
         │    logs by session           │
         └──────────────┬───────────────┘
                        │
         ┌──────────────┴───────────────┐
         │                              │
         ▼                              ▼
┌─────────────────────┐   ┌──────────────────────────┐
│ Stage 3a: Frontend  │   │ Stage 3b: Lifecycle      │
│  log viewer         │   │  notifications           │
│                     │   │                          │
│  - Log panel per    │   │  - SessionExited message │
│    session          │   │    from launcher         │
│  - Live streaming   │   │  - Exit code + signal    │
│    via WebSocket    │   │  - Auto-cleanup of DB    │
│  - Level filtering  │   │    session status        │
│  - Search / scroll  │   │  - Frontend toast on     │
│                     │   │    unexpected exit       │
└─────────────────────┘   └──────────────────────────┘
```

Stages 1a and 1b are independent and can be worked in parallel.
Stages 3a and 3b are independent and can be worked in parallel.

---

## Stage 0: Foundation

### 0a. Refactor ProcessManager to wrap child I/O

**File:** `launcher/src/process_manager.rs`

Currently `ManagedProcess` holds a bare `Child`. Change it to take
ownership of the child's stdout/stderr handles and spawn async reader
tasks for each.

```
ManagedProcess {
    pid: u32,
    child: Child,
}
```
becomes:
```
ManagedProcess {
    pid: u32,
    child: Child,
    log_rx: mpsc::UnboundedReceiver<LogLine>,
    reader_handles: Vec<JoinHandle<()>>,
}
```

Each reader task reads lines from `BufReader<ChildStdout>` /
`BufReader<ChildStderr>`, parses them, and sends `LogLine` structs into
the channel. The launcher's supervision loop drains these channels.

### 0b. Add `ProxyLog` to shared protocol

**File:** `shared/src/lib.rs`

```rust
ProxyLog {
    session_id: Uuid,
    level: String,      // "error", "warn", "info", "debug", "trace"
    message: String,
    timestamp: String,  // ISO 8601
}
```

---

## Stage 1a: Launcher stdout/stderr capture

**Files:** `launcher/src/process_manager.rs`, `launcher/src/connection.rs`

- Spawn two `tokio::io::BufReader` line-reader tasks per child process
  (one for stdout, one for stderr).
- Parse JSON-structured log lines when the proxy outputs them. Fall back
  to treating raw text as `info`-level.
- Store recent lines in a per-session ring buffer (e.g. last 500 lines)
  so the launcher can serve a snapshot on reconnect.
- Surface log lines through a channel that the connection loop can drain
  and forward.

---

## Stage 1b: Proxy session-tagged logging (#323)

**Files:** `proxy/src/main.rs`, launcher spawn code

- Add `--session-id <UUID>` CLI flag to the proxy.
- When set, wrap `tracing_subscriber` with a default span containing
  `session_id`.
- Switch to JSON log format (`tracing_subscriber::fmt::format::json()`)
  when stdout is not a TTY (i.e. launched by the daemon). This gives
  the launcher structured fields to parse.
- The launcher sets `--session-id` when spawning, using the UUID it
  generates in `ProcessManager::spawn()`.

---

## Stage 2: Log Forwarding

**Files:** `launcher/src/connection.rs`, `backend/src/handlers/websocket/launcher_socket.rs`, new `backend/src/handlers/logs.rs`, migration for `proxy_logs` table

- Launcher drains log channels and sends `ProxyLog` messages over its
  existing WebSocket connection to the backend.
- Batch up to N lines per WebSocket frame to avoid per-line overhead.
- Backend handler stores logs in a `proxy_logs` table:
  ```sql
  CREATE TABLE proxy_logs (
      id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
      session_id UUID NOT NULL REFERENCES sessions(id),
      level VARCHAR(10) NOT NULL,
      message TEXT NOT NULL,
      timestamp TIMESTAMPTZ NOT NULL,
      created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
  );
  CREATE INDEX idx_proxy_logs_session ON proxy_logs(session_id, timestamp);
  ```
- Add `GET /api/sessions/:id/logs?level=&limit=&before=` REST endpoint
  for paginated log retrieval.
- Apply backpressure: if the backend can't keep up, the launcher drops
  older debug/trace lines first.

---

## Stage 3a: Frontend log viewer

**Files:** `frontend/src/components/log_viewer.rs`, `frontend/src/pages/session/`

- Add a collapsible log panel to the session detail view.
- Fetch initial logs via REST, then subscribe to live updates via the
  existing WebSocket connection.
- Level-based filtering (toggle error/warn/info/debug).
- Auto-scroll with a "pinned to bottom" toggle.
- Monospace, syntax-highlighted log lines with timestamps.

---

## Stage 3b: Process lifecycle notifications

**Files:** `shared/src/lib.rs`, `launcher/src/connection.rs`, `backend/src/handlers/websocket/launcher_socket.rs`

- Add `SessionExited` message:
  ```rust
  SessionExited {
      session_id: Uuid,
      exit_code: Option<i32>,
      signal: Option<i32>,
  }
  ```
- Launcher sends this when `reap_exited()` detects a child has exited,
  along with the last N log lines as context.
- Backend updates the session status to `disconnected` and broadcasts
  to the user's web clients.
- Frontend shows a toast notification for unexpected exits (non-zero
  exit code).

---

## Cleanup / Polish (post Stage 3)

- Remove redundant `--auth-token` CLI arg (already passed via
  `PORTAL_AUTH_TOKEN` env var).
- Remove `--foreground` from service files (not a real CLI flag) or add
  it as a no-op for compat.
- Add install script for systemd/launchd service setup.
- Add launcher config file support (`~/.config/claude-portal/launcher.toml`)
  so users don't need to pass all args on the command line.
- `StopSession` button in the frontend session view.
- Launcher selection UI in `LaunchDialog` (show name, hostname, load).
- Log rotation / TTL for the `proxy_logs` table.
