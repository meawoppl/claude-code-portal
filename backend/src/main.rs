mod db;
mod handlers;
mod models;
mod schema;

use axum::{
    routing::{get, post},
    Router,
};
use clap::Parser;
use crate::db::DbPool;
use crate::handlers::device_flow::DeviceFlowStore;
use oauth2::{basic::BasicClient, AuthUrl, ClientId, ClientSecret, RedirectUrl, TokenUrl};
use std::{env, sync::Arc};
use tower_http::{
    cors::{Any, CorsLayer},
    services::ServeDir,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use handlers::{
    websocket::SessionManager,
};

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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse CLI arguments
    let args = Args::parse();

    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "backend=debug,tower_http=debug".into()),
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
        let client_id = ClientId::new(env::var("GOOGLE_CLIENT_ID").expect("GOOGLE_CLIENT_ID must be set"));
        let client_secret = ClientSecret::new(env::var("GOOGLE_CLIENT_SECRET").expect("GOOGLE_CLIENT_SECRET must be set"));
        let auth_url = AuthUrl::new("https://accounts.google.com/o/oauth2/v2/auth".to_string())?;
        let token_url = TokenUrl::new("https://oauth2.googleapis.com/token".to_string())?;
        let redirect_uri = RedirectUrl::new(env::var("GOOGLE_REDIRECT_URI").expect("GOOGLE_REDIRECT_URI must be set"))?;

        Some(BasicClient::new(
            client_id,
            Some(client_secret),
            auth_url,
            Some(token_url),
        ).set_redirect_uri(redirect_uri))
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

    // Create app state
    let app_state = Arc::new(AppState {
        dev_mode: args.dev_mode,
        db_pool: pool.clone(),
        session_manager: session_manager.clone(),
        oauth_basic_client,
        device_flow_store: if args.dev_mode { None } else { Some(device_flow_store.clone()) },
    });

    // Setup CORS
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // Build our application with routes
    let mut app = Router::new()
        // Health check / root
        .route("/", get(|| async { "Claude Code Proxy Backend" }))

        // Session API routes
        .route("/api/sessions", get(handlers::sessions::list_sessions))
        .route("/api/sessions/:id", get(handlers::sessions::get_session))
        .route("/api/sessions/:id/messages", post(handlers::sessions::send_message))

        // WebSocket routes
        .route("/ws/session", get(handlers::websocket::handle_session_websocket))
        .route("/ws/client", get(handlers::websocket::handle_web_client_websocket))

        // Auth routes
        .route("/auth/google", get(handlers::auth::login))
        .route("/auth/google/callback", get(handlers::auth::callback))
        .route("/auth/me", get(handlers::auth::me))

        // Device flow endpoints for CLI
        .route("/auth/device/code", post(handlers::device_flow::device_code))
        .route("/auth/device/poll", post(handlers::device_flow::device_poll))
        .route("/auth/device", get(handlers::device_flow::device_verify_page))

        // Dev mode routes
        .route("/auth/dev-login", get(handlers::auth::dev_login))

        // Add single unified state
        .with_state(app_state.clone());

    // Serve frontend static files if path exists
    if std::path::Path::new(&args.frontend_dist).exists() {
        tracing::info!("Serving frontend from: {}", args.frontend_dist);
        app = app.nest_service(
            "/app",
            ServeDir::new(&args.frontend_dist)
        );
    } else {
        tracing::warn!("Frontend dist not found at: {}", args.frontend_dist);
    }

    // Add CORS
    let app = app.layer(cors);

    // Run the server
    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let addr = format!("{}:{}", host, port);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("Listening on {}", listener.local_addr()?);

    axum::serve(listener, app).await?;

    Ok(())
}
