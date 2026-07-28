mod version;
use std::env;
use std::net::SocketAddr;
use tracing::{info, error, warn, trace, debug};
use version::get_version;
use axum::{
    routing::get,
    Router,
};
use std::sync::{Arc, Mutex};
use std::collections::HashMap;



mod pages;
mod error_handling;
use pages::login::AppState;
use pages::{get_homepage, get_debug, post_debug, get_login, get_login_discord, get_login_discord_callback, get_login_discord_complete, get_login_discord_manual_check, get_login_discord_status, get_login_reddit, get_userinfo, login_threads};
use error_handling::{error_method, error_not_found};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv::dotenv().ok();

    let level = env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());

    let filter = tracing_subscriber::EnvFilter::new(format!(
        "{}={},off",
        env!("CARGO_PKG_NAME").replace('-', "_"),
        level,
    ));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
    .init();

    trace!("Logging system initialised");

    info!("Starting tg-voting server version {}", get_version());

    trace!("Reading BIND_HOST from environment...");

    let bind_host = env::var("BIND_HOST").unwrap_or_else(|_| {
        warn!("BIND_HOST is not set, defaulting to 0.0.0.0");
        warn!("It is recommended to set a more restrictive bind address.");
        "0.0.0.0".into()
    });

    trace!("Reading BIND_PORT from environment...");

    let bind_port = env::var("BIND_PORT").unwrap_or_else(|_| {
        warn!("BIND_PORT is not set, defaulting to 3000");
        "3000".into()
    });

    let addr: SocketAddr = match format!("{bind_host}:{bind_port}").parse() {
        Ok(addr) => addr,
        Err(e) => {
            error!("Invalid bind address '{bind_host}:{bind_port}': {e}");
            return Err(e.into());
        }
    };

    let database_url = get_database_url()?;

    let pool = match sqlx::PgPool::connect(&database_url).await {
        Ok(pool) => pool,
        Err(e) => {
            error!("Failed to connect to database: {e}");
            return Err(e.into());
        }
    };

    debug!("Applying database migrations...");

    sqlx::migrate!("./migrations").run(&pool).await?;

    debug!("Starting webserver server on {}", addr);

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(e) => {
            error!("Failed to bind to {addr}: {e}");
            return Err(e.into());
        }
    };

    let (discord_id, discord_secret, public_host) = match get_oauth_info() {
        Ok(info) => info,
        Err(e) => {
            error!("Failed to get OAuth info: {e}");
            return Err(e);
        }
    };

    let state = AppState {
        pool,
        pending_logins: Arc::new(Mutex::new(HashMap::new())),
        discord_id,
        discord_secret,
        public_host,
        http_client: reqwest::Client::new(),
    };

    let router = build_router(state.clone()).await;

    trace!("Starting background tasks...");
    tokio::spawn(login_threads(state));

    axum::serve(listener, router).await.unwrap();

    Ok(())
}

async fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(get_homepage))
        .route("/login", get(get_login))
        .route("/login/discord", get(get_login_discord))
        .route("/login/discord/callback", get(get_login_discord_callback))
        .route("/login/discord/status/{request_id}", get(get_login_discord_status))
        .route("/login/discord/manual-check/{request_id}", get(get_login_discord_manual_check))
        .route("/login/discord/complete/{request_id}", get(get_login_discord_complete))
        .route("/login/reddit", get(get_login_reddit))
        .route("/userinfo", get(get_userinfo))
        .route("/debug/{path}", get(get_debug).post(post_debug))
        .fallback(error_not_found)
        .with_state(state)
}

fn get_database_url() -> Result<String, Box<dyn std::error::Error>> {
    trace!("Reading database credentials...");

        let db_user = match env::var("DATABASE_USER") {
        Ok(user) => user,
        Err(e) => {
            error!("DATABASE_USER is not set.");
            return Err(e.into());
        }
    };

    let db_pass = match env::var("DATABASE_PASSWORD") {
        Ok(pass) => pass,
        Err(e) => {
            error!("DATABASE_PASSWORD is not set.");
            return Err(e.into());
        }
    };

    let db_host = match env::var("DATABASE_HOST") {
        Ok(host) => host,
        Err(e) => {
            error!("DATABASE_HOST is not set.");
            return Err(e.into());
        }
    };

    let db_port = match env::var("DATABASE_PORT") {
        Ok(port) => port,
        Err(e) => {
            error!("DATABASE_PORT is not set.");
            return Err(e.into());
        }
    };

    let db_name = match env::var("DATABASE_NAME") {
        Ok(name) => name,
        Err(e) => {
            error!("DATABASE_NAME is not set.");
            return Err(e.into());
        }
    };

    trace!("Database URL: {}", format!("postgresql://{db_user}:*****@{db_host}:{db_port}/{db_name}"));

    let database_url = format!("postgresql://{db_user}:{db_pass}@{db_host}:{db_port}/{db_name}");

    debug!("Connecting to database at {db_host}:{db_port}");


    Ok(database_url)
}

fn get_oauth_info() -> Result<(String, String, String), Box<dyn std::error::Error>> {

    let discord_id = match env::var("DISCORD_CLIENT_ID") {
        Ok(id) => id,
        Err(e) => {
            error!("DISCORD_CLIENT_ID is not set.");
            return Err(e.into());
        }
    };

    debug!("Discord Client ID: {}", discord_id);

    let discord_secret = match env::var("DISCORD_CLIENT_SECRET") {
        Ok(secret) => secret,
        Err(e) => {
            error!("DISCORD_CLIENT_SECRET is not set.");
            return Err(e.into());
        }
    };

    trace!("Discord Client secret read.");

    let public_host = match env::var("PUBLIC_HOST") {
        Ok(host) => host,
        Err(e) => {
            error!("PUBLIC_HOST is not set.");
            return Err(e.into());
        }
    };

    debug!("Public host: {public_host}");

    Ok((discord_id, discord_secret, public_host))
}
