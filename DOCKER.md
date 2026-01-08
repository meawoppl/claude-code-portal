# Docker Deployment Guide

This guide covers running CC-Proxy backend in Docker with 1Password secret injection.

## Quick Start (Local Development)

For local development without 1Password:

```bash
# Start PostgreSQL and backend
docker-compose up -d

# Run database migrations
docker-compose exec backend /app/backend migrate

# View logs
docker-compose logs -f backend

# Stop everything
docker-compose down
```

## Production Deployment with 1Password

### Option 1: Using 1Password Service Account (Recommended)

1. **Create a 1Password Service Account**
   - Go to your 1Password account settings
   - Create a service account with read access to your vault
   - Copy the service account token

2. **Set up your secrets in 1Password**
   - Follow the guide in `SETUP_1PASSWORD.md`
   - Ensure all secrets are stored in 1Password

3. **Build the Docker image**
   ```bash
   docker build -f backend/Dockerfile -t cc-proxy-backend .
   ```

4. **Run with service account token**
   ```bash
   docker run -d \
     --name cc-proxy-backend \
     -p 3000:3000 \
     -e OP_SERVICE_ACCOUNT_TOKEN="your_service_account_token" \
     cc-proxy-backend
   ```

### Option 2: Using 1Password Connect

1. **Deploy 1Password Connect Server**
   ```bash
   # Follow: https://developer.1password.com/docs/connect/get-started/
   docker run -d \
     --name op-connect \
     -p 8080:8080 \
     -v /path/to/1password-credentials.json:/home/opuser/.op/1password-credentials.json \
     1password/connect-api:latest
   ```

2. **Configure backend to use Connect**
   ```bash
   docker run -d \
     --name cc-proxy-backend \
     -p 3000:3000 \
     -e OP_CONNECT_HOST="http://op-connect:8080" \
     -e OP_CONNECT_TOKEN="your_connect_token" \
     --link op-connect \
     cc-proxy-backend
   ```

### Option 3: Mount 1Password CLI Config (Local Testing)

```bash
# Sign in to 1Password CLI locally first
op signin

# Run container with mounted config
docker run -d \
  --name cc-proxy-backend \
  -p 3000:3000 \
  -v ~/.config/op:/root/.config/op:ro \
  cc-proxy-backend
```

## Docker Compose Production Setup

Create a `docker-compose.prod.yml`:

```yaml
services:
  backend:
    build:
      context: .
      dockerfile: backend/Dockerfile
    container_name: cc-proxy-backend
    ports:
      - "3000:3000"
    environment:
      # 1Password Service Account Token
      OP_SERVICE_ACCOUNT_TOKEN: ${OP_SERVICE_ACCOUNT_TOKEN}

      # Or 1Password Connect
      # OP_CONNECT_HOST: http://op-connect:8080
      # OP_CONNECT_TOKEN: ${OP_CONNECT_TOKEN}
    restart: always
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:3000/"]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 10s
    networks:
      - cc-proxy-network

  # Optional: 1Password Connect
  op-connect:
    image: 1password/connect-api:latest
    container_name: op-connect
    ports:
      - "8080:8080"
    volumes:
      - ./1password-credentials.json:/home/opuser/.op/1password-credentials.json:ro
    environment:
      OP_SESSION: ${OP_CONNECT_TOKEN}
    networks:
      - cc-proxy-network

networks:
  cc-proxy-network:
    driver: bridge
```

Run with:

```bash
# Set your service account token
export OP_SERVICE_ACCOUNT_TOKEN="your_token_here"

# Start services
docker-compose -f docker-compose.prod.yml up -d

# View logs
docker-compose -f docker-compose.prod.yml logs -f
```

## Kubernetes Deployment

### With 1Password Operator

1. **Install 1Password Operator**
   ```bash
   helm repo add 1password https://1password.github.io/connect-helm-charts
   helm install connect 1password/connect \
     --set-file connect.credentials=1password-credentials.json
   ```

2. **Create OnePasswordItem resource**
   ```yaml
   # k8s/secrets.yaml
   apiVersion: onepassword.com/v1
   kind: OnePasswordItem
   metadata:
     name: cc-proxy-secrets
   spec:
     itemPath: "vaults/Development/items/cc-proxy-secrets"
   ```

3. **Deploy backend**
   ```yaml
   # k8s/deployment.yaml
   apiVersion: apps/v1
   kind: Deployment
   metadata:
     name: cc-proxy-backend
   spec:
     replicas: 2
     selector:
       matchLabels:
         app: cc-proxy-backend
     template:
       metadata:
         labels:
           app: cc-proxy-backend
       spec:
         containers:
         - name: backend
           image: cc-proxy-backend:latest
           ports:
           - containerPort: 3000
           env:
           - name: DATABASE_URL
             valueFrom:
               secretKeyRef:
                 name: cc-proxy-secrets
                 key: database_url
           - name: GOOGLE_CLIENT_ID
             valueFrom:
               secretKeyRef:
                 name: cc-proxy-secrets
                 key: google_client_id
           # ... etc
   ```

