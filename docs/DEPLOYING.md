# Deployment Guide

This guide covers deploying claude-code-portal to production.

## Prerequisites

- **PostgreSQL Database**
  - [NeonDB](https://neon.tech) (recommended, serverless)
  - Or any PostgreSQL 12+ instance

- **Google OAuth Credentials**
  - [Create OAuth Client](https://console.cloud.google.com/apis/credentials)
  - Set authorized redirect URI: `https://your-domain.com/auth/google/callback`

## Environment Variables

Create a `.env` file or set these environment variables:

```bash
# Database Connection (required)
DATABASE_URL=postgresql://user:password@host:5432/database?sslmode=require

# Google OAuth (required for production)
GOOGLE_CLIENT_ID=your-client-id.apps.googleusercontent.com
GOOGLE_CLIENT_SECRET=your-client-secret
GOOGLE_REDIRECT_URI=https://your-domain.com/auth/google/callback

# Server Configuration
HOST=0.0.0.0
PORT=3000

# Security (required for production)
SESSION_SECRET=generate-a-random-32-char-secret-here

# Frontend Path (usually auto-detected)
FRONTEND_DIST=frontend/dist
```

## Docker Deployment (Recommended)

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

See [DOCKER.md](../DOCKER.md) for detailed Docker deployment instructions.

## Manual Deployment

### 1. Set up PostgreSQL database

Create a database and note the connection string.

### 2. Configure environment

```bash
export DATABASE_URL="postgresql://..."
export GOOGLE_CLIENT_ID="..."
export GOOGLE_CLIENT_SECRET="..."
export GOOGLE_REDIRECT_URI="https://yourdomain.com/auth/google/callback"
export SESSION_SECRET="$(openssl rand -base64 32)"
```

### 3. Build frontend

```bash
cd frontend
trunk build --release
cd ..
```

### 4. Run migrations

```bash
cd backend
diesel migration run
cd ..
```

### 5. Start backend

```bash
cargo run --release -p backend
```

### 6. Distribute proxy binary

```bash
cargo build --release -p proxy
# Copy target/release/claude-portal to dev machines
```

## Backend Command-Line Options

```bash
cargo run -p backend -- [OPTIONS]

Options:
  --dev-mode              Enable development mode (bypasses OAuth)
  --frontend-dist <PATH>  Path to frontend dist directory [default: frontend/dist]
  -h, --help              Print help
```

## Proxy Command-Line Options

```bash
claude-portal [OPTIONS] -- [CLAUDE_ARGS]

Options:
  --backend-url <URL>     Backend WebSocket URL [default: ws://localhost:3000]
  --session-name <NAME>   Session name [default: hostname]
  --auth-token <TOKEN>    Authentication token (skips OAuth)
  --reauth                Force re-authentication
  --logout                Remove cached credentials

  # All other arguments are forwarded to claude CLI
```

## Admin Setup

To grant admin privileges to a user:

```bash
# Open a database shell
./scripts/db-shell.sh

# Or connect directly with psql
psql $DATABASE_URL
```

```sql
-- Grant admin privileges to a user
UPDATE users SET is_admin = true WHERE email = 'your@email.com';
```

Admins can access the admin dashboard at `/admin` which provides:
- System statistics (users, sessions, spend)
- User management (enable/disable, grant/revoke admin)
- Session management (view all sessions, force delete)

## Security Considerations

- **OAuth Tokens**: Stored securely in database, never exposed to frontend
- **WebSocket Auth**: All WebSocket connections require valid auth tokens
- **Session Isolation**: Users can only access their own sessions
- **HTTPS**: Use HTTPS in production (handled by reverse proxy)
- **Environment Secrets**: Never commit `.env` to version control
- **Database**: Use SSL/TLS for database connections in production

## Platform Support

| Platform | Status | Notes |
|----------|--------|-------|
| Linux (x86_64) | Tested | Primary development platform |
| macOS (Apple Silicon) | Untested | Builds in CI, PRs welcome |
| macOS (Intel) | Untested | Builds in CI, PRs welcome |
| Windows (x86_64) | Untested | Builds in CI, PRs welcome |

Pre-built binaries for all platforms are available from [GitHub Releases](https://github.com/meawoppl/claude-code-portal/releases/latest).

## Troubleshooting

See [TROUBLESHOOTING.md](../TROUBLESHOOTING.md) for common issues and solutions.
