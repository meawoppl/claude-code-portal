# End-to-End Testing Guide

This guide walks through testing the complete CC-Proxy system locally.

## Prerequisites

```bash
# Install required tools
cargo install trunk diesel_cli --no-default-features --features postgres

# Verify installations
trunk --version
diesel --version
```

## Setup Database (One-Time)

```bash
# Option 1: Use Docker Compose (recommended for testing)
docker-compose up -d db

# Wait for DB to be ready
sleep 5

# Set database URL
export DATABASE_URL="postgresql://ccproxy:dev_password_change_in_production@localhost:5432/ccproxy"

# Run migrations
cd backend
diesel migration run
cd ..
```

## Terminal 1: Build Frontend

```bash
cd frontend

# Build the frontend to dist/
trunk build --release

# Verify build
ls -la dist/
# Should see index.html, *.wasm, *.js files

cd ..
```

## Terminal 2: Run Backend (Dev Mode)

```bash
# Set dev mode and minimal required env vars
export DEV_MODE=true
export DATABASE_URL="postgresql://ccproxy:dev_password_change_in_production@localhost:5432/ccproxy"
export HOST=0.0.0.0
export PORT=3000

# Run backend (will create test user and serve frontend)
cargo run -p backend -- --dev-mode

# You should see:
# 🚧 DEV MODE ENABLED - OAuth is bypassed, test user will be used
# ✓ Created test user: testing@testing.local
# Serving frontend from: ../frontend/dist
# Listening on 0.0.0.0:3000
```

## Terminal 3: Run Proxy

```bash
# Run the proxy to create a test session
cargo run -p proxy -- \
  --backend-url ws://localhost:3000 \
  --session-name "test-machine" \
  # Any other claude args...

# You should see:
# Starting Claude CLI proxy wrapper
# Session name: test-machine
# Backend URL: ws://localhost:3000
# Connecting to backend at ws://localhost:3000/ws/session
# Session registered with backend
# Claude CLI process spawned
```

## Test the Web Interface

1. **Open browser** to http://localhost:3000/app/

2. **On splash page**, click "Sign in with Google"
   - In dev mode, this redirects to `/auth/dev-login`
   - Auto-logs you in as `testing@testing.local`

3. **Dashboard** should show your active sessions
   - You should see the "test-machine" session portal
   - Status indicator should be green (active)

4. **Click the session portal** to open terminal interface

5. **Type a message** in the input box
   - Message should be sent to the proxy
   - Proxy forwards to Claude CLI
   - Response streams back to browser

## Verify Data Flow

### Check Database

```bash
# Connect to database
psql $DATABASE_URL

# Verify test user
SELECT * FROM users WHERE email = 'testing@testing.local';

# Verify session
SELECT * FROM sessions;

# Verify messages
SELECT * FROM messages;

# Exit
\q
```

### Check WebSocket Connections

In Terminal 2 (backend logs), you should see:
```
[backend] Registering session: test-machine
[backend] Session registered: test-machine
[backend] Claude stdout: {...}
```

### Check Proxy Logs

In Terminal 3 (proxy logs), you should see:
```
[proxy] Claude stdout: {...}
[proxy] Received input from backend
```

## Testing Checklist

- [ ] Database migrations applied successfully
- [ ] Frontend builds without errors
- [ ] Backend starts in dev mode
- [ ] Test user `testing@testing.local` created
- [ ] Backend serves frontend at `/app/`
- [ ] Proxy connects to backend WebSocket
- [ ] Proxy spawns Claude CLI process
- [ ] Session appears on dashboard
- [ ] Session portal shows correct status
- [ ] Can click portal to open terminal
- [ ] Can type message in terminal
- [ ] Message reaches proxy
- [ ] Proxy forwards to Claude
- [ ] Claude response streams back
- [ ] Messages stored in database

## Troubleshooting

### Frontend not found

```bash
# Build frontend first
cd frontend && trunk build --release && cd ..

# Verify dist exists
ls frontend/dist/

# Check backend logs for frontend path
# Should see: "Serving frontend from: ../frontend/dist"
```

### Proxy can't connect

```bash
# Verify backend is running
curl http://localhost:3000/

# Check WebSocket endpoint
wscat -c ws://localhost:3000/ws/session
# Should connect (you'll need to install wscat: npm install -g wscat)
```

### Database connection failed

```bash
# Check if PostgreSQL is running
docker-compose ps db

# Test connection manually
psql $DATABASE_URL -c "SELECT 1"
```

### Claude CLI not found

```bash
# Verify claude is in PATH
which claude

# Or specify full path in proxy args
cargo run -p proxy -- \
  --backend-url ws://localhost:3000 \
  /full/path/to/claude --other-args
```

## Clean Up

```bash
# Stop all running services
# Ctrl+C in each terminal

# Stop and remove database
docker-compose down -v

# Clean build artifacts
cargo clean
cd frontend && trunk clean && cd ..
```

## Next Steps

### Production Testing

To test without dev mode:

1. **Set up 1Password** (see SETUP_1PASSWORD.md)
2. **Configure OAuth** (see README.md)
3. **Run without --dev-mode**:
   ```bash
   op run --env-file=.env -- cargo run -p backend
   ```

### Deploy

See DOCKER.md for deployment instructions.

## Quick Test Script

```bash
#!/bin/bash
# test.sh - Run all services for testing

set -e

echo "🧹 Cleaning up..."
docker-compose down -v 2>/dev/null || true
killall backend proxy 2>/dev/null || true

echo "🗄️ Starting database..."
docker-compose up -d db
sleep 5

echo "🔄 Running migrations..."
export DATABASE_URL="postgresql://ccproxy:dev_password_change_in_production@localhost:5432/ccproxy"
cd backend && diesel migration run && cd ..

echo "🎨 Building frontend..."
cd frontend && trunk build --release && cd ..

echo "🚀 Starting backend..."
export DEV_MODE=true
cargo run -p backend -- --dev-mode &
BACKEND_PID=$!

echo "⏳ Waiting for backend..."
sleep 3

echo "🔌 Starting proxy..."
cargo run -p proxy -- \
  --backend-url ws://localhost:3000 \
  --session-name "test-session" &
PROXY_PID=$!

echo "✅ All services started!"
echo ""
echo "🌐 Open: http://localhost:3000/app/"
echo ""
echo "Press Ctrl+C to stop all services"

# Cleanup on exit
trap "kill $BACKEND_PID $PROXY_PID 2>/dev/null; docker-compose down" EXIT

wait
```

Make it executable:
```bash
chmod +x test.sh
./test.sh
```
