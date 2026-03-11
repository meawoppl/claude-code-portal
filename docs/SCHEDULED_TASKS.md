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

### Launcher Pinning

A user may have launchers on multiple hostnames (work laptop, CI server, home machine). Each scheduled task can be **pinned to a specific hostname** via the `hostname` column, or left `NULL` to run on any connected launcher.

When the backend sends `ScheduleSync`, it filters tasks:
- Tasks with `hostname = NULL` → sent to all of the user's launchers (first one to fire wins)
- Tasks with `hostname = 'my-laptop'` → sent only to the launcher registered with that hostname

If the pinned launcher is offline when a tick is due, the task is simply skipped (same as overlap policy). The next tick fires normally when the launcher reconnects.

### Timezone Handling

Cron expressions default to **UTC**. Each task has an optional `timezone` field (IANA format, e.g. `"America/New_York"`). The launcher uses the `chrono-tz` crate to evaluate cron expressions in the specified timezone.

UTC is the default because:
- It avoids DST ambiguity (no skipped or doubled runs at clock changes)
- Server logs and `next_run_at` are always in UTC for consistency
- Users can override per-task when local time matters (e.g. "every weekday at 9am Eastern")

The frontend displays `next_run_at` in the user's browser timezone for readability.

## Data Model

### New Table: `scheduled_tasks`

```sql
CREATE TABLE scheduled_tasks (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID NOT NULL REFERENCES users(id),
    name            VARCHAR(255) NOT NULL,
    cron_expression VARCHAR(128) NOT NULL,       -- standard 5-field cron
    timezone        VARCHAR(64) NOT NULL DEFAULT 'UTC',  -- IANA timezone (e.g. "America/New_York")
    hostname        VARCHAR(255),                -- pin to specific launcher; NULL = any launcher
    working_directory TEXT NOT NULL,
    prompt          TEXT NOT NULL,                -- initial message sent to agent
    claude_args     JSONB NOT NULL DEFAULT '[]',  -- extra CLI args
    agent_type      VARCHAR(16) NOT NULL DEFAULT 'claude',
    enabled         BOOLEAN NOT NULL DEFAULT true,
    max_runtime_minutes INTEGER NOT NULL DEFAULT 30,
    last_session_id UUID REFERENCES sessions(id) ON DELETE SET NULL,  -- current long-lived session
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

1. First run: `last_session_id` is `NULL` in `ScheduleSync` → launcher creates a new session with `resume: false`, reports `session_id` via `ScheduledRunStarted`
2. Subsequent runs: `last_session_id` is present → launcher spawns with `resume: true` using that `session_id`
3. If `SessionNotFound`: the existing retry logic creates a fresh session (new UUID) and marks the old one `replaced`; the launcher reports the new `session_id` via `ScheduledRunStarted`, backend updates `last_session_id`

The task→session mapping lives entirely in the `scheduled_tasks.last_session_id` database column, delivered to the launcher via `ScheduleSync`. No local persistence file needed.

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
    pub timezone: String,               // IANA timezone, e.g. "UTC" or "America/New_York"
    pub working_directory: String,
    pub prompt: String,
    pub claude_args: Vec<String>,
    pub agent_type: AgentType,
    pub enabled: bool,
    pub max_runtime_minutes: i32,
    pub last_session_id: Option<Uuid>,  // server-provided task→session mapping
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
4. **Sends the prompt** via `LauncherToServer::InjectInput` after the session connects (see [Prompt Injection](#prompt-injection))
5. **Enforces max runtime** — kills the process after `max_runtime_minutes`
6. **Reports results** back to backend via `ScheduledRunStarted` / `ScheduledRunCompleted`

```rust
pub struct Scheduler {
    tasks: HashMap<Uuid, ScheduledTask>,
}

struct ScheduledTask {
    config: ScheduledTaskConfig,       // includes last_session_id from server
    next_fire: DateTime<Utc>,
    running_session: Option<Uuid>,     // non-None while a run is active
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
                    // Overlap policy: skip — previous run still active
                    log::info!("Skipping task {}: previous run still active", task.config.name);
                    continue;
                }
                let (session_id, resume) = match task.config.last_session_id {
                    Some(id) => (id, true),
                    None => (Uuid::new_v4(), false),
                };
                process_manager.spawn(session_id, resume, scheduled_task_id: task.config.id, ...);
                task.running_session = Some(session_id);
                // After session connects, send InjectInput with task.config.prompt
            }
        }
        sync = schedule_rx.recv() => {
            // ScheduleSync received — replace task configs, recompute next_fire times
            // Preserves running_session state for tasks that are still active
        }
    }
}
```

### Prompt Injection

After the session connects and Claude is ready, the launcher needs to send the task's prompt. The launcher sends a `LauncherToServer::InjectInput { session_id, content }` message, which flows through the existing input pipeline:

```
Launcher ─── InjectInput { session_id, content } ──→ Backend
                                                        │
                                                        ├─ Saves to messages table (type: "human")
                                                        ├─ Broadcasts to connected web clients (ServerToClient::ClaudeOutput)
                                                        │
                                                        └─ Forwards as ServerToProxy::SequencedInput ──→ Proxy ──→ Claude stdin
