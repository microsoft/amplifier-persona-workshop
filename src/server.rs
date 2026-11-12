use crate::{
    agents, auth, calibration,
    error::{ApiError, ApiResult},
    evidence, imports, jobs,
    model::*,
    portable, profiles,
    state::{AppState, now_ms},
    views,
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};
use std::sync::Arc;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/markdown-notices.txt",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                    include_str!("../ui/MARKDOWN-NOTICES.txt"),
                )
            }),
        )
        .route(
            "/markdown.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    concat!(
                        include_str!("../ui/markdown.js"),
                        include_str!("../ui/markdown-renderer.js")
                    ),
                )
            }),
        )
        .route(
            "/",
            get(|| async { Html(include_str!("../ui/index.html")) }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../ui/app.js"),
                )
            }),
        )
        .route(
            "/app.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    concat!(
                        include_str!("../ui/app.css"),
                        "\n",
                        include_str!("../ui/markdown.css")
                    ),
                )
            }),
        )
        .route(
            "/health",
            get(|| async { Json(json!({"ok":true,"version":env!("CARGO_PKG_VERSION")})) }),
        )
        .route("/auth/login", post(auth::login))
        .route("/auth/first-account", post(auth::first_account))
        .route("/auth/session", get(auth::session))
        .route("/auth/logout", post(auth::logout))
        .route("/api/v1/descriptor", get(descriptor))
        .route("/api/v1/local-proof", get(auth::local_proof))
        .route(
            "/api/v1/startup",
            get(crate::desktop::startup_get).post(crate::desktop::startup_set),
        )
        .route("/api/v1/status", get(status))
        .route(
            "/api/v1/desktop/updates",
            get(crate::native_update::get).post(crate::native_update::command),
        )
        .route(
            "/api/v1/desktop/update-readiness",
            get(crate::native_update::readiness),
        )
        .route(
            "/api/v1/desktop/update-stop",
            post(crate::native_update::stop),
        )
        .route("/api/v1/tokens", get(tokens).post(create_token))
        .route("/api/v1/tokens/{id}", axum::routing::delete(revoke_token))
        .route("/api/v1/activity", get(activity))
        .route(
            "/api/v1/agent-targets",
            get(agents::list).post(agents::connect),
        )
        .route(
            "/api/v1/agent-targets/{id}",
            get(agents::status).delete(agents::forget),
        )
        .route("/api/v1/agent-targets/{id}/restore", post(agents::restore))
        .route("/api/v1/projects", get(list).post(create))
        .route("/api/v1/projects/{id}", get(read).put(edit))
        .route("/api/v1/projects/{id}/artifacts", post(import))
        .route("/api/v1/import-capabilities", get(imports::capabilities))
        .route("/api/v1/projects/{id}/file-import", post(imports::upload))
        .route(
            "/api/v1/projects/{id}/comparisons",
            get(calibration::list).post(calibration::start),
        )
        .route(
            "/api/v1/projects/{id}/comparisons/{cid}",
            get(calibration::get),
        )
        .route(
            "/api/v1/projects/{id}/comparisons/{cid}/rating",
            axum::routing::put(calibration::rate),
        )
        .route(
            "/api/v1/projects/{id}/comparisons/{cid}/reveal",
            post(calibration::reveal),
        )
        .route(
            "/api/v1/projects/{id}/revision-diff/{from}/{to}",
            get(calibration::diff),
        )
        .route(
            "/api/v1/projects/{id}/artifacts/{aid}",
            get(artifact).put(edit_artifact).delete(delete_artifact),
        )
        .route("/api/v1/projects/{id}/evidence/{sid}", get(evidence_span))
        .route("/api/v1/projects/{id}/builds", post(build))
        .route("/api/v1/jobs/{jid}", get(job))
        .route("/api/v1/jobs/{jid}/cancel", post(cancel))
        .route("/api/v1/projects/{id}/observations", post(add_observation))
        .route(
            "/api/v1/projects/{id}/observations/{oid}",
            axum::routing::put(edit_observation),
        )
        .route("/api/v1/projects/{id}/revisions", post(compile))
        .route("/api/v1/projects/{id}/revisions/{rid}", get(revision))
        .route("/api/v1/projects/{id}/revisions/{rid}/export", get(export))
        .route(
            "/api/v1/projects/{id}/revisions/{rid}/exports/{format}",
            get(portable::get).post(portable::merge),
        )
        .route(
            "/api/v1/projects/{id}/revisions/{rid}/exports/{format}/download",
            get(portable::download).post(portable::merge_download),
        )
        .route(
            "/api/v1/projects/{id}/revisions/{rid}/apply",
            post(agents::apply),
        )
        .route(
            "/api/v1/projects/{id}/identity",
            get(identity_get).put(identity_put),
        )
        .route("/api/v1/projects/{id}/evaluations", post(evaluate))
        .route(
            "/api/v1/projects/{id}/evaluations/{eid}",
            axum::routing::put(rate),
        )
        .route("/api/v1/views", get(view_list).post(view_create))
        .route("/api/v1/views/{id}", get(view_read))
        .route("/api/v1/views/{id}/commands", post(view_commands))
        .route("/api/v1/views/{id}/ack", post(view_ack))
        .layer(DefaultBodyLimit::max(30 * 1024 * 1024))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::native_update::admission,
        ))
        .layer(middleware::from_fn_with_state(state.clone(), headers))
        .with_state(state)
}
async fn headers(
    State(state): State<Arc<AppState>>,
    mut request: axum::extract::Request,
    next: Next,
) -> Response {
    if !request.headers().contains_key(header::HOST)
        && let Some(a) = request.uri().authority().map(|a| a.as_str().to_owned())
        && let Ok(v) = a.parse()
    {
        request.headers_mut().insert(header::HOST, v);
    }
    if state.config.web_transport == crate::config::WebTransport::LocalHttp {
        let headers = request.headers();
        let expected = state.config.public_url.trim_start_matches("http://");
        if headers.get(header::HOST).and_then(|v| v.to_str().ok()) != Some(expected)
            || headers
                .get(header::ORIGIN)
                .is_some_and(|v| v.to_str().ok() != Some(state.config.public_url.as_str()))
            || headers
                .get("sec-fetch-site")
                .is_some_and(|v| v != "same-origin" && v != "none")
        {
            return ApiError::forbidden().into_response();
        }
    }
    let mut r = next.run(request).await;
    let h = r.headers_mut();
    h.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    h.insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    h.insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    let embed = state.config.embed_origins.join(" ");
    h.insert(header::CONTENT_SECURITY_POLICY,format!("default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; object-src 'none'; base-uri 'none'; frame-ancestors 'self' {embed}; form-action 'self'").parse().unwrap());
    r
}
type S = State<Arc<AppState>>;
fn expected(v: &Value) -> ApiResult<u64> {
    v["expected_revision"]
        .as_u64()
        .ok_or_else(|| ApiError::bad("Supply the current project revision"))
}
fn strings(v: &Value) -> ApiResult<Vec<String>> {
    serde_json::from_value(v.clone()).map_err(|_| ApiError::bad("Use an array of text values"))
}
fn body_string(v: &Value, k: &str, max: usize) -> ApiResult<String> {
    let s = v[k].as_str().unwrap_or("").trim();
    if s.is_empty() || s.len() > max {
        return Err(ApiError::bad(format!("Enter a valid {k}")));
    }
    Ok(s.into())
}
fn snapshot(s: &AppState, p: &Project) -> ApiResult<Json<Value>> {
    Ok(Json(s.snapshot(p)?))
}
async fn descriptor(State(s): S, h: HeaderMap) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "observe", false)?;
    Ok(Json(
        json!({"name":"Persona Workshop","version":env!("CARGO_PKG_VERSION"),"kind":"evidence-to-profile","package_schema":"twin-profile/1","capabilities":["text-import","document-import","recording-provider","blind-comparison","revision-diff","speaker-attribution","two-collections","evidence-review","profile-export","agents-md-export","soul-md-export","agents-md-merge","agent-profile-management","simulation-evaluation","semantic-views"],"ui":"./","embed":"?embed=1","api":"api/v1","contract":"persona-workshop.management/1"}),
    ))
}
async fn status(State(s): S, h: HeaderMap) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "observe", false)?;
    let ready = s.config.extractor.is_some();
    Ok(Json(
        json!({"id":s.config.id,"pid":std::process::id(),"version":env!("CARGO_PKG_VERSION"),"extractor_ready":ready,"extractor":{"ready":ready,"note":if ready{"ACP analysis endpoint configured"}else{"Configure an ACP endpoint using the installer or config file"}},"owner":s.config.owner,"public_url":s.config.public_url,"calibrated":false}),
    ))
}
async fn activity(State(s): S, h: HeaderMap) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "observe", false)?;
    let db = s.db.lock().unwrap();
    let mut stmt =
        db.prepare("SELECT seq,at,kind,project_id,data FROM audit ORDER BY seq DESC LIMIT 100")?;
    let items=stmt.query_map([],|r|Ok(json!({"seq":r.get::<_,i64>(0)?,"at":r.get::<_,i64>(1)?,"kind":r.get::<_,String>(2)?,"project_id":r.get::<_,Option<String>>(3)?,"data":serde_json::from_str::<Value>(&r.get::<_,String>(4)?).unwrap_or(Value::Null)})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Json(json!({"items":items})))
}
async fn list(State(s): S, h: HeaderMap) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "observe", false)?;
    Ok(Json(json!({"items":s.list()?})))
}
async fn create(State(s): S, h: HeaderMap, Json(v): Json<Value>) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "edit", true)?;
    let p = s.create(
        body_string(&v, "alias", 160)?,
        v["purpose"].as_str().unwrap_or("").into(),
        strings(v.get("aliases").unwrap_or(&json!([])))?,
        v["subject_type"].as_str().unwrap_or("person").into(),
    )?;
    snapshot(&s, &p)
}
async fn read(State(s): S, h: HeaderMap, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "observe", false)?;
    snapshot(&s, &s.project(&id)?)
}
async fn edit(
    State(s): S,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(v): Json<Value>,
) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "edit", true)?;
    let p = s.change(&id, expected(&v)?, |p| {
        if let Some(x) = v.get("alias") {
            p.alias = x
                .as_str()
                .ok_or_else(|| ApiError::bad("Invalid alias"))?
                .into();
        }
        if let Some(x) = v.get("purpose") {
            p.purpose = x
                .as_str()
                .ok_or_else(|| ApiError::bad("Invalid purpose"))?
                .into();
        }
        if let Some(x) = v.get("aliases") {
            p.aliases = strings(x)?;
        }
        crate::state::validate_project(&p.alias, &p.purpose, &p.aliases)
    })?;
    s.invalidate(&p)?;
    snapshot(&s, &p)
}
async fn import(
    State(s): S,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(v): Json<Value>,
) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "edit", true)?;
    let p = s.project(&id)?;
    let authors = if let Some(a) = v.get("authors") {
        strings(a)?
    } else {
        p.aliases.clone()
    };
    let artifact = evidence::import(
        body_string(&v, "name", 240)?,
        body_string(&v, "kind", 30)?,
        body_string(&v, "text", 2 * 1024 * 1024)?,
        v.get("collections").map(strings).transpose()?,
        authors,
        v["holdout"] == true,
    )
    .map_err(|e| ApiError::bad(e.to_string()))?;
    let aid = artifact.id.clone();
    let p = s.change(&id, expected(&v)?, |p| {
        if p.artifacts.len() >= 200 {
            return Err(ApiError::bad(
                "This project has reached its 200-source limit",
            ));
        }
        if p.artifacts
            .iter()
            .any(|a| !a.deleted && a.sha256 == artifact.sha256)
        {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "This source is already present. Change its collections to reuse it.".into(),
            ));
        }
        p.artifacts.push(artifact);
        Ok(())
    })?;
    s.audit("source_imported", Some(&id), json!({"artifact_id":aid}))?;
    snapshot(&s, &p)
}
async fn artifact(
    State(s): S,
    h: HeaderMap,
    Path((id, aid)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "observe", false)?;
    let p = s.project(&id)?;
    let a = p
        .artifacts
        .iter()
        .find(|a| a.id == aid && !a.deleted)
        .ok_or_else(ApiError::missing)?;
    let mut v = evidence::summary(a);
    v["spans"] = json!(a.spans);
    if a.kind == "recording"
        && let Ok(raw) = serde_json::from_str::<Value>(&a.raw)
        && let (Some(messages), Some(spans)) =
            (raw["messages"].as_array(), v["spans"].as_array_mut())
    {
        for (span, message) in spans.iter_mut().zip(messages) {
            span["start_seconds"] = message["start"].clone();
            span["end_seconds"] = message["end"].clone();
        }
    }
    Ok(Json(v))
}
async fn edit_artifact(
    State(s): S,
    h: HeaderMap,
    Path((id, aid)): Path<(String, String)>,
    Json(v): Json<Value>,
) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "edit", true)?;
    let p = s.change(&id, expected(&v)?, |p| {
        let a = p
            .artifacts
            .iter_mut()
            .find(|a| a.id == aid && !a.deleted)
            .ok_or_else(ApiError::missing)?;
        if let Some(c) = v.get("collections") {
            let list = strings(c)?;
            evidence::collections(&list).map_err(|e| ApiError::bad(e.to_string()))?;
            a.collections = list;
        }
        if let Some(c) = v.get("authors") {
            let list = strings(c)?;
            if list.len() > 100
                || list
                    .iter()
                    .any(|label| !a.spans.iter().any(|s| &s.speaker == label))
            {
                return Err(ApiError::bad("Choose existing speaker labels"));
            }
            a.authors = list;
        }
        if let Some(c) = v.get("selected_span_ids") {
            a.selected_span_ids = if c.is_null() {
                None
            } else {
                let ids = strings(c)?;
                if ids.iter().any(|id| !a.spans.iter().any(|s| &s.id == id)) {
                    return Err(ApiError::bad("Unknown evidence span"));
                }
                Some(ids)
            };
        }
        if let Some(c) = v.get("holdout") {
            a.holdout = c
                .as_bool()
                .ok_or_else(|| ApiError::bad("Holdout must be true or false"))?;
        }
        profiles::refresh_validity(p);
        Ok(())
    })?;
    s.invalidate(&p)?;
    s.audit(
        "source_selection_changed",
        Some(&id),
        json!({"artifact_id":aid}),
    )?;
    snapshot(&s, &p)
}
async fn delete_artifact(
    State(s): S,
    h: HeaderMap,
    Path((id, aid)): Path<(String, String)>,
    Json(v): Json<Value>,
) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "edit", true)?;
    let p = s.change(&id, expected(&v)?, |p| {
        let a = p
            .artifacts
            .iter_mut()
            .find(|a| a.id == aid && !a.deleted)
            .ok_or_else(ApiError::missing)?;
        a.deleted = true;
        a.raw.clear();
        a.spans.clear();
        profiles::refresh_validity(p);
        Ok(())
    })?;
    s.invalidate(&p)?;
    s.audit(
        "source_deleted",
        Some(&id),
        json!({"artifact_id":aid,"exports_may_need_replacement":true}),
    )?;
    snapshot(&s, &p)
}
async fn evidence_span(
    State(s): S,
    h: HeaderMap,
    Path((id, sid)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "observe", false)?;
    let p = s.project(&id)?;
    for a in p.artifacts.iter().filter(|a| !a.deleted) {
        if let Some(span) = a.spans.iter().find(|x| x.id == sid) {
            return Ok(Json(
                json!({"artifact":{"id":a.id,"name":a.name,"kind":a.kind},"span":span}),
            ));
        }
    }
    Err(ApiError::missing())
}
async fn build(
    State(s): S,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(v): Json<Value>,
) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "build", true)?;
    let p = s.project(&id)?;
    Ok(Json(jobs::start(s, p, expected(&v)?, None)?))
}
async fn job(State(s): S, h: HeaderMap, Path(jid): Path<String>) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "observe", false)?;
    Ok(Json(s.record("jobs", &jid)?))
}
async fn cancel(State(s): S, h: HeaderMap, Path(jid): Path<String>) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "build", true)?;
    Ok(Json(jobs::cancel(&s, &jid)?))
}
async fn add_observation(
    State(s): S,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(v): Json<Value>,
) -> ApiResult<Json<Value>> {
    let who = auth::check(&s, &h, "edit", true)?;
    let c = body_string(&v, "collection", 20)?;
    evidence::collections(std::slice::from_ref(&c)).map_err(|e| ApiError::bad(e.to_string()))?;
    let section = match v.get("section") {
        None => String::new(),
        Some(value) => value
            .as_str()
            .ok_or_else(|| ApiError::bad("Invalid section"))?
            .to_owned(),
    };
    profiles::validate_section(&c, &section)?;
    let o = Observation {
        id: crate::model::id(),
        collection: c,
        section,
        statement: body_string(&v, "statement", 3000)?,
        scope: if v.get("scope").is_some() {
            body_string(&v, "scope", 1000)?
        } else {
            "Selected contexts".into()
        },
        confidence: Confidence {
            attribution: "confirmed".into(),
            support: "limited".into(),
            generalization: "narrow".into(),
            reason: "Operator-specified preference; not inferred from source evidence".into(),
        },
        evidence_ids: vec![],
        conflicts: vec![],
        status: "accepted".into(),
        source: "owner".into(),
        valid: true,
        reviewer: Some(who.actor),
    };
    let p = s.change(&id, expected(&v)?, |p| {
        if p.observations.len() >= 200 {
            return Err(ApiError::bad("Too many observations"));
        }
        p.observations.push(o);
        Ok(())
    })?;
    snapshot(&s, &p)
}
async fn edit_observation(
    State(s): S,
    h: HeaderMap,
    Path((id, oid)): Path<(String, String)>,
    Json(v): Json<Value>,
) -> ApiResult<Json<Value>> {
    let who = auth::check(&s, &h, "edit", true)?;
    let p = s.change(&id, expected(&v)?, |p| {
        let o = p
            .observations
            .iter_mut()
            .find(|o| o.id == oid)
            .ok_or_else(ApiError::missing)?;
        if v.get("statement").is_some() {
            o.statement = body_string(&v, "statement", 3000)?;
        }
        if let Some(section) = v.get("section") {
            let section = section
                .as_str()
                .ok_or_else(|| ApiError::bad("Invalid section"))?;
            profiles::validate_section(&o.collection, section)?;
            o.section = section.into();
        }
        if v.get("scope").is_some() {
            o.scope = body_string(&v, "scope", 1000)?;
        }
        if v.get("status").is_some() {
            let status = body_string(&v, "status", 20)?;
            if !["pending", "accepted", "rejected"].contains(&status.as_str()) {
                return Err(ApiError::bad("Invalid review status"));
            }
            o.status = status;
        }
        if let Some(c) = v.get("confidence") {
            o.confidence = serde_json::from_value(c.clone())
                .map_err(|_| ApiError::bad("Invalid confidence"))?;
            profiles::validate_confidence(&o.confidence)?;
        }
        o.reviewer = Some(who.actor);
        Ok(())
    })?;
    s.audit(
        "observation_reviewed",
        Some(&id),
        json!({"observation_id":oid}),
    )?;
    snapshot(&s, &p)
}
async fn compile(
    State(s): S,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(v): Json<Value>,
) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "export", true)?;
    let p = s.project(&id)?;
    if p.revision != expected(&v)? {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Refresh this project before compiling".into(),
        ));
    }
    let revision = profiles::compile(&p)?;
    s.change_record(
        &id,
        p.revision,
        Some(("revisions", &json!(revision))),
        |_| Ok(()),
    )?;
    s.audit(
        "profiles_compiled",
        Some(&id),
        json!({"revision_id":revision.id,"status":"draft"}),
    )?;
    Ok(Json(json!(revision)))
}
pub(crate) fn get_revision(s: &AppState, id: &str, rid: &str) -> ApiResult<Revision> {
    let r: Revision = serde_json::from_value(s.record("revisions", rid)?)
        .map_err(|_| ApiError::bad("Invalid revision"))?;
    if r.project_id != id {
        return Err(ApiError::missing());
    }
    if !r.valid {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Source evidence changed. Build replacement profiles before using this revision."
                .into(),
        ));
    }
    let p = s.project(id)?;
    if !profiles::revision_current(&p, &r.manifest) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This revision depends on changed evidence or private identity settings. Create replacement profiles.".into(),
        ));
    }
    Ok(r)
}
async fn revision(
    State(s): S,
    h: HeaderMap,
    Path((id, rid)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "observe", false)?;
    Ok(Json(json!(get_revision(&s, &id, &rid)?)))
}
async fn export(
    State(s): S,
    h: HeaderMap,
    Path((id, rid)): Path<(String, String)>,
) -> ApiResult<Response> {
    auth::check(&s, &h, "export", false)?;
    let r = get_revision(&s, &id, &rid)?;
    let package = json!({"schema":"twin-profile/1","manifest":r.manifest,"files":r.files});
    Ok((
        [
            (
                header::CONTENT_TYPE,
                "application/json; charset=utf-8".to_owned(),
            ),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"twin-{}.twin.json\"", r.id),
            ),
        ],
        serde_json::to_vec_pretty(&package).unwrap(),
    )
        .into_response())
}
async fn identity_get(State(s): S, h: HeaderMap, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "identity", false)?;
    Ok(Json(json!({"bindings":s.project(&id)?.identity})))
}
async fn identity_put(
    State(s): S,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(v): Json<Value>,
) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "identity", true)?;
    let bindings: std::collections::BTreeMap<String, String> =
        serde_json::from_value(v["bindings"].clone())
            .map_err(|_| ApiError::bad("Identity bindings must be named text fields"))?;
    if bindings.len() > 50
        || bindings
            .iter()
            .any(|(k, v)| k.len() > 100 || v.len() > 1000)
    {
        return Err(ApiError::bad("Identity details exceed their limits"));
    }
    let p = s.change(&id, expected(&v)?, |p| {
        p.identity = bindings;
        Ok(())
    })?;
    s.invalidate(&p)?;
    Ok(Json(json!({"bindings":p.identity,"revision":p.revision})))
}
async fn evaluate(
    State(s): S,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(v): Json<Value>,
) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "build", true)?;
    let rid = body_string(&v, "revision_id", 100)?;
    let revision = get_revision(&s, &id, &rid)?;
    let p = s.project(&id)?;
    let expected = v["expected_revision"].as_u64().unwrap_or(p.revision);
    let input = jobs::EvaluationInput {
        revision,
        scenario: body_string(&v, "scenario", 12000)?,
        variant: body_string(&v, "variant", 30)?,
    };
    Ok(Json(jobs::start(s, p, expected, Some(input))?))
}
async fn rate(
    State(s): S,
    h: HeaderMap,
    Path((id, eid)): Path<(String, String)>,
    Json(v): Json<Value>,
) -> ApiResult<Json<Value>> {
    let who = auth::check(&s, &h, "edit", true)?;
    let all = s.records("evaluations", &id)?;
    let mut e = all
        .into_iter()
        .find(|e| e["id"] == eid)
        .ok_or_else(ApiError::missing)?;
    let rating = &v["rating"];
    if !rating.is_object()
        || rating["notes"].as_str().is_some_and(|x| x.len() > 4000)
        || ["communication_fit", "working_fit"].iter().any(|k| {
            rating
                .get(k)
                .is_some_and(|v| !v.is_null() && !v.as_u64().is_some_and(|x| (1..=5).contains(&x)))
        })
    {
        return Err(ApiError::bad("Use ratings from 1 to 5 and a short note"));
    }
    e["rating"] = rating.clone();
    e["rating"]["reviewer"] = json!(who.actor);
    let p = s.change_record(&id, expected(&v)?, Some(("evaluations", &e)), |_| Ok(()))?;
    snapshot(&s, &p)
}
async fn view_list(State(s): S, h: HeaderMap) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "ui", false)?;
    let db = s.db.lock().unwrap();
    let mut stmt = db.prepare("SELECT value FROM views ORDER BY rowid DESC LIMIT 100")?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Json(
        json!({"items":rows.iter().filter_map(|x|serde_json::from_str::<Value>(x).ok()).collect::<Vec<_>>()}),
    ))
}
async fn view_create(State(s): S, h: HeaderMap) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "ui", true)?;
    Ok(Json(views::create(&s)?))
}
async fn view_read(State(s): S, h: HeaderMap, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "ui", false)?;
    Ok(Json(views::read(&s, &id)?))
}
async fn view_commands(
    State(s): S,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(v): Json<Value>,
) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "ui", true)?;
    Ok(Json(views::commands(&s, &id, &v)?))
}
async fn view_ack(
    State(s): S,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(v): Json<Value>,
) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "ui", true)?;
    Ok(Json(views::ack(
        &s,
        &id,
        v["revision"]
            .as_u64()
            .ok_or_else(|| ApiError::bad("Supply the rendered revision"))?,
    )?))
}

