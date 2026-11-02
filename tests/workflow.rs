//! Black-box workflow checks using synthetic evidence and a deterministic ACP process.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};
use tempfile::TempDir;
use tower::ServiceExt;
use twin_workbench::{
    auth,
    config::{Config, Extractor},
    server,
    state::{AppState, now_ms},
};

struct Fixture {
    _home: TempDir,
    state: Arc<AppState>,
    app: Router,
    token: String,
    endpoint: PathBuf,
}
impl Fixture {
    fn new(mode: &str) -> Self {
        let home = tempfile::tempdir().unwrap();
        let endpoint = home.path().join("fixture");
        std::fs::create_dir(&endpoint).unwrap();
        let config = Config {
            id: uuid::Uuid::new_v4().to_string(),
            owner: "fixture-owner".into(),
            bind: "127.0.0.1".into(),
            port: 0,
            public_url: "https://fixture.test:18448".into(),
            pam_service: "twin-workbench".into(),
            tls_cert: home.path().join("unused.crt"),
            tls_key: home.path().join("unused.key"),
            tls_ca: None,
            embed_origins: vec![],
            transcription: None,
            extractor: Some(Extractor {
                command: "python3".into(),
                args: vec![format!(
                    "{}/tests/fixture_acp.py",
                    env!("CARGO_MANIFEST_DIR")
                )],
                cwd: home.path().to_string_lossy().into_owned(),
                timeout_seconds: 10,
                env: BTreeMap::from([
                    ("TWIN_FIXTURE_MODE".into(), mode.into()),
                    (
                        "TWIN_FIXTURE_DIR".into(),
                        endpoint.to_string_lossy().into_owned(),
                    ),
                    (
                        "SYNTHETIC_AUTH_SECRET".into(),
                        "Never expose extractor configuration".into(),
                    ),
                ]),
            }),
        };
        config.save(home.path()).unwrap();
        let state = AppState::open(home.path().to_owned()).unwrap();
        let app = server::router(state.clone());
        let token = std::fs::read_to_string(home.path().join("secrets/control-token")).unwrap();
        Self {
            _home: home,
            state,
            app,
            token,
            endpoint,
        }
    }
    async fn request(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        request(&self.app, method, path, Some(&self.token), body).await
    }
    async fn ok(&self, method: &str, path: &str, body: Option<Value>) -> Value {
        let (status, body) = self.request(method, path, body).await;
        assert!(status.is_success(), "{method} {path}: {status}: {body}");
        body
    }
    async fn project(&self) -> Value {
        self.ok("POST", "/api/v1/projects", Some(json!({
            "alias":"Synthetic subject", "purpose":"Test a draft simulation", "aliases":["Avery"], "subject_type":"fictional"
        }))).await
    }
    async fn read(&self, p: &Value) -> Value {
        self.ok("GET", &project_path(p), None).await
    }
    async fn upload(&self, p: &Value, name: &str, kind: &str, text: &str) -> Value {
        self.ok(
            "POST",
            &format!("{}/artifacts", project_path(p)),
            Some(json!({
                "expected_revision":p["revision"],"name":name,"kind":kind,"text":text
            })),
        )
        .await
    }
    async fn start_build(&self, p: &Value) -> Value {
        self.ok(
            "POST",
            &format!("{}/builds", project_path(p)),
            Some(json!({"expected_revision":p["revision"]})),
        )
        .await
    }
    async fn wait(&self, job: &Value) -> Value {
        for _ in 0..300 {
            let j = self
                .ok(
                    "GET",
                    &format!("/api/v1/jobs/{}", job["id"].as_str().unwrap()),
                    None,
                )
                .await;
            if !["queued", "running"].contains(&j["status"].as_str().unwrap()) {
                return j;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("Fixture job did not finish")
    }
    async fn built(&self, p: &Value) -> Value {
        let job = self.start_build(p).await;
        let finished = self.wait(&job).await;
        assert_eq!(finished["status"], "complete", "{finished}");
        let after = self.read(p).await;
        let result = finished["result_id"]
            .as_str()
            .expect("A successful build creates profiles automatically");
        let revision = self
            .ok(
                "GET",
                &format!("{}/revisions/{result}", project_path(p)),
                None,
            )
            .await;
        assert_eq!(revision["id"], finished["result_id"]);
        assert!(
            after["revisions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["id"] == finished["result_id"])
        );
        after
    }
    async fn compile(&self, p: &Value) -> Value {
        self.ok(
            "POST",
            &format!("{}/revisions", project_path(p)),
            Some(json!({"expected_revision":p["revision"]})),
        )
        .await
    }
    async fn start_evaluation(&self, p: &Value, r: &Value, variant: &str) -> Value {
        self.ok(
            "POST",
            &format!("{}/evaluations", project_path(p)),
            Some(
                json!({"expected_revision":p["revision"],"revision_id":r["id"],
                "scenario":"A fictional team needs a reversible experiment.","variant":variant}),
            ),
        )
        .await
    }
    async fn owner_preference(&self, p: &Value) -> Value {
        self.ok("POST", &format!("{}/observations", project_path(p)), Some(json!({
            "expected_revision":p["revision"],"collection":"working",
            "statement":"Compare evidence before making a decision.","scope":"Synthetic planning"
        }))).await
    }
    fn frames(&self) -> Vec<Value> {
        std::fs::read_dir(&self.endpoint)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|e| e == "jsonl"))
            .flat_map(|e| {
                std::fs::read_to_string(e.path())
                    .unwrap()
                    .lines()
                    .map(|l| serde_json::from_str(l).unwrap())
                    .collect::<Vec<Value>>()
            })
            .collect()
    }
    async fn wait_started(&self, count: usize) {
        for _ in 0..200 {
            let n = std::fs::read_dir(&self.endpoint)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|e| e.file_name().to_string_lossy().starts_with("ready-"))
                .count();
            if n >= count {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("Fixture endpoint did not start {count} prompts")
    }
}
fn project_path(p: &Value) -> String {
    format!("/api/v1/projects/{}", p["id"].as_str().unwrap())
}
fn revision_path(p: &Value, r: &Value) -> String {
    format!(
        "{}/revisions/{}",
        project_path(p),
        r["id"].as_str().unwrap()
    )
}
async fn request(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let body = if let Some(body) = body {
        req = req.header(header::CONTENT_TYPE, "application/json");
        Body::from(body.to_string())
    } else {
        Body::empty()
    };
    response(app, req.body(body).unwrap()).await
}
async fn response(app: &Router, req: Request<Body>) -> (StatusCode, Value) {
    let r = app.clone().oneshot(req).await.unwrap();
    let status = r.status();
    let bytes = to_bytes(r.into_body(), 5 * 1024 * 1024).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn attributed_collections_exclude_assistant_quotes_peers_and_holdouts() {
    let f = Fixture::new("normal");
    let p = f.project().await;
    let p = f.upload(&p, "Synthetic meeting", "meeting", "Avery: COMMUNICATION-ONLY concise question.\n\nPeer: PEER-EXCLUDED proposal.\n\nAvery: Another concise question.\n> QUOTED-EXCLUDED copied statement.\n\nassistant: ASSISTANT-EXCLUDED generated reply.").await;
    let ai = json!({"messages":[
        {"role":"user","author":"Avery","text":"WORKING-ONLY compare reversible experiments."},
        {"role":"assistant","author":"Avery","text":"GENERATED-EXCLUDED inferred preferences."},
        {"role":"user","author":"Avery","text":"QUOTED-JSON-EXCLUDED","quoted":true},
        {"role":"tool","author":"Avery","text":"TOOL-EXCLUDED result"}
    ]})
    .to_string();
    let p = f.upload(&p, "Synthetic AI session", "ai_chat", &ai).await;
    let p = f.ok("POST", &format!("{}/artifacts", project_path(&p)), Some(json!({"expected_revision":p["revision"],"name":"Held out","kind":"notes","text":"HOLDOUT-EXCLUDED","authors":["Avery"],"holdout":true}))).await;
    assert_eq!(p["stats"]["communication_spans"], 2);
    assert_eq!(p["stats"]["working_spans"], 1);
    let p = f.built(&p).await;
    assert_eq!(p["observations"].as_array().unwrap().len(), 2);
    let frames = f.frames();
    let prompt = frames
        .iter()
        .find(|v| v["method"] == "session/prompt")
        .unwrap()["params"]["prompt"][0]["text"]
        .as_str()
        .unwrap();
    assert!(prompt.contains("COMMUNICATION-ONLY") && prompt.contains("WORKING-ONLY"));
    for excluded in [
        "PEER-EXCLUDED",
        "QUOTED-EXCLUDED",
        "ASSISTANT-EXCLUDED",
        "GENERATED-EXCLUDED",
        "QUOTED-JSON-EXCLUDED",
        "TOOL-EXCLUDED",
        "HOLDOUT-EXCLUDED",
    ] {
        assert!(
            !prompt.contains(excluded),
            "Leaked {excluded} into extraction"
        );
    }
    let refs = p["observations"].as_array().unwrap();
    for o in refs {
        assert_eq!(o["status"], "pending");
        assert_eq!(o["source"], "inferred");
        for id in o["evidence_ids"].as_array().unwrap() {
            let source = f
                .ok(
                    "GET",
                    &format!("{}/evidence/{}", project_path(&p), id.as_str().unwrap()),
                    None,
                )
                .await;
            assert_eq!(source["span"]["speaker"], "Avery");
            if o["collection"] == "working" {
                assert_eq!(source["artifact"]["kind"], "ai_chat");
            }
        }
    }
    let revision = f.compile(&p).await;
    let files = &revision["files"];
    assert!(
        files["communication.md"]
            .as_str()
            .unwrap()
            .contains("concise")
    );
    assert!(
        !files["communication.md"]
            .as_str()
            .unwrap()
            .contains("Compare evidence")
    );
    assert!(
        files["working-style.md"]
            .as_str()
            .unwrap()
            .contains("Compare evidence")
    );
    assert_eq!(revision["manifest"]["calibrated"], false);
}

#[tokio::test]
async fn normalized_json_roles_cannot_smuggle_generated_text_into_authorship() {
    let f = Fixture::new("normal");
    let p = f.project().await;
    let source = json!({"messages":[
        {"role":"user","author":"Avery","text":"A real user choice"},
        {"role":"Assistant","author":"Avery","text":"Uppercase generated text"},
        {"role":"SYSTEM","author":"Avery","text":"System-generated instructions"}
    ]})
    .to_string();
    let p = f.upload(&p, "Role normalization", "ai_chat", &source).await;
    assert_eq!(
        p["stats"]["working_spans"], 1,
        "Role casing must not convert generated text into subject evidence"
    );
}

#[tokio::test]
async fn source_changes_invalidate_compiled_revisions_and_cannot_export_stale_evidence() {
    for alteration in ["delete", "authors", "holdout", "collections", "selection"] {
        let f = Fixture::new("normal");
        let p = f.project().await;
        let p = f
            .upload(
                &p,
                "Source used by a profile",
                "meeting",
                "Avery: Use short concrete examples.\nPeer: Prefer lengthy abstracts.",
            )
            .await;
        let p = f.built(&p).await;
        let r = f.compile(&p).await;
        let p = f.read(&p).await;
        let aid = p["artifacts"][0]["id"].as_str().unwrap();
        let path = format!("{}/artifacts/{aid}", project_path(&p));
        let mut body = json!({"expected_revision":p["revision"]});
        let method = match alteration {
            "delete" => "DELETE",
            "authors" => {
                body["authors"] = json!(["Peer"]);
                "PUT"
            }
            "holdout" => {
                body["holdout"] = json!(true);
                "PUT"
            }
            "collections" => {
                body["collections"] = json!(["working"]);
                "PUT"
            }
            _ => {
                body["selected_span_ids"] = json!([]);
                "PUT"
            }
        };
        let changed = f.ok(method, &path, Some(body)).await;
        assert_eq!(changed["observations"][0]["valid"], false, "{alteration}");
        let (status, _) = f
            .request("GET", &format!("{}/export", revision_path(&p, &r)), None)
            .await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "A {alteration} source change must invalidate prior export"
        );
        assert_eq!(
            changed["revisions"][0]["valid"], false,
            "The UI must show invalidation for {alteration}"
        );
    }
}

#[tokio::test]
async fn extraction_rejects_invented_or_wrong_collection_evidence_atomically() {
    for mode in [
        "wrong_collection",
        "missing_evidence",
        "wrong_section",
        "invalid_section_type",
    ] {
        let f = Fixture::new(mode);
        let p = f.project().await;
        let p = f
            .upload(
                &p,
                "Synthetic meeting",
                "meeting",
                "Avery: Prefer concrete examples.",
            )
            .await;
        let job = f.start_build(&p).await;
        let job = f.wait(&job).await;
        assert_eq!(job["status"], "failed", "{mode}: {job}");
        let after = f.read(&p).await;
        assert_eq!(after["revision"], p["revision"]);
        assert!(after["observations"].as_array().unwrap().is_empty());
        assert!(after["revisions"].as_array().unwrap().is_empty());
        assert!(job["result_id"].is_null());
    }
}

#[tokio::test]
async fn export_keeps_profiles_separate_and_excludes_known_identity_raw_evidence_and_authority() {
    let f = Fixture::new("normal");
    let p = f.project().await;
    let p = f
        .upload(
            &p,
            "Private source title",
            "notes",
            "RAW-EVIDENCE-MARKER-DO-NOT-EXPORT",
        )
        .await;
    f.ok("PUT", &format!("{}/identity",project_path(&p)),Some(json!({"expected_revision":p["revision"],"bindings":{"legal_name":"Avery Person","phone":"+1 (555) 010-0011","email":"avery@example.invalid","organization":"PrivaScope"}}))).await;
    let p = f.read(&p).await;
    assert!(p.to_string().contains("identity_present"));
    assert!(!p.to_string().contains("+1 (555) 010-0011"));
    let p = f.ok("POST", &format!("{}/observations",project_path(&p)),Some(json!({"expected_revision":p["revision"],"collection":"communication","statement":"Avery Person prefers brief calls at +1 (555) 010-0011, or avery@example.invalid.","scope":"PrivaScope planning"}))).await;
    let r = f.compile(&p).await;
    let export = f
        .ok("GET", &format!("{}/export", revision_path(&p, &r)), None)
        .await;
    let encoded = export.to_string();
    for private in [
        "Avery Person",
        "+1 (555) 010-0011",
        "avery@example.invalid",
        "PrivaScope",
        "RAW-EVIDENCE-MARKER-DO-NOT-EXPORT",
        "Private source title",
        "fixture-owner",
        "Never expose extractor configuration",
    ] {
        assert!(!encoded.contains(private), "Export leaked {private}");
    }
    assert_eq!(export["manifest"]["raw_evidence_included"], false);
    assert_eq!(export["manifest"]["identity_included"], false);
    assert_eq!(
        export["files"]["working-style.md"].as_str().unwrap().trim(),
        "# Thinking and working"
    );
    let claims: Value =
        serde_json::from_str(export["files"]["claims.json"].as_str().unwrap()).unwrap();
    assert_eq!(claims[0]["source"], "owner");
    assert_eq!(claims[0]["evidence_ids"], json!([]));
    assert!(claims[0].get("reviewer").is_none());
}

#[tokio::test]
async fn newly_classified_identity_cannot_leak_from_an_older_export() {
    let f = Fixture::new("normal");
    let p = f.project().await;
    let p = f.ok("POST", &format!("{}/observations",project_path(&p)),Some(json!({"expected_revision":p["revision"],"collection":"working","statement":"Keep NebulaPrivateIdentifier in a separate planning lane.","scope":"Personal planning"}))).await;
    let r = f.compile(&p).await;
    let p = f.read(&p).await;
    f.ok("PUT", &format!("{}/identity",project_path(&p)),Some(json!({"expected_revision":p["revision"],"bindings":{"private_project":"NebulaPrivateIdentifier"}}))).await;
    let (status, package) = f
        .request("GET", &format!("{}/export", revision_path(&p, &r)), None)
        .await;
    assert!(
        status == StatusCode::CONFLICT
            || (status.is_success() && !package.to_string().contains("NebulaPrivateIdentifier")),
        "Known private identity remained in an old export"
    );
}

#[tokio::test]
async fn portable_exports_require_current_authorized_profiles_and_keep_merge_inputs_ephemeral() {
    let f = Fixture::new("normal");
    let p = f.project().await;
    let p = f.ok("POST", &format!("{}/observations", project_path(&p)), Some(json!({
        "expected_revision":p["revision"],"collection":"communication",
        "statement":"Begin informal peer messages with a short reaction, then explain what changed your view.","scope":"Peer chat"
    }))).await;
    let r = f.compile(&p).await;
    let path = format!("{}/exports/agents", revision_path(&p, &r));
    let package = f.ok("GET", &path, None).await;
    assert!(
        package["files"]["AGENTS.md"]
            .as_str()
            .unwrap()
            .contains("persona/communication.md")
    );
    assert_eq!(
        package["files"]["persona/communication.md"],
        r["files"]["communication.md"]
    );
    let existing = "# Repository instructions\n\nKEEP-ONLY-IN-DOWNLOAD-92738\n";
    let merged = f
        .ok("POST", &path, Some(json!({"existing_agents":existing})))
        .await;
    let merged_again = f
        .ok(
            "POST",
            &path,
            Some(json!({"existing_agents":merged["files"]["AGENTS.md"]})),
        )
        .await;
    assert_eq!(
        merged_again["files"]["AGENTS.md"],
        merged["files"]["AGENTS.md"]
    );
    assert!(
        merged["files"]["AGENTS.md"]
            .as_str()
            .unwrap()
            .starts_with(existing)
    );
    assert!(
        !f.read(&p)
            .await
            .to_string()
            .contains("KEEP-ONLY-IN-DOWNLOAD")
    );
    for table in ["projects", "revisions", "views"] {
        let count: i64 = f
            .state
            .db
            .lock()
            .unwrap()
            .query_row(
                &format!("SELECT count(*) FROM {table} WHERE value LIKE '%KEEP-ONLY-IN-DOWNLOAD%'"),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
    let count: i64 = f
        .state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM audit WHERE data LIKE '%KEEP-ONLY-IN-DOWNLOAD%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);

    let downloaded = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("{path}/download"))
                .header(header::AUTHORIZATION, format!("Bearer {}", f.token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(downloaded.status(), StatusCode::OK);
    assert_eq!(
        downloaded.headers()[header::CONTENT_TYPE],
        "application/zip"
    );
    let bytes = to_bytes(downloaded.into_body(), 1024 * 1024).await.unwrap();
    let zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    assert!(zip.file_names().any(|name| name == "AGENTS.md"));

    assert_eq!(
        request(&f.app, "GET", &path, None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let observe = f
        .ok(
            "POST",
            "/api/v1/tokens",
            Some(json!({"label":"observe-only", "scopes":["observe"],"expires_hours":1})),
        )
        .await;
    assert_eq!(
        request(&f.app, "GET", &path, observe["token"].as_str(), None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let view = f.ok("POST", "/api/v1/views", Some(json!({}))).await;
    let view_path = format!("/api/v1/views/{}/commands", view["id"].as_str().unwrap());
    let shown = f.ok("POST", &view_path, Some(json!({"expected_revision":view["revision"],"controller":"fixture","commands":[
        {"op":"set","field":"modal","value":"profile.export"},
        {"op":"set","field":"draft","value":{"export_format":"agents","export_merge":true}}
    ]}))).await;
    assert_eq!(shown["modal"], "profile.export");
    assert_eq!(
        f.request(
            "POST",
            &view_path,
            Some(
                json!({"expected_revision":shown["revision"],"controller":"fixture","commands":[
                    {"op":"set","field":"draft","value":{"existing_agents":existing}}
                ]})
            )
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let p = f.read(&p).await;
    f.ok("PUT", &format!("{}/identity", project_path(&p)), Some(json!({"expected_revision":p["revision"],"bindings":{"name":"Changed private identity"}}))).await;
    for suffix in [
        "exports/agents",
        "exports/soul",
        "exports/agents/download",
        "exports/soul/download",
    ] {
        assert_eq!(
            f.request("GET", &format!("{}/{suffix}", revision_path(&p, &r)), None)
                .await
                .0,
            StatusCode::CONFLICT
        );
    }
    assert_eq!(
        f.request("POST", &path, Some(json!({"existing_agents":existing})))
            .await
            .0,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn optimistic_project_revisions_prevent_silent_overwrites() {
    let f = Fixture::new("normal");
    let p = f.project().await;
    let changed = f
        .ok(
            "PUT",
            &project_path(&p),
            Some(json!({"expected_revision":p["revision"],"purpose":"Updated purpose"})),
        )
        .await;
    let (status, _) = f
        .request(
            "PUT",
            &project_path(&p),
            Some(json!({"expected_revision":p["revision"],"purpose":"Lost update"})),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(f.read(&p).await["purpose"], changed["purpose"]);
    let (status,_) = f.request("POST",&format!("{}/artifacts",project_path(&p)),Some(json!({"expected_revision":p["revision"],"name":"Stale source","kind":"notes","text":"Do not append me"}))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(f.read(&p).await["artifacts"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn semantic_view_commands_are_atomic_scoped_and_cannot_store_private_payloads() {
    let f = Fixture::new("normal");
    let view = f.ok("POST", "/api/v1/views", Some(json!({}))).await;
    let path = format!("/api/v1/views/{}", view["id"].as_str().unwrap());
    let commands = format!("{path}/commands");
    let body = |revision: &Value, commands: Value| json!({"expected_revision":revision,"controller":"fixture-controller","commands":commands});
    let updated = f.ok("POST",&commands,Some(body(&view["revision"],json!([{"op":"set","field":"screen","value":"review"},{"op":"set","field":"tab","value":"working"}])))).await;
    assert_eq!(updated["screen"], "review");
    assert_eq!(updated["tab"], "working");
    assert_eq!(
        f.request("POST", &commands, Some(body(&view["revision"], json!([]))))
            .await
            .0,
        StatusCode::CONFLICT
    );
    for invalid in [
        json!({"op":"set","field":"draft","value":{"nested":{"password":"secret"}}}),
        json!({"op":"set","field":"project_id","value":{"raw":"RAW-SOURCE-BYPASS"}}),
        json!({"op":"set","field":"focus","value":{"token":"AUTH-BYPASS"}}),
        json!({"op":"set","field":"filter","value":{"identity":"PRIVATE-BYPASS"}}),
        json!({"op":"set","field":"draft","value":{"unrecognized":{"memo":"UNSCHEMATIZED-RAW-SOURCE"}}}),
        json!({"op":"set","field":"not_a_field","value":"value"}),
    ] {
        let (status, _) = f
            .request(
                "POST",
                &commands,
                Some(body(
                    &updated["revision"],
                    json!([{"op":"set","field":"screen","value":"profiles"},invalid]),
                )),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "Unsafe semantic payload was accepted"
        );
        assert_eq!(
            f.ok("GET", &path, None).await,
            updated,
            "A rejected batch partially mutated the view"
        );
    }
    f.ok(
        "POST",
        &format!("{path}/ack"),
        Some(json!({"revision":updated["revision"]})),
    )
    .await;
    let activity = f.ok("GET", "/api/v1/activity", None).await;
    assert!(
        !activity.to_string().contains("view"),
        "Navigation acknowledgments should not pollute activity"
    );
}

#[tokio::test]
async fn auth_blocks_unauthorized_reads_and_requires_same_origin_csrf_with_http2_cookies() {
    let f = Fixture::new("normal");
    let p = f.project().await;
    for path in [
        "/api/v1/projects".to_owned(),
        project_path(&p),
        format!("{}/identity", project_path(&p)),
        "/api/v1/views".into(),
        "/api/v1/status".into(),
        "/api/v1/activity".into(),
    ] {
        let (status, body) = request(&f.app, "GET", &path, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(!body.to_string().contains("Synthetic subject"));
    }
    let token = "synthetic-browser-cookie";
    let csrf = "synthetic-csrf";
    f.state.db.lock().unwrap().execute("INSERT INTO browser_sessions(hash,owner,csrf,expires_at) VALUES(?1,'fixture-owner',?2,?3)",rusqlite::params![auth::digest(token),csrf,now_ms()+60_000]).unwrap();
    let cookie = format!("{}={token}", auth::cookie_name(&f.state));
    let build = |origin: &str, csrf_value: Option<&str>| {
        let mut r = Request::builder()
            .method("POST")
            .uri("https://fixture.test:18448/api/v1/views")
            .version(axum::http::Version::HTTP_2)
            .header(header::ORIGIN, origin)
            .header(header::COOKIE, "other_application=first")
            .header(header::COOKIE, &cookie)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(v) = csrf_value {
            r = r.header("x-csrf-token", v);
        }
        r.body(Body::from("{}")).unwrap()
    };
    assert_eq!(
        response(&f.app, build("https://fixture.test:18448", None))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        response(&f.app, build("https://attacker.invalid", Some(csrf)))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        response(&f.app, build("https://fixture.test:18448", Some("wrong")))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert!(
        response(&f.app, build("https://fixture.test:18448", Some(csrf)))
            .await
            .0
            .is_success()
    );
    let status = f.ok("GET", "/api/v1/status", None).await.to_string();
    assert!(!status.contains("Never expose extractor configuration"));
    assert!(!status.contains(&f.token));
}

#[tokio::test]
async fn queued_work_is_bounded_cancellable_and_does_not_apply_stale_results() {
    let f = Fixture::new("hold");
    let mut projects = vec![];
    let mut jobs = vec![];
    for n in 0..3 {
        let p = f.project().await;
        let p = f
            .upload(
                &p,
                &format!("Source {n}"),
                "notes",
                &format!("Synthetic choice {n}"),
            )
            .await;
        jobs.push(f.start_build(&p).await);
        projects.push(p);
    }
    f.wait_started(2).await;
    assert_eq!(
        f.frames()
            .iter()
            .filter(|v| v["method"] == "session/prompt")
            .count(),
        2,
        "Only two analysis processes may run concurrently"
    );
    assert_eq!(
        f.request(
            "POST",
            &format!("{}/builds", project_path(&projects[0])),
            Some(json!({"expected_revision":projects[0]["revision"]}))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    f.ok(
        "POST",
        &format!("/api/v1/jobs/{}/cancel", jobs[2]["id"].as_str().unwrap()),
        Some(json!({})),
    )
    .await;
    assert_eq!(f.wait(&jobs[2]).await["status"], "cancelled");
    f.ok("PUT",&project_path(&projects[0]),Some(json!({"expected_revision":projects[0]["revision"],"purpose":"Changed while extraction runs"}))).await;
    f.ok(
        "POST",
        &format!("/api/v1/jobs/{}/cancel", jobs[1]["id"].as_str().unwrap()),
        Some(json!({})),
    )
    .await;
    assert_eq!(f.wait(&jobs[1]).await["status"], "cancelled");
    std::fs::write(f.endpoint.join("release"), "ready").unwrap();
    assert_eq!(f.wait(&jobs[0]).await["status"], "stale");
    for p in &projects {
        let after = f.read(p).await;
        assert!(after["observations"].as_array().unwrap().is_empty());
        assert!(
            after["revisions"].as_array().unwrap().is_empty(),
            "Cancelled or stale builds cannot publish profiles"
        );
    }
    assert_eq!(
        f.frames()
            .iter()
            .filter(|v| v["method"] == "session/prompt")
            .count(),
        2,
        "Cancelling queued work must not launch another process"
    );
}

#[tokio::test]
async fn text_only_analysis_declines_callbacks_and_rejects_tool_using_endpoints() {
    let f = Fixture::new("callbacks");
    let p = f.project().await;
    let p = f
        .upload(
            &p,
            "Synthetic notes",
            "notes",
            "Use a reversible experiment.",
        )
        .await;
    f.built(&p).await;
    let frames = f.frames();
    assert_eq!(
        frames.iter().find(|v| v["method"] == "initialize").unwrap()["params"]["clientCapabilities"],
        json!({})
    );
    assert_eq!(
        frames
            .iter()
            .find(|v| v["method"] == "session/new")
            .unwrap()["params"]["mcpServers"],
        json!([])
    );
    assert_eq!(
        frames
            .iter()
            .find(|v| v["id"] == "permission-fixture")
            .unwrap()["result"]["outcome"]["outcome"],
        "cancelled"
    );
    assert_eq!(
        frames.iter().find(|v| v["id"] == "file-fixture").unwrap()["error"]["code"],
        -32601
    );
    let f = Fixture::new("tool");
    let p = f.project().await;
    let p = f
        .upload(&p, "Synthetic notes", "notes", "Keep this purely textual.")
        .await;
    let job = f.start_build(&p).await;
    assert_eq!(f.wait(&job).await["status"], "failed");
    assert!(
        f.read(&p).await["observations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn compiling_is_compare_and_swap_and_rolls_back_on_record_failure() {
    let f = Fixture::new("normal");
    let p = f.project().await;
    let p = f.owner_preference(&p).await;
    // A storage failure must not consume a project revision or leave a phantom profile.
    f.state.db.lock().unwrap().execute_batch("CREATE TRIGGER fixture_reject_profile BEFORE INSERT ON revisions BEGIN SELECT RAISE(ABORT,'synthetic storage failure'); END;").unwrap();
    let path = format!("{}/revisions", project_path(&p));
    let body = json!({"expected_revision":p["revision"]});
    let (status, _) = f.request("POST", &path, Some(body.clone())).await;
    assert!(status.is_server_error());
    let unchanged = f.read(&p).await;
    assert_eq!(unchanged["revision"], p["revision"]);
    assert!(unchanged["revisions"].as_array().unwrap().is_empty());
    f.state
        .db
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER fixture_reject_profile;")
        .unwrap();
    let gate = Arc::new(tokio::sync::Barrier::new(3));
    let mut workers = vec![];
    for _ in 0..2 {
        let app = f.app.clone();
        let token = f.token.clone();
        let path = path.clone();
        let body = body.clone();
        let gate = gate.clone();
        workers.push(tokio::spawn(async move {
            gate.wait().await;
            request(&app, "POST", &path, Some(&token), Some(body)).await
        }));
    }
    gate.wait().await;
    let mut statuses = vec![];
    for worker in workers {
        statuses.push(worker.await.unwrap().0.as_u16());
    }
    statuses.sort();
    assert_eq!(statuses, [200, 409]);
    let after = f.read(&p).await;
    assert_eq!(
        after["revision"].as_u64().unwrap(),
        p["revision"].as_u64().unwrap() + 1
    );
    assert_eq!(after["revisions"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn parallel_evaluation_variants_complete_without_invalidating_each_other() {
    let f = Fixture::new("hold");
    let p = f.project().await;
    let p = f.owner_preference(&p).await;
    let r = f.compile(&p).await;
    let p = f.read(&p).await;
    let a = f.start_evaluation(&p, &r, "communication").await;
    let b = f.start_evaluation(&p, &r, "working").await;
    f.wait_started(2).await;
    std::fs::write(f.endpoint.join("release"), "ready").unwrap();
    assert_eq!(f.wait(&a).await["status"], "complete");
    assert_eq!(f.wait(&b).await["status"], "complete");
    let after = f.read(&p).await;
    assert_eq!(
        after["revision"].as_u64().unwrap(),
        p["revision"].as_u64().unwrap() + 2
    );
    let evaluations = after["evaluations"].as_array().unwrap();
    assert_eq!(evaluations.len(), 2);
    for result in evaluations {
        assert_eq!(result["revision_id"], r["id"]);
        assert!(
            result["response"]
                .as_str()
                .unwrap()
                .starts_with("SIMULATION:")
        );
        assert!(!result.to_string().contains("HIDDEN-FIXTURE-REASONING"));
        assert!(!result.to_string().contains("WRONG-SESSION-FIXTURE"));
    }
    f.ok("GET", &format!("{}/export", revision_path(&p, &r)), None)
        .await;
    let eid = evaluations[0]["id"].as_str().unwrap();
    let path = format!("{}/evaluations/{eid}", project_path(&p));
    f.state.db.lock().unwrap().execute_batch("CREATE TRIGGER fixture_reject_rating BEFORE UPDATE ON evaluations BEGIN SELECT RAISE(ABORT,'synthetic rating storage failure'); END;").unwrap();
    let (status, _) = f.request("PUT", &path, Some(json!({"expected_revision":after["revision"],"rating":{"communication_fit":4,"working_fit":5,"notes":"Synthetic judgment"}}))).await;
    assert!(status.is_server_error(), "{status}");
    let unchanged = f.read(&p).await;
    assert_eq!(unchanged["revision"], after["revision"]);
    assert!(unchanged["evaluations"][0]["rating"].is_null());
}

#[tokio::test]
async fn evaluation_cannot_publish_after_private_identity_changes() {
    let f = Fixture::new("hold");
    let p = f.project().await;
    let p = f.owner_preference(&p).await;
    let r = f.compile(&p).await;
    let p = f.read(&p).await;
    let job = f.start_evaluation(&p, &r, "both").await;
    f.wait_started(1).await;
    f.ok(
        "PUT",
        &format!("{}/identity", project_path(&p)),
        Some(json!({
            "expected_revision":p["revision"],"bindings":{"private_alias":"Synthetic private label"}
        })),
    )
    .await;
    std::fs::write(f.endpoint.join("release"), "ready").unwrap();
    assert_eq!(f.wait(&job).await["status"], "stale");
    let after = f.read(&p).await;
    assert!(after["evaluations"].as_array().unwrap().is_empty());
    assert_eq!(
        after["revision"].as_u64().unwrap(),
        p["revision"].as_u64().unwrap() + 1
    );
}

#[tokio::test]
async fn automatic_profiles_contain_distinct_runtime_guidance_without_extraction_metadata() {
    let f = Fixture::new("profiles");
    let p = f.project().await;
    let p = f.upload(&p, "Peer chat", "chat", "Avery: Yeah, that tracks. Let's try the smaller change first because the next conversation will tell us where people hesitate. What would you test?\n\nAvery: I'd put the next move up front, then explain the part that clicked.").await;
    let p = f.upload(&p, "Working choices", "ai_chat", &json!({"messages":[{"role":"user","author":"Avery","text":"Sketch the user experience first. Use a reversible trial to resolve the assumption; give an owner an outcome and let them choose the method. Revisit repeated handoffs."}]}).to_string()).await;
    let before_revision = p["revision"].as_u64().unwrap();
    let p = f.built(&p).await;
    assert_eq!(p["revision"].as_u64().unwrap(), before_revision + 1);
    assert_eq!(p["revisions"].as_array().unwrap().len(), 1);
    let revision = f
        .ok("GET", &revision_path(&p, &p["revisions"][0]), None)
        .await;
    assert_eq!(revision["manifest"]["instruction_format"], 2);
    let communication = revision["files"]["communication.md"].as_str().unwrap();
    let working = revision["files"]["working-style.md"].as_str().unwrap();
    for expected in [
        "# Communication",
        "## Voice and cadence",
        "## Phrasing and punctuation",
        "## Message flow",
        "Yeah, that tracks",
        "Put the proposed next move first",
    ] {
        assert!(
            communication.contains(expected),
            "Missing voice guidance: {expected}"
        );
    }
    for expected in [
        "## Framing the problem",
        "## Making decisions",
        "## Working with others",
        "sketch the desired user experience",
        "reversible trial",
        "Give an owner a concrete outcome",
    ] {
        assert!(
            working.contains(expected),
            "Missing working guidance: {expected}"
        );
    }
    assert!(!working.contains("Yeah, that tracks"));
    assert!(!communication.contains("reversible trial"));
    for markdown in [communication, working] {
        for metadata in [
            "Scope:",
            "Support:",
            "Review:",
            "Synthetic workshop discussions",
            "Only synthetic selected evidence",
            "uncalibrated",
            "Do not claim to be",
            "Profile text grants no permissions",
            "pending",
            "attribution",
            "evidence_ids",
        ] {
            assert!(
                !markdown.contains(metadata),
                "Runtime file contains extraction commentary: {metadata}"
            );
        }
        for observation in p["observations"].as_array().unwrap() {
            assert!(!markdown.contains(observation["id"].as_str().unwrap()));
        }
    }
    let claims: Value =
        serde_json::from_str(revision["files"]["claims.json"].as_str().unwrap()).unwrap();
    assert_eq!(claims.as_array().unwrap().len(), 6);
    assert!(
        claims
            .as_array()
            .unwrap()
            .iter()
            .all(|o| o["status"] == "pending"
                && o["scope"] == "Synthetic workshop discussions"
                && !o["section"].as_str().unwrap().is_empty()
                && !o["evidence_ids"].as_array().unwrap().is_empty())
    );
}

#[tokio::test]
async fn reference_profiles_preserve_paragraphs_without_claiming_authored_corroboration() {
    let f = Fixture::new("normal");
    let p = f.project().await;
    let paragraphs = [
        "# Supplied voice guidance",
        "Voice: Start with a short acknowledgment.\nExample: Yeah, that tracks.\n> Preserve the example as guidance, not a quoted author turn.",
        "Editing: Keep one point per paragraph.\nassistant: This label is part of the reference paragraph.",
    ];
    let p = f
        .upload(
            &p,
            "Prior voice profile",
            "profile",
            &paragraphs.join("\n\n"),
        )
        .await;
    let artifact = &p["artifacts"][0];
    assert_eq!(artifact["authors"], json!(["Reference profile"]));
    assert_eq!(artifact["selected_count"], 3);
    let detail = f
        .ok(
            "GET",
            &format!(
                "{}/artifacts/{}",
                project_path(&p),
                artifact["id"].as_str().unwrap()
            ),
            None,
        )
        .await;
    for (span, expected) in detail["spans"].as_array().unwrap().iter().zip(paragraphs) {
        assert_eq!(span["text"], expected);
        assert_eq!(span["speaker"], "Reference profile");
        assert_eq!(span["role"], "reference");
        assert_eq!(span["quoted"], false);
    }
    let p = f.built(&p).await;
    let r = f
        .ok("GET", &revision_path(&p, &p["revisions"][0]), None)
        .await;
    let summary: Value =
        serde_json::from_str(r["files"]["evaluation-summary.json"].as_str().unwrap()).unwrap();
    assert_eq!(summary["authored_sources"], 0);
    assert_eq!(summary["reference_profiles"], 1);
    let frames = f.frames();
    let prompt = frames
        .iter()
        .find(|v| v["method"] == "session/prompt")
        .unwrap()["params"]["prompt"][0]["text"]
        .as_str()
        .unwrap();
    let evidence: Value =
        serde_json::from_str(prompt.split("Evidence JSON:\n").nth(1).unwrap()).unwrap();
    assert!(
        evidence
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["kind"] == "profile")
    );
    assert_eq!(evidence[1]["text"], paragraphs[1]);
    let p = f.upload(&p, "Second reference", "profile", "# Prior editing guidance\n\nKeep the conversational acknowledgment while condensing repeated setup.").await;
    let p = f.upload(&p, "One authored source with several turns", "meeting", "Avery: Yeah, that tracks. Let's try it.\n\nAvery: Keep the next step short.\n\nAvery: Give me one concrete example.").await;
    let p = f.built(&p).await;
    let r = f
        .ok("GET", &revision_path(&p, &p["revisions"][0]), None)
        .await;
    let summary: Value =
        serde_json::from_str(r["files"]["evaluation-summary.json"].as_str().unwrap()).unwrap();
    assert_eq!(
        summary["authored_sources"], 1,
        "Several turns from one artifact are one authored source"
    );
    assert_eq!(summary["reference_profiles"], 2);
    assert_eq!(summary["calibrated"], false);
}

#[tokio::test]
async fn sections_and_statement_limits_are_validated_without_breaking_legacy_guidance() {
    let f = Fixture::new("normal");
    let mut p = f.project().await;
    for section in [
        json!("decisions"),
        json!("unknown"),
        json!({"nested":"voice"}),
        json!(42),
        Value::Null,
    ] {
        let (status, _) = f.request("POST", &format!("{}/observations", project_path(&p)), Some(json!({"expected_revision":p["revision"],"collection":"communication","section":section,"statement":"Keep the concrete acknowledgment."}))).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "Malformed section accepted: {section}"
        );
        assert_eq!(f.read(&p).await["revision"], p["revision"]);
    }
    for (collection, sections) in [
        (
            "communication",
            vec![
                "voice",
                "phrasing",
                "structure",
                "tone",
                "context",
                "editing",
                "",
            ],
        ),
        (
            "working",
            vec![
                "framing",
                "decisions",
                "execution",
                "collaboration",
                "feedback",
                "tradeoffs",
                "",
            ],
        ),
    ] {
        for section in sections {
            p = f.ok("POST", &format!("{}/observations", project_path(&p)), Some(json!({"expected_revision":p["revision"],"collection":collection,"section":section,"statement":format!("Synthetic {collection} {section} guidance.")}))).await;
            let observation = p["observations"].as_array().unwrap().last().unwrap();
            assert_eq!(observation["section"], section);
            assert_eq!(observation["scope"], "Selected contexts");
        }
    }
    let oid = p["observations"][0]["id"].as_str().unwrap();
    for section in [json!("decisions"), json!(false)] {
        let (status, _) = f.request("PUT", &format!("{}/observations/{oid}", project_path(&p)), Some(json!({"expected_revision":p["revision"],"section":section,"statement":"This update must not partially apply."}))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(f.read(&p).await["observations"], p["observations"]);
    }
    p = f.ok("POST", &format!("{}/observations", project_path(&p)), Some(json!({"expected_revision":p["revision"],"collection":"working","statement":"x".repeat(3000)}))).await;
    let (status, _) = f.request("POST", &format!("{}/observations", project_path(&p)), Some(json!({"expected_revision":p["revision"],"collection":"working","statement":"x".repeat(3001)}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Simulate a stored pre-section project to exercise the persisted record migration.
    let mut legacy =
        serde_json::to_value(f.state.project(p["id"].as_str().unwrap()).unwrap()).unwrap();
    for observation in legacy["observations"].as_array_mut().unwrap() {
        observation.as_object_mut().unwrap().remove("section");
    }
    f.state
        .db
        .lock()
        .unwrap()
        .execute(
            "UPDATE projects SET value=?2 WHERE id=?1",
            rusqlite::params![p["id"].as_str().unwrap(), legacy.to_string()],
        )
        .unwrap();
    let legacy = f.read(&p).await;
    assert!(
        legacy["observations"]
            .as_array()
            .unwrap()
            .iter()
            .all(|o| o["section"] == "")
    );
    let revision = f.compile(&legacy).await;
    assert!(
        revision["files"]["communication.md"]
            .as_str()
            .unwrap()
            .contains("## Guidance")
    );
}

#[tokio::test]
async fn automatic_build_rolls_back_observations_and_profiles_on_compile_or_storage_failure() {
    for failure in ["compile_overflow", "storage"] {
        let f = Fixture::new(if failure == "storage" {
            "normal"
        } else {
            failure
        });
        let p = f.project().await;
        let p = f
            .upload(
                &p,
                "Synthetic source",
                "notes",
                "Try one concrete question before asking for a larger plan.",
            )
            .await;
        let p = f.owner_preference(&p).await;
        if failure == "storage" {
            f.state.db.lock().unwrap().execute_batch("CREATE TRIGGER fixture_reject_automatic_profile BEFORE INSERT ON revisions BEGIN SELECT RAISE(ABORT,'synthetic storage failure'); END;").unwrap();
        }
        let job = f.start_build(&p).await;
        let finished = f.wait(&job).await;
        assert_eq!(finished["status"], "failed", "{failure}: {finished}");
        assert!(finished["result_id"].is_null());
        let after = f.read(&p).await;
        assert_eq!(
            after["revision"], p["revision"],
            "{failure} consumed a project revision"
        );
        assert_eq!(
            after["observations"], p["observations"],
            "{failure} partially applied observations"
        );
        assert!(
            after["revisions"].as_array().unwrap().is_empty(),
            "{failure} published a profile"
        );
    }
}

#[tokio::test]
async fn rebuild_preserves_owner_choices_and_manual_edits_to_pending_guidance() {
    let f = Fixture::new("profiles");
    let p = f.project().await;
    let p = f.ok("POST", &format!("{}/artifacts", project_path(&p)), Some(json!({"expected_revision":p["revision"],"name":"Synthetic behavior sample","kind":"notes","text":"Use a conversational acknowledgment and one concrete question. Shape a reversible trial around the desired user experience, with explicit ownership.","collections":["communication","working"]}))).await;
    let mut p = f.built(&p).await;
    let original = p["observations"].as_array().unwrap().clone();
    let accepted = &original[0];
    let rejected = &original[1];
    let edited = &original[2];
    for (observation, change) in [
        (accepted, json!({"status":"accepted"})),
        (rejected, json!({"status":"rejected"})),
        (
            edited,
            json!({"statement":"Keep my manually chosen opening, then one concrete question.","section":"phrasing"}),
        ),
    ] {
        let mut body = change;
        body["expected_revision"] = p["revision"].clone();
        p = f
            .ok(
                "PUT",
                &format!(
                    "{}/observations/{}",
                    project_path(&p),
                    observation["id"].as_str().unwrap()
                ),
                Some(body),
            )
            .await;
    }
    p = f.owner_preference(&p).await;
    let preserved: Vec<Value> = p["observations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| {
            o["source"] == "owner"
                || o["id"] == accepted["id"]
                || o["id"] == rejected["id"]
                || o["id"] == edited["id"]
        })
        .cloned()
        .collect();
    let before_revision = p["revision"].as_u64().unwrap();
    let p = f.built(&p).await;
    assert_eq!(p["revision"].as_u64().unwrap(), before_revision + 1);
    assert_eq!(p["revisions"].as_array().unwrap().len(), 2);
    for expected in &preserved {
        assert_eq!(
            p["observations"]
                .as_array()
                .unwrap()
                .iter()
                .find(|o| o["id"] == expected["id"]),
            Some(expected),
            "A deliberate choice was overwritten"
        );
    }
    for replaced in &original[3..] {
        assert!(
            !p["observations"]
                .as_array()
                .unwrap()
                .iter()
                .any(|o| o["id"] == replaced["id"]),
            "Unreviewed inference was not refreshed"
        );
    }
    let revision = f
        .ok("GET", &revision_path(&p, &p["revisions"][0]), None)
        .await;
    let communication = revision["files"]["communication.md"].as_str().unwrap();
    assert!(communication.contains("Keep my manually chosen opening"));
    assert!(communication.contains(accepted["statement"].as_str().unwrap()));
    assert!(!communication.contains(rejected["statement"].as_str().unwrap()));
    assert!(
        !p["observations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["statement"] == rejected["statement"] && o["status"] != "rejected"),
        "Rebuild resurrected rejected guidance"
    );
}

struct AgentFixture {
    url: String,
    ca: String,
    data: Arc<std::sync::Mutex<Value>>,
    handle: tokio::task::JoinHandle<()>,
}
impl Drop for AgentFixture {
    fn drop(&mut self) {
        self.handle.abort();
    }
}
impl AgentFixture {
    async fn new() -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let certificate = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let ca = certificate.cert.pem();
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
            ca.clone().into_bytes(),
            certificate.signing_key.serialize_pem().into_bytes(),
        )
        .await
        .unwrap();
        let data = Arc::new(std::sync::Mutex::new(json!({
            "agent_id":"test-agent", "calls":[], "redirect":false,
            "installation":{"revision":0,"active":null,"previous":[]}
        })));
        let captured = data.clone();
        let app = Router::new().fallback(move |request: Request<Body>| {
            let data = captured.clone();
            async move {
                let method = request.method().to_string();
                let path = request.uri().path().to_owned();
                let auth = request.headers().get(header::AUTHORIZATION).and_then(|v|v.to_str().ok()).unwrap_or("").to_owned();
                let bytes = to_bytes(request.into_body(), 3*1024*1024).await.unwrap();
                let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                let mut data = data.lock().unwrap();
                data["calls"].as_array_mut().unwrap().push(json!({"method":method,"path":path,"body":body}));
                use axum::response::IntoResponse;
                if auth != "Bearer TEST_REMOTE_CREDENTIAL_123456789" {
                    return (StatusCode::FORBIDDEN, axum::Json(json!({"secret":"NEVER_ECHO_REMOTE_BODY"}))).into_response();
                }
                if data["redirect"] == true && path == "/api/v1/status" {
                    return (StatusCode::TEMPORARY_REDIRECT, [(header::LOCATION,"/api/v1/leaked")]).into_response();
                }
                if path == "/api/v1/status" {
                    return axum::Json(json!({"instance_id":data["agent_id"],"name":"Alex (simulation)"})).into_response();
                }
                if path == "/api/v1/leaked" { return axum::Json(json!({"leaked":true})).into_response(); }
                if method == "POST" {
                    if body["expected_revision"] != data["installation"]["revision"] {
                        return (StatusCode::CONFLICT,axum::Json(json!({"secret":"NEVER_ECHO_REMOTE_BODY"}))).into_response();
                    }
                    let current = data["installation"]["active"].clone();
                    let mut previous = data["installation"]["previous"].as_array().unwrap().clone();
                    let active = if path.ends_with("/restore") {
                        let Some(index) = previous.iter().position(|v|v["id"]==body["id"]) else {
                            return StatusCode::NOT_FOUND.into_response();
                        };
                        previous.remove(index)
                    } else {
                        let package = &body["package"];
                        assert_eq!(package["schema"],"twin-profile/1");
                        for name in ["communication.md","working-style.md"] {
                            assert_eq!(auth::digest(package["files"][name].as_str().unwrap()), package["manifest"]["files"][name]);
                        }
                        json!({"id":uuid::Uuid::new_v4(),"kind":"package","source":{"schema":"twin-profile/1","subject_key":package["manifest"]["subject_key"],"revision":package["manifest"]["revision"]},"files":package["manifest"]["files"]})
                    };
                    previous.insert(0, if current.is_null() { json!({"id":uuid::Uuid::new_v4(),"source":null,"kind":"baseline"}) } else { current });
                    data["installation"] = json!({"revision":data["installation"]["revision"].as_u64().unwrap()+1,"active":active,"previous":previous});
                }
                axum::Json(data["installation"].clone()).into_response()
            }
        });
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move {
            axum_server::from_tcp_rustls(listener, tls)
                .unwrap()
                .serve(app.into_make_service())
                .await
                .unwrap();
        });
        Self {
            url: format!("https://localhost:{port}"),
            ca,
            data,
            handle,
        }
    }
    fn connect_body(&self) -> Value {
        json!({"url":self.url,"token":"TEST_REMOTE_CREDENTIAL_123456789","ca_pem":self.ca})
    }
}

#[tokio::test]
async fn agent_profile_connector_verifies_tls_applies_a_saved_pair_and_restores_with_revision_checks()
 {
    let f = Fixture::new("valid");
    let agent = AgentFixture::new().await;
    let mut without_ca = agent.connect_body();
    without_ca["ca_pem"] = json!("");
    assert_eq!(
        f.request("POST", "/api/v1/agent-targets", Some(without_ca))
            .await
            .0,
        StatusCode::BAD_GATEWAY
    );
    let linked = f
        .ok("POST", "/api/v1/agent-targets", Some(agent.connect_body()))
        .await;
    let target = linked["target"]["id"].as_str().unwrap();
    assert_eq!(linked["installation"]["revision"], 0);
    assert!(!linked.to_string().contains("CREDENTIAL"));
    let targets = f.ok("GET", "/api/v1/agent-targets", None).await;
    assert_eq!(targets["items"].as_array().unwrap().len(), 1);
    assert!(!targets.to_string().contains("CREDENTIAL"));
    let reconnect = f
        .ok("POST", "/api/v1/agent-targets", Some(agent.connect_body()))
        .await;
    assert_eq!(reconnect["target"]["id"], target);
    let p = f.project().await;
    let p = f.ok("POST",&format!("{}/observations",project_path(&p)),Some(json!({"expected_revision":p["revision"],"collection":"communication","section":"voice","statement":"Open with a brief question about the shared task."}))).await;
    let r = f
        .ok(
            "POST",
            &format!("{}/revisions", project_path(&p)),
            Some(json!({"expected_revision":p["revision"]})),
        )
        .await;
    let endpoint = format!("{}/apply", revision_path(&p, &r));
    let installed = f
        .ok(
            "POST",
            &endpoint,
            Some(json!({"target_id":target,"expected_revision":0})),
        )
        .await;
    assert_eq!(
        installed["installation"]["active"]["source"]["revision"],
        r["id"]
    );
    assert_eq!(installed["installation"]["revision"], 1);
    let (code, body) = f
        .request(
            "POST",
            &endpoint,
            Some(json!({"target_id":target,"expected_revision":0})),
        )
        .await;
    assert_eq!(code, StatusCode::CONFLICT);
    assert!(!body.to_string().contains("NEVER_ECHO"));
    let restored = f
        .ok(
            "POST",
            &format!("/api/v1/agent-targets/{target}/restore"),
            Some(
                json!({"expected_revision":1,"id":installed["installation"]["previous"][0]["id"]}),
            ),
        )
        .await;
    assert_eq!(restored["installation"]["revision"], 2);
    assert_eq!(restored["installation"]["active"]["kind"], "baseline");
    let status = f
        .ok("GET", &format!("/api/v1/agent-targets/{target}"), None)
        .await;
    assert_eq!(status["installation"]["revision"], 2);
    let activity = f.ok("GET", "/api/v1/activity", None).await.to_string();
    assert!(activity.contains("profile_applied") && activity.contains("profile_restored"));
    assert!(!activity.contains("CREDENTIAL") && !activity.contains("BEGIN CERTIFICATE"));
    let secret = f
        .state
        .home
        .join(format!("secrets/agent-target-{target}.json"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&secret).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    // The same URL may be reassigned; require an explicit reconnect before writing.
    agent.data.lock().unwrap()["agent_id"] = json!("replacement-agent");
    assert_eq!(
        f.request(
            "POST",
            &endpoint,
            Some(json!({"target_id":target,"expected_revision":2}))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(agent.data.lock().unwrap()["installation"]["revision"], 2);
    f.ok("DELETE", &format!("/api/v1/agent-targets/{target}"), None)
        .await;
    assert!(!secret.exists());
    assert_eq!(agent.data.lock().unwrap()["installation"]["revision"], 2);
}

#[tokio::test]
async fn agent_connector_does_not_follow_redirects_or_put_secrets_in_views_and_rejects_stale_profiles()
 {
    let f = Fixture::new("valid");
    let agent = AgentFixture::new().await;
    agent.data.lock().unwrap()["redirect"] = json!(true);
    let (status, _) = f
        .request("POST", "/api/v1/agent-targets", Some(agent.connect_body()))
        .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(
        agent.data.lock().unwrap()["calls"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        f.ok("GET", "/api/v1/agent-targets", None).await["items"],
        json!([])
    );
    agent.data.lock().unwrap()["redirect"] = json!(false);
    let linked = f
        .ok("POST", "/api/v1/agent-targets", Some(agent.connect_body()))
        .await;
    let target = linked["target"]["id"].as_str().unwrap();
    let p = f.project().await;
    let p = f.ok("POST",&format!("{}/observations",project_path(&p)),Some(json!({"expected_revision":p["revision"],"collection":"communication","statement":"Keep the opening brief."}))).await;
    let r = f
        .ok(
            "POST",
            &format!("{}/revisions", project_path(&p)),
            Some(json!({"expected_revision":p["revision"]})),
        )
        .await;
    let p = f.read(&p).await;
    f.ok(
        "PUT",
        &format!("{}/identity", project_path(&p)),
        Some(json!({"expected_revision":p["revision"],"bindings":{"account":"new private value"}})),
    )
    .await;
    let prior_calls = agent.data.lock().unwrap()["calls"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(
        f.request(
            "POST",
            &format!("{}/apply", revision_path(&p, &r)),
            Some(json!({"target_id":target,"expected_revision":0}))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        agent.data.lock().unwrap()["calls"]
            .as_array()
            .unwrap()
            .len(),
        prior_calls
    );
    let view = f.ok("POST", "/api/v1/views", Some(json!({}))).await;
    let endpoint = format!("/api/v1/views/{}/commands", view["id"].as_str().unwrap());
    for field in ["agent_token", "agent_ca", "agent_url"] {
        assert_eq!(f.request("POST",&endpoint,Some(json!({"expected_revision":1,"commands":[{"op":"set","field":"draft","value":{(field):"TEST_REMOTE_CREDENTIAL_123456789"}}]}))).await.0,StatusCode::BAD_REQUEST);
    }
    f.ok("POST",&endpoint,Some(json!({"expected_revision":1,"commands":[{"op":"set","field":"modal","value":"agent.apply"},{"op":"set","field":"draft","value":{"agent_target_id":target,"agent_history":true}}]}))).await;
    let (status, _) = request(&f.app, "GET", "/api/v1/agent-targets", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn blind_comparison_isolates_references_records_real_feedback_and_reveals_only_after_rating()
{
    let f = Fixture::new("normal");
    let p = f.project().await;
    let p = f.owner_preference(&p).await;
    let r = f.compile(&p).await;
    let p = f.read(&p).await;
    let p = f.ok("POST", &format!("{}/artifacts",project_path(&p)),Some(json!({"expected_revision":p["revision"],"name":"Untouched reference","kind":"notes","text":"HOLDOUT_CANARY_DO_NOT_GENERATE_FROM_THIS","authors":["Avery"],"holdout":true}))).await;
    let aid = p["artifacts"][0]["id"].clone();
    let start=f.ok("POST", &format!("{}/comparisons",project_path(&p)),Some(json!({"expected_revision":p["revision"],"scenario":"A fictional colleague asks for a reversible plan.","holdout_ids":[aid],"candidates":[{"revision_id":r["id"],"variant":"both"},{"revision_id":r["id"],"variant":"baseline"}]}))).await;
    let finished = f.wait(&json!({"id":start["job_id"]})).await;
    assert_eq!(finished["status"], "complete", "{finished}");
    let url = format!(
        "{}/comparisons/{}",
        project_path(&p),
        start["id"].as_str().unwrap()
    );
    let blind = f.ok("GET", &url, None).await;
    assert_eq!(blind["reference_mode"], "held_out_evidence");
    assert_eq!(
        blind["references"][0]["spans"][0]["text"],
        "HOLDOUT_CANARY_DO_NOT_GENERATE_FROM_THIS"
    );
    assert!(blind["rating"].is_null());
    for candidate in blind["candidates"].as_array().unwrap() {
        assert!(candidate.get("revision_id").is_none());
        assert!(candidate.get("variant").is_none());
        assert!(!candidate["response"].as_str().unwrap().is_empty());
    }
    let prompts = std::fs::read_dir(&f.endpoint)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("ready-"))
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(prompts.len(), 2);
    assert!(prompts.iter().all(|p| !p.contains("HOLDOUT_CANARY")));
    assert_eq!(
        prompts
            .iter()
            .filter(|p| p.contains("Profile variant: baseline"))
            .count(),
        1
    );
    assert_eq!(
        f.request("POST", &format!("{url}/reveal"), Some(json!({})))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.request(
            "PUT",
            &format!("{url}/rating"),
            Some(json!({"ratings":{"A":{"communication_fit":5,"working_fit":5}},"preference":"A"}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let feedback = json!({"ratings":{"A":{"communication_fit":3,"working_fit":4,"notes":"Synthetic test review"},"B":{"communication_fit":2,"working_fit":3}},"preference":"A","perspective":"self"});
    let rated = f
        .ok("PUT", &format!("{url}/rating"), Some(feedback.clone()))
        .await;
    assert_eq!(rated["rating"]["reviewer"], "token:local-control");
    assert_eq!(rated["rating"]["blind"], true);
    assert!(rated["candidates"][0].get("variant").is_none());
    let revealed = f
        .ok("POST", &format!("{url}/reveal"), Some(json!({})))
        .await;
    assert!(revealed["revealed_at"].is_i64());
    assert_eq!(revealed["calibrated"], false);
    assert!(revealed["candidates"][0]["variant"].is_string());
    assert_eq!(
        f.request("PUT", &format!("{url}/rating"), Some(feedback))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let other = f.project().await;
    assert_eq!(
        f.request(
            "GET",
            &format!(
                "{}/comparisons/{}",
                project_path(&other),
                start["id"].as_str().unwrap()
            ),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let p = f.read(&p).await;
    f.ok(
        "DELETE",
        &format!("{}/artifacts/{}", project_path(&p), aid.as_str().unwrap()),
        Some(json!({"expected_revision":p["revision"]})),
    )
    .await;
    let invalid = f.ok("GET", &url, None).await;
    assert_eq!(invalid["valid"], false);
    assert!(invalid["references"].as_array().unwrap().is_empty());
    assert_eq!(invalid["candidates"][0]["response"], "");
}

#[tokio::test]
async fn comparison_rejects_training_references_and_profile_diff_shows_instruction_changes() {
    let f = Fixture::new("normal");
    let p = f.project().await;
    let p = f
        .upload(
            &p,
            "Training",
            "notes",
            "Avery: Work from a concrete example.",
        )
        .await;
    let p = f.built(&p).await;
    let r = p["revisions"][0].clone();
    let aid = p["artifacts"][0]["id"].clone();
    let body = json!({"expected_revision":p["revision"],"scenario":"A fresh case","holdout_ids":[aid],"candidates":[{"revision_id":r["id"],"variant":"both"},{"revision_id":r["id"],"variant":"baseline"}]});
    assert_eq!(
        f.request(
            "POST",
            &format!("{}/comparisons", project_path(&p)),
            Some(body)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let p = f.owner_preference(&p).await;
    let next = f.compile(&p).await;
    let diff = f
        .ok(
            "GET",
            &format!(
                "{}/revision-diff/{}/{}",
                project_path(&p),
                r["id"].as_str().unwrap(),
                next["id"].as_str().unwrap()
            ),
            None,
        )
        .await;
    assert!(
        diff["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["changed"] == true && !f["added_paragraphs"].as_array().unwrap().is_empty())
    );
}

#[tokio::test]
async fn richer_import_excludes_quoted_email_and_requires_recording_speaker_identification() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let f = Fixture::new("normal");
    let p = f.project().await;
    let mail = "From: Avery <avery@example.test>\r\nTo: Peer <peer@example.test>\r\nSubject: Small plan\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nLet's try one small example first.\r\n\r\n> A peer's older words.\r\n\r\nOn Monday, Peer wrote:\r\nTheir earlier plan is not Avery's writing.\r\n";
    let p=f.ok("POST",&format!("{}/file-import",project_path(&p)),Some(json!({"expected_revision":p["revision"],"filename":"example.eml","content_base64":STANDARD.encode(mail),"collections":["communication","working"]}))).await;
    let a = f
        .ok(
            "GET",
            &format!(
                "{}/artifacts/{}",
                project_path(&p),
                p["artifacts"][0]["id"].as_str().unwrap()
            ),
            None,
        )
        .await;
    assert_eq!(a["kind"], "email");
    assert_eq!(a["selected_count"], 1);
    assert!(
        a["spans"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["quoted"] == true)
    );
    let transcript=json!({"segments":[{"speaker":"speaker_0","start":0.0,"end":1.5,"text":"A recording contribution."},{"speaker":"speaker_1","start":1.6,"end":2.3,"text":"A different person."}]}).to_string();
    let p=f.ok("POST",&format!("{}/file-import",project_path(&p)),Some(json!({"expected_revision":p["revision"],"filename":"meeting.transcript.json","content_base64":STANDARD.encode(transcript),"authors":["speaker_0"],"collections":["working"]}))).await;
    let record = p["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == "recording")
        .unwrap();
    assert_eq!(
        record["selected_count"], 0,
        "Imported diarization labels must not auto-identify a person"
    );
    let p = f
        .ok(
            "PUT",
            &format!(
                "{}/artifacts/{}",
                project_path(&p),
                record["id"].as_str().unwrap()
            ),
            Some(json!({"expected_revision":p["revision"],"authors":["speaker_0"]})),
        )
        .await;
    assert_eq!(p["artifacts"][1]["selected_count"], 1);
    let (status,error)=f.request("POST",&format!("{}/file-import",project_path(&p)),Some(json!({"expected_revision":p["revision"],"filename":"meeting.wav","content_base64":STANDARD.encode(b"synthetic recording") }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("transcription provider")
    );
}

#[tokio::test]
async fn configured_recording_provider_receives_private_file_and_output_is_unassigned_until_review()
{
    use base64::{Engine, engine::general_purpose::STANDARD};
    let mut f = Fixture::new("normal");
    let mut config = f.state.config.clone();
    config.transcription=Some(Extractor{command:"python3".into(),args:vec!["-c".into(),"import sys,json,pathlib; r=json.load(sys.stdin); assert r['schema']=='persona-transcription/1'; assert pathlib.Path(r['media_path']).read_bytes()==b'SYNTHETIC_MEDIA'; print(json.dumps({'segments':[{'speaker':'speaker_1','start':0,'end':1,'text':'Synthetic attributed phrase.'}]}))".into()],cwd:f.state.home.to_string_lossy().into_owned(),env:Default::default(),timeout_seconds:10});
    config.save(&f.state.home).unwrap();
    f.state = AppState::open(f.state.home.clone()).unwrap();
    f.app = server::router(f.state.clone());
    let p = f.project().await;
    let p=f.ok("POST",&format!("{}/file-import",project_path(&p)),Some(json!({"expected_revision":p["revision"],"filename":"meeting.wav","content_base64":STANDARD.encode(b"SYNTHETIC_MEDIA"),"collections":["communication","working"]}))).await;
    assert_eq!(p["artifacts"][0]["kind"], "recording");
    assert_eq!(p["artifacts"][0]["selected_count"], 0);
    let detail = f
        .ok(
            "GET",
            &format!(
                "{}/artifacts/{}",
                project_path(&p),
                p["artifacts"][0]["id"].as_str().unwrap()
            ),
            None,
        )
        .await;
    assert_eq!(detail["spans"][0]["start_seconds"], 0.0);
    assert_eq!(detail["spans"][0]["end_seconds"], 1.0);
    assert_eq!(
        p["artifacts"][0]["collections"],
        json!(["communication", "working"])
    );
    assert!(
        std::fs::read_dir(&f.state.home)
            .unwrap()
            .filter_map(Result::ok)
            .all(|e| !e.file_name().to_string_lossy().starts_with(".recording-"))
    );
}

#[tokio::test]
async fn scoped_observation_credentials_cannot_edit_or_mint_and_revocation_is_immediate() {
    let f = Fixture::new("normal");
    let issued = f
        .ok(
            "POST",
            "/api/v1/tokens",
            Some(json!({"scopes":["observe"],"expires_in_seconds":60})),
        )
        .await;
    let token = issued["token"].as_str().unwrap();
    assert_eq!(
        request(&f.app, "GET", "/api/v1/status", Some(token), None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &f.app,
            "POST",
            "/api/v1/tokens",
            Some(token),
            Some(json!({"scopes":["admin"]}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let listed = f.ok("GET", "/api/v1/tokens", None).await;
    assert!(!listed.to_string().contains(token));
    assert_eq!(listed["tokens"][0]["id"], issued["id"]);
    f.ok(
        "DELETE",
        &format!("/api/v1/tokens/{}", issued["id"].as_str().unwrap()),
        None,
    )
    .await;
    assert_eq!(
        request(&f.app, "GET", "/api/v1/status", Some(token), None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}
