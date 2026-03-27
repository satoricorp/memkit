use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::oneshot;

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("{}-{}-{}", prefix, std::process::id(), ts));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn config_file_path(xdg_config_home: &Path) -> PathBuf {
    xdg_config_home.join("memkit").join("memkit.json")
}

fn write_config(xdg_config_home: &Path, value: &Value) {
    let path = config_file_path(xdg_config_home);
    fs::create_dir_all(path.parent().expect("config dir")).expect("create config dir");
    fs::write(
        &path,
        serde_json::to_vec_pretty(value).expect("serialize config"),
    )
    .expect("write config");
}

fn read_config(xdg_config_home: &Path) -> Value {
    let path = config_file_path(xdg_config_home);
    let bytes = fs::read(path).expect("read config");
    serde_json::from_slice(&bytes).expect("parse config")
}

fn run_mk(args: &[&str], xdg_config_home: &Path, auth_base_url: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mk"));
    command.args(args);
    command.env("XDG_CONFIG_HOME", xdg_config_home);
    command.env("NO_COLOR", "1");
    match auth_base_url {
        Some(url) => {
            command.env("MEMKIT_AUTH_BASE_URL", url);
        }
        None => {
            command.env_remove("MEMKIT_AUTH_BASE_URL");
        }
    }
    command.output().expect("run mk")
}

fn stdout_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "mk failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("stdout json")
}

#[derive(Debug, Clone)]
struct StubState {
    refresh_status: u16,
    refresh_response: Value,
    logout_status: u16,
    logout_response: Value,
    refresh_calls: usize,
    logout_calls: usize,
    last_refresh_session_token: Option<String>,
    last_logout_session_token: Option<String>,
}

impl Default for StubState {
    fn default() -> Self {
        Self {
            refresh_status: 200,
            refresh_response: json!({}),
            logout_status: 200,
            logout_response: json!({ "success": true }),
            refresh_calls: 0,
            logout_calls: 0,
            last_refresh_session_token: None,
            last_logout_session_token: None,
        }
    }
}

#[derive(Deserialize)]
struct SessionTokenBody {
    #[serde(rename = "sessionToken")]
    session_token: String,
}

struct StubAuthServer {
    base_url: String,
    state: Arc<Mutex<StubState>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl StubAuthServer {
    fn start(initial_state: StubState) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind auth stub");
        let port = listener.local_addr().expect("local addr").port();
        listener.set_nonblocking(true).expect("nonblocking");

        let state = Arc::new(Mutex::new(initial_state));
        let app_state = Arc::clone(&state);
        let app = Router::new()
            .route("/api/auth/cli/refresh", post(refresh_handler))
            .route("/api/auth/cli/logout", post(logout_handler))
            .with_state(app_state);

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let handle = thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().expect("runtime");
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).expect("tokio listener");
                axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = shutdown_rx.await;
                    })
                    .await
                    .expect("serve auth stub");
            });
        });

        Self {
            base_url: format!("http://127.0.0.1:{}", port),
            state,
            shutdown_tx: Some(shutdown_tx),
            handle: Some(handle),
        }
    }

    fn snapshot(&self) -> StubState {
        self.state.lock().expect("stub state").clone()
    }
}

impl Drop for StubAuthServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

async fn refresh_handler(
    State(state): State<Arc<Mutex<StubState>>>,
    Json(body): Json<SessionTokenBody>,
) -> (StatusCode, Json<Value>) {
    let mut guard = state.lock().expect("refresh state");
    guard.refresh_calls += 1;
    guard.last_refresh_session_token = Some(body.session_token);
    (
        StatusCode::from_u16(guard.refresh_status).expect("refresh status"),
        Json(guard.refresh_response.clone()),
    )
}

async fn logout_handler(
    State(state): State<Arc<Mutex<StubState>>>,
    Json(body): Json<SessionTokenBody>,
) -> (StatusCode, Json<Value>) {
    let mut guard = state.lock().expect("logout state");
    guard.logout_calls += 1;
    guard.last_logout_session_token = Some(body.session_token);
    (
        StatusCode::from_u16(guard.logout_status).expect("logout status"),
        Json(guard.logout_response.clone()),
    )
}

#[test]
fn missing_config_does_not_create_auth_file() {
    let config_home = unique_temp_dir("memkit-auth-missing");

    let output = run_mk(&["whoami", "--output", "json"], &config_home, None);
    let body = stdout_json(&output);

    assert_eq!(
        body.get("authenticated").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        body.get("profile"),
        Some(&Value::Null),
        "missing config should report no profile"
    );
    assert!(
        !config_file_path(&config_home).exists(),
        "whoami should not create memkit.json when config is missing"
    );
}

