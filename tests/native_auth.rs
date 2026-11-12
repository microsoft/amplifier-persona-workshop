//! Native local setup uses installation credentials, never OS impersonation.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode},
};
use persona_workshop::{
    auth,
    config::{AuthMode, Config, WebTransport},
    server,
    state::AppState,
};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};
use tempfile::TempDir;
use tower::ServiceExt;

const PASSWORD: &str = "one installation password";
const LOCAL_ORIGIN: &str = "http://127.0.0.1:49152";
fn configuration(local: bool) -> Config {
    Config {
        id: uuid::Uuid::new_v4().to_string(),
        owner: "owner".into(),
        bind: "127.0.0.1".into(),
        port: 49152,
        public_url: if local {
            LOCAL_ORIGIN
        } else {
            "https://persona.invalid"
        }
        .into(),
        auth_mode: AuthMode::LocalPassword,
        web_transport: if local {
            WebTransport::LocalHttp
        } else {
            WebTransport::Https
        },
        pam_service: String::new(),
        tls_cert: PathBuf::new(),
        tls_key: PathBuf::new(),
        tls_ca: None,
        embed_origins: vec![],
        extractor: None,
        transcription: None,
        document_python: None,
        pdf_backend: Default::default(),
    }
}
fn fixture(local: bool) -> (TempDir, Arc<AppState>, Router) {
    let home = tempfile::tempdir().unwrap();
    auth::initialize_local_password(home.path(), PASSWORD).unwrap();
    configuration(local).save(home.path()).unwrap();
    let state = AppState::open(home.path().into()).unwrap();
    let app = server::router(state.clone());
    (home, state, app)
}
async fn request(
    app: &Router,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Value,
) -> (StatusCode, HeaderMap, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(if body.is_null() {
                    Body::empty()
                } else {
                    Body::from(body.to_string())
                })
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), 65536).await.unwrap();
    (status, headers, serde_json::from_slice(&body).unwrap())
}
async fn login(app: &Router, password: &str, local: bool) -> (StatusCode, HeaderMap, Value) {
    let host = if local {
        "127.0.0.1:49152"
    } else {
        "persona.invalid"
    };
    let origin = if local {
        LOCAL_ORIGIN
    } else {
        "https://persona.invalid"
    };
    request(
        app,
        "POST",
        "/auth/login",
        &[("host", host), ("origin", origin)],
        json!({"username":"owner","password":password}),
    )
    .await
}

