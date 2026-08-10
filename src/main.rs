mod backend;
mod version;
use axum::{
    Router,
    extract::Request,
    middleware::{self, Next},
    response::Response,
    routing::get,
    routing::post,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tracing::{debug, error, info, trace, warn};
use version::get_version;

mod error_handling;
mod pages;
mod render;
use error_handling::{error_method, error_not_found};
use pages::login::AppState;
use pages::{
    get_about, get_account_page, get_candidate_registration, get_debug, get_edit_election,
    get_election_candidates, get_election_changes, get_elections, get_homepage,
    get_list_themes_page, get_login, get_login_oauth, get_login_oauth_callback,
    get_login_oauth_complete, get_login_oauth_device, get_login_oauth_manual_check,
    get_login_oauth_status, get_logout, get_manage_election_status, get_manage_elections,
        get_settings, get_userinfo, login_threads, post_account_role, post_account_theme, post_candidate_registration,
    post_debug, post_delete_account_session, post_delete_all_account_sessions, post_edit_election,
    post_election_status, post_manage_council_candidate, post_manage_elections,
    post_manage_presidential_ticket, post_settings, post_withdraw_candidate, get_contact, get_staging
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dotenv_loaded = dotenv::dotenv().is_ok();

    let level = env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());

    let filter = tracing_subscriber::EnvFilter::new(format!(
        "{}={},off",
        env!("CARGO_PKG_NAME").replace('-', "_"),
        level,
    ));

    tracing_subscriber::fmt().with_env_filter(filter).init();

    trace!(%level, dotenv_loaded, "Logging system initialised");

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
    debug!(%addr, "Resolved webserver bind address");

    let database_url = get_database_url()?;

    trace!("Opening database connection pool");
    let pool = match sqlx::PgPool::connect(&database_url).await {
        Ok(pool) => {
            info!("Connected to database");
            pool
        }
        Err(e) => {
            error!("Failed to connect to database: {e}");
            return Err(e.into());
        }
    };

    debug!("Applying database migrations...");

    sqlx::migrate!("./migrations").run(&pool).await?;
    info!("Database migrations are current");

    debug!("Starting webserver server on {}", addr);

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => {
            info!(%addr, "Webserver listener ready");
            listener
        }
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

    let app_mode = match env::var("APP_MODE") {
        Ok(mode) => match mode.parse::<u8>() {
            Ok(num) if num <= 2 => num,
            _ => {
                warn!("APP_MODE is set to an invalid value, defaulting to 2 (production)");
                warn!("It is recommended to set APP_MODE for a more predictable behavior.");
                warn!("Valid values for APP_MODE are: 0 = development, 1 = staging, 2 = production");
                2
            }
        },
        Err(_) => {
            warn!("APP_MODE is not set, defaulting to 2 (production)");
            warn!("It is recommended to set APP_MODE for a more predictable behavior.");
            warn!("Valid values for APP_MODE are: 0 = development, 1 = staging, 2 = production");
            2
        }
    };

    let state = AppState {
        pool,
        pending_logins: Arc::new(Mutex::new(HashMap::new())),
        oauth_client_id: oauth_info.client_id,
        oauth_client_secret: oauth_info.client_secret,
        oauth_authorize_url: oauth_info.authorize_url,
        oauth_device_authorization_url: oauth_info.device_authorization_url,
        oauth_token_url: oauth_info.token_url,
        oauth_userinfo_url: oauth_info.userinfo_url,
        oauth_issuer: oauth_info.issuer,
        oauth_scope: oauth_info.scope,
        public_host: oauth_info.public_host,
        http_client: reqwest::Client::new(),
        app_mode
    };
    debug!(
        device_login_configured = state.oauth_device_authorization_url.is_some(),
        "Application state initialised"
    );

    let router = build_router(state.clone()).await;
    debug!("Application router built");

    trace!("Starting background tasks...");
    tokio::spawn(login_threads(state));

    info!(%addr, "Webserver accepting requests");
    if let Err(error) = axum::serve(listener, router).await {
        error!(?error, "Webserver stopped with an error");
        return Err(error.into());
    }

    Ok(())
}