```

This ensures the prompt is:
1. **Logged** — visible in the session message history, so the frontend shows what triggered each run
2. **Sequenced** — uses the same sequence numbering as user-typed messages, preventing ordering bugs
3. **Observable** — web clients watching the session see the prompt arrive in real time

The alternative (writing directly to proxy stdin) was rejected because it bypasses message logging and makes scheduled runs invisible in the session view.

New protocol message:

```rust
LauncherToServer::InjectInput {
    session_id: Uuid,
    content: String,
}
```

The backend handler for `InjectInput` is identical to the existing `ClientToServer::ClaudeInput` handler — it sequences the message, stores it, and forwards it to the proxy.

### No Local Persistence

The task→session mapping is stored **server-side** in `scheduled_tasks.last_session_id` and delivered to the launcher via `ScheduleSync`. This eliminates the need for a local `scheduled_tasks.json` file, which avoids:

- **Stale state** after machine wipes or config loss
- **Split-brain** between launcher and backend about which session belongs to which task
- **Complexity** of reconciling local and remote state on reconnect

When a task runs for the first time, the launcher generates a new `session_id` and reports it via `ScheduledRunStarted`. The backend stores it in `scheduled_tasks.last_session_id`. On subsequent `ScheduleSync` messages, the launcher receives the mapping back.

If a `SessionNotFound` retry creates a replacement session, the launcher reports the new `session_id` via `ScheduledRunStarted`, and the backend updates `last_session_id` accordingly.

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

When a scheduled task is created/updated/deleted via API, the backend sends `ScheduleSync` to the user's connected launcher(s). Tasks are filtered per-launcher:

- Tasks with `hostname = NULL` → included in every launcher's sync
- Tasks with `hostname = 'foo'` → only included in the launcher registered as `foo`

The launcher replaces its task set and recomputes timers, preserving `running_session` state for tasks that are currently executing.

On launcher connect, the backend also sends an initial `ScheduleSync` with the user's applicable tasks.

### Run Reporting

When the backend receives `ScheduledRunStarted`, it:
1. Updates `scheduled_tasks.last_run_at`
2. Stores the `session_id` in `scheduled_tasks.last_session_id`

When it receives `ScheduledRunCompleted`, it recomputes `next_run_at` from the cron expression and timezone.

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
     timezone: "America/New_York"
     hostname: "dev-laptop"          (pinned to this machine)
     working_directory: "/home/user/myproject"
     prompt: "Check for outdated dependencies and create a PR if any need updating"

2. Backend saves to scheduled_tasks table (last_session_id = NULL)
   Backend sends ScheduleSync to user's "dev-laptop" launcher
   (Other launchers on different hostnames don't receive this task)

3. Launcher receives ScheduleSync
   Computes next_fire: tonight at 3:00 AM Eastern (07:00 UTC)
   last_session_id is NULL → first run will create new session

4. At 3:00 AM Eastern, scheduler fires:
   - Generates new session_id (first run, last_session_id was NULL)
   - Calls process_manager.spawn(session_id, resume: false, scheduled_task_id: task.id, ...)
   - Proxy starts, connects to backend, registers session with scheduled_task_id
   - Launcher sends ScheduledRunStarted { task_id, session_id }
   - Backend stores session_id in scheduled_tasks.last_session_id
   - Launcher sends InjectInput { session_id, prompt }
   - Backend logs prompt as human message, forwards to proxy
   - Claude runs, does its work
   - Claude finishes → proxy exits → session status becomes "inactive"
   - Launcher sends ScheduledRunCompleted { task_id, session_id, exit_code, duration }

5. Next night at 3:00 AM Eastern:
   - Scheduler fires again
   - last_session_id is now set (from ScheduleSync on reconnect or from step 4)
   - Calls process_manager.spawn(session_id, resume: true, ...)
   - Claude resumes with full context of previous run
   - "I reviewed deps yesterday and created PR #42. Let me check if there are new updates..."

6. Session pill in frontend shows:
   [● myproject  dev-laptop  main  ⏱]
   (the ⏱ badge indicates this is a scheduled run)
```

## Resolved Decisions

1. **Overlap policy**: **Skip if running.** If a previous run is still active when the next cron tick fires, the tick is skipped. This is the simplest policy and prevents runaway cost from overlapping long-running tasks. The skip is logged. Future enhancement: make this configurable per-task (skip / queue / kill).

2. **Timezone**: **UTC by default**, with optional per-task IANA timezone override. Avoids DST ambiguity while letting users say "9am Eastern" when they need local time. See [Timezone Handling](#timezone-handling).

3. **Multiple launchers**: Tasks can be **pinned to a hostname** or left unpinned (any launcher). See [Launcher Pinning](#launcher-pinning).

4. **Task→session mapping**: **Server-side only** via `scheduled_tasks.last_session_id`. No local persistence file. See [No Local Persistence](#no-local-persistence).

5. **Prompt injection**: **Backend relay** via `LauncherToServer::InjectInput`. See [Prompt Injection](#prompt-injection).

## Open Questions

1. **Failure handling**: Should a task auto-disable after N consecutive failures? Or just keep trying? Leaning toward: keep trying, but surface failure streaks in the UI so the user notices.

2. **Cost controls**: Should there be a per-task or per-schedule cost cap? The existing `total_cost_usd` on sessions tracks spend, but there's no automatic cutoff. Could add a `max_cost_usd` column that disables the task when exceeded.

3. **Output notification**: Should completed scheduled runs trigger a notification (email, webhook, browser notification)? Or is checking the session history sufficient? This could be a follow-up feature.