#[tokio::test]
async fn local_installation_proof_is_not_available_on_https_deployments() {
    let (_home, _state, app) = fixture(false);
    let (status, _, _) = request(
        &app,
        "GET",
        &format!("/api/v1/local-proof?nonce={}", auth::random_token()),
        &[("host", "persona.invalid")],
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[test]
fn old_configuration_defaults_to_pam_and_https_and_local_mode_cannot_expose_lan() {
    let mut old = serde_json::to_value(configuration(false)).unwrap();
    old.as_object_mut().unwrap().remove("auth_mode");
    old.as_object_mut().unwrap().remove("web_transport");
    old["pam_service"] = json!("original-service");
    let parsed: Config = serde_json::from_value(old).unwrap();
    assert_eq!(parsed.auth_mode, AuthMode::Pam);
    assert_eq!(parsed.web_transport, WebTransport::Https);
    assert_eq!(parsed.pam_service, "original-service");
    assert!(parsed.validate().is_ok());
    let good = configuration(true);
    assert!(good.validate().is_ok());
    for (field, value) in [
        ("bind", json!("0.0.0.0")),
        ("bind", json!("192.168.1.2")),
        ("public_url", json!("http://remote.invalid:49152")),
        ("public_url", json!("http://127.0.0.1:49153")),
        ("public_url", json!("http://127.0.0.1:49152/other")),
        ("auth_mode", json!("pam")),
        ("embed_origins", json!(["https://external.invalid"])),
    ] {
        let mut invalid = serde_json::to_value(&good).unwrap();
        invalid[field] = value;
        assert!(
            serde_json::from_value::<Config>(invalid)
                .unwrap()
                .validate()
                .is_err(),
            "{field}"
        );
    }
}

#[test]
fn local_password_is_salted_private_and_create_only() {
    let home = tempfile::tempdir().unwrap();
    assert!(auth::initialize_local_password(home.path(), "short").is_err());
    auth::initialize_local_password(home.path(), PASSWORD).unwrap();
    let path = home.path().join("secrets/local-password.json");
    let first = std::fs::read(&path).unwrap();
    let saved: Value = serde_json::from_slice(&first).unwrap();
    assert!(
        saved["password_hash"]
            .as_str()
            .unwrap()
            .starts_with("$argon2id$v=19$m=19456,t=2,p=1$")
    );
    assert!(!String::from_utf8_lossy(&first).contains(PASSWORD));
    assert!(auth::initialize_local_password(home.path(), "replacement password").is_err());
    assert_eq!(first, std::fs::read(&path).unwrap());
    let other = tempfile::tempdir().unwrap();
    auth::initialize_local_password(other.path(), PASSWORD).unwrap();
    assert_ne!(
        first,
        std::fs::read(other.path().join("secrets/local-password.json")).unwrap()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[tokio::test]
async fn native_login_preserves_cookie_csrf_scopes_and_logout() {
    let (_home, state, app) = fixture(true);
    assert_eq!(
        login(&app, "wrong password", true).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, headers, body) = login(&app, PASSWORD, true).await;
    assert_eq!(status, StatusCode::OK);
    let set_cookie = headers["set-cookie"].to_str().unwrap();
    assert!(set_cookie.contains("HttpOnly; SameSite=Strict"));
    assert!(!set_cookie.contains("Secure"));
    let cookie = set_cookie.split(';').next().unwrap();
    let csrf = body["csrf"].as_str().unwrap();
    let base = [
        ("host", "127.0.0.1:49152"),
        ("origin", LOCAL_ORIGIN),
        ("cookie", cookie),
    ];
    assert_eq!(
        request(&app, "GET", "/api/v1/projects", &base, Value::Null)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/tokens",
            &base,
            json!({"name":"forbidden without csrf","scopes":["admin"]})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let mut full = base.to_vec();
    full.push(("x-csrf-token", csrf));
    let (_, _, created) = request(
        &app,
        "POST",
        "/api/v1/tokens",
        &full,
        json!({"name":"read only","scopes":["observe"]}),
    )
    .await;
    let bearer = format!("Bearer {}", created["token"].as_str().unwrap());
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/tokens",
            &[("host", "127.0.0.1:49152"), ("authorization", &bearer)],
            json!({"name":"not allowed","scopes":["admin"]})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, "POST", "/auth/logout", &full, json!({}))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "GET", "/api/v1/projects", &base, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let audit = state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT data FROM audit WHERE kind='auth.signed_in'",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap();
    assert!(audit.contains("local-password"));
    assert!(!audit.contains(PASSWORD));
}

#[tokio::test]
async fn loopback_origin_and_host_are_pinned_before_login_or_api_access() {
    let (_home, _state, app) = fixture(true);
    for headers in [
        vec![],
        vec![("host", "attacker.invalid:49152")],
        vec![("host", "127.0.0.1:49153")],
        vec![
            ("host", "127.0.0.1:49152"),
            ("origin", "https://attacker.invalid"),
        ],
        vec![
            ("host", "127.0.0.1:49152"),
            ("sec-fetch-site", "cross-site"),
        ],
        vec![("host", "127.0.0.1:49152"), ("sec-fetch-site", "same-site")],
    ] {
        assert_eq!(
            request(&app, "GET", "/auth/session", &headers, Value::Null)
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(
                &app,
                "POST",
                "/auth/login",
                &headers,
                json!({"username":"owner","password":PASSWORD})
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        request(
            &app,
            "GET",
            "/auth/session",
            &[("host", "127.0.0.1:49152"), ("sec-fetch-site", "none")],
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn failed_local_logins_are_rate_limited_and_corrupt_hash_never_falls_back() {
    let (home, _state, app) = fixture(true);
    let hash = home.path().join("secrets/local-password.json");
    let mut value: Value = serde_json::from_slice(&std::fs::read(&hash).unwrap()).unwrap();
    value["password_hash"] = json!(
        value["password_hash"]
            .as_str()
            .unwrap()
            .replace("m=19456", "m=4294967295")
    );
    std::fs::write(&hash, value.to_string()).unwrap();
    for _ in 0..5 {
        assert_eq!(
            login(&app, PASSWORD, true).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        login(&app, PASSWORD, true).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn https_local_auth_retains_secure_cookie_and_old_origin_rules() {
    let (_home, _state, app) = fixture(false);
    let (status, headers, _) = login(&app, PASSWORD, false).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers["set-cookie"]
            .to_str()
            .unwrap()
            .contains("Secure; HttpOnly; SameSite=Strict")
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/auth/login",
            &[
                ("host", "persona.invalid"),
                ("origin", "https://foreign.invalid")
            ],
            json!({"username":"owner","password":PASSWORD})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn first_account_requires_exact_local_capability_and_preserves_committed_credential() {
    let home = tempfile::tempdir().unwrap();
    let config = configuration(true);
    config.save(home.path()).unwrap();
    let state = AppState::open(home.path().into()).unwrap();
    let app = server::router(state.clone());
    let cap = auth::random_token();
    let setup = home.path().join("secrets/first-account.json");
    persona_workshop::config::private_write(&setup,&serde_json::to_vec(&json!({"installation_id":config.id,"token":cap,"expires_at":persona_workshop::state::now_ms()+60_000})).unwrap()).unwrap();
    let headers = [("host", "127.0.0.1:49152"), ("origin", LOCAL_ORIGIN)];
    assert_eq!(
        request(
            &app,
            "POST",
            "/auth/first-account",
            &headers,
            json!({"setup_token":"0".repeat(64),"password":PASSWORD})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert!(!home.path().join("secrets/local-password.json").exists());
    let (status, response, _) = request(
        &app,
        "POST",
        "/auth/first-account",
        &headers,
        json!({"setup_token":cap,"password":PASSWORD}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(response["set-cookie"].to_str().unwrap().starts_with("twn_"));
    let original = std::fs::read(home.path().join("secrets/local-password.json")).unwrap();
    assert_eq!(
        request(
            &app,
            "POST",
            "/auth/first-account",
            &headers,
            json!({"setup_token":cap,"password":"replacement-password"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        std::fs::read(home.path().join("secrets/local-password.json")).unwrap(),
        original
    );
    assert!(!auth::first_account_needed(&state));
    assert_eq!(login(&app, PASSWORD, true).await.0, StatusCode::OK);
    assert!(
        !original
            .windows(PASSWORD.len())
            .any(|w| w == PASSWORD.as_bytes())
    );
}
#[tokio::test]
async fn expired_first_account_does_not_create_credentials_or_alter_saved_records() {
    let home = tempfile::tempdir().unwrap();
    let config = configuration(true);
    config.save(home.path()).unwrap();
    let state = AppState::open(home.path().into()).unwrap();
    let app = server::router(state.clone());
    let cap = auth::random_token();
    persona_workshop::config::private_write(
        &home.path().join("secrets/first-account.json"),
        &serde_json::to_vec(&json!({"installation_id":config.id,"token":cap,"expires_at":0}))
            .unwrap(),
    )
    .unwrap();
    let before = std::fs::read(home.path().join("config.json")).unwrap();
    assert_eq!(
        request(
            &app,
            "POST",
            "/auth/first-account",
            &[("host", "127.0.0.1:49152"), ("origin", LOCAL_ORIGIN)],
            json!({"setup_token":cap,"password":PASSWORD})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        std::fs::read(home.path().join("config.json")).unwrap(),
        before
    );
    assert!(!home.path().join("secrets/local-password.json").exists());
}

#[tokio::test]
async fn native_update_waits_for_work_and_keeps_login_and_status_available() {
    let (home, state, app) = fixture(true);
    let token = std::fs::read_to_string(home.path().join("secrets/control-token")).unwrap();
    let bearer = format!("Bearer {token}");
    let headers = [
        ("host", "127.0.0.1:49152"),
        ("authorization", bearer.as_str()),
    ];
    let (_, _, ready) = request(
        &app,
        "GET",
        "/api/v1/desktop/update-readiness",
        &headers,
        Value::Null,
    )
    .await;
    assert_eq!(ready["idle"], true);
    let permit = state.work_gate.acquire().await.unwrap();
    let (_, _, ready) = request(
        &app,
        "GET",
        "/api/v1/desktop/update-readiness",
        &headers,
        Value::Null,
    )
    .await;
    assert_eq!(ready["idle"], false);
    drop(permit);
    state
        .db
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO jobs VALUES('pending','project','{\"status\":\"queued\"}')",
            [],
        )
        .unwrap();
    let (_, _, ready) = request(
        &app,
        "GET",
        "/api/v1/desktop/update-readiness",
        &headers,
        Value::Null,
    )
    .await;
    assert_eq!(ready["idle"], false);
    assert_eq!(ready["pending_work"], 1);
    let id = uuid::Uuid::new_v4().to_string();
    let folder = home.path().join("native-updates").join(&id);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("plan.json"),
        json!({"id":id,"state":"recovery_required"}).to_string(),
    )
    .unwrap();
    std::fs::write(
        home.path().join("native-updates/active.json"),
        json!({"id":id}).to_string(),
    )
    .unwrap();
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/projects",
            &headers,
            json!({"alias":"Should not be created"})
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        request(&app, "GET", "/api/v1/status", &headers, Value::Null)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        login(&app, PASSWORD, true).await.0,
        StatusCode::OK,
        "Owner must still be able to sign in to recover"
    );
}

#[tokio::test]
async fn update_admission_waits_for_inflight_mutations_without_blocking_reads() {
    use fs2::FileExt;
    let (home, _state, app) = fixture(true);
    std::fs::create_dir_all(home.path().join("updates")).unwrap();
    let guard = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(home.path().join("updates/admission.lock"))
        .unwrap();
    guard.try_lock_exclusive().unwrap();
    let headers = [("host", "127.0.0.1:49152")];
    assert_eq!(
        request(&app, "POST", "/api/v1/projects", &headers, json!({}))
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        request(&app, "GET", "/auth/session", &headers, Value::Null)
            .await
            .0,
        StatusCode::OK
    );
    drop(guard);
    assert_eq!(
        request(&app, "POST", "/api/v1/projects", &headers, json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}
