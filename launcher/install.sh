#!/usr/bin/env bash
set -euo pipefail

BINARY_NAME="claude-portal-launcher"
CONFIG_DIR="${HOME}/.config/claude-portal"
CONFIG_FILE="${CONFIG_DIR}/launcher.toml"

echo "=== Claude Portal Launcher Installer ==="
echo

# Detect OS
OS="$(uname -s)"
case "${OS}" in
    Linux*)  PLATFORM="linux";;
    Darwin*) PLATFORM="macos";;
    *)       echo "Unsupported OS: ${OS}"; exit 1;;
esac

echo "Platform: ${PLATFORM}"

# Check if binary is available
BINARY_PATH="$(command -v "${BINARY_NAME}" 2>/dev/null || true)"
if [ -z "${BINARY_PATH}" ]; then
    # Check if it was built locally
    REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
    if [ -f "${REPO_ROOT}/target/release/${BINARY_NAME}" ]; then
        BINARY_PATH="${REPO_ROOT}/target/release/${BINARY_NAME}"
        echo "Found release binary: ${BINARY_PATH}"
    elif [ -f "${HOME}/.cargo/bin/${BINARY_NAME}" ]; then
        BINARY_PATH="${HOME}/.cargo/bin/${BINARY_NAME}"
        echo "Found cargo binary: ${BINARY_PATH}"
    else
        echo "Binary not found. Building from source..."
        cargo install --path "$(dirname "$0")"
        BINARY_PATH="${HOME}/.cargo/bin/${BINARY_NAME}"
    fi
else
    echo "Found binary: ${BINARY_PATH}"
fi

# Create config directory
mkdir -p "${CONFIG_DIR}"

# Create config file if it doesn't exist
if [ ! -f "${CONFIG_FILE}" ]; then
    echo "Creating config file: ${CONFIG_FILE}"
    cat > "${CONFIG_FILE}" << 'TOML'
# Claude Portal Launcher Configuration
# CLI arguments and environment variables override these values.

# Backend WebSocket URL (required)
# backend_url = "wss://portal.example.com"

# Auth token (or set LAUNCHER_AUTH_TOKEN env var)
# auth_token = "your-jwt-token"

# Human-readable name (default: hostname)
# name = "my-workstation"

# Path to the claude-portal proxy binary
# proxy_path = "claude-portal"

# Maximum concurrent proxy processes
# max_processes = 5
TOML
    echo "  Edit ${CONFIG_FILE} to configure your launcher."
else
    echo "Config file already exists: ${CONFIG_FILE}"
fi

echo

# Install service
if [ "${PLATFORM}" = "linux" ]; then
    SERVICE_DIR="${HOME}/.config/systemd/user"
    SERVICE_FILE="${SERVICE_DIR}/claude-portal-launcher.service"
    TEMPLATE="$(dirname "$0")/service/claude-portal-launcher.service"

    mkdir -p "${SERVICE_DIR}"

    if [ -f "${TEMPLATE}" ]; then
        # Replace %h with $HOME for the ExecStart path
        sed "s|%h|${HOME}|g" "${TEMPLATE}" > "${SERVICE_FILE}"
        echo "Installed systemd service: ${SERVICE_FILE}"
        echo
        echo "To start the launcher:"
        echo "  systemctl --user daemon-reload"
        echo "  systemctl --user enable claude-portal-launcher"
        echo "  systemctl --user start claude-portal-launcher"
        echo
        echo "To check status:"
        echo "  systemctl --user status claude-portal-launcher"
        echo "  journalctl --user -u claude-portal-launcher -f"
    else
        echo "Warning: service template not found at ${TEMPLATE}"
    fi

elif [ "${PLATFORM}" = "macos" ]; then
    PLIST_DIR="${HOME}/Library/LaunchAgents"
    PLIST_FILE="${PLIST_DIR}/com.claude-portal.launcher.plist"
    TEMPLATE="$(dirname "$0")/service/com.claude-portal.launcher.plist"

    mkdir -p "${PLIST_DIR}"

    if [ -f "${TEMPLATE}" ]; then
        # Replace the binary path with the actual path
        sed "s|/usr/local/bin/${BINARY_NAME}|${BINARY_PATH}|g" "${TEMPLATE}" > "${PLIST_FILE}"
        echo "Installed launchd agent: ${PLIST_FILE}"
        echo
        echo "To start the launcher:"
        echo "  launchctl load ${PLIST_FILE}"
        echo
        echo "To check status:"
        echo "  launchctl list | grep claude-portal"
        echo "  tail -f /tmp/claude-portal-launcher.stdout.log"
        echo
        echo "To stop:"
        echo "  launchctl unload ${PLIST_FILE}"
    else
        echo "Warning: plist template not found at ${TEMPLATE}"
    fi
fi

echo
echo "Done! Edit ${CONFIG_FILE} before starting the service."