async fn build_router(state: AppState) -> Router {
    trace!("Building application router");
    let mut router = Router::new()
        .route("/", get(get_homepage))
        .route("/about", get(get_about))
        .route("/elections", get(get_elections))
        .route("/login", get(get_login))
        .route("/login/oauth", get(get_login_oauth))
        .route("/login/oauth/device", get(get_login_oauth_device))
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
        .route("/logout", post(get_logout))
        .route("/userinfo", get(get_userinfo))
        .route("/account", get(get_account_page))
        .route("/account/set-theme", post(post_account_theme))
            .route("/account/set-role", post(post_account_role))
        .route(
            "/account/sessions/{session_uuid}/delete",
            post(post_delete_account_session),
        )
        .route(
            "/account/sessions/delete-all",
            post(post_delete_all_account_sessions),
        )
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
            post(post_manage_council_candidate),
        )
        .route(
            "/manage/elections/{election_uuid}/status/tickets/{ticket_uuid}",
            post(post_manage_presidential_ticket),
        )
        .route(
            "/elections/{election_uuid}/register",
            get(get_candidate_registration).post(post_candidate_registration),
        )
        .route(
            "/elections/{election_uuid}/withdraw",
            post(post_withdraw_candidate),
        )
        .route(
            "/elections/{election_uuid}/candidates",
            get(get_election_candidates),
        )
        .route(
            "/elections/{election_uuid}/changes",
            get(get_election_changes),
        )
        .route("/contact", get(get_contact))
        .route("/themes", get(get_list_themes_page))
        .route("/settings", get(get_settings).post(post_settings))
        .route("/staging", get(get_staging))
        .fallback(error_not_found)
        .layer(middleware::from_fn(log_request));

    if state.app_mode == 0 {
        trace!("Adding debug routes for development mode");
        router = router.route("/debug/{path}", get(get_debug).post(post_debug))
    }

    router.with_state(state)
}

async fn log_request(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let started_at = Instant::now();
    debug!(%method, %path, "Request started");
    let response = next.run(request).await;
    let status = response.status();
    info!(%method, %path, %status, elapsed_ms = started_at.elapsed().as_millis(), "Request completed");
    response
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
    device_authorization_url: Option<String>,
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
    device_authorization_endpoint: Option<String>,
    token_endpoint: String,
    userinfo_endpoint: String,
}

async fn get_oauth_info() -> Result<OAuthInfo, Box<dyn std::error::Error>> {
    trace!("Loading OAuth configuration");
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
    debug!("Requesting OpenID Connect discovery document");
    let configuration: OpenIdConfiguration = reqwest::Client::new()
        .get(&discovery_url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    debug!(
        device_authorization_discovered = configuration.device_authorization_endpoint.is_some(),
        "Loaded OpenID Connect discovery document"
    );
    if configuration.issuer != issuer {
        error!(expected_issuer = %issuer, discovered_issuer = %configuration.issuer, "OpenID Connect issuer mismatch");
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
        device_authorization_url: env::var("OAUTH_DEVICE_AUTHORIZATION_URL")
            .ok()
            .or(configuration.device_authorization_endpoint),
        token_url: configuration.token_endpoint,
        userinfo_url: configuration.userinfo_endpoint,
        issuer,
        scope: env::var("OAUTH_SCOPE").unwrap_or_else(|_| "openid profile email".to_string()),
        public_host: required("PUBLIC_HOST")?,
    };
    debug!(
        issuer = %info.issuer,
        public_host = %info.public_host,
        device_login_configured = info.device_authorization_url.is_some(),
        "OAuth configuration loaded"
    );
    Ok(info)
}
