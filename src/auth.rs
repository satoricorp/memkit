use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use anyhow::{Context, Result, anyhow};
use axum::{
    Router,
    extract::{Query, State},
    response::{Html, IntoResponse},
    routing::get,
};
use chrono::{Duration, Utc};
use convex::{ConvexClient, FunctionResult, Value as ConvexValue};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Mutex, oneshot};
use url::Url;
use urlencoding::encode;

use crate::config::{self, AuthProfile, PersistedAuth};

const JWT_FRESH_SKEW_SECS: i64 = 30;
const CALLBACK_WAIT_TIMEOUT: StdDuration = StdDuration::from_secs(300);
const AUTH_HTTP_TIMEOUT: StdDuration = StdDuration::from_secs(30);
const CONVEX_ACTION_EXCHANGE_LOGIN: &str = "cli_auth:exchangeLoginGrant";
const CONVEX_ACTION_REFRESH_SESSION: &str = "cli_auth:refreshCliSession";
const CONVEX_ACTION_LOGOUT_SESSION: &str = "cli_auth:logoutCliSession";

#[derive(Debug, Clone)]
pub struct RuntimeAuthState {
    pub persisted: Option<PersistedAuth>,
    pub authenticated: bool,
    pub refresh_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WhoAmIResponse {
    pub authenticated: bool,
    pub profile: Option<crate::config::AuthProfile>,
    #[serde(rename = "jwtExpiresAt", skip_serializing_if = "Option::is_none")]
    pub jwt_expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_error: Option<String>,
}

#[derive(Debug)]
enum BackendAuthError {
    Unauthorized(String),
    Other(anyhow::Error),
}

#[derive(Debug)]
pub enum CloudSessionAuthError {
    Unauthorized(String),
    Misconfigured(String),
    Backend(String),
}

impl std::fmt::Display for CloudSessionAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthorized(msg) => write!(f, "{msg}"),
            Self::Misconfigured(msg) => write!(f, "{msg}"),
            Self::Backend(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for CloudSessionAuthError {}

impl std::fmt::Display for BackendAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthorized(msg) => write!(f, "{msg}"),
            Self::Other(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for BackendAuthError {}

#[derive(Debug, Deserialize)]
struct AuthCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

#[derive(Clone)]
struct CallbackServerState {
    expected_state: String,
    sender: Arc<Mutex<Option<oneshot::Sender<Result<String, String>>>>>,
}

#[derive(Debug, Serialize)]
struct ExchangeCodeRequest<'a> {
    code: &'a str,
}

#[derive(Debug, Serialize)]
struct SessionTokenRequest<'a> {
    #[serde(rename = "sessionToken")]
    session_token: &'a str,
}

#[derive(Debug, Deserialize)]
struct AuthActionEnvelope {
    ok: bool,
    #[serde(default)]
    auth: Option<PersistedAuth>,
    #[serde(default)]
    error: Option<AuthActionError>,
}

#[derive(Debug, Deserialize)]
struct LogoutActionEnvelope {
    ok: bool,
    #[serde(default)]
    error: Option<AuthActionError>,
}

#[derive(Debug, Deserialize)]
struct AuthActionError {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Clone)]
enum AuthBackendTarget {
    Convex(String),
    HttpStub(String),
}

pub fn auth_base_url() -> Option<String> {
    std::env::var("MEMKIT_AUTH_BASE_URL")
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
}

pub fn require_auth_base_url() -> Result<String> {
    auth_base_url().ok_or_else(|| {
        anyhow!(
            "MEMKIT_AUTH_BASE_URL is not set. Set it to the Convex auth origin before running `mk login`."
        )
    })
}

pub fn jwt_is_fresh(auth: &PersistedAuth) -> bool {
    match auth.jwt_expires_at_utc() {
        Some(exp) => exp > Utc::now() + Duration::seconds(JWT_FRESH_SKEW_SECS),
        None => false,
    }
}