## Building for Different Architectures

### Multi-platform build

```bash
# Enable buildx
docker buildx create --use

# Build for multiple platforms
docker buildx build \
  --platform linux/amd64,linux/arm64 \
  -f backend/Dockerfile \
  -t your-registry/cc-proxy-backend:latest \
  --push \
  .
```

## Optimizing Build Times

### Use BuildKit cache

```bash
# Enable BuildKit
export DOCKER_BUILDKIT=1

# Build with cache
docker build \
  --cache-from cc-proxy-backend:latest \
  -f backend/Dockerfile \
  -t cc-proxy-backend:latest \
  .
```

### Pre-cache dependencies

```bash
# Build a dependencies-only image first
docker build \
  --target builder \
  -f backend/Dockerfile \
  -t cc-proxy-backend:deps \
  .

# Then build full image (will use cached deps)
docker build \
  --cache-from cc-proxy-backend:deps \
  -f backend/Dockerfile \
  -t cc-proxy-backend:latest \
  .
```

## Environment Variables

The Docker container supports these environment variables:

### Required (via 1Password)
- `DATABASE_URL` - PostgreSQL connection string
- `GOOGLE_CLIENT_ID` - Google OAuth client ID
- `GOOGLE_CLIENT_SECRET` - Google OAuth client secret
- `SESSION_SECRET` - Session encryption key

### Optional
- `GOOGLE_REDIRECT_URI` - OAuth callback (default: http://localhost:3000/auth/google/callback)
- `HOST` - Bind host (default: 0.0.0.0)
- `PORT` - Bind port (default: 3000)

### 1Password Configuration
- `OP_SERVICE_ACCOUNT_TOKEN` - Service account token for secret access
- `OP_CONNECT_HOST` - 1Password Connect server URL
- `OP_CONNECT_TOKEN` - 1Password Connect access token

## Troubleshooting

### Container exits immediately

Check logs:
```bash
docker logs cc-proxy-backend
```

Common issues:
- 1Password CLI can't authenticate (missing token)
- Database connection failed
- Missing required environment variables

### 1Password authentication failed

Verify your service account token:
```bash
docker run --rm \
  -e OP_SERVICE_ACCOUNT_TOKEN="your_token" \
  cc-proxy-backend \
  op vault list
```

### Database connection failed

Test database connectivity:
```bash
docker run --rm \
  -e DATABASE_URL="postgresql://..." \
  cc-proxy-backend \
  bash -c 'apt-get update && apt-get install -y postgresql-client && psql $DATABASE_URL -c "SELECT 1"'
```

### Health check failing

Test the endpoint manually:
```bash
docker exec cc-proxy-backend curl -f http://localhost:3000/
```

## Production Checklist

- [ ] Use 1Password Service Account or Connect for secret management
- [ ] Set up proper database backups
- [ ] Configure TLS/SSL termination (via reverse proxy)
- [ ] Set up monitoring and logging
- [ ] Use a production-grade PostgreSQL (not docker-compose db)
- [ ] Configure proper resource limits
- [ ] Set up auto-restart policies
- [ ] Use secrets management for OP_SERVICE_ACCOUNT_TOKEN
- [ ] Enable Docker Content Trust for image verification
- [ ] Regular security updates (`docker pull` latest base images)

## Example Production Stack

```yaml
# docker-compose.prod.yml - Full production stack
services:
  # Traefik reverse proxy with TLS
  traefik:
    image: traefik:v2.10
    command:
      - "--providers.docker=true"
      - "--entrypoints.web.address=:80"
      - "--entrypoints.websecure.address=:443"
      - "--certificatesresolvers.letsencrypt.acme.email=admin@example.com"
      - "--certificatesresolvers.letsencrypt.acme.storage=/letsencrypt/acme.json"
      - "--certificatesresolvers.letsencrypt.acme.httpchallenge.entrypoint=web"
    ports:
      - "80:80"
      - "443:443"
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock:ro
      - ./letsencrypt:/letsencrypt

  backend:
    image: cc-proxy-backend:latest
    environment:
      OP_SERVICE_ACCOUNT_TOKEN: ${OP_SERVICE_ACCOUNT_TOKEN}
    labels:
      - "traefik.enable=true"
      - "traefik.http.routers.backend.rule=Host(`api.yoursite.com`)"
      - "traefik.http.routers.backend.entrypoints=websecure"
      - "traefik.http.routers.backend.tls.certresolver=letsencrypt"
    restart: always
```
