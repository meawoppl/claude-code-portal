#!/bin/bash
# Update Rust to latest stable version

set -e

GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m'

log() { echo -e "${BLUE}[claude-portal]${NC} $1"; }
success() { echo -e "${GREEN}✓${NC} $1"; }
warn() { echo -e "${YELLOW}⚠${NC} $1"; }

log "Updating Rust..."
echo ""

# Check current version
if command -v rustc &> /dev/null; then
    log "Current version: $(rustc --version)"
else
    warn "Rust not found. Install from: https://rustup.rs"
    exit 1
fi

# Update rustup
log "Updating rustup..."
rustup self update

# Update stable toolchain
log "Updating stable toolchain..."
rustup update stable

# Set stable as default
log "Setting stable as default..."
rustup default stable

echo ""
success "Rust updated to: $(rustc --version)"
echo ""

log "Now try installing diesel CLI:"
echo "  ./scripts/install-deps.sh"