pub async fn load_runtime_auth(allow_refresh: bool) -> Result<RuntimeAuthState> {
    let cfg = config::load_config()?;
    let Some(auth) = cfg.auth.clone() else {
        return Ok(RuntimeAuthState {
            persisted: None,
            authenticated: false,
            refresh_error: None,
        });
    };

    if jwt_is_fresh(&auth) {
        return Ok(RuntimeAuthState {
            persisted: Some(auth),
            authenticated: true,
            refresh_error: None,
        });
    }

    if !allow_refresh {
        return Ok(RuntimeAuthState {
            persisted: Some(auth),
            authenticated: false,
            refresh_error: None,
        });
    }

    let target = match resolve_auth_backend_target(None) {
        Ok(Some(target)) => target,
        Ok(None) => {
            return Ok(RuntimeAuthState {
                persisted: Some(auth),
                authenticated: false,
                refresh_error: None,
            });
        }
        Err(err) => {
            return Ok(RuntimeAuthState {
                persisted: Some(auth),
                authenticated: false,
                refresh_error: Some(err.to_string()),
            });
        }
    };

    match refresh_session(&target, &auth.session_token).await {
        Ok(refreshed) => {
            config::set_auth(Some(refreshed.clone()))?;
            Ok(RuntimeAuthState {
                persisted: Some(refreshed),
                authenticated: true,
                refresh_error: None,
            })
        }
        Err(BackendAuthError::Unauthorized(msg)) => {
            config::set_auth(None)?;
            Ok(RuntimeAuthState {
                persisted: None,
                authenticated: false,
                refresh_error: Some(msg),
            })
        }
        Err(BackendAuthError::Other(err)) => Ok(RuntimeAuthState {
            persisted: Some(auth),
            authenticated: false,
            refresh_error: Some(err.to_string()),
        }),
    }
}

pub fn whoami_response(state: &RuntimeAuthState) -> WhoAmIResponse {
    WhoAmIResponse {
        authenticated: state.authenticated,
        profile: state.persisted.as_ref().map(|auth| auth.profile.clone()),
        jwt_expires_at: state
            .persisted
            .as_ref()
            .map(|auth| auth.jwt_expires_at.clone()),
        refresh_error: state.refresh_error.clone(),
    }
}

pub async fn authenticate_cloud_session(
    session_token: &str,
) -> Result<AuthProfile, CloudSessionAuthError> {
    let token = session_token.trim();
    if token.is_empty() {
        return Err(CloudSessionAuthError::Unauthorized(
            "cloud session token is required".to_string(),
        ));
    }

    let target = resolve_auth_backend_target(None)
        .map_err(|err| CloudSessionAuthError::Misconfigured(err.to_string()))?
        .ok_or_else(|| {
            CloudSessionAuthError::Misconfigured(
                "cloud auth backend is not configured; set MEMKIT_AUTH_BASE_URL or MEMKIT_CONVEX_URL".to_string(),
            )
        })?;

    match refresh_session(&target, token).await {
        Ok(auth) => Ok(auth.profile),
        Err(BackendAuthError::Unauthorized(msg)) => Err(CloudSessionAuthError::Unauthorized(msg)),
        Err(BackendAuthError::Other(err)) => Err(CloudSessionAuthError::Backend(err.to_string())),
    }
}

