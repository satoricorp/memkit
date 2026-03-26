use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::blocking::Client;
use serde_json::{Value, json};

fn expected_git_version() -> String {
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("git rev-parse");
    assert!(
        output.status.success(),
        "git rev-parse failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git rev-parse utf8")
        .trim()
        .to_string()
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_millis();
    let dir = std::env::temp_dir().join(format!("{}-{}", prefix, ts));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn create_pack_fixture(root: &Path) {
    let pack_dir = root.join(".memkit");
    fs::create_dir_all(pack_dir.join("state")).expect("create pack state dir");
    let manifest = json!({
        "format_version": "1.0.0",
        "pack_id": "test-pack",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z",
        "embedding": {
            "provider": "hash",
            "model": "test",
            "dimension": 384
        },
        "chunking": {
            "strategy": "char_window",
            "target_chars": 1200,
            "overlap_chars": 200
        },
        "sources": []
    });
    fs::write(
        pack_dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).expect("serialize manifest"),
    )
    .expect("write manifest");
    fs::write(pack_dir.join("state/file_state.json"), b"[]").expect("write state");
}

fn pick_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);
    port
}

fn start_server(pack_root: &Path, port: u16) -> Child {
    Command::new(env!("CARGO_BIN_EXE_mk"))
        .args([
            "start",
            "--pack",
            &pack_root.to_string_lossy(),
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--foreground",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start mk start")
}

fn wait_for_health(base_url: &str) {
    let client = Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .expect("build client");
    for _ in 0..30 {
        let ok = client
            .get(format!("{}/health", base_url))
            .send()
            .map(|r| r.status().is_success())
            .unwrap_or(false);
        if ok {
            return;
        }
        thread::sleep(Duration::from_millis(300));
    }
    panic!("server did not become healthy");
}

fn stop_server(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn smoke_health_status_add_query_flows() {
    let pack_root = unique_temp_dir("memkit-smoke-pack");
    create_pack_fixture(&pack_root);
    let expected_version = expected_git_version();

    let docs_dir = pack_root.join("docs");
    fs::create_dir_all(&docs_dir).expect("create docs dir");
    fs::write(docs_dir.join("note.md"), "# hello\nsmoke").expect("write note");

    let port = pick_port();
    let base_url = format!("http://127.0.0.1:{}", port);
    let mut child = start_server(&pack_root, port);
    wait_for_health(&base_url);

    let client = Client::new();

    let health: Value = client
        .get(format!("{}/health", base_url))
        .send()
        .expect("health response")
        .json()
        .expect("health json");
    assert_eq!(health.get("status").and_then(Value::as_str), Some("ok"));
    assert_eq!(
        health.get("version").and_then(Value::as_str),
        Some(expected_version.as_str())
    );

    let initialize: Value = client
        .post(format!("{}/mcp", base_url))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        }))
        .send()
        .expect("mcp initialize response")
        .json()
        .expect("mcp initialize json");
    assert_eq!(initialize.get("jsonrpc").and_then(Value::as_str), Some("2.0"));
    assert_eq!(
        initialize
            .get("result")
            .and_then(|result| result.get("serverInfo"))
            .and_then(|server_info| server_info.get("version"))
            .and_then(Value::as_str),
        Some(expected_version.as_str())
    );

    let status: Value = client
        .get(format!("{}/status", base_url))
        .send()
        .expect("status response")
        .json()
        .expect("status json");
    assert_eq!(status.get("status").and_then(Value::as_str), Some("ok"));
    assert!(status.get("pack_path").is_some());

    // Add directory mode should enqueue an indexing job.
    let add_resp = client
        .post(format!("{}/add", base_url))
        .json(&json!({ "path": docs_dir.to_string_lossy().to_string() }))
        .send()
        .expect("add response");
    assert!(add_resp.status().is_success());
    let add_body: Value = add_resp.json().expect("add json");
    assert_eq!(add_body.get("status").and_then(Value::as_str), Some("accepted"));
    assert!(add_body.get("job").is_some());

    // Query endpoint should return either success payload or structured error payload.
    let query_resp = client
        .post(format!("{}/query", base_url))
        .json(&json!({ "query": "hello", "raw": true }))
        .send()
        .expect("query response");
    if query_resp.status().is_success() {
        let body: Value = query_resp.json().expect("query json");
        assert!(body.get("results").is_some() || body.get("retrieval_results").is_some());
    } else {
        let body: Value = query_resp.json().expect("query error json");
        let err = body.get("error").expect("error envelope");
        assert!(err.get("code").is_some());
        assert!(err.get("message").is_some());
    }

    stop_server(&mut child);
    let _ = fs::remove_dir_all(&pack_root);
}

#[test]
fn api_error_contract_shape_for_bad_publish_destination() {
    let pack_root = unique_temp_dir("memkit-error-pack");
    create_pack_fixture(&pack_root);

    let port = pick_port();
    let base_url = format!("http://127.0.0.1:{}", port);
    let mut child = start_server(&pack_root, port);
    wait_for_health(&base_url);

    let client = Client::new();
    let resp = client
        .post(format!("{}/publish", base_url))
        .json(&json!({
            "path": pack_root.to_string_lossy().to_string(),
            "destination": "not-s3"
        }))
        .send()
        .expect("publish response");
    assert_eq!(resp.status().as_u16(), 400);
    let body: Value = resp.json().expect("publish error json");
    let err = body.get("error").expect("error object");
    assert!(err.get("code").and_then(Value::as_str).is_some());
    assert!(err.get("message").and_then(Value::as_str).is_some());

    stop_server(&mut child);
    let _ = fs::remove_dir_all(&pack_root);
}