pub async fn serve(home: std::path::PathBuf) -> anyhow::Result<()> {
    serve_with_browser(home, false, None).await
}
pub async fn serve_with_browser(
    home: std::path::PathBuf,
    open_browser: bool,
    listener: Option<std::net::TcpListener>,
) -> anyhow::Result<()> {
    use fs2::FileExt;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(home.join("service.lock"))?;
    lock.try_lock_exclusive()
        .map_err(|_| anyhow::anyhow!("This workbench is already running"))?;
    amplifier_native_platform::protect_file(&home.join("service.lock"))?;
    let c = crate::config::Config::load(&home)?;
    let listener = match listener {
        Some(listener) => listener,
        None => std::net::TcpListener::bind((c.bind.as_str(), c.port))?,
    };
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let s = AppState::open(home)?;
    let local = s.config.web_transport == crate::config::WebTransport::LocalHttp;
    let scheme = if local { "http" } else { "https" };
    let _ = rustls::crypto::ring::default_provider().install_default();
    crate::config::private_write(
        &s.home.join("runtime.json"),
        &serde_json::to_vec_pretty(
            &json!({"id":s.config.id,"port":port,"url":format!("{scheme}://127.0.0.1:{port}"),"public_url":s.config.public_url,"version":env!("CARGO_PKG_VERSION"),"started_at":now_ms()}),
        )?,
    )?;
    println!("Persona Workshop is listening on {scheme} port {port}");
    if !s.config.public_url.is_empty() {
        println!("Open {}", s.config.public_url);
    }
    if open_browser {
        crate::desktop::ready(&s.home, &s.config, true)?;
    }
    if local {
        if !open_browser {
            crate::desktop::ready(&s.home, &s.config, false)?;
        }
        s.update_shutdown_available
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let shutdown = s.update_shutdown.clone();
        axum::serve(tokio::net::TcpListener::from_std(listener)?, router(s))
            .with_graceful_shutdown(async move { shutdown.notified().await })
            .await?;
        return Ok(());
    }
    let tls =
        axum_server::tls_rustls::RustlsConfig::from_pem_file(&s.config.tls_cert, &s.config.tls_key)
            .await?;
    axum_server::from_tcp_rustls(listener, tls)?
        .serve(router(s).into_make_service())
        .await?;
    Ok(())
}