pub async fn login(output_json: bool) -> Result<Value> {
    let base_url = require_auth_base_url()?;
    let callback_state = uuid::Uuid::new_v4().to_string();
    let (callback_url, callback_rx, shutdown_tx, server_handle) =
        spawn_callback_listener(callback_state.clone()).await?;
    let start_url = format!(
        "{}/api/auth/cli/start?callback={}&state={}",
        base_url,
        encode(&callback_url),
        encode(&callback_state)
    );
    let backend_target = resolve_auth_backend_target(Some(base_url.as_str()))?
        .ok_or_else(|| anyhow!("unable to resolve Convex auth backend URL"))?;

    let browser_opened = opener::open(&start_url).is_ok();
    if !browser_opened {
        eprintln!("Open this URL to continue signing in:\n{}", start_url);
    }

    let callback_result = tokio::time::timeout(CALLBACK_WAIT_TIMEOUT, callback_rx)
        .await
        .context("timed out waiting for browser sign-in")?
        .context("callback channel closed")?;
    let code = callback_result.map_err(anyhow::Error::msg)?;

    let _ = shutdown_tx.send(());
    let _ = server_handle.await;

    let auth = exchange_login(&backend_target, &code).await?;
    config::set_auth(Some(auth.clone()))?;

    if !browser_opened && !output_json {
        eprintln!("Sign-in completed.");
    }

    Ok(json!({
        "authenticated": true,
        "browser_opened": browser_opened,
        "profile": auth.profile,
        "jwtExpiresAt": auth.jwt_expires_at,
    }))
}

pub async fn logout() -> Result<Value> {
    let cfg = config::load_config()?;
    let had_auth = cfg.auth.is_some();

    if let Some(auth) = cfg.auth.as_ref() {
        if let Ok(Some(target)) = resolve_auth_backend_target(None) {
            let _ = logout_session(&target, &auth.session_token).await;
        }
    }

    if had_auth {
        config::set_auth(None)?;
    }

    Ok(json!({
        "authenticated": false,
        "logged_out": had_auth,
    }))
}

fn env_url(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
}

fn parse_url(raw: &str, label: &str) -> Result<Url> {
    Url::parse(raw).with_context(|| format!("{label} must be a valid URL"))
}

fn is_loopback_url(raw: &str) -> bool {
    Url::parse(raw)
        .ok()
        .and_then(|url| {
            url.host_str()
                .map(|host| host == "127.0.0.1" || host == "localhost")
        })
        .unwrap_or(false)
}

fn derive_convex_deployment_url(auth_base_url: &str) -> Result<String> {
    let mut url = parse_url(auth_base_url, "MEMKIT_AUTH_BASE_URL")?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("MEMKIT_AUTH_BASE_URL is missing a hostname"))?;
    let prefix = host
        .strip_suffix(".convex.site")
        .ok_or_else(|| {
            anyhow!(
                "MEMKIT_CONVEX_URL is not set and MEMKIT_AUTH_BASE_URL must point at a standard *.convex.site URL so the Convex deployment URL can be derived"
            )
        })?;
    let convex_host = format!("{prefix}.convex.cloud");
    url.set_host(Some(&convex_host))
        .map_err(|_| anyhow!("failed to derive Convex deployment URL from MEMKIT_AUTH_BASE_URL"))?;
    url.set_path("");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string().trim_end_matches('/').to_string())
}

fn resolve_auth_backend_target(
    auth_base_url_override: Option<&str>,
) -> Result<Option<AuthBackendTarget>> {
    if let Some(convex_url) = env_url("MEMKIT_CONVEX_URL") {
        parse_url(&convex_url, "MEMKIT_CONVEX_URL")?;
        return Ok(Some(if is_loopback_url(&convex_url) {
            AuthBackendTarget::HttpStub(convex_url)
        } else {
            AuthBackendTarget::Convex(convex_url)
        }));
    }

    let auth_base_url = auth_base_url_override
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .or_else(auth_base_url);
    let Some(auth_base_url) = auth_base_url else {
        return Ok(None);
    };

    parse_url(&auth_base_url, "MEMKIT_AUTH_BASE_URL")?;
    if is_loopback_url(&auth_base_url) {
        return Ok(Some(AuthBackendTarget::HttpStub(auth_base_url)));
    }
    Ok(Some(AuthBackendTarget::Convex(
        derive_convex_deployment_url(&auth_base_url)?,
    )))
}

fn auth_action_args(
    entries: impl IntoIterator<Item = (String, ConvexValue)>,
) -> BTreeMap<String, ConvexValue> {
    entries.into_iter().collect()
}

