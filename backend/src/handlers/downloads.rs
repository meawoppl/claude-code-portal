//! Download handlers for serving the proxy binary and install script

use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, Method, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
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
    State(_app_state): State<Arc<AppState>>,
    Query(params): Query<InstallScriptParams>,
) -> impl IntoResponse {
    // Generate the init section if an init_url was provided
    let init_section = if let Some(ref init_url) = params.init_url {
        // Add --backend-url flag if explicitly provided
        let backend_flag = params
            .backend_url
            .as_ref()
            .map(|url| format!(r#" --backend-url "{url}""#))
            .unwrap_or_default();

        format!(
            r##"
# Initialize with provided token
echo "Initializing claude-proxy..."
"${{BIN_PATH}}" --init "{init_url}"{backend_flag}
echo ""
echo "Setup complete! Run 'claude-proxy' to start a session."
"##
        )
    } else {
        r##"
echo "Next steps:"
echo "  1. Restart your shell or source your rc file"
echo "  2. Initialize with your token: claude-proxy --init <URL>"
echo "  3. Start a session: claude-proxy"
"##
        .to_string()
    };

    let script = format!(
        r##"#!/bin/bash
# CC-Proxy Installer
# This script downloads and installs the claude-proxy binary from GitHub releases
# If already installed, skips download and just runs init

set -e

CONFIG_DIR="${{HOME}}/.config/cc-proxy"
BIN_NAME="claude-proxy"
BIN_PATH="${{CONFIG_DIR}}/${{BIN_NAME}}"
GITHUB_RELEASE_URL="https://github.com/meawoppl/cc-proxy/releases/download/latest"

echo "CC-Proxy Installer"
echo "=================="
echo ""

# Check if binary already exists
if [ -x "${{BIN_PATH}}" ]; then
    echo "claude-proxy is already installed at: ${{BIN_PATH}}"
    echo ""
{init_section}
    exit 0
fi

# Create config directory
mkdir -p "${{CONFIG_DIR}}"

# Detect OS and architecture
OS="$(uname -s)"
ARCH="$(uname -m)"

case "${{OS}}" in
    Linux)
        case "${{ARCH}}" in
            x86_64|amd64)
                BINARY_NAME="claude-proxy-linux-x86_64"
                ;;
            *)
                echo "Error: Unsupported Linux architecture: ${{ARCH}}"
                echo "Supported: x86_64"
                exit 1
                ;;
        esac
        ;;
    Darwin)
        case "${{ARCH}}" in
            arm64|aarch64)
                BINARY_NAME="claude-proxy-darwin-aarch64"
                ;;
            x86_64)
                BINARY_NAME="claude-proxy-darwin-x86_64"
                ;;
            *)
                echo "Error: Unsupported macOS architecture: ${{ARCH}}"
                echo "Supported: arm64 (Apple Silicon), x86_64 (Intel)"
                exit 1
                ;;
        esac
        ;;
    *)
        echo "Error: Unsupported operating system: ${{OS}}"
        echo "Supported: Linux, Darwin (macOS)"
        echo "For Windows, download manually from:"
        echo "  ${{GITHUB_RELEASE_URL}}/claude-proxy-windows-x86_64.exe"
        exit 1
        ;;
esac

DOWNLOAD_URL="${{GITHUB_RELEASE_URL}}/${{BINARY_NAME}}"

echo "Detected: ${{OS}} ${{ARCH}}"
echo "Binary: ${{BINARY_NAME}}"
echo "Installing to: ${{BIN_PATH}}"
echo ""

# Download the binary
echo "Downloading claude-proxy from GitHub releases..."
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

# macOS: Remove quarantine attribute if present
if [ "${{OS}}" = "Darwin" ]; then
    xattr -d com.apple.quarantine "${{BIN_PATH}}" 2>/dev/null || true
fi

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
"##
    );

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/x-shellscript")
        .header(
            header::CONTENT_DISPOSITION,
            "attachment; filename=\"install.sh\"",
        )
        .body(Body::from(script))
        .unwrap()
}

/// Resolve the path to the proxy binary
fn resolve_binary_path(dev_mode: bool) -> Result<std::path::PathBuf, (StatusCode, String)> {
    if dev_mode {
        // Try release first, then debug
        let release_path = std::path::Path::new("target/release/claude-proxy");
        let debug_path = std::path::Path::new("target/debug/claude-proxy");

        if release_path.exists() {
            Ok(release_path.to_path_buf())
        } else if debug_path.exists() {
            Ok(debug_path.to_path_buf())
        } else {
            Err((
                StatusCode::NOT_FOUND,
                "Proxy binary not found. Run 'cargo build -p claude-proxy --release' first."
                    .to_string(),
            ))
        }
    } else {
        // Production: use env var or default location
        let path = std::env::var("PROXY_BINARY_PATH")
            .unwrap_or_else(|_| "/app/bin/claude-proxy".to_string());
        let path = std::path::PathBuf::from(path);

        if !path.exists() {
            Err((
                StatusCode::NOT_FOUND,
                format!("Proxy binary not found at: {:?}", path),
            ))
        } else {
            Ok(path)
        }
    }
}

/// Compute SHA256 hash of a file
async fn compute_binary_sha256(path: &std::path::Path) -> Result<String, (StatusCode, String)> {
    let bytes = tokio::fs::read(path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read binary: {}", e),
        )
    })?;

    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let hash = hasher.finalize();
    Ok(hex::encode(hash))
}

/// Serve the proxy binary (GET) or return hash info (HEAD)
///
/// GET: Returns the binary file with X-Binary-SHA256 header
/// HEAD: Returns empty body with X-Binary-SHA256 header (for update checks)
pub async fn proxy_binary(
    method: Method,
    State(app_state): State<Arc<AppState>>,
) -> Result<Response<Body>, (StatusCode, String)> {
    let binary_path = resolve_binary_path(app_state.dev_mode)?;
    let sha256_hash = compute_binary_sha256(&binary_path).await?;

    if method == Method::HEAD {
        // HEAD request: return just headers for update check
        let metadata = tokio::fs::metadata(&binary_path).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to read metadata: {}", e),
            )
        })?;

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::CONTENT_LENGTH, metadata.len())
            .header("X-Binary-SHA256", &sha256_hash)
            .body(Body::empty())
            .unwrap())
    } else {
        // GET request: return the binary with hash header
        let file = tokio::fs::File::open(&binary_path).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to open binary: {}", e),
            )
        })?;

        let stream = ReaderStream::new(file);
        let body = Body::from_stream(stream);

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"claude-proxy\"",
            )
            .header("X-Binary-SHA256", &sha256_hash)
            .body(body)
            .unwrap())
    }
}
