# Launcher Roadmap

## What's Done

- **Child I/O capture** (PR #324): Launcher reads stdout/stderr from
  spawned proxy processes via async line readers instead of discarding them.
- **Session-tagged logging** (PR #325): Proxy outputs JSON logs with
  session UUID when launched by the daemon. Launcher forwards `ProxyLog`
  messages to the backend, which re-logs them with `session_id` field.
- **Exit notifications** (PR #324): Launcher sends `SessionExited` to
  the backend when a proxy process terminates.

## Remaining Work

### Cleanup

- Remove redundant `--auth-token` CLI arg from launcher spawn (already
  passed via `PORTAL_AUTH_TOKEN` env var).
- Remove `--foreground` from service files (not a real CLI flag) or add
  it as a no-op.
- `StopSession` button in the frontend session view.
- Launcher selection UI in `LaunchDialog` (show name, hostname, load).

### Install / Config

- Install script for systemd/launchd service setup.
- Launcher config file (`~/.config/claude-portal/launcher.toml`) so
  users don't need CLI args for everything.