fn parse_convex_action_result(
    result: FunctionResult,
    action_name: &str,
) -> Result<Value, BackendAuthError> {
    match result {
        FunctionResult::Value(value) => Ok(value.export()),
        FunctionResult::ErrorMessage(message) => Err(BackendAuthError::Other(anyhow!(
            "Convex action `{action_name}` failed: {message}"
        ))),
        FunctionResult::ConvexError(error) => Err(BackendAuthError::Other(anyhow!(
            "Convex action `{action_name}` failed: {error}"
        ))),
    }
}

async fn call_convex_action(
    deployment_url: &str,
    action_name: &str,
    args: BTreeMap<String, ConvexValue>,
) -> Result<Value, BackendAuthError> {
    let mut client = ConvexClient::new(deployment_url).await.map_err(|err| {
        BackendAuthError::Other(
            anyhow!(err).context("failed to connect to Convex deployment for CLI auth"),
        )
    })?;
    let result = client.action(action_name, args).await.map_err(|err| {
        BackendAuthError::Other(
            anyhow!(err).context(format!("failed to call Convex action `{action_name}`")),
        )
    })?;
    parse_convex_action_result(result, action_name)
}

fn action_error_message(error: Option<AuthActionError>, default_msg: &str) -> String {
    error
        .and_then(|payload| payload.message)
        .filter(|message| !message.trim().is_empty())
        .unwrap_or_else(|| default_msg.to_string())
}

fn action_error_code(error: Option<&AuthActionError>) -> Option<String> {
    error.and_then(|payload| payload.code.clone())
}

fn parse_auth_action_envelope(
    payload: Value,
    action_name: &str,
) -> Result<AuthActionEnvelope, BackendAuthError> {
    serde_json::from_value(payload).map_err(|err| {
        BackendAuthError::Other(anyhow!(err).context(format!(
            "invalid payload returned from Convex action `{action_name}`"
        )))
    })
}

fn parse_logout_action_envelope(
    payload: Value,
    action_name: &str,
) -> Result<LogoutActionEnvelope, BackendAuthError> {
    serde_json::from_value(payload).map_err(|err| {
        BackendAuthError::Other(anyhow!(err).context(format!(
            "invalid payload returned from Convex action `{action_name}`"
        )))
    })
}

fn auth_from_action_envelope(
    envelope: AuthActionEnvelope,
    default_msg: &str,
) -> Result<PersistedAuth, BackendAuthError> {
    let AuthActionEnvelope { ok, auth, error } = envelope;
    if ok {
        return auth.ok_or_else(|| {
            BackendAuthError::Other(anyhow!(
                "auth backend succeeded but did not return an auth payload"
            ))
        });
    }

    let code = action_error_code(error.as_ref());
    let message = action_error_message(error, default_msg);
    if matches!(
        code.as_deref(),
        Some("INVALID_SESSION" | "INVALID_LOGIN_CODE")
    ) {
        return Err(BackendAuthError::Unauthorized(message));
    }
    Err(BackendAuthError::Other(anyhow!(message)))
}

async fn exchange_login_convex(
    deployment_url: &str,
    code: &str,
) -> Result<PersistedAuth, BackendAuthError> {
    let payload = call_convex_action(
        deployment_url,
        CONVEX_ACTION_EXCHANGE_LOGIN,
        auth_action_args([(String::from("code"), code.to_string().into())]),
    )
    .await?;
    let envelope = parse_auth_action_envelope(payload, CONVEX_ACTION_EXCHANGE_LOGIN)?;
    auth_from_action_envelope(envelope, "Login code is invalid or expired.")
}

async fn refresh_session_convex(
    deployment_url: &str,
    session_token: &str,
) -> Result<PersistedAuth, BackendAuthError> {
    let payload = call_convex_action(
        deployment_url,
        CONVEX_ACTION_REFRESH_SESSION,
        auth_action_args([(
            String::from("sessionToken"),
            session_token.to_string().into(),
        )]),
    )
    .await?;
    let envelope = parse_auth_action_envelope(payload, CONVEX_ACTION_REFRESH_SESSION)?;
    auth_from_action_envelope(envelope, "CLI session is no longer valid")
}

