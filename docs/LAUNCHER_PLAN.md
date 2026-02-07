# Persistent Launcher Service for Claude Portal

## Problem

Session creation is manual: user gets an init URL from the web UI, then pastes `claude-portal --init <url>` in their terminal. There's no way for the web UI to directly launch a proxy instance on the host.

## Solution

A new `launcher` daemon binary that runs on the host OS, connects to the backend via WebSocket, and spawns `claude-portal` child processes on demand when the user clicks "Launch Session" in the browser.

```
User clicks "Launch Session" in browser
  → Frontend sends POST /api/launch to backend
  → Backend forwards LaunchSession to registered launcher via WS
  → Launcher spawns `claude-portal` child process
  → `claude-portal` connects back to backend (existing flow)
  → Session appears live in the web UI
```

## Architecture Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Separate binary vs embedded in backend | **Separate `claude-portal-launcher` binary** | Backend runs in Docker; launcher must run on host to access user's filesystem and `claude` binary |
| Communication model | **WebSocket push from backend** | Low latency (user expects immediate response), matches existing proxy pattern, supports multi-launcher |
| Auth model | **Reuse existing JWT proxy tokens** | Same device flow, same token infrastructure. Add `token_type` field to distinguish launcher vs proxy |
| Daemon management | **systemd (Linux) / launchd (macOS)** | Standard OS-level service management with restart-on-crash |

---

## Implementation Phases

### Phase 1: Protocol Extensions (`shared/src/lib.rs`)

Add new `ProxyMessage` variants (all WASM-compatible serde types):

```rust
// Launcher registration (launcher → backend)
LauncherRegister {
    launcher_id: Uuid,
    launcher_name: String,          // e.g. "matt-laptop"
    auth_token: Option<String>,     // JWT
    hostname: String,
    version: Option<String>,
}

// Registration acknowledgment (backend → launcher)
LauncherRegisterAck {
    success: bool,
    launcher_id: Uuid,
    error: Option<String>,
}

// Launch request (backend → launcher)
LaunchSession {
    request_id: Uuid,
    user_id: Uuid,
    auth_token: String,             // Fresh short-lived token for the child process
    working_directory: String,
    session_name: Option<String>,
    claude_args: Vec<String>,
}

// Launch result (launcher → backend)
LaunchSessionResult {
    request_id: Uuid,
    success: bool,
    session_id: Option<Uuid>,
    pid: Option<u32>,
    error: Option<String>,
}

// Stop request (backend → launcher)
StopSession { session_id: Uuid }

// Periodic health (launcher → backend)
LauncherHeartbeat {
    launcher_id: Uuid,
    running_sessions: Vec<Uuid>,
    uptime_secs: u64,
}
```

Add shared types:
```rust
pub enum ProcessState { Starting, Running, Stopping, Exited, Crashed }

pub struct LauncherInfo {
    pub launcher_id: Uuid,
    pub launcher_name: String,
    pub hostname: String,
    pub connected: bool,
    pub running_sessions: u32,
}
```

**Files:** `shared/src/lib.rs`

---

### Phase 2: Launcher Crate (new `launcher/` crate)

New workspace member: `launcher/`

#### `launcher/src/main.rs` — CLI entry point

