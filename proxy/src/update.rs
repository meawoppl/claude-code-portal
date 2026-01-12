//! Auto-update functionality for the claude-proxy binary
//!
//! On startup, checks if a newer version is available from the backend
//! and self-updates if necessary.

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use tracing::info;

/// Result of an update check
#[derive(Debug)]
pub enum UpdateResult {
    /// Binary is up to date
    UpToDate,
    /// Binary was updated, needs relaunch
    Updated,
}

/// Convert backend URL (ws:// or wss://) to HTTP URL for downloads
fn ws_to_http_url(ws_url: &str) -> String {
    ws_url
        .replace("wss://", "https://")
        .replace("ws://", "http://")
}

/// Compute SHA256 hash of bytes
fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let hash = hasher.finalize();
    hex::encode(hash)
}

/// Check for updates and self-update if necessary
///
/// This function:
/// 1. Computes SHA256 of the current binary
/// 2. Makes a HEAD request to get the server's binary hash
/// 3. If hashes differ, downloads the new binary
/// 4. Verifies the download hash matches
/// 5. Atomically replaces self
///
/// # Platform Notes
/// - Works on Unix (Linux, macOS) where running binaries can be overwritten
/// - Windows support is TODO (requires different approach due to exe locking)
pub fn check_for_update(backend_url: &str) -> Result<UpdateResult> {
    let self_path = std::env::current_exe().context("Failed to get current executable path")?;
    let self_bytes = fs::read(&self_path).context("Failed to read current binary")?;
    let self_hash = sha256_hex(&self_bytes);

    info!("Current binary hash: {}", &self_hash[..16]);

    // Convert WebSocket URL to HTTP for the download endpoint
    let http_base = ws_to_http_url(backend_url);
    let download_url = format!("{}/api/download/proxy", http_base);

    // HEAD request to get remote hash
    info!("Checking for updates at {}", download_url);
    let client = reqwest::blocking::Client::new();
    let resp = client
        .head(&download_url)
        .send()
        .context("Failed to check for updates")?;

    if !resp.status().is_success() {
        bail!(
            "Update check failed: server returned {}",
            resp.status()
        );
    }

    let remote_hash = resp
        .headers()
        .get("X-Binary-SHA256")
        .context("Server did not return X-Binary-SHA256 header")?
        .to_str()
        .context("Invalid X-Binary-SHA256 header")?;

    info!("Remote binary hash: {}", &remote_hash[..16]);

    if self_hash == remote_hash {
        info!("Binary is up to date");
        return Ok(UpdateResult::UpToDate);
    }

    info!("Update available, downloading...");

    // Download the new binary
    let resp = client
        .get(&download_url)
        .send()
        .context("Failed to download update")?;

    if !resp.status().is_success() {
        bail!("Download failed: server returned {}", resp.status());
    }

    let new_binary = resp.bytes().context("Failed to read download response")?;

    // Verify downloaded binary hash matches what we expected
    let download_hash = sha256_hex(&new_binary);
    if download_hash != remote_hash {
        bail!(
            "Downloaded binary hash mismatch! Expected {} but got {}. Download may be corrupted.",
            &remote_hash[..16],
            &download_hash[..16]
        );
    }

    info!("Download verified, installing update...");

    // Atomic replacement: write to temp file, then rename
    let temp_path = self_path.with_extension("tmp");
    fs::write(&temp_path, &new_binary).context("Failed to write temporary file")?;

    // Set executable permission on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&temp_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&temp_path, perms)?;
    }

    // Atomic rename
    fs::rename(&temp_path, &self_path).context("Failed to replace binary")?;

    info!("Update installed successfully");
    Ok(UpdateResult::Updated)
}

/// Get the backend URL for update checks from config
///
/// Returns None if no backend URL is configured
pub fn get_update_backend_url() -> Option<String> {
    use crate::config::ProxyConfig;

    let config = ProxyConfig::load().ok()?;

    // Try global default first
    if let Some(url) = config.preferences.default_backend_url {
        return Some(url);
    }

    // Fall back to any per-directory URL
    for session in config.sessions.values() {
        if let Some(url) = &session.backend_url {
            return Some(url.clone());
        }
    }

    None
}
