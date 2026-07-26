mod version;
use std::env;
use std::net::SocketAddr;
use tracing::{info, error, warn, trace, debug};
use version::get_version;
use axum::{
    routing::get,
    Router
};
use sqlx::postgres::PgPool;

mod pages;
mod error_handling;
use pages::{get_homepage, get_debug, post_debug};
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

    let router = build_router(&pool).await;

    axum::serve(listener, router).await.unwrap();

    Ok(())
}

async fn build_router(pool: &PgPool) -> Router {
    Router::new()
        .route("/", get(get_homepage))
        .route("/debug/{path}", get(get_debug).post(post_debug))
        .fallback(error_not_found)
        .with_state(pool.clone())
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