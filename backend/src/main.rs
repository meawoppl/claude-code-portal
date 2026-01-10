mod db;
mod handlers;
mod jwt;
mod models;
mod schema;

use crate::db::DbPool;
use crate::handlers::device_flow::DeviceFlowStore;
use axum::{
    routing::{get, post},
    Router,
};
use clap::Parser;
use oauth2::{basic::BasicClient, AuthUrl, ClientId, ClientSecret, RedirectUrl, TokenUrl};
use std::{env, sync::Arc};
use tower_cookies::{CookieManagerLayer, Key};
use tower_http::{
    cors::{Any, CorsLayer},
    services::{ServeDir, ServeFile},
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use handlers::websocket::SessionManager;

#[derive(Parser, Debug, Clone)]
#[command(name = "cc-proxy-backend")]
#[command(about = "CC-Proxy backend server")]
struct Args {
    /// Enable development mode (bypasses OAuth, creates test user)
    #[arg(long)]
    dev_mode: bool,

    /// Path to frontend dist directory to serve
    #[arg(long, default_value = "frontend/dist")]
    frontend_dist: String,
}

#[derive(Clone)]
pub struct AppState {
    pub dev_mode: bool,
    pub db_pool: DbPool,
    pub session_manager: SessionManager,
    pub oauth_basic_client: Option<BasicClient>,
    pub device_flow_store: Option<DeviceFlowStore>,
    pub public_url: String,
    pub cookie_key: Key,
    pub jwt_secret: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse CLI arguments
    let args = Args::parse();

    // Initialize tracing with info level by default
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    if args.dev_mode {
        tracing::warn!("🚧 DEV MODE ENABLED - OAuth is bypassed, test user will be used");
    }

    // Load environment variables
    dotenvy::dotenv().ok();

    // Create database pool
    let pool = db::create_pool()?;

    // Create device flow store
    let device_flow_store = handlers::device_flow::DeviceFlowStore::default();

    // Create OAuth client (skip in dev mode)
    let oauth_basic_client = if !args.dev_mode {
        let client_id =
            ClientId::new(env::var("GOOGLE_CLIENT_ID").expect("GOOGLE_CLIENT_ID must be set"));
        let client_secret = ClientSecret::new(
            env::var("GOOGLE_CLIENT_SECRET").expect("GOOGLE_CLIENT_SECRET must be set"),
        );
        let auth_url = AuthUrl::new("https://accounts.google.com/o/oauth2/v2/auth".to_string())?;
        let token_url = TokenUrl::new("https://oauth2.googleapis.com/token".to_string())?;
        let redirect_uri = RedirectUrl::new(
            env::var("GOOGLE_REDIRECT_URI").expect("GOOGLE_REDIRECT_URI must be set"),
        )?;

        Some(
            BasicClient::new(client_id, Some(client_secret), auth_url, Some(token_url))
                .set_redirect_uri(redirect_uri),
        )
    } else {
        None
    };

    // Create test user in dev mode
    if args.dev_mode {
        use diesel::prelude::*;
        use models::NewUser;
        use schema::users::dsl::*;

        let mut conn = pool.get()?;
        let test_user = users
            .filter(email.eq("testing@testing.local"))
            .first::<models::User>(&mut conn)
            .optional()?;

        if test_user.is_none() {
            let new_user = NewUser {
                google_id: "dev_mode_test_user".to_string(),
                email: "testing@testing.local".to_string(),
                name: Some("Test User".to_string()),
                avatar_url: None,
            };

            diesel::insert_into(users)
                .values(&new_user)
                .execute(&mut conn)?;

            tracing::info!("✓ Created test user: testing@testing.local");
        }
    }

    // Create session manager for WebSocket connections
    let session_manager = SessionManager::new();

    // Cleanup stale sessions on startup (mark all "active" sessions as "disconnected"
    // since they can't be active if we just started)
    {
        use diesel::prelude::*;
        use schema::sessions::dsl::*;
        let mut conn = pool.get()?;
        let updated = diesel::update(sessions.filter(status.eq("active")))
            .set(status.eq("disconnected"))
            .execute(&mut conn)?;
        if updated > 0 {
            tracing::info!(
                "Marked {} stale sessions as disconnected on startup",
                updated
            );
        }
    }

    // Get base URL from env or construct from host/port
    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let public_url = env::var("BASE_URL").unwrap_or_else(|_| {
        // Default to localhost for development
        format!("http://localhost:{}", port)
    });

    // Create cookie signing key from SESSION_SECRET or generate random for dev
    let session_secret = env::var("SESSION_SECRET").ok();
    let cookie_key = match &session_secret {
        Some(secret) => {
            let bytes = secret.as_bytes();
            if bytes.len() < 64 {
                tracing::warn!("SESSION_SECRET should be at least 64 bytes, padding with zeros");
                let mut padded = vec![0u8; 64];
                padded[..bytes.len()].copy_from_slice(bytes);
                Key::from(&padded)
            } else {
                Key::from(&bytes[..64])
            }
        }
        None => {
            if args.dev_mode {
                tracing::warn!("No SESSION_SECRET set, using random key (sessions won't persist across restarts)");
                Key::generate()
            } else {
                panic!("SESSION_SECRET must be set in production mode");
            }
        }
    };

    // JWT secret for proxy tokens (uses SESSION_SECRET or generates for dev)
    let jwt_secret = session_secret.unwrap_or_else(|| {
        if args.dev_mode {
            "dev-mode-jwt-secret-not-for-production".to_string()
        } else {
            panic!("SESSION_SECRET must be set in production mode");
        }
    });

    // Create app state
    let app_state = Arc::new(AppState {
        dev_mode: args.dev_mode,
        db_pool: pool.clone(),
        session_manager: session_manager.clone(),
        oauth_basic_client,
        device_flow_store: if args.dev_mode {
            None
        } else {
            Some(device_flow_store.clone())
        },
        public_url: public_url.clone(),
        cookie_key,
        jwt_secret,
    });

    // Setup CORS
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // Build our application with routes
    let mut app = Router::new()
        // Health check endpoint
        .route("/api/health", get(|| async { "OK" }))
        // Session API routes
        .route("/api/sessions", get(handlers::sessions::list_sessions))
        .route("/api/sessions/:id", get(handlers::sessions::get_session))
        .route(
            "/api/sessions/:id",
            axum::routing::delete(handlers::sessions::delete_session),
        )
        .route(
            "/api/sessions/:id/messages",
            post(handlers::sessions::send_message),
        )
        // Proxy token management endpoints
        .route(
            "/api/proxy-tokens",
            get(handlers::proxy_tokens::list_tokens_handler)
                .post(handlers::proxy_tokens::create_token_handler),
        )
        .route(
            "/api/proxy-tokens/:id",
            axum::routing::delete(handlers::proxy_tokens::revoke_token_handler),
        )
        // Auth routes (under /api/auth)
        .route("/api/auth/google", get(handlers::auth::login))
        .route("/api/auth/google/callback", get(handlers::auth::callback))
        .route("/api/auth/me", get(handlers::auth::me))
        .route("/api/auth/dev-login", get(handlers::auth::dev_login))
        // Device flow endpoints for CLI (under /api/auth)
        .route(
            "/api/auth/device/code",
            post(handlers::device_flow::device_code),
        )
        .route(
            "/api/auth/device/poll",
            post(handlers::device_flow::device_poll),
        )
        .route(
            "/api/auth/device",
            get(handlers::device_flow::device_verify_page),
        )
        // WebSocket routes
        .route(
            "/ws/session",
            get(handlers::websocket::handle_session_websocket),
        )
        .route(
            "/ws/client",
            get(handlers::websocket::handle_web_client_websocket),
        )
        // Add single unified state
        .with_state(app_state.clone());

    // Serve frontend static files at root with SPA fallback
    if std::path::Path::new(&args.frontend_dist).exists() {
        tracing::info!("Serving frontend from: {}", args.frontend_dist);
        let index_path = format!("{}/index.html", args.frontend_dist);
        app = app.fallback_service(
            ServeDir::new(&args.frontend_dist).fallback(ServeFile::new(&index_path)),
        );
    } else {
        tracing::warn!("Frontend dist not found at: {}", args.frontend_dist);
    }

    // Add CORS and cookie management
    let app = app.layer(CookieManagerLayer::new()).layer(cors);

    // Run the server
    let addr = format!("{}:{}", host, port);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("Listening on {}", listener.local_addr()?);

    axum::serve(listener, app).await?;

    Ok(())
}
