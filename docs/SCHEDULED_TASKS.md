# Scheduled Tasks (Cron Jobs)

Design doc for adding periodic agent tasks to Agent Portal, managed by the launcher (`agent-portal`) binary.

## Motivation

Users want agents to perform recurring work — nightly code reviews, periodic dependency audits, daily report generation, etc. The launcher already manages session lifecycles and is unique per hostname, making it the natural place to own scheduling.

## Core Concepts

### Session Preservation

Scheduled tasks **resume the same Claude session** across runs rather than creating a fresh one each time. This lets the agent build context incrementally — a nightly code reviewer remembers what it reviewed yesterday, a dependency auditor tracks which upgrades it already attempted.

The existing resume mechanism (`--resume <session-id>`) handles this. If Claude's local session data is lost (machine wipe, etc.), the `SessionNotFound` retry logic already creates a fresh session and marks the old one as `replaced`.

### Launcher-Driven Scheduling

The launcher is unique per `(hostname, user_id)` and already manages process lifecycles. Scheduling lives here rather than the backend because:

- The launcher knows which directories exist on the local machine
- No distributed coordination needed — one launcher, one schedule
- Tasks can fire even during brief backend disconnects (queued and reported on reconnect)
- Cron expressions are evaluated locally, avoiding clock skew issues

The backend stores task definitions for persistence and UI display, but the launcher is the scheduler.

## Data Model

### New Table: `scheduled_tasks`

```sql
CREATE TABLE scheduled_tasks (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID NOT NULL REFERENCES users(id),
    name            VARCHAR(255) NOT NULL,
    cron_expression VARCHAR(128) NOT NULL,       -- standard 5-field cron
    working_directory TEXT NOT NULL,
    prompt          TEXT NOT NULL,                -- initial message sent to agent
    claude_args     JSONB NOT NULL DEFAULT '[]',  -- extra CLI args
    agent_type      VARCHAR(16) NOT NULL DEFAULT 'claude',
    enabled         BOOLEAN NOT NULL DEFAULT true,
    max_runtime_minutes INTEGER NOT NULL DEFAULT 30,
    last_run_at     TIMESTAMP,
    next_run_at     TIMESTAMP,
    created_at      TIMESTAMP NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_scheduled_tasks_user_id ON scheduled_tasks(user_id);
```

### New Column on `sessions`

```sql
ALTER TABLE sessions ADD COLUMN scheduled_task_id UUID REFERENCES scheduled_tasks(id) ON DELETE SET NULL;
```

This links every cron-spawned session back to its parent task. It serves double duty:

1. **Frontend**: session pills can show a cron badge when `scheduled_task_id.is_some()`
2. **Run history**: `SELECT * FROM sessions WHERE scheduled_task_id = ? ORDER BY created_at DESC` gives full history

### Session Reuse Strategy

Each scheduled task has **one long-lived session**. The flow:

1. First run: launcher creates a new session with `resume: false`, stores the `session_id` locally
2. Subsequent runs: launcher spawns with `resume: true` using the stored `session_id`
3. If `SessionNotFound`: the existing retry logic creates a fresh session (new UUID) and marks the old one `replaced`; the launcher updates its stored `session_id`

The launcher persists the mapping `task_id → session_id` in its local config file (`~/.config/claude-code-portal/scheduled_tasks.json`).

## Protocol Changes

### `shared/src/endpoints.rs`

Extend the launcher protocol:

```rust
// Backend → Launcher: sync task definitions on connect and on changes
ServerToLauncher::ScheduleSync {
    tasks: Vec<ScheduledTaskConfig>,
}

// Launcher → Backend: report that a scheduled run started
LauncherToServer::ScheduledRunStarted {
    task_id: Uuid,
    session_id: Uuid,
}

// Launcher → Backend: report that a scheduled run completed
LauncherToServer::ScheduledRunCompleted {
    task_id: Uuid,
    session_id: Uuid,
    exit_code: Option<i32>,
    duration_secs: u64,
}
```

```rust
pub struct ScheduledTaskConfig {
    pub id: Uuid,
    pub name: String,
    pub cron_expression: String,
    pub working_directory: String,
    pub prompt: String,
    pub claude_args: Vec<String>,
    pub agent_type: AgentType,
    pub enabled: bool,
    pub max_runtime_minutes: i32,
}
```

### `shared/src/lib.rs`

Add to `SessionInfo`:

```rust
pub scheduled_task_id: Option<Uuid>,
```

Add to `RegisterFields`:

```rust
pub scheduled_task_id: Option<Uuid>,
```

## Launcher Changes

### New Module: `launcher/src/scheduler.rs`

The scheduler is a long-lived tokio task that:

1. **Receives task configs** via `ScheduleSync` from backend on connect
2. **Evaluates cron expressions** using the `croner` crate (lightweight, no-std compatible)
3. **Fires tasks** at the right time by calling `ProcessManager::spawn()` with `resume: true` and the stored `session_id`
4. **Sends the prompt** as the first message after the session connects (via a new `ProxyToServer::ScheduledInput` or by having the launcher inject it through the backend)
5. **Enforces max runtime** — kills the process after `max_runtime_minutes`
6. **Reports results** back to backend via `ScheduledRunStarted` / `ScheduledRunCompleted`

```rust
pub struct Scheduler {
    tasks: HashMap<Uuid, ScheduledTask>,
    session_map: HashMap<Uuid, Uuid>,  // task_id → session_id (persisted)
}

struct ScheduledTask {
    config: ScheduledTaskConfig,
    next_fire: DateTime<Utc>,
    running_session: Option<Uuid>,  // non-None while a run is active
}
```

### Tick Loop

