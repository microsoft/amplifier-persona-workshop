//! Optional management connector. Profile installation is deliberately outside ACP.
use crate::{
    auth,
    config::{private_write, valid_origin},
    error::{ApiError, ApiResult},
    model::id,
    state::{AppState, now_ms},
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use reqwest::{Client, Method};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

#[derive(Clone, Serialize, Deserialize)]
pub struct Target {
    pub id: String,
    pub url: String,
    pub name: String,
    pub agent_id: String,
    pub created_at: i64,
}
#[derive(Serialize, Deserialize)]
struct Credential {
    token: String,
    #[serde(default)]
    ca_pem: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connect {
    url: String,
    token: String,
    #[serde(default)]
    ca_pem: String,
}
fn credential_path(s: &AppState, target: &Target) -> ApiResult<std::path::PathBuf> {
    uuid::Uuid::parse_str(&target.id).map_err(|_| ApiError::bad("Invalid connection identity"))?;
    Ok(s.home
        .join("secrets")
        .join(format!("agent-target-{}.json", target.id)))
}
fn read(s: &AppState, id: &str) -> ApiResult<Target> {
    let raw: Option<String> =
        s.db.lock()
            .unwrap()
            .query_row("SELECT value FROM agent_targets WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .optional()?;
    serde_json::from_str(&raw.ok_or_else(ApiError::missing)?)
        .map_err(|_| ApiError::bad("Invalid agent connection"))
}
fn credential(s: &AppState, target: &Target) -> ApiResult<Credential> {
    serde_json::from_slice(&std::fs::read(credential_path(s, target)?)?)
        .map_err(|_| ApiError::bad("Reconnect this agent to replace its credential"))
}
fn client(c: &Credential) -> ApiResult<Client> {
    let mut builder = Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(25));
    if !c.ca_pem.is_empty() {
        let certificate = reqwest::Certificate::from_pem(c.ca_pem.as_bytes())
            .map_err(|_| ApiError::bad("Use the agent installation's PEM root certificate"))?;
        builder = builder.add_root_certificate(certificate);
    }
    builder
        .build()
        .map_err(|_| ApiError::bad("Could not prepare the secure agent connection"))
}
fn validate_input(v: &Connect) -> ApiResult<()> {
    if v.url.len() > 2048 || !valid_origin(&v.url) {
        return Err(ApiError::bad(
            "Use the agent's HTTPS management address, without a path or credentials",
        ));
    }
    if v.token.trim().len() < 16
        || v.token.len() > 8192
        || v.token.trim().chars().any(char::is_whitespace)
    {
        return Err(ApiError::bad(
            "Enter the management access token from the agent",
        ));
    }
    if v.ca_pem.len() > 32_000 {
        return Err(ApiError::bad("The certificate is too large"));
    }
    Ok(())
}
async fn request(
    client: &Client,
    target: &Target,
    c: &Credential,
    method: Method,
    endpoint: &str,
    body: Option<Value>,
) -> ApiResult<Value> {
    let mut request = client
        .request(
            method,
            format!("{}/api/v1/{endpoint}", target.url.trim_end_matches('/')),
        )
        .bearer_auth(&c.token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    // Never return the remote body or reqwest error: neither is a trusted error
    // string, and both may contain credentials or arbitrary server output.
    let mut response = request.send().await.map_err(|_| ApiError(StatusCode::BAD_GATEWAY,
        "Could not securely reach the agent. Check its address, availability and certificate trust.".into()))?;
    let status = response.status();
    if !status.is_success() {
        let (code, message) = match status.as_u16() {
            401 | 403 => (
                StatusCode::BAD_GATEWAY,
                "The agent rejected this management credential. Reconnect with an access token that can observe and manage profiles.",
            ),
            404 | 405 => (
                StatusCode::BAD_GATEWAY,
                "This agent does not support versioned profile installation. Update its management service first.",
            ),
            409 => (
                StatusCode::CONFLICT,
                "The agent's installed profile changed. Refresh its status and review before applying again.",
            ),
            300..=399 => (
                StatusCode::BAD_GATEWAY,
                "The agent redirected this address. Connect directly to its HTTPS management address; credentials were not forwarded.",
            ),
            _ => (
                StatusCode::BAD_GATEWAY,
                "The agent could not complete this operation. Refresh its status before retrying.",
            ),
        };
        return Err(ApiError(code, message.into()));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ApiError::bad("The agent response was interrupted; refresh its status"))?
    {
        if bytes.len() + chunk.len() > 1_048_576 {
            return Err(ApiError::bad("The agent response exceeded its limit"));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| ApiError::bad("The agent returned an unsupported management response"))
}
async fn identity(client: &Client, target: &Target, c: &Credential) -> ApiResult<Value> {
    let status = request(client, target, c, Method::GET, "status", None).await?;
    let agent_id = status["instance_id"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 160)
        .ok_or_else(|| ApiError::bad("The endpoint did not identify an agent installation"))?;
    if !target.agent_id.is_empty() && target.agent_id != agent_id {
        return Err(ApiError(StatusCode::CONFLICT, "This address now belongs to a different agent. Connect it again before applying a profile.".into()));
    }
    Ok(status)
}
fn installation(value: Value) -> ApiResult<Value> {
    if value["revision"].as_u64().is_none()
        || !(value["active"].is_null() || value["active"].is_object())
        || !value["previous"].is_array()
    {
        return Err(ApiError::bad(
            "The agent returned an unsupported profile installation status",
        ));
    }
    Ok(value)
}
pub async fn list(State(s): State<Arc<AppState>>, h: HeaderMap) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "admin", false)?;
    let db = s.db.lock().unwrap();
    let mut query =
        db.prepare("SELECT value FROM agent_targets ORDER BY json_extract(value,'$.name')")?;
    let targets = query
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(|s| serde_json::from_str::<Target>(&s).map(|t| json!(t)))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ApiError::bad("Invalid agent connections"))?;
    Ok(Json(json!({"items":targets})))
}
pub async fn connect(
    State(s): State<Arc<AppState>>,
    h: HeaderMap,
    Json(v): Json<Connect>,
) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "admin", true)?;
    validate_input(&v)?;
    let credential = Credential {
        token: v.token.trim().into(),
        ca_pem: v.ca_pem,
    };
    let client = client(&credential)?;
    let mut target = Target {
        id: id(),
        url: v.url.trim_end_matches('/').into(),
        name: String::new(),
        agent_id: String::new(),
        created_at: now_ms(),
    };
    let status = identity(&client, &target, &credential).await?;
    let package = installation(
        request(
            &client,
            &target,
            &credential,
            Method::GET,
            "profile-package",
            None,
        )
        .await?,
    )?;
    target.agent_id = status["instance_id"].as_str().unwrap().into();
    target.name = status["name"]
        .as_str()
        .unwrap_or("Agent")
        .chars()
        .take(160)
        .collect();
    let existing: Option<String> =
        s.db.lock()
            .unwrap()
            .query_row(
                "SELECT value FROM agent_targets WHERE json_extract(value,'$.agent_id')=?1",
                [&target.agent_id],
                |r| r.get(0),
            )
            .optional()?;
    if let Some(existing) = existing {
        let old: Target = serde_json::from_str(&existing)
            .map_err(|_| ApiError::bad("Invalid agent connection"))?;
        target.id = old.id;
        target.created_at = old.created_at;
    }
    private_write(
        &credential_path(&s, &target)?,
        &serde_json::to_vec(&credential).unwrap(),
    )?;
    s.db.lock().unwrap().execute("INSERT INTO agent_targets VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET value=excluded.value", params![target.id, serde_json::to_string(&target).unwrap()])?;
    s.audit(
        "agent_connected",
        None,
        json!({"target_id":target.id,"agent_id":target.agent_id}),
    )?;
    Ok(Json(json!({"target":target,"installation":package})))
}
pub async fn status(
    State(s): State<Arc<AppState>>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "admin", false)?;
    let mut target = read(&s, &id)?;
    let credential = credential(&s, &target)?;
    let client = client(&credential)?;
    let info = identity(&client, &target, &credential).await?;
    target.name = info["name"]
        .as_str()
        .unwrap_or(&target.name)
        .chars()
        .take(160)
        .collect();
    let package = installation(
        request(
            &client,
            &target,
            &credential,
            Method::GET,
            "profile-package",
            None,
        )
        .await?,
    )?;
    Ok(Json(json!({"target":target,"installation":package})))
}
pub async fn forget(
    State(s): State<Arc<AppState>>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "admin", true)?;
    let target = read(&s, &id)?;
    let path = credential_path(&s, &target)?;
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    s.db.lock()
        .unwrap()
        .execute("DELETE FROM agent_targets WHERE id=?1", [&id])?;
    s.audit("agent_disconnected", None, json!({"target_id":id}))?;
    Ok(Json(json!({"ok":true})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Apply {
    target_id: String,
    expected_revision: u64,
}
pub async fn apply(
    State(s): State<Arc<AppState>>,
    h: HeaderMap,
    Path((pid, rid)): Path<(String, String)>,
    Json(v): Json<Apply>,
) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "admin", true)?;
    let revision = crate::server::get_revision(&s, &pid, &rid)?;
    let target = read(&s, &v.target_id)?;
    let credential = credential(&s, &target)?;
    let client = client(&credential)?;
    identity(&client, &target, &credential).await?;
    // Recheck local evidence after the network lookup; do not promote an invalidated profile.
    crate::server::get_revision(&s, &pid, &rid)?;
    let package = installation(request(&client, &target, &credential, Method::POST, "profile-package", Some(json!({
        "expected_revision":v.expected_revision,
        "package":{"schema":"twin-profile/1","manifest":revision.manifest,"files":revision.files}
    }))).await?)?;
    s.audit("profile_applied", Some(&pid), json!({"target_id":target.id,"revision_id":rid,"agent_revision":package["revision"],"snapshot_id":package["active"]["id"]}))?;
    Ok(Json(json!({"target":target,"installation":package})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Restore {
    expected_revision: u64,
    id: String,
}
pub async fn restore(
    State(s): State<Arc<AppState>>,
    h: HeaderMap,
    Path(target_id): Path<String>,
    Json(v): Json<Restore>,
) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "admin", true)?;
    uuid::Uuid::parse_str(&v.id)
        .map_err(|_| ApiError::bad("Select a previous profile installation"))?;
    let target = read(&s, &target_id)?;
    let credential = credential(&s, &target)?;
    let client = client(&credential)?;
    identity(&client, &target, &credential).await?;
    let package = installation(
        request(
            &client,
            &target,
            &credential,
            Method::POST,
            "profile-package/restore",
            Some(json!({"expected_revision":v.expected_revision,"id":v.id})),
        )
        .await?,
    )?;
    s.audit(
        "profile_restored",
        None,
        json!({"target_id":target.id,"snapshot_id":v.id,"agent_revision":package["revision"]}),
    )?;
    Ok(Json(json!({"target":target,"installation":package})))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn connection_requires_secure_origin_and_keeps_credentials_out_of_urls() {
        for url in [
            "http://example.test",
            "https://user:secret@example.test",
            "https://example.test/api/v1",
            "https://example.test/?token=secret",
            "https://example.test/#token",
        ] {
            assert!(
                validate_input(&Connect {
                    url: url.into(),
                    token: "a".repeat(32),
                    ca_pem: String::new()
                })
                .is_err(),
                "{url}"
            );
        }
        assert!(
            validate_input(&Connect {
                url: "https://agent.example.test:18449".into(),
                token: "a".repeat(32),
                ca_pem: String::new()
            })
            .is_ok()
        );
    }
}
