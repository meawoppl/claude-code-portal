# cc-proxy

A web-based proxy for Claude Code sessions, enabling remote access to Claude Code running on dedicated computers through a beautiful web interface.

## Try It Out

**Live Demo**: [txcl.io](https://txcl.io)

You can try cc-proxy right now at [txcl.io](https://txcl.io). Sign in with Google to get started - your sessions are isolated and secure. This is a great way to evaluate the project before self-hosting.

## Overview

cc-proxy allows you to:
- Run Claude Code on a powerful remote machine
- Access it from anywhere via a web browser
- Share sessions across multiple clients
- Maintain persistent chat history
- Authenticate securely with Google OAuth

Perfect for teams with dedicated AI workstations or for accessing your home setup while traveling.

## Platform Support

| Platform | Status | Notes |
|----------|--------|-------|
| Linux (x86_64) | ✅ Tested | Primary development platform |
| macOS (Apple Silicon) | ⚠️ Untested | Builds in CI, PRs welcome |
| macOS (Intel) | ⚠️ Untested | Builds in CI, PRs welcome |
| Windows (x86_64) | ⚠️ Untested | Builds in CI, PRs welcome |

Pre-built binaries for all platforms are available from [GitHub Releases](https://github.com/meawoppl/cc-proxy/releases/latest).

**Help Wanted**: If you use macOS or Windows, we'd love your help testing and improving support! Please open issues for any problems you encounter, or submit PRs with fixes.

## Architecture

```mermaid
flowchart TB
    subgraph dev["Dev Machine"]
        subgraph proxy["claude-proxy binary"]
            subgraph codes["claude-codes crate"]
                claude["claude CLI binary"]
            end
        end
    end

    subgraph server["Backend Server"]
        axum["Axum Web Server"]
        db[(PostgreSQL)]
        axum <--> db
    end

    subgraph browser["Web Browser"]
        yew["Yew WASM Frontend"]
    end

    proxy <-->|"WebSocket"| axum
    yew <-->|"WebSocket"| axum
    axum -->|"Serves"| yew
```

### Workspace Structure

This project consists of four Rust crates:

1. **shared** (`shared/`)
   - Common types and message definitions
   - WASM-compatible (no std features)
   - Defines `ProxyMessage` protocol
   - Session and user info types

2. **backend** (`backend/`)
   - Axum async web server
   - PostgreSQL database with Diesel ORM
   - Google OAuth 2.0 authentication
   - Device flow OAuth for CLI
   - WebSocket proxy coordination
   - Session and message persistence

3. **frontend** (`frontend/`)
   - Yew WebAssembly application
   - Reactive UI components
   - Terminal-style session portals
   - Real-time WebSocket communication
   - Beautiful splash and dashboard pages

4. **proxy** (`proxy/`)
   - CLI wrapper for `claude` binary
   - Forwards all arguments transparently
   - Connects to backend via WebSocket
   - Device flow OAuth authentication
   - Per-directory credential caching

## Prerequisites

### Required

- **Rust** (1.78.0 or later)
  ```bash
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  ```

- **Docker** and **Docker Compose**
  - [Install Docker Desktop](https://docs.docker.com/get-docker/)
  - Or: `brew install docker` (macOS)
  - Or: `sudo apt-get install docker.io docker-compose` (Ubuntu)

- **Claude CLI** (for using the proxy)
  - Follow [Claude CLI installation](https://docs.anthropic.com/claude/docs/claude-cli)

### Auto-Installed by Scripts

These will be installed automatically when you run `./scripts/dev.sh`:

- **diesel_cli** - Database migration tool
- **trunk** - WASM build tool and dev server
- **wasm32 target** - Rust WebAssembly compilation target

### For Production Deployment

- **PostgreSQL Database**
  - [NeonDB](https://neon.tech) (recommended, serverless)
  - Or any PostgreSQL 12+ instance

- **Google OAuth Credentials**
  - [Create OAuth Client](https://console.cloud.google.com/apis/credentials)
  - Set authorized redirect URI: `https://your-domain.com/auth/google/callback`

## Quick Start

### Option 1: Automated Dev Mode (Recommended)

The fastest way to get started:

```bash
# Clone the repository
git clone https://github.com/meawoppl/cc-proxy.git
cd cc-proxy

# Start everything (auto-installs dependencies)
./scripts/dev.sh start
```

This will:
1. ✅ Start PostgreSQL in Docker
2. ✅ Run database migrations
3. ✅ Build the frontend
4. ✅ Start the backend in dev mode (no OAuth needed)
5. ✅ Auto-install diesel/trunk if missing

Then open: **http://localhost:3000/**

You'll be automatically logged in as `testing@testing.local`

**Development Commands:**
```bash
./scripts/dev.sh status   # Show status of all services
./scripts/dev.sh logs     # Tail backend logs
./scripts/dev.sh stop     # Stop all services
./scripts/dev.sh restart  # Restart all services
./scripts/dev.sh build    # Rebuild frontend only
```

### Option 2: Manual Setup

For more control over the setup process:

#### 1. Install Dependencies

```bash
# Install Rust build tools (one-time setup)
./scripts/install-deps.sh

# Or manually:
rustup target add wasm32-unknown-unknown
cargo install diesel_cli --no-default-features --features postgres
cargo install trunk
```

#### 2. Start PostgreSQL

```bash
# Using Docker (recommended for testing)
docker-compose -f docker-compose.test.yml up -d db

# Or use your own PostgreSQL instance
# Make sure it's accessible at localhost:5432
```

#### 3. Configure Environment

```bash
# Copy example environment file
cp .env.example .env

# For dev mode, this is sufficient:
export DATABASE_URL="postgresql://ccproxy:dev_password_change_in_production@localhost:5432/ccproxy"

# For production, edit .env with your credentials
```

#### 4. Run Database Migrations

```bash
cd backend
diesel migration run
cd ..
```

#### 5. Build and Run

```bash
# Terminal 1: Start backend in dev mode
cargo run -p backend -- --dev-mode

# Terminal 2: Build and serve frontend
cd frontend
trunk serve --open
# Opens browser to http://localhost:8080

# Terminal 3: Run proxy (optional)
cargo run -p proxy -- --backend-url ws://localhost:3000
```

## Configuration

### Environment Variables

Create a `.env` file in the project root:

```bash
# Database Connection
DATABASE_URL=postgresql://user:password@host:5432/database?sslmode=require

# Google OAuth (Required for production)
GOOGLE_CLIENT_ID=your-client-id.apps.googleusercontent.com
GOOGLE_CLIENT_SECRET=your-client-secret
GOOGLE_REDIRECT_URI=https://your-domain.com/auth/google/callback

# Server Configuration
HOST=0.0.0.0
PORT=3000

# Security
SESSION_SECRET=generate-a-random-32-char-secret-here

# Frontend Path (usually auto-detected)
FRONTEND_DIST=frontend/dist

# Development Mode (bypasses OAuth)
DEV_MODE=false
```

### Backend Command-Line Options

```bash
cargo run -p backend -- [OPTIONS]

Options:
  --dev-mode              Enable development mode (bypasses OAuth)
  --frontend-dist <PATH>  Path to frontend dist directory [default: frontend/dist]
  -h, --help              Print help
```

### Proxy Command-Line Options

```bash
claude-proxy [OPTIONS] -- [CLAUDE_ARGS]

Options:
  --backend-url <URL>     Backend WebSocket URL [default: ws://localhost:3000]
  --session-name <NAME>   Session name [default: hostname]
  --auth-token <TOKEN>    Authentication token (skips OAuth)
  --reauth                Force re-authentication
  --logout                Remove cached credentials

  # All other arguments are forwarded to claude CLI
```

## Usage

### For End Users (Web Interface)

1. **Open the web interface**
   ```
   https://your-deployed-domain.com
   ```

2. **Sign in with Google**
   - Click "Sign in with Google"
   - Authorize the application
   - You'll be redirected to the dashboard

3. **View active sessions**
   - See all Claude Code sessions running on remote machines
   - Sessions appear as interactive terminal portals
   - Shows session name, working directory, and status

4. **Chat with Claude**
   - Click any session portal to open terminal
   - Type messages to interact with Claude
   - All history is preserved

### For Developers (Running Proxy)

On your development machine where you want Claude Code to run:

```bash
# Basic usage
claude-proxy \
  --backend-url wss://your-domain.com \
  --session-name "my-dev-machine"

# With custom session name
claude-proxy \
  --backend-url wss://your-domain.com \
  --session-name "$(whoami)@$(hostname)" \
  # Any claude CLI args:
  --model claude-opus-4 \
  --verbose

# Force re-authentication
claude-proxy --reauth --backend-url wss://your-domain.com

# Logout (clear cached credentials)
claude-proxy --logout
```

On first run, the proxy will:
1. Display a verification URL and code
2. You open the URL in a browser
3. Sign in with Google
4. Enter the verification code
5. Credentials are cached in `~/.config/cc-proxy/config.json`

### Authentication Flow

```
┌──────────────┐
│ Run proxy    │
└──────┬───────┘
       │
       ▼
┌──────────────────────────────┐
│ Already authenticated?       │
│ (check ~/.config/cc-proxy/)  │
└──────┬───────────────────────┘
       │
   Yes │ No
       │
       ├─────────────────────┐
       │                     ▼
       │            ┌─────────────────┐
       │            │ Device Flow     │
       │            │ - Display code  │
       │            │ - Show URL      │
       │            │ - Wait for auth │
       │            └────────┬────────┘
       │                     │
       │                     ▼
       │            ┌─────────────────┐
       │            │ User visits URL │
       │            │ - Signs in      │
       │            │ - Enters code   │
       │            └────────┬────────┘
       │                     │
       │                     ▼
       │            ┌─────────────────┐
       │            │ Cache creds     │
       │            └────────┬────────┘
       │                     │
       ├─────────────────────┘
       │
       ▼
┌──────────────┐
│ Connect to   │
│ backend      │
└──────────────┘
```

## Development

### Project Structure

```
cc-proxy/
├── Cargo.toml              # Workspace definition
├── Cargo.lock              # Dependency lock file
│
├── shared/                 # Common types (WASM-compatible)
│   ├── Cargo.toml
│   └── src/
│       └── lib.rs          # ProxyMessage, SessionInfo, etc.
│
├── backend/                # Axum server
│   ├── Cargo.toml
│   ├── diesel.toml         # Diesel CLI config
│   ├── migrations/         # Database schemas
│   │   └── 00000000000000_initial_setup/
│   │       ├── up.sql
│   │       └── down.sql
│   ├── src/
│   │   ├── main.rs         # Server entry point
│   │   ├── db.rs           # Database connection pool
│   │   ├── models.rs       # Diesel models
│   │   ├── schema.rs       # Generated by Diesel
│   │   └── handlers/
│   │       ├── mod.rs
│   │       ├── auth.rs     # OAuth handlers
│   │       ├── device_flow.rs  # CLI OAuth
│   │       ├── sessions.rs # Session API
│   │       └── websocket.rs    # WebSocket coordination
│   └── Dockerfile          # Container image
│
├── frontend/               # Yew WebAssembly app
│   ├── Cargo.toml
│   ├── index.html          # HTML shell
│   ├── styles.css          # Global styles
│   └── src/
│       ├── lib.rs          # App entry point
│       └── pages/
│           ├── mod.rs
│           ├── splash.rs   # Landing page
│           ├── dashboard.rs    # Session list
│           └── terminal.rs     # Chat interface
│
├── proxy/                  # CLI wrapper
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs         # Proxy entry point
│       ├── auth.rs         # Device flow client
│       └── config.rs       # Credential storage
│
├── scripts/                # Helper scripts
│   ├── README.md
│   ├── install-deps.sh     # Install build tools
│   ├── dev.sh              # Development environment (start/stop/status)
│   ├── test-dev.sh         # Legacy: foreground dev mode
│   ├── test-oauth.sh       # Test with real OAuth
│   ├── clean.sh            # Clean up everything
│   ├── db-shell.sh         # Open database shell
│   └── update-rust.sh      # Update Rust toolchain
│
├── docker-compose.yml      # Production Docker setup
├── docker-compose.test.yml # Testing setup (just DB)
├── .dockerignore
├── .gitignore
├── .env.example            # Environment template
│
├── README.md               # This file
├── CLAUDE.md               # AI assistant instructions
├── TROUBLESHOOTING.md      # Common issues
├── DOCKER.md               # Docker deployment
├── PROXY_AUTH.md           # Auth flow details
└── TEST.md                 # Testing guide
```

### Building Individual Components

```bash
# Build everything
cargo build --workspace

# Build specific crate
cargo build -p backend
cargo build -p frontend
cargo build -p proxy

# Build for release (optimized)
cargo build --workspace --release

# Build frontend (WASM)
cd frontend
trunk build --release
```

### Running Tests

```bash
# Run Rust tests
cargo test --workspace

# Test backend only
cargo test -p backend

# Check code without building
cargo check --workspace

# Lint and format
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

### Database Management

```bash
# Run migrations
cd backend
diesel migration run

# Revert last migration
diesel migration revert

# Create new migration
diesel migration generate add_new_feature

# Reset database (caution!)
diesel database reset

# Open psql shell
./scripts/db-shell.sh
```

### Development Workflow

**Typical workflow for adding a feature:**

1. **Create a branch**
   ```bash
   git checkout -b feature/new-feature
   ```

2. **Make changes**
   - Modify code in appropriate crate
   - If changing shared protocol, update `shared/src/lib.rs`
   - If adding database fields, create migration

3. **Test locally**
   ```bash
   ./scripts/dev.sh start
   # Verify changes work at http://localhost:3000/
   ./scripts/dev.sh stop
   ```

4. **Run checks**
   ```bash
   cargo test --workspace
   cargo clippy --workspace
   cargo fmt
   ```

5. **Commit and push**
   ```bash
   git add .
   git commit -m "Add new feature"
   git push origin feature/new-feature
   ```

### Hot Reload Development

For rapid iteration:

```bash
# Terminal 1: Backend with auto-reload
cargo watch -x 'run -p backend -- --dev-mode'

# Terminal 2: Frontend with hot reload
cd frontend
trunk serve

# Terminal 3: Proxy
cargo run -p proxy
```

## Deployment

### Docker (Recommended)

```bash
# Build images
docker-compose build

# Start services
docker-compose up -d

# View logs
docker-compose logs -f backend

# Stop services
docker-compose down
```

See [DOCKER.md](DOCKER.md) for detailed deployment instructions.

### Manual Deployment

1. **Set up PostgreSQL database**
   - Create database
   - Note connection string

2. **Configure environment**
   ```bash
   export DATABASE_URL="postgresql://..."
   export GOOGLE_CLIENT_ID="..."
   export GOOGLE_CLIENT_SECRET="..."
   export GOOGLE_REDIRECT_URI="https://yourdomain.com/auth/google/callback"
   export SESSION_SECRET="$(openssl rand -base64 32)"
   ```

3. **Build frontend**
   ```bash
   cd frontend
   trunk build --release
   cd ..
   ```

4. **Run migrations**
   ```bash
   cd backend
   diesel migration run
   cd ..
   ```

5. **Start backend**
   ```bash
   cargo run --release -p backend
   ```

6. **Distribute proxy binary**
   ```bash
   cargo build --release -p proxy
   # Copy target/release/claude-proxy to dev machines
   ```

## Troubleshooting

See [TROUBLESHOOTING.md](TROUBLESHOOTING.md) for detailed solutions to common issues.

### Quick Fixes

**Backend won't start:**
```bash
# Check database connection
psql $DATABASE_URL

# Check if port 3000 is in use
lsof -i :3000

# Check logs
tail -f /tmp/cc-proxy-backend.log
```

**Frontend won't build:**
```bash
# Ensure wasm target is installed
rustup target add wasm32-unknown-unknown

# Clean and rebuild
cd frontend
trunk clean
trunk build
```

**Proxy won't connect:**
```bash
# Check backend is running
curl http://localhost:3000/

# Try re-authenticating
claude-proxy --reauth --backend-url ws://localhost:3000

# Check credentials
cat ~/.config/cc-proxy/config.json
```

**"diesel CLI not found":**
```bash
# Update Rust first
rustup update stable

# Install diesel
cargo install diesel_cli --no-default-features --features postgres
```

## Security Considerations

- **OAuth Tokens**: Stored securely in database, never exposed to frontend
- **WebSocket Auth**: All WebSocket connections require valid auth tokens
- **Session Isolation**: Users can only access their own sessions
- **HTTPS**: Use HTTPS in production (handled by reverse proxy)
- **Environment Secrets**: Never commit `.env` to version control
- **Database**: Use SSL/TLS for database connections in production

## Technologies

- **Backend Framework**: [Axum](https://github.com/tokio-rs/axum) 0.7
- **Frontend Framework**: [Yew](https://yew.rs/) 0.21
- **Database ORM**: [Diesel](https://diesel.rs/) 2.2
- **WebSockets**: [tokio-tungstenite](https://github.com/snapview/tokio-tungstenite)
- **OAuth**: [oauth2-rs](https://github.com/ramosbugs/oauth2-rs)
- **Serialization**: [Serde](https://serde.rs/)
- **Async Runtime**: [Tokio](https://tokio.rs/)
- **Claude Integration**: [claude-codes](https://crates.io/crates/claude-codes)

## API Reference

### HTTP Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/` | Health check |
| GET | `/auth/google` | Initiate OAuth flow |
| GET | `/auth/google/callback` | OAuth callback |
| GET | `/auth/me` | Get current user |
| POST | `/auth/device/code` | Request device code |
| POST | `/auth/device/poll` | Poll for auth completion |
| GET | `/auth/device` | Device verification page |
| GET | `/api/sessions` | List user's sessions |
| GET | `/api/sessions/:id` | Get session details |
| POST | `/api/sessions/:id/messages` | Send message to session |

### WebSocket Endpoints

**`/ws/session`** - Proxy Connection
- Connects a Claude Code proxy to the backend
- Sends `ProxyMessage::Register` on connection
- Receives messages to forward to Claude
- Sends Claude output back to backend

**`/ws/client`** - Browser Connection
- Connects a web browser client
- Can subscribe to specific session
- Receives real-time updates
- Sends messages to Claude through proxy

### Message Protocol

```rust
enum ProxyMessage {
    Register {
        session_name: String,
        auth_token: Option<String>,
        working_directory: String,
    },
    ClaudeOutput { content: Value },
    ClaudeInput { content: Value },
    Heartbeat,
    Error { message: String },
    SessionStatus { status: SessionStatus },
}
```

## Contributing

Contributions are welcome! Please:

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Add tests if applicable
5. Run `cargo test` and `cargo clippy`
6. Submit a pull request

Please open an issue first to discuss major changes.

## License

MIT License - see [LICENSE](LICENSE) file for details

## Support

- **Issues**: [GitHub Issues](https://github.com/meawoppl/cc-proxy/issues)
- **Discussions**: [GitHub Discussions](https://github.com/meawoppl/cc-proxy/discussions)
- **Documentation**: See additional docs in project root

## Roadmap

- [ ] Support for multiple OAuth providers
- [ ] Session sharing and collaboration
- [ ] Enhanced terminal emulation
- [ ] File upload/download support
- [ ] Session recording and playback
- [ ] Kubernetes deployment templates
- [ ] CLI session management commands
- [ ] End-to-end encryption option
- [ ] Rate limiting and quotas
- [ ] Admin dashboard
