mod backend;
mod version;
use axum::{Router, routing::get};
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tracing::{debug, error, info, trace, warn};
use version::get_version;

mod error_handling;
mod pages;
use error_handling::{error_method, error_not_found};
use pages::login::AppState;
use pages::{
    get_account_page, get_candidate_registration, get_debug, get_edit_election,
    get_election_candidates, get_election_changes, get_homepage, get_login, get_login_oauth,
    get_login_oauth_callback, get_login_oauth_complete, get_login_oauth_manual_check,
    get_login_oauth_status, get_login_reddit, get_manage_election_status, get_manage_elections,
    get_userinfo, login_threads, post_candidate_registration, post_debug, post_edit_election,
    post_election_status, post_manage_council_candidate, post_manage_elections,
    post_manage_presidential_ticket, post_withdraw_candidate,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv::dotenv().ok();

    let level = env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());

    let filter = tracing_subscriber::EnvFilter::new(format!(
        "{}={},off",
        env!("CARGO_PKG_NAME").replace('-', "_"),
        level,
    ));

    tracing_subscriber::fmt().with_env_filter(filter).init();

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

    let oauth_info = match get_oauth_info().await {
        Ok(info) => info,
        Err(e) => {
            error!("Failed to get OAuth info: {e}");
            return Err(e);
        }
    };

    let state = AppState {
        pool,
        pending_logins: Arc::new(Mutex::new(HashMap::new())),
        oauth_client_id: oauth_info.client_id,
        oauth_client_secret: oauth_info.client_secret,
        oauth_authorize_url: oauth_info.authorize_url,
        oauth_token_url: oauth_info.token_url,
        oauth_userinfo_url: oauth_info.userinfo_url,
        oauth_issuer: oauth_info.issuer,
        oauth_scope: oauth_info.scope,
        public_host: oauth_info.public_host,
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
        .route("/login/oauth", get(get_login_oauth))
        .route("/login/oauth/callback", get(get_login_oauth_callback))
        .route(
            "/login/oauth/status/{request_id}",
            get(get_login_oauth_status),
        )
        .route(
            "/login/oauth/manual-check/{request_id}",
            get(get_login_oauth_manual_check),
        )
        .route(
            "/login/oauth/complete/{request_id}",
            get(get_login_oauth_complete),
        )
        .route("/login/reddit", get(get_login_reddit))
        .route("/userinfo", get(get_userinfo))
        .route("/debug/{path}", get(get_debug).post(post_debug))
        .route("/account", get(get_account_page))
        .route(
            "/manage/elections",
            get(get_manage_elections).post(post_manage_elections),
        )
        .route(
            "/manage/elections/{election_uuid}/edit",
            get(get_edit_election).post(post_edit_election),
        )
        .route(
            "/manage/elections/{election_uuid}/status",
            get(get_manage_election_status).post(post_election_status),
        )
        .route(
            "/manage/elections/{election_uuid}/status/council/{candidate_uuid}",
            axum::routing::post(post_manage_council_candidate),
        )
        .route(
            "/manage/elections/{election_uuid}/status/tickets/{ticket_uuid}",
            axum::routing::post(post_manage_presidential_ticket),
        )
        .route(
            "/elections/{election_uuid}/register",
            get(get_candidate_registration).post(post_candidate_registration),
        )
        .route(
            "/elections/{election_uuid}/withdraw",
            axum::routing::post(post_withdraw_candidate),
        )
        .route(
            "/elections/{election_uuid}/candidates",
            get(get_election_candidates),
        )
        .route(
            "/elections/{election_uuid}/changes",
            get(get_election_changes),
        )
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

    trace!(
        "Database URL: {}",
        format!("postgresql://{db_user}:*****@{db_host}:{db_port}/{db_name}")
    );

    let database_url = format!("postgresql://{db_user}:{db_pass}@{db_host}:{db_port}/{db_name}");

    debug!("Connecting to database at {db_host}:{db_port}");

    Ok(database_url)
}

struct OAuthInfo {
    client_id: String,
    client_secret: String,
    authorize_url: String,
    token_url: String,
    userinfo_url: String,
    issuer: String,
    scope: String,
    public_host: String,
}

#[derive(Deserialize)]
struct OpenIdConfiguration {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    userinfo_endpoint: String,
}

async fn get_oauth_info() -> Result<OAuthInfo, Box<dyn std::error::Error>> {
    fn required(name: &str) -> Result<String, Box<dyn std::error::Error>> {
        env::var(name).map_err(|error| {
            error!("{name} is not set.");
            error.into()
        })
    }
    let issuer = required("OAUTH_ISSUER")?;
    let discovery_url = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    );
    let configuration: OpenIdConfiguration = reqwest::Client::new()
        .get(&discovery_url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if configuration.issuer != issuer {
        return Err(format!(
            "OIDC discovery issuer mismatch: expected {issuer}, got {}",
            configuration.issuer
        )
        .into());
    }

    let info = OAuthInfo {
        client_id: required("OAUTH_CLIENT_ID")?,
        client_secret: required("OAUTH_CLIENT_SECRET")?,
        authorize_url: configuration.authorization_endpoint,
        token_url: configuration.token_endpoint,
        userinfo_url: configuration.userinfo_endpoint,
        issuer,
        scope: env::var("OAUTH_SCOPE").unwrap_or_else(|_| "openid profile email".to_string()),
        public_host: required("PUBLIC_HOST")?,
    };
    debug!(
        "OAuth issuer: {}; public host: {}",
        info.issuer, info.public_host
    );
    Ok(info)
}