async fn logout_session_convex(
    deployment_url: &str,
    session_token: &str,
) -> std::result::Result<(), BackendAuthError> {
    let payload = call_convex_action(
        deployment_url,
        CONVEX_ACTION_LOGOUT_SESSION,
        auth_action_args([(
            String::from("sessionToken"),
            session_token.to_string().into(),
        )]),
    )
    .await?;
    let envelope = parse_logout_action_envelope(payload, CONVEX_ACTION_LOGOUT_SESSION)?;
    if envelope.ok {
        return Ok(());
    }

    let code = action_error_code(envelope.error.as_ref());
    if matches!(code.as_deref(), Some("INVALID_SESSION")) {
        return Ok(());
    }
    Err(BackendAuthError::Other(anyhow!(action_error_message(
        envelope.error,
        "logout failed",
    ))))
}

async fn exchange_login_http(base_url: &str, code: &str) -> Result<PersistedAuth> {
    let client = auth_http_client()?;
    let url = format!("{}/api/auth/cli/exchange", base_url);
    parse_http_auth_response(
        client
            .post(url)
            .json(&ExchangeCodeRequest { code })
            .send()
            .await
            .context("failed to exchange CLI login")?,
    )
    .await
    .map_err(to_anyhow)
}

async fn refresh_session_http(
    base_url: &str,
    session_token: &str,
) -> Result<PersistedAuth, BackendAuthError> {
    let client = auth_http_client().map_err(BackendAuthError::Other)?;
    let url = format!("{}/api/auth/cli/refresh", base_url);
    parse_http_auth_response(
        client
            .post(url)
            .json(&SessionTokenRequest { session_token })
            .send()
            .await
            .map_err(|e| {
                BackendAuthError::Other(anyhow!(e).context("failed to refresh CLI session"))
            })?,
    )
    .await
}

async fn logout_session_http(base_url: &str, session_token: &str) -> Result<()> {
    let client = auth_http_client()?;
    let url = format!("{}/api/auth/cli/logout", base_url);
    let response = client
        .post(url)
        .json(&SessionTokenRequest { session_token })
        .send()
        .await
        .context("failed to reach CLI logout endpoint")?;
    if response.status().is_success()
        || response.status() == StatusCode::UNAUTHORIZED
        || response.status() == StatusCode::NOT_FOUND
    {
        return Ok(());
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Err(anyhow!("logout failed with {}: {}", status, body))
}

async fn exchange_login(target: &AuthBackendTarget, code: &str) -> Result<PersistedAuth> {
    match target {
        AuthBackendTarget::Convex(deployment_url) => exchange_login_convex(deployment_url, code)
            .await
            .map_err(to_anyhow),
        AuthBackendTarget::HttpStub(base_url) => exchange_login_http(base_url, code).await,
    }
}

async fn refresh_session(
    target: &AuthBackendTarget,
    session_token: &str,
) -> Result<PersistedAuth, BackendAuthError> {
    match target {
        AuthBackendTarget::Convex(deployment_url) => {
            refresh_session_convex(deployment_url, session_token).await
        }
        AuthBackendTarget::HttpStub(base_url) => {
            refresh_session_http(base_url, session_token).await
        }
    }
}

async fn logout_session(target: &AuthBackendTarget, session_token: &str) -> Result<()> {
    match target {
        AuthBackendTarget::Convex(deployment_url) => {
            logout_session_convex(deployment_url, session_token)
                .await
                .map_err(to_anyhow)
        }
        AuthBackendTarget::HttpStub(base_url) => logout_session_http(base_url, session_token).await,
    }
}

async fn spawn_callback_listener(
    expected_state: String,
) -> Result<(
    String,
    oneshot::Receiver<Result<String, String>>,
    oneshot::Sender<()>,
    tokio::task::JoinHandle<std::io::Result<()>>,
)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .context("failed to bind local callback listener")?;
    let addr = listener.local_addr().context("callback local addr")?;
    let (result_tx, result_rx) = oneshot::channel();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    let state = CallbackServerState {
        expected_state,
        sender: Arc::new(Mutex::new(Some(result_tx))),
    };
    let app = Router::new()
        .route("/callback", get(callback_handler))
        .with_state(state);

    let handle = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    Ok((
        format!("http://{}/callback", addr),
        result_rx,
        shutdown_tx,
        handle,
    ))
}

