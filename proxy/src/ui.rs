//! Terminal UI helpers for the proxy CLI.

use colored::Colorize;
use std::io::Write;

/// Print the startup banner
pub fn print_startup_banner() {
    println!();
    println!(
        "{}",
        "╭──────────────────────────────────────╮".bright_blue()
    );
    println!(
        "{}",
        "│       Claude Code Proxy Starting     │".bright_blue()
    );
    println!(
        "{}",
        "╰──────────────────────────────────────╯".bright_blue()
    );
    println!();
}

/// Print session information
pub fn print_session_info(session_name: &str, session_id: &str, backend_url: &str, resuming: bool) {
    println!("  {} {}", "Session:".dimmed(), session_name.bright_white());
    println!("  {} {}", "ID:".dimmed(), session_id[..8].bright_cyan());
    println!("  {} {}", "Backend:".dimmed(), backend_url.bright_white());
    println!(
        "  {} {}",
        "Mode:".dimmed(),
        if resuming {
            "resume".bright_yellow()
        } else {
            "new".bright_green()
        }
    );
    println!();
}

/// Print the "proxy ready" banner
pub fn print_ready_banner() {
    println!();
    println!(
        "{}",
        "╭──────────────────────────────────────╮".bright_green()
    );
    println!(
        "{}",
        "│         ✓ Proxy Ready                │".bright_green()
    );
    println!(
        "{}",
        "╰──────────────────────────────────────╯".bright_green()
    );
    println!();
    println!("  Session is now visible in the web interface.");
    println!("  Press {} to stop.", "Ctrl+C".bright_yellow());
    println!();
}

/// Print dev mode status
pub fn print_dev_mode() {
    println!(
        "  {} {}",
        "Mode:".dimmed(),
        "development (no auth)".bright_yellow()
    );
    println!();
}

/// Print authenticated user
pub fn print_user(email: &str) {
    println!("  {} {}", "User:".dimmed(), email.bright_white());
    println!();
}

/// Print "new session" status message
pub fn print_new_session_forced() {
    println!(
        "  {} {} Starting new session (--new-session flag)",
        "⚠".bright_yellow(),
        "WARNING:".bright_yellow()
    );
}

/// Print "no previous session" message
pub fn print_no_previous_session() {
    println!(
        "  {} No previous session found, starting fresh",
        "→".bright_blue()
    );
}

/// Print "resuming session" message
pub fn print_resuming_session(session_id: &str, created_at: &str) {
    println!(
        "  {} Resuming session {} from {}",
        "→".bright_green(),
        session_id[..8].bright_cyan(),
        created_at.bright_white()
    );
}

/// Print a status line with spinner prefix
pub fn print_status(message: &str) {
    print!("  {} {} ", "→".bright_blue(), message);
    let _ = std::io::stdout().flush();
}

/// Print "connected" result
pub fn print_connected() {
    println!("{}", "connected".bright_green());
}

/// Print "registered" result
pub fn print_registered() {
    println!("{}", "registered".bright_green());
}

/// Print "started" result
pub fn print_started() {
    println!("{}", "started".bright_green());
}

/// Print "failed" result
pub fn print_failed() {
    println!("{}", "failed".bright_red());
}

/// Print connection restored message
pub fn print_connection_restored() {
    println!("  {} Connection restored", "✓".bright_green());
    println!();
}

/// Print disconnection message with backoff
pub fn print_disconnected(backoff_secs: u64) {
    println!();
    println!(
        "  {} WebSocket disconnected. Reconnecting in {}s...",
        "⚠".bright_yellow(),
        backoff_secs
    );
}

/// Print logout success
pub fn print_logout_success(email: &str) {
    println!("{} Logged out from {}", "✓".bright_green(), email);
}

/// Print no cached auth message
pub fn print_no_cached_auth() {
    println!("No cached authentication found for this directory");
}

/// Print init success
pub fn print_init_start(email: &str) {
    println!(
        "{} Initializing proxy with token for {}",
        "→".bright_blue(),
        email
    );
}

/// Print init complete
pub fn print_init_complete(email: &str, backend_url: &str) {
    println!("{} Configuration saved for {}", "✓".bright_green(), email);
    println!("  Backend: {}", backend_url);
    println!();
    println!(
        "You can now run {} without arguments.",
        "claude-proxy".bright_cyan()
    );
}

/// Print update complete message
pub fn print_update_complete() {
    println!();
    println!("{}", "╭──────────────────────────────────────╮".bright_green());
    println!("{}", "│         ✓ Update Installed           │".bright_green());
    println!("{}", "╰──────────────────────────────────────╯".bright_green());
    println!();
    println!(
        "  A new version of {} has been installed.",
        "claude-proxy".bright_cyan()
    );
    println!("  Please run the command again to use the updated version.");
    println!();
}
