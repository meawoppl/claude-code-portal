# Launcher Roadmap

## What's Done

- **Child I/O capture** (PR #324): Launcher reads stdout/stderr from
  spawned proxy processes via async line readers instead of discarding them.
- **Session-tagged logging** (PR #325): Proxy outputs JSON logs with
  session UUID when launched by the daemon. Launcher forwards `ProxyLog`
  messages to the backend, which re-logs them with `session_id` field.
- **Exit notifications** (PR #324): Launcher sends `SessionExited` to
  the backend when a proxy process terminates.

- **StopSession** (PR #332): Frontend stop button sends REST request
  through backend to launcher, which kills the proxy process.
- **Auth token cleanup**: Proxy reads `PORTAL_AUTH_TOKEN` env var via
  clap; launcher no longer passes redundant `--auth-token` CLI arg.
- **Service file cleanup**: Removed non-existent `--foreground` flag
  from systemd and launchd service files.

- **Launcher selection UI**: LaunchDialog shows launcher cards with
  name, hostname, and running session count; user picks target launcher.

- **Config file**: Launcher reads `~/.config/claude-portal/launcher.toml`;
  CLI args override config values.
- **Install script**: `launcher/install.sh` detects OS, builds if needed,
  creates config template, installs systemd/launchd service.
- **Device flow auth**: Shared `portal-auth` crate provides browser-based
  OAuth device flow. Launcher authenticates automatically on first run
  and saves the token to config.
