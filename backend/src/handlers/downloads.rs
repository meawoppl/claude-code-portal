//! Download handlers for serving the proxy binary and install script

use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::sync::Arc;
use tokio_util::io::ReaderStream;

use crate::AppState;

#[derive(Deserialize)]
pub struct InstallScriptParams {
    /// Optional init URL to automatically initialize after install
    init_url: Option<String>,
    /// Optional backend URL override (WebSocket URL for runtime connection)
    backend_url: Option<String>,
}

/// Serve the install script that downloads and sets up the proxy
pub async fn install_script(
    State(app_state): State<Arc<AppState>>,
    Query(params): Query<InstallScriptParams>,
) -> impl IntoResponse {
    let base_url = &app_state.public_url;

    // Generate the init section if an init_url was provided
    let init_section = if let Some(ref init_url) = params.init_url {
        // Add --backend-url flag if explicitly provided
        let backend_flag = params.backend_url.as_ref()
            .map(|url| format!(r#" --backend-url "{url}""#))
            .unwrap_or_default();

        format!(r##"
# Initialize with provided token
echo "Initializing claude-proxy..."
"${{BIN_PATH}}" --init "{init_url}"{backend_flag}
echo ""
echo "Setup complete! Run 'claude-proxy' to start a session."
"##)
    } else {
        r##"
echo "Next steps:"
echo "  1. Restart your shell or source your rc file"
echo "  2. Initialize with your token: claude-proxy --init <URL>"
echo "  3. Start a session: claude-proxy"
"##.to_string()
    };

    let script = format!(r##"#!/bin/bash
# CC-Proxy Installer
# This script downloads and installs the claude-proxy binary

set -e

CONFIG_DIR="${{HOME}}/.config/cc-proxy"
BIN_NAME="claude-proxy"
BIN_PATH="${{CONFIG_DIR}}/${{BIN_NAME}}"
DOWNLOAD_URL="{base_url}/api/download/proxy"

echo "CC-Proxy Installer"
echo "=================="
echo ""

# Create config directory
mkdir -p "${{CONFIG_DIR}}"

# Detect OS and architecture
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "${{ARCH}}" in
    x86_64|amd64)
        ARCH="x86_64"
        ;;
    aarch64|arm64)
        ARCH="aarch64"
        ;;
    *)
        echo "Error: Unsupported architecture: ${{ARCH}}"
        exit 1
        ;;
esac

echo "Detected: ${{OS}}-${{ARCH}}"
echo "Installing to: ${{BIN_PATH}}"
echo ""

# Download the binary
echo "Downloading claude-proxy..."
if command -v curl &> /dev/null; then
    curl -fsSL "${{DOWNLOAD_URL}}" -o "${{BIN_PATH}}"
elif command -v wget &> /dev/null; then
    wget -q "${{DOWNLOAD_URL}}" -O "${{BIN_PATH}}"
else
    echo "Error: curl or wget required"
    exit 1
fi

# Make executable
chmod +x "${{BIN_PATH}}"

echo "Binary installed successfully!"
echo ""

# Add to PATH in shell rc files
add_to_path() {{
    local rc_file="$1"
    local path_line="export PATH=\"\$PATH:${{CONFIG_DIR}}\""

    if [ -f "${{rc_file}}" ]; then
        if ! grep -q "cc-proxy" "${{rc_file}}" 2>/dev/null; then
            echo "" >> "${{rc_file}}"
            echo "# CC-Proxy binary path" >> "${{rc_file}}"
            echo "${{path_line}}" >> "${{rc_file}}"
            echo "Updated: ${{rc_file}}"
            return 0
        fi
    fi
    return 1
}}

echo "Adding to PATH..."

# Try common shell rc files
UPDATED=0
if add_to_path "${{HOME}}/.bashrc"; then UPDATED=1; fi
if add_to_path "${{HOME}}/.zshrc"; then UPDATED=1; fi
if add_to_path "${{HOME}}/.profile"; then UPDATED=1; fi

if [ "${{UPDATED}}" -eq 1 ]; then
    echo ""
    echo "PATH updated! Restart your shell or run: source ~/.bashrc"
else
    echo "PATH already configured or no rc files found."
fi

echo ""
echo "Installation complete!"
{init_section}
"##);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/x-shellscript")
        .header(header::CONTENT_DISPOSITION, "attachment; filename=\"install.sh\"")
        .body(Body::from(script))
        .unwrap()
}

/// Serve the proxy binary
pub async fn proxy_binary(State(app_state): State<Arc<AppState>>) -> Result<impl IntoResponse, (StatusCode, String)> {
    // In dev mode, try to find the binary in target/release or target/debug
    // In production, use the PROXY_BINARY_PATH env var or a default location
    let binary_path = if app_state.dev_mode {
        // Try release first, then debug
        let release_path = std::path::Path::new("target/release/claude-proxy");
        let debug_path = std::path::Path::new("target/debug/claude-proxy");

        if release_path.exists() {
            release_path.to_path_buf()
        } else if debug_path.exists() {
            debug_path.to_path_buf()
        } else {
            return Err((
                StatusCode::NOT_FOUND,
                "Proxy binary not found. Run 'cargo build -p proxy --release' first.".to_string(),
            ));
        }
    } else {
        // Production: use env var or default location
        let path = std::env::var("PROXY_BINARY_PATH")
            .unwrap_or_else(|_| "/app/bin/claude-proxy".to_string());
        std::path::PathBuf::from(path)
    };

    if !binary_path.exists() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("Proxy binary not found at: {:?}", binary_path),
        ));
    }

    let file = tokio::fs::File::open(&binary_path)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to open binary: {}", e)))?;

    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_DISPOSITION, "attachment; filename=\"claude-proxy\"")
        .body(body)
        .unwrap())
}
