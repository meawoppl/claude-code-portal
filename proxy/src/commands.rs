//! Subcommand handlers for logout and init.

use anyhow::Result;

use crate::config::{ProxyConfig, SessionAuth};
use crate::ui;
use crate::util;

/// Handle the --logout command
pub fn handle_logout(config: &mut ProxyConfig, cwd: &str) -> Result<()> {
    if let Some(removed) = config.remove_session_auth(cwd) {
        config.save()?;
        ui::print_logout_success(&removed.user_email.unwrap_or_default());
    } else {
        ui::print_no_cached_auth();
    }
    Ok(())
}

/// Handle the --init command
pub fn handle_init(
    config: &mut ProxyConfig,
    cwd: &str,
    init_value: &str,
    default_backend_url: &str,
) -> Result<()> {
    let (backend_url, token, session_prefix) = util::parse_init_value(init_value)?;

    // Extract user info from JWT (basic parsing without verification)
    let user_email = util::extract_email_from_jwt(&token);

    ui::print_init_start(user_email.as_deref().unwrap_or("unknown user"));

    // Save to config
    config.set_session_auth(
        cwd.to_string(),
        SessionAuth {
            user_id: String::new(),
            auth_token: token,
            user_email: user_email.clone(),
            last_used: chrono::Utc::now().to_rfc3339(),
            backend_url: backend_url.clone(),
            session_prefix: session_prefix.clone(),
        },
    );

    // Also save the backend URL
    if let Some(ref url) = backend_url {
        config.set_backend_url(cwd, url);
    }

    // Save session name prefix if provided
    if let Some(prefix) = session_prefix {
        config.set_session_prefix(cwd, &prefix);
    }

    config.save()?;

    ui::print_init_complete(
        &user_email.unwrap_or_else(|| "this directory".to_string()),
        backend_url.as_deref().unwrap_or(default_backend_url),
    );

    Ok(())
}