CLI args (modeled after `proxy/src/main.rs`):
- `--backend-url <URL>` — backend WebSocket URL
- `--auth-token <JWT>` — launcher auth token
- `--name <NAME>` — launcher display name (default: hostname)
- `--proxy-path <PATH>` — path to `claude-portal` binary (default: `claude-portal` in PATH)
- `--max-processes <N>` — max concurrent processes (default: 5)
- `--dev` — dev mode
- `--foreground` — don't daemonize
- `--init <URL>` — first-time setup (reuse proxy's init URL flow)
- `--install-service` / `--uninstall-service` — install/remove systemd/launchd unit

Startup flow:
1. Parse args, load config from `~/.config/claude-code-portal/launcher.json`
2. Resolve auth token (device flow or cached, same as proxy)
3. Connect to backend WebSocket at `/ws/launcher`
4. Send `LauncherRegister`
5. Enter main loop: handle `LaunchSession`, `StopSession`, supervise children

#### `launcher/src/process_manager.rs` — Spawn and track child processes

```rust
struct ManagedProcess {
    session_id: Uuid,
    pid: u32,
    child: tokio::process::Child,
    started_at: DateTime<Utc>,
    working_directory: String,
    user_id: Uuid,
}

struct ProcessManager {
    processes: HashMap<Uuid, ManagedProcess>,
    proxy_path: PathBuf,
    backend_url: String,
    max_processes: usize,
}
```

Key methods:
- `spawn(req)` — runs `tokio::process::Command` for `claude-portal`, sets `current_dir`, passes auth token via env var (not CLI arg, to avoid `/proc` exposure)
- `stop(session_id)` — sends SIGTERM, then SIGKILL after timeout
- `check_exited()` — polls `try_wait()` on all children, returns exited sessions

#### `launcher/src/connection.rs` — WebSocket loop

Modeled after `proxy/src/session.rs` main loop pattern:
- Connects to `/ws/launcher`, sends `LauncherRegister`
- Reconnects with exponential backoff on disconnect
- Handles incoming: `LaunchSession`, `StopSession`
- Sends outgoing: `LaunchSessionResult`, `LauncherHeartbeat` (every 30s)
- Supervision check every 1s for exited children

**Files:** `launcher/Cargo.toml`, `launcher/src/main.rs`, `launcher/src/process_manager.rs`, `launcher/src/connection.rs`

---

### Phase 3: Backend Integration

#### New WebSocket handler: `backend/src/handlers/websocket/launcher_socket.rs`

New endpoint: `GET /ws/launcher`

Follows same pattern as `proxy_socket.rs`:
1. Receive `LauncherRegister`, validate auth token
2. Register launcher in `SessionManager`
3. Message loop: forward `LaunchSession` requests, receive results
4. On disconnect: mark launcher offline, fail pending requests

#### Extend `SessionManager` (`backend/src/handlers/websocket/mod.rs`)

Add field:
```rust
pub launchers: Arc<DashMap<Uuid, LauncherConnection>>,
```

New methods:
- `register_launcher(id, connection)`
- `unregister_launcher(id)`
- `get_launchers_for_user(user_id) -> Vec<LauncherInfo>`
- `send_to_launcher(id, msg) -> bool`

#### New REST endpoints: `backend/src/handlers/launchers.rs`

```
GET  /api/launchers      — List connected launchers for current user
POST /api/launch         — Request a new session launch
```

`POST /api/launch` flow:
1. Auth user from session cookie
2. Find launcher (explicit ID or auto-select by user)
3. Mint a fresh short-lived proxy token for the child process
4. Send `LaunchSession` to launcher via WebSocket
5. Return `{ "request_id": "uuid" }`

**Files:** `backend/src/handlers/websocket/launcher_socket.rs` (new), `backend/src/handlers/websocket/mod.rs` (modify), `backend/src/handlers/launchers.rs` (new), `backend/src/main.rs` (modify router)

---

### Phase 4: Frontend Integration

#### New component: `frontend/src/components/launch_dialog.rs`

Dialog showing:
- List of connected launchers (from `GET /api/launchers`)
- Working directory text input
- Optional session name input
- "Launch" button

#### Dashboard modifications

- Show "Launch Session" button when launchers are connected
- Handle `LaunchSessionResult` in WebSocket message handler
- Show toast/notification on launch success/failure

**Files:** `frontend/src/components/launch_dialog.rs` (new), `frontend/src/pages/dashboard/page.rs` (modify), `frontend/src/pages/dashboard/session_rail.rs` (modify)

---

### Phase 5: Auth Token Extensions (`shared/src/proxy_tokens.rs`)

Add `token_type` field to `ProxyTokenClaims`:
```rust
#[serde(default = "default_proxy")]
pub token_type: String,  // "proxy" or "launcher"
```

Launcher tokens get longer expiration (365 days vs default 30 days). Backend routes by endpoint (`/ws/session` vs `/ws/launcher`), not token type.

**Files:** `shared/src/proxy_tokens.rs`, `backend/src/handlers/proxy_tokens.rs`

---

### Phase 6: Daemon Packaging

Service file templates in `launcher/service/`:
- `claude-portal-launcher.service` (systemd)
- `com.claude-portal.launcher.plist` (launchd)

`--install-service` / `--uninstall-service` CLI commands to register them.

---

## Security Model

| Concern | Mitigation |
|---------|-----------|
| Launcher has host access | Runs as unprivileged user (same user who installs it) |
| Token scope | Launcher token scoped to single user; children get fresh short-lived tokens |
| Working directory | Launcher validates path exists and is accessible before spawning |
| Runaway spawning | `--max-processes` limit (default 5) |
| Token in process args | Pass auth token via env var, not CLI arg |

---

## File Summary

| Action | File |
|--------|------|
| Modify | `Cargo.toml` (workspace members) |
| Modify | `shared/src/lib.rs` (new ProxyMessage variants) |
| Modify | `shared/src/proxy_tokens.rs` (token_type field) |
| **New** | `launcher/Cargo.toml` |
| **New** | `launcher/src/main.rs` |
| **New** | `launcher/src/process_manager.rs` |
| **New** | `launcher/src/connection.rs` |
| **New** | `launcher/service/claude-portal-launcher.service` |
| **New** | `launcher/service/com.claude-portal.launcher.plist` |
| **New** | `backend/src/handlers/websocket/launcher_socket.rs` |
| **New** | `backend/src/handlers/launchers.rs` |
| Modify | `backend/src/handlers/websocket/mod.rs` (SessionManager) |
| Modify | `backend/src/main.rs` (routes) |
| Modify | `backend/src/handlers/proxy_tokens.rs` (launcher tokens) |
| **New** | `frontend/src/components/launch_dialog.rs` |
| Modify | `frontend/src/pages/dashboard/page.rs` |
| Modify | `frontend/src/pages/dashboard/session_rail.rs` |

## Recommended Build Order

Phase 1 (protocol) → Phase 2 (launcher binary) → Phase 3 (backend) → Phase 5 (auth) → Phase 4 (frontend) → Phase 6 (daemon packaging)

## Verification

1. `cargo test --workspace` / `cargo clippy --workspace` / `cargo fmt --check`
2. Start backend with `./scripts/dev.sh start`
3. Run launcher with `cargo run -p launcher -- --dev --foreground`
4. Open browser, click "Launch Session", enter working directory
5. Verify proxy spawns and session appears in dashboard
6. Send a message, verify Claude responds
7. Stop session from UI, verify child process terminates
8. Kill launcher, verify it reconnects on restart
