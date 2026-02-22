#!/bin/bash
# One-command portal startup: ensures Docker is running, then starts dev environment
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Check if Docker daemon is running
if ! docker info >/dev/null 2>&1; then
    echo "Starting Docker Desktop..."
    open -a Docker

    echo -n "Waiting for Docker"
    for i in {1..60}; do
        if docker info >/dev/null 2>&1; then
            echo " ready!"
            break
        fi
        echo -n "."
        sleep 2
    done

    if ! docker info >/dev/null 2>&1; then
        echo ""
        echo "Error: Docker failed to start after 2 minutes"
        exit 1
    fi
fi

exec "$SCRIPT_DIR/dev.sh" start