async fn callback_handler(
    State(state): State<CallbackServerState>,
    Query(query): Query<AuthCallbackQuery>,
) -> impl IntoResponse {
    let result = match (&query.code, &query.state, &query.error) {
        (_, _, Some(err)) => Err(err.clone()),
        (Some(code), Some(actual_state), _) if actual_state == &state.expected_state => {
            Ok(code.clone())
        }
        (Some(_), Some(_), _) => Err("state mismatch in callback".to_string()),
        _ => Err("missing login code in callback".to_string()),
    };

    if let Some(sender) = state.sender.lock().await.take() {
        let _ = sender.send(result.clone());
    }

    match result {
        Ok(_) => Html("memkit login complete. You can close this window.").into_response(),
        Err(err) => Html(format!("memkit login failed: {}", err)).into_response(),
    }
}

fn auth_http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(StdDuration::from_secs(10))
        .timeout(AUTH_HTTP_TIMEOUT)
        .build()
        .context("failed to build auth HTTP client")
}

async fn parse_http_auth_response(
    response: reqwest::Response,
) -> Result<PersistedAuth, BackendAuthError> {
    let status = response.status();
    if status.is_success() {
        return response.json::<PersistedAuth>().await.map_err(|e| {
            BackendAuthError::Other(anyhow!(e).context("invalid auth response payload"))
        });
    }

    let body = response.text().await.unwrap_or_default();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(BackendAuthError::Unauthorized(non_empty_error_message(
            body,
            "CLI session is no longer valid",
        )));
    }

    Err(BackendAuthError::Other(anyhow!(
        "auth backend returned {}: {}",
        status,
        non_empty_error_message(body, "unknown auth error"),
    )))
}

fn non_empty_error_message(body: String, default_msg: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return default_msg.to_string();
    }
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        if let Some(msg) = v
            .get("error")
            .and_then(|err| err.get("message"))
            .and_then(Value::as_str)
        {
            return msg.to_string();
        }
    }
    trimmed.to_string()
}

fn to_anyhow(err: BackendAuthError) -> anyhow::Error {
    match err {
        BackendAuthError::Unauthorized(msg) => anyhow!(msg),
        BackendAuthError::Other(err) => err,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuthProfile, PersistedAuth};

    #[test]
    fn jwt_freshness_respects_future_and_past_expiry() {
        let fresh = PersistedAuth {
            session_token: "session".to_string(),
            jwt: "jwt".to_string(),
            jwt_expires_at: (Utc::now() + Duration::minutes(10)).to_rfc3339(),
            profile: AuthProfile::default(),
        };
        let expired = PersistedAuth {
            session_token: "session".to_string(),
            jwt: "jwt".to_string(),
            jwt_expires_at: (Utc::now() - Duration::minutes(10)).to_rfc3339(),
            profile: AuthProfile::default(),
        };

        assert!(jwt_is_fresh(&fresh));
        assert!(!jwt_is_fresh(&expired));
    }

    #[test]
    fn derive_convex_cloud_url_from_convex_site_url() {
        let derived =
            derive_convex_deployment_url("https://quiet-river-123.convex.site").expect("derive");
        assert_eq!(derived, "https://quiet-river-123.convex.cloud");
    }
}
