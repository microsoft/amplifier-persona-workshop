//! Disposable loopback listeners verify the native launcher's trust boundary.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use persona_workshop::{
    auth,
    config::{AuthMode, Config, WebTransport},
    desktop, server,
    state::AppState,
};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tower::ServiceExt;

fn installation(port: u16) -> (TempDir, Arc<AppState>, Router) {
    let home = tempfile::tempdir().unwrap();
    let config = Config {
        id: uuid::Uuid::new_v4().to_string(),
        owner: "owner".into(),
        bind: "127.0.0.1".into(),
        port,
        public_url: format!("http://127.0.0.1:{port}"),
        auth_mode: AuthMode::LocalPassword,
        web_transport: WebTransport::LocalHttp,
        pam_service: String::new(),
        tls_cert: PathBuf::new(),
        tls_key: PathBuf::new(),
        tls_ca: None,
        embed_origins: vec![],
        extractor: None,
        transcription: None,
        document_python: None,
        pdf_backend: Default::default(),
    };
    config.save(home.path()).unwrap();
    let state = AppState::open(home.path().into()).unwrap();
    let app = server::router(state.clone());
    (home, state, app)
}
async fn headers(socket: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    loop {
        let b = socket.read_u8().await.unwrap();
        bytes.push(b);
        assert!(bytes.len() <= 8192);
        if bytes.ends_with(b"\r\n\r\n") {
            return String::from_utf8(bytes).unwrap();
        }
    }
}
async fn reply(socket: &mut TcpStream, body: &str, close: bool) {
    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: {}\r\n\r\n{body}", body.len(), if close {"close"} else {"keep-alive"}).as_bytes()).await.unwrap();
}
async fn challenge(app: &Router, config: &Config, query: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/local-proof?{query}"))
                .header("host", config.public_url.trim_start_matches("http://"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

#[tokio::test]
async fn local_api_authenticates_actual_installation_before_management_request() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (home, state, app) = installation(listener.local_addr().unwrap().port());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let value = desktop::local_api(home.path(), "GET", "status", None)
        .await
        .unwrap();
    assert_eq!(value["id"], state.config.id);
    let records = desktop::local_api(home.path(), "GET", "projects", None)
        .await
        .unwrap();
    assert!(records.is_array() || records["items"].is_array());
    task.abort();
}

#[tokio::test]
async fn impostor_listener_never_receives_saved_control_credential() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (home, state, _) = installation(listener.local_addr().unwrap().port());
    let token = std::fs::read_to_string(home.path().join("secrets/control-token")).unwrap();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let captured = headers(&mut socket).await;
        // A copied public identity is not proof of installation ownership.
        reply(
            &mut socket,
            &json!({"id": state.config.id, "schema":1, "proof":"0".repeat(64)}).to_string(),
            true,
        )
        .await;
        captured
    });
    assert!(
        desktop::local_api(home.path(), "GET", "status", None)
            .await
            .is_err()
    );
    let captured = task.await.unwrap();
    assert!(captured.starts_with("GET /api/v1/local-proof?nonce="));
    assert!(!captured.to_ascii_lowercase().contains("authorization:"));
    assert!(!captured.contains(token.trim()));
}

#[tokio::test]
async fn authentic_proof_cannot_be_reused_after_connection_closes() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (home, state, app) = installation(listener.local_addr().unwrap().port());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let captured = headers(&mut socket).await;
        assert!(!captured.to_ascii_lowercase().contains("authorization:"));
        let query = captured
            .split_whitespace()
            .nth(1)
            .unwrap()
            .split_once('?')
            .unwrap()
            .1;
        let (status, proof) = challenge(&app, &state.config, query).await;
        assert_eq!(status, StatusCode::OK);
        reply(&mut socket, &proof.to_string(), true).await;
        socket.shutdown().await.unwrap();
        drop(socket);
        // Rebinding/reconnecting could send the credential to a new process.
        tokio::time::timeout(Duration::from_millis(200), listener.accept())
            .await
            .is_err()
    });
    assert!(
        desktop::local_api(home.path(), "GET", "status", None)
            .await
            .is_err()
    );
    assert!(task.await.unwrap());
}

#[tokio::test]
async fn local_proof_binds_nonce_installation_and_origin_without_granting_access() {
    let (home, state, app) = installation(49152);
    let token = std::fs::read_to_string(home.path().join("secrets/control-token")).unwrap();
    let nonce = auth::random_token();
    let (status, proof) = challenge(&app, &state.config, &format!("nonce={nonce}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(auth::verify_local_proof(
        &state.config,
        &nonce,
        token.trim(),
        &proof
    ));
    for (key, value) in [
        ("nonce", json!(auth::random_token())),
        ("id", json!(uuid::Uuid::new_v4())),
        ("origin", json!("http://127.0.0.1:49153")),
        ("proof", json!("0".repeat(64))),
        ("schema", json!(2)),
    ] {
        let mut altered = proof.clone();
        altered[key] = value;
        assert!(
            !auth::verify_local_proof(&state.config, &nonce, token.trim(), &altered),
            "{key}"
        );
    }
    for query in [
        "nonce=short".to_owned(),
        format!("nonce={}", "A".repeat(64)),
        format!("nonce={nonce}&extra=true"),
    ] {
        assert_eq!(
            challenge(&app, &state.config, &query).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/status")
                .header("host", "127.0.0.1:49152")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn excessive_proof_response_never_triggers_authenticated_request() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (home, _, _) = installation(listener.local_addr().unwrap().port());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let captured = headers(&mut socket).await;
        reply(&mut socket, &" ".repeat(5000), true).await;
        assert!(!captured.to_ascii_lowercase().contains("authorization:"));
    });
    assert!(
        desktop::local_api(home.path(), "GET", "status", None)
            .await
            .is_err()
    );
    task.await.unwrap();
}