#[test]
fn fresh_jwt_uses_cached_auth_without_refresh() {
    let config_home = unique_temp_dir("memkit-auth-fresh");
    let server = StubAuthServer::start(StubState {
        refresh_response: json!({
            "sessionToken": "should-not-be-used",
            "jwt": "should-not-be-used",
            "jwtExpiresAt": "2030-01-01T00:00:00Z",
            "profile": {
                "email": "refreshed@example.com"
            }
        }),
        ..StubState::default()
    });

    write_config(
        &config_home,
        &json!({
            "model": "openai:gpt-5.4",
            "auth": {
                "sessionToken": "durable-fresh",
                "jwt": "fresh-jwt",
                "jwtExpiresAt": "2030-01-01T00:00:00Z",
                "profile": {
                    "name": "Fresh User",
                    "email": "fresh@example.com"
                }
            }
        }),
    );

    let output = run_mk(
        &["whoami", "--output", "json"],
        &config_home,
        Some(&server.base_url),
    );
    let body = stdout_json(&output);

    assert_eq!(
        body.get("authenticated").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        body.get("profile")
            .and_then(|profile| profile.get("email"))
            .and_then(Value::as_str),
        Some("fresh@example.com")
    );

    let state = server.snapshot();
    assert_eq!(state.refresh_calls, 0, "fresh JWT should skip refresh");
}

#[test]
fn expired_jwt_refreshes_and_preserves_non_auth_config() {
    let config_home = unique_temp_dir("memkit-auth-refresh");
    let server = StubAuthServer::start(StubState {
        refresh_response: json!({
            "sessionToken": "durable-expired",
            "jwt": "refreshed-jwt",
            "jwtExpiresAt": "2030-02-01T00:00:00Z",
            "profile": {
                "name": "Refreshed User",
                "email": "refreshed@example.com"
            }
        }),
        ..StubState::default()
    });

    write_config(
        &config_home,
        &json!({
            "model": "openai:gpt-5.4",
            "auth": {
                "sessionToken": "durable-expired",
                "jwt": "expired-jwt",
                "jwtExpiresAt": "2024-01-01T00:00:00Z",
                "profile": {
                    "name": "Expired User",
                    "email": "expired@example.com"
                }
            }
        }),
    );

    let output = run_mk(
        &["whoami", "--output", "json"],
        &config_home,
        Some(&server.base_url),
    );
    let body = stdout_json(&output);

    assert_eq!(
        body.get("authenticated").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        body.get("profile")
            .and_then(|profile| profile.get("email"))
            .and_then(Value::as_str),
        Some("refreshed@example.com")
    );

    let state = server.snapshot();
    assert_eq!(state.refresh_calls, 1, "expired JWT should refresh once");
    assert_eq!(
        state.last_refresh_session_token.as_deref(),
        Some("durable-expired")
    );

    let updated = read_config(&config_home);
    assert_eq!(
        updated.get("model").and_then(Value::as_str),
        Some("openai:gpt-5.4"),
        "refresh should preserve non-auth config fields"
    );
    assert_eq!(
        updated
            .get("auth")
            .and_then(|auth| auth.get("jwt"))
            .and_then(Value::as_str),
        Some("refreshed-jwt")
    );
}

#[test]
fn logout_clears_local_state_even_if_remote_logout_fails() {
    let config_home = unique_temp_dir("memkit-auth-logout");
    let server = StubAuthServer::start(StubState {
        logout_status: 500,
        logout_response: json!({
            "error": {
                "code": "LOGOUT_FAILED",
                "message": "forced test failure"
            }
        }),
        ..StubState::default()
    });

    write_config(
        &config_home,
        &json!({
            "model": "openai:gpt-5.4",
            "auth": {
                "sessionToken": "durable-logout",
                "jwt": "jwt-logout",
                "jwtExpiresAt": "2030-01-01T00:00:00Z",
                "profile": {
                    "email": "logout@example.com"
                }
            }
        }),
    );

    let output = run_mk(
        &["logout", "--output", "json"],
        &config_home,
        Some(&server.base_url),
    );
    let body = stdout_json(&output);

    assert_eq!(
        body.get("authenticated").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(body.get("logged_out").and_then(Value::as_bool), Some(true));

    let state = server.snapshot();
    assert_eq!(state.logout_calls, 1, "logout should call remote endpoint");
    assert_eq!(
        state.last_logout_session_token.as_deref(),
        Some("durable-logout")
    );

    let updated = read_config(&config_home);
    assert_eq!(
        updated.get("model").and_then(Value::as_str),
        Some("openai:gpt-5.4"),
        "logout should preserve non-auth config fields"
    );
    assert!(
        updated.get("auth").is_none(),
        "logout should clear the local auth subtree even if remote revoke fails"
    );
}