async fn tokens(State(s): S, h: HeaderMap) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "admin", false)?;
    let db = s.db.lock().unwrap();
    let mut q = db.prepare("SELECT id,scopes,expires_at FROM tokens WHERE id!='local-control' ORDER BY expires_at DESC")?;
    let rows = q.query_map([], |r| Ok(json!({"id":r.get::<_,String>(0)?,"scopes":serde_json::from_str::<Value>(&r.get::<_,String>(1)?).unwrap_or(json!([])),"expires_at":r.get::<_,i64>(2)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Json(json!({"tokens":rows})))
}
async fn create_token(
    State(s): S,
    h: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&s, &h, "admin", true)?.actor;
    let scopes: Vec<String> = serde_json::from_value(body["scopes"].clone())
        .map_err(|_| ApiError::bad("Choose token scopes"))?;
    let seconds = body["expires_in_seconds"].as_u64().unwrap_or(30 * 86400);
    if scopes.is_empty()
        || scopes.iter().any(|x| {
            ![
                "admin", "observe", "edit", "build", "export", "identity", "ui",
            ]
            .contains(&x.as_str())
        })
        || !(60..=366 * 86400).contains(&seconds)
    {
        return Err(ApiError::bad(
            "Choose supported scopes and a lifetime from one minute to 366 days",
        ));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let value = auth::random_token();
    let expires = now_ms() + seconds as i64 * 1000;
    s.db.lock().unwrap().execute(
        "INSERT INTO tokens(hash,id,scopes,expires_at) VALUES(?1,?2,?3,?4)",
        rusqlite::params![auth::digest(&value), id, json!(scopes).to_string(), expires],
    )?;
    s.audit(
        "access.issued",
        None,
        json!({"actor":actor,"id":id,"scopes":scopes}),
    )?;
    Ok(Json(
        json!({"id":id,"scopes":scopes,"expires_at":expires,"token":value}),
    ))
}
async fn revoke_token(State(s): S, h: HeaderMap, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let actor = auth::check(&s, &h, "admin", true)?.actor;
    if id == "local-control" {
        return Err(ApiError::bad(
            "The local installation credential cannot be revoked here",
        ));
    }
    s.db.lock()
        .unwrap()
        .execute("DELETE FROM tokens WHERE id=?1", [&id])?;
    s.audit("access.revoked", None, json!({"actor":actor,"id":id}))?;
    Ok(Json(json!({"revoked":true})))
}