```
loop {
    // Sleep until next task fires (or until tasks change)
    let next = tasks.values().map(|t| t.next_fire).min();
    tokio::select! {
        _ = sleep_until(next) => {
            // Find and fire all due tasks
            for task in due_tasks() {
                if task.running_session.is_some() {
                    // Skip — previous run still active
                    continue;
                }
                let session_id = session_map.get(task.id).copied()
                    .unwrap_or_else(Uuid::new_v4);
                process_manager.spawn(session_id, resume: session_map.contains(task.id), ...);
                task.running_session = Some(session_id);
            }
        }
        sync = schedule_rx.recv() => {
            // ScheduleSync received — update task configs, recompute next_fire times
        }
    }
}
```

### Prompt Injection

After the session connects and Claude is ready, the launcher needs to send the task's prompt. Two options:

**Option A — Launcher sends via backend relay**: The launcher sends a new `LauncherToServer::InjectInput { session_id, content }` message. The backend routes it to the proxy as `ServerToProxy::SequencedInput`. This reuses the existing input pipeline.

**Option B — Launcher writes to proxy stdin directly**: The launcher spawns the proxy as a child process and can write to its stdin. This is simpler but bypasses the backend's message logging.

**Recommended: Option A** — keeps the message visible in the session history so the frontend can display what triggered the run.

### Local Persistence

File: `~/.config/claude-code-portal/scheduled_tasks.json`

```json
{
  "task_sessions": {
    "<task-uuid>": "<session-uuid>",
    ...
  }
}
```

Updated when:
- A task runs for the first time (new session_id stored)
- A `SessionNotFound` retry creates a replacement session (updated to new UUID)

## Backend Changes

### API Endpoints

| Method | Path | Auth | Purpose |
|--------|------|------|---------|
| `GET` | `/api/scheduled-tasks` | User | List user's tasks |
| `POST` | `/api/scheduled-tasks` | User | Create task |
| `PATCH` | `/api/scheduled-tasks/:id` | User | Update task (name, cron, enabled, etc.) |
| `DELETE` | `/api/scheduled-tasks/:id` | User | Delete task |
| `GET` | `/api/scheduled-tasks/:id/runs` | User | List past sessions for a task |

### Schedule Sync

When a scheduled task is created/updated/deleted via API, the backend sends `ScheduleSync` to the user's connected launcher(s) with the full updated task list. The launcher replaces its local state and recomputes timers.

On launcher connect, the backend also sends `ScheduleSync` with the user's current tasks.

### Run Reporting

When the backend receives `ScheduledRunStarted`, it updates `scheduled_tasks.last_run_at`. When it receives `ScheduledRunCompleted`, it recomputes `next_run_at` from the cron expression.

### Registration

`register_or_update_session()` stores `scheduled_task_id` on the session row when present in `RegisterFields`.

## Frontend Changes

### Session Pills

Minimal change — add a cron badge:

```rust
if session.scheduled_task_id.is_some() {
    html! { <span class="pill-agent-badge cron">{ "⏱" }</span> }
}
```

Styled similarly to the existing Codex badge. Scheduled sessions appear in the rail like any other session — they connect when running, disconnect when done.

### Scheduled Tasks Management

A new section accessible from the dashboard (settings gear or dedicated tab):

- **Task list**: name, cron expression (with human-readable description like "Every day at 3am"), enabled toggle, last/next run times
- **Create/edit form**: name, cron expression, working directory (dropdown from known launcher directories?), prompt (textarea), agent type, max runtime
- **Run history**: click a task to see its past sessions with timestamps and exit codes

This could live as a new admin-style page or as a panel within the existing dashboard, depending on UX preference.

## Lifecycle Example

```
1. User creates task via frontend:
     name: "Nightly dep audit"
     cron: "0 3 * * *"
     working_directory: "/home/user/myproject"
     prompt: "Check for outdated dependencies and create a PR if any need updating"

2. Backend saves to scheduled_tasks table
   Backend sends ScheduleSync to user's launcher

3. Launcher receives ScheduleSync
   Computes next_fire: tonight at 3:00 AM
   No session_id stored yet for this task

4. At 3:00 AM, scheduler fires:
   - Generates new session_id (first run)
   - Calls process_manager.spawn(session_id, resume: false, scheduled_task_id: task.id, ...)
   - Proxy starts, connects to backend, registers session with scheduled_task_id
   - Launcher sends InjectInput with the prompt
   - Claude runs, does its work
   - Claude finishes → proxy exits → session status becomes "inactive"
   - Launcher sends ScheduledRunCompleted
   - Launcher stores task_id → session_id mapping locally

5. Next night at 3:00 AM:
   - Scheduler fires again
   - Finds stored session_id for this task
   - Calls process_manager.spawn(session_id, resume: true, ...)
   - Claude resumes with full context of previous run
   - "I reviewed deps yesterday and created PR #42. Let me check if there are new updates..."

6. Session pill in frontend shows:
   [● myproject  hostname v2.0.23  main  ⏱]
   (the ⏱ badge indicates this is a scheduled run)
```

## Open Questions

1. **Overlap policy**: What happens if a scheduled run is still active when the next cron tick fires? Current sketch skips the tick. Alternatives: queue it, kill the running one.

2. **Failure handling**: Should a task auto-disable after N consecutive failures? Or just keep trying?

3. **Cost controls**: Should there be a per-task or per-schedule cost cap? The existing `total_cost_usd` on sessions tracks spend, but there's no automatic cutoff.

4. **Timezone**: Cron expressions need a timezone. Default to the launcher's system timezone? Allow per-task override?

5. **Output notification**: Should completed scheduled runs trigger a notification (email, webhook, browser notification)? Or is checking the session history sufficient?

6. **Multiple launchers**: While each launcher is unique per hostname, a user could have launchers on multiple machines. Should tasks be pinned to a specific launcher, or routable to any?
