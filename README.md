# cc-proxy

A web-based proxy for Claude Code sessions, enabling remote access to Claude Code running on dedicated computers through a beautiful web interface.

## Architecture

This workspace consists of four Rust crates:

1. **shared** - Common types and message definitions (WASM-compatible)
2. **backend** - Axum web server with Google OAuth, WebSocket proxying, and PostgreSQL storage
3. **frontend** - Yew-based WebAssembly UI with terminal-style session portals
4. **proxy** - CLI wrapper that forwards Claude Code sessions to the backend

## How It Works

1. Run the `claude-proxy` binary on your dedicated development machine
2. It starts a Claude Code session and connects to the backend via WebSocket
3. Log into the web interface with Google OAuth
4. See all your active sessions as interactive terminal portals
5. Click any portal to chat with that remote Claude Code session

## Prerequisites

- **Rust** (latest stable) - [Install from rustup.rs](https://rustup.rs)
- **Docker** and **Docker Compose** - [Install Docker](https://docs.docker.com/get-docker/)

**Optional** (installed automatically by test scripts):
- diesel CLI - for database migrations
- trunk - for building WASM frontend

**For production:**
- A PostgreSQL database (NeonDB recommended)
- Google OAuth credentials ([Get them here](https://console.cloud.google.com/apis/credentials))

## Setup

### Quick Start (Recommended for Testing)

```bash
# First time only: Install dependencies
./scripts/install-deps.sh

# Start everything in dev mode
./scripts/test-dev.sh

# Open browser to http://localhost:3000/app/
# Automatically logged in as testing@testing.local
```

The test script will:
- Start PostgreSQL in Docker
- Run database migrations
- Build the frontend
- Start backend and proxy
- Auto-install diesel/trunk if missing

For full OAuth testing or production deployment, see [Docker](#docker) section below.

### 1. Clone and Configure

```bash
git clone <repo-url>
cd cc-proxy

# Copy and configure environment variables
cp .env.example .env
# Edit .env with your actual credentials
```

### 2. Set Up Database

```bash
cd backend
diesel setup
diesel migration run
cd ..
```

### 3. Run the Backend

#### Option A: Dev Mode (Easiest for Testing)

```bash
# Uses test user, bypasses OAuth
export DEV_MODE=true
export DATABASE_URL="postgresql://ccproxy:dev_password_change_in_production@localhost:5432/ccproxy"

cargo run -p backend -- --dev-mode
```

#### Option B: With OAuth (Production)

```bash
# Create .env with real OAuth credentials
cp .env.example .env
# Edit .env with your Google OAuth credentials

cargo run -p backend
```

#### Option C: Docker

```bash
docker-compose up backend
```

### 4. Run the Frontend (in a new terminal)

```bash
cd frontend
trunk serve
# Frontend will run on http://localhost:8080
```

### 5. Connect a Claude Code Session (on your dev machine)

```bash
# Build the proxy binary
cargo build --release -p proxy

# Run it (forwards all args to claude CLI)
./target/release/claude-proxy \
  --backend-url ws://localhost:3000 \
  --session-name "my-dev-machine" \
  # ... any other claude flags
```

## Environment Variables

Configure these in your `.env` file:

```bash
# Database (NeonDB)
DATABASE_URL=postgresql://user:pass@host/database?sslmode=require

# Google OAuth
GOOGLE_CLIENT_ID=your_client_id.apps.googleusercontent.com
GOOGLE_CLIENT_SECRET=your_client_secret
GOOGLE_REDIRECT_URI=http://localhost:3000/auth/google/callback

# Server
HOST=0.0.0.0
PORT=3000

# Session encryption
SESSION_SECRET=generate_a_random_secret_here
```

## Usage

### Starting a Proxied Session

On your remote development machine:

```bash
claude-proxy \
  --backend-url ws://your-server.com \
  --session-name "$(whoami)@$(hostname)" \
  --auth-token your_optional_token \
  # Forward any claude CLI arguments:
  --model opus \
  --verbose
```

### Accessing via Web Interface

1. Open your browser to `http://localhost:8080` (or your deployed URL)
2. Click "Sign in with Google"
3. View your active sessions as terminal portals
4. Click any portal to interact with that session

## Development

### Project Structure

```
cc-proxy/
├── shared/          # Common types (WASM-compatible)
│   └── src/
│       └── lib.rs   # ProxyMessage, SessionInfo, etc.
├── backend/         # Axum server
│   ├── migrations/  # Database schemas
│   └── src/
│       ├── handlers/  # WebSocket, OAuth, API
│       ├── models.rs  # Database models
│       └── main.rs
├── frontend/        # Yew WebAssembly app
│   └── src/
│       ├── pages/     # Splash, Dashboard, Terminal
│       └── lib.rs
└── proxy/           # CLI wrapper
    └── src/
        └── main.rs  # claude-proxy binary
```

### Testing Locally

**Easy way:**
```bash
./scripts/test-dev.sh  # All-in-one dev mode
```

**Manual way:**
1. Terminal 1: `docker-compose -f docker-compose.test.yml up db`
2. Terminal 2: `cargo run -p backend -- --dev-mode`
3. Terminal 3: `cargo run -p proxy`
4. Browser: Open `http://localhost:3000/app/`

See [scripts/README.md](scripts/README.md) for more testing options.

## Technologies

- **Backend**: Axum, Diesel, PostgreSQL, OAuth2, WebSockets
- **Frontend**: Yew, WebAssembly, CSS3
- **Proxy**: Claude-codes crate, Tokio, WebSockets
- **Shared**: Serde, WASM-compatible types

## API Endpoints

### HTTP

- `GET /auth/google` - Initiate OAuth flow
- `GET /auth/google/callback` - OAuth callback
- `GET /api/sessions` - List active sessions
- `GET /api/sessions/:id` - Get session details
- `POST /api/sessions/:id/messages` - Send message

### WebSocket

- `WS /ws/session` - Connect a Claude Code proxy
- `WS /ws/client` - Connect a web browser client

## License

MIT

## Contributing

Contributions welcome! Please open an issue first to discuss changes.