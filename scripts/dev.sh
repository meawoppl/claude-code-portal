#!/bin/bash
# Development environment script
# Usage:
#   ./scripts/dev.sh start    - Start all services in background
#   ./scripts/dev.sh stop     - Stop all services
#   ./scripts/dev.sh restart  - Restart all services
#   ./scripts/dev.sh status   - Show status of services
#   ./scripts/dev.sh logs     - Tail all logs
#   ./scripts/dev.sh          - Run in foreground (default, Ctrl+C to stop)

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR"

# Config
export DATABASE_URL="postgresql://ccproxy:dev_password_change_in_production@localhost:5432/ccproxy"
BACKEND_LOG="/tmp/cc-proxy-backend.log"
PROXY_LOG="/tmp/cc-proxy-proxy.log"
PID_FILE="/tmp/cc-proxy.pids"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log() { echo -e "${BLUE}[cc-proxy]${NC} $1"; }
success() { echo -e "${GREEN}✓${NC} $1"; }
error() { echo -e "${RED}✗${NC} $1"; }
warn() { echo -e "${YELLOW}⚠${NC} $1"; }

start_db() {
    log "Starting PostgreSQL..."
    docker compose -f docker-compose.test.yml up -d db

    log "Waiting for database..."
    for i in {1..30}; do
        if docker compose -f docker-compose.test.yml exec -T db pg_isready -U ccproxy > /dev/null 2>&1; then
            success "Database ready"
            return 0
        fi
        sleep 1
    done
    error "Database failed to start"
    return 1
}

run_migrations() {
    log "Running migrations..."
    cd backend
    if diesel migration run 2>/dev/null; then
        success "Migrations complete"
    else
        warn "Migrations may have already run"
    fi
    cd ..
}

build_frontend() {
    log "Building frontend..."
    cd frontend
    if trunk build 2>/dev/null; then
        success "Frontend built"
    else
        error "Frontend build failed"
        return 1
    fi
    cd ..
}

start_backend() {
    local background=$1
    log "Starting backend..."

    if [ "$background" = "bg" ]; then
        cargo run -p backend -- --dev-mode > "$BACKEND_LOG" 2>&1 &
        echo $! >> "$PID_FILE"
        sleep 3
        if curl -sf http://localhost:3000/ > /dev/null; then
            success "Backend running (PID: $(tail -1 "$PID_FILE"))"
        else
            error "Backend failed to start. Check: tail -f $BACKEND_LOG"
            return 1
        fi
    else
        cargo run -p backend -- --dev-mode
    fi
}

stop_all() {
    log "Stopping services..."

    # Kill tracked PIDs
    if [ -f "$PID_FILE" ]; then
        while read pid; do
            kill "$pid" 2>/dev/null && echo "Killed PID $pid" || true
        done < "$PID_FILE"
        rm -f "$PID_FILE"
    fi

    # Kill by name as backup
    pkill -f "target/debug/backend" 2>/dev/null || true
    pkill -f "target/debug/claude-proxy" 2>/dev/null || true

    # Stop database
    docker compose -f docker-compose.test.yml down 2>/dev/null || true

    success "All services stopped"
}

show_status() {
    echo ""
    echo "Service Status:"
    echo "───────────────"

    # Database
    if docker compose -f docker-compose.test.yml exec -T db pg_isready -U ccproxy > /dev/null 2>&1; then
        echo -e "  Database:  ${GREEN}●${NC} running"
    else
        echo -e "  Database:  ${RED}○${NC} stopped"
    fi

    # Backend
    if curl -sf http://localhost:3000/ > /dev/null 2>&1; then
        echo -e "  Backend:   ${GREEN}●${NC} running (http://localhost:3000)"
    else
        echo -e "  Backend:   ${RED}○${NC} stopped"
    fi

    # Check for proxy
    if pgrep -f "target/debug/claude-proxy" > /dev/null 2>&1; then
        echo -e "  Proxy:     ${GREEN}●${NC} running"
    else
        echo -e "  Proxy:     ${YELLOW}○${NC} not started"
    fi

    echo ""
    echo "URLs:"
    echo "  Frontend:  http://localhost:3000/app/"
    echo "  Dashboard: http://localhost:3000/app/dashboard"
    echo "  Dev Login: http://localhost:3000/auth/dev-login"
    echo ""
}

show_logs() {
    echo "Tailing logs (Ctrl+C to stop)..."
    tail -f "$BACKEND_LOG" "$PROXY_LOG" 2>/dev/null
}

run_foreground() {
    # Cleanup on exit
    trap stop_all EXIT INT TERM

    start_db || exit 1
    run_migrations
    build_frontend || exit 1

    echo ""
    echo "╔══════════════════════════════════════════════════════╗"
    echo "║          CC-Proxy Dev Environment                    ║"
    echo "╚══════════════════════════════════════════════════════╝"
    echo ""
    echo "  Frontend:  http://localhost:3000/app/"
    echo "  Dashboard: http://localhost:3000/app/dashboard"
    echo "  Dev Login: http://localhost:3000/auth/dev-login"
    echo ""
    echo "  Test user: testing@testing.local"
    echo ""
    echo "  Press Ctrl+C to stop all services"
    echo ""

    start_backend "fg"
}

run_background() {
    start_db || exit 1
    run_migrations
    build_frontend || exit 1
    start_backend "bg" || exit 1

    echo ""
    success "All services started in background"
    show_status
    echo "Run './scripts/dev.sh stop' to stop"
    echo "Run './scripts/dev.sh logs' to view logs"
}

# Main
case "${1:-}" in
    start)
        run_background
        ;;
    stop)
        stop_all
        ;;
    restart)
        stop_all
        sleep 2
        run_background
        ;;
    status)
        show_status
        ;;
    logs)
        show_logs
        ;;
    ""|fg|foreground)
        run_foreground
        ;;
    *)
        echo "Usage: $0 {start|stop|restart|status|logs|fg}"
        echo ""
        echo "  start   - Start all services in background"
        echo "  stop    - Stop all services"
        echo "  restart - Restart all services"
        echo "  status  - Show service status"
        echo "  logs    - Tail service logs"
        echo "  fg      - Run in foreground (default)"
        exit 1
        ;;
esac
