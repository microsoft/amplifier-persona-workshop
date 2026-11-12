use crate::{
    config::{AuthMode, Config, WebTransport},
    error::{ApiError, ApiResult},
    state::{AppState, now_ms},
};
use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use rand::RngCore;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
#[cfg(target_os = "linux")]
use std::ffi::{CStr, CString};
use std::path::Path;
use std::sync::Arc;

#[cfg(target_os = "linux")]
struct PasswordConversation {
    password: String,
    answered: bool,
}
#[cfg(target_os = "linux")]
impl pam_client::ConversationHandler for PasswordConversation {
    fn prompt_echo_on(&mut self, _: &CStr) -> Result<CString, pam_client::ErrorCode> {
        Err(pam_client::ErrorCode::CONV_ERR)
    }
    fn prompt_echo_off(&mut self, prompt: &CStr) -> Result<CString, pam_client::ErrorCode> {
        // This first UI supports a single ordinary password, not MFA or password changes.
        if self.answered
            || !prompt
                .to_string_lossy()
                .trim()
                .eq_ignore_ascii_case("password:")
        {
            return Err(pam_client::ErrorCode::CONV_ERR);
        }
        self.answered = true;
        CString::new(self.password.clone()).map_err(|_| pam_client::ErrorCode::CONV_ERR)
    }
    fn text_info(&mut self, _: &CStr) {}
    fn error_msg(&mut self, _: &CStr) {}
}

/// This password belongs only to this installation, not to an OS account.
pub fn validate_local_password(password: &str) -> anyhow::Result<()> {
    amplifier_native_platform::local_auth::validate_password(password)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalPassword {
    schema: u32,
    password_hash: String,
}

/// Create-only: normal startup cannot replace an existing password.
pub fn initialize_local_password(home: &Path, password: &str) -> anyhow::Result<()> {
    let hash = amplifier_native_platform::local_auth::hash_password(password)?;
    crate::config::private_dir(&home.join("secrets"))?;
    crate::config::private_create(
        &home.join("secrets/local-password.json"),
        &serde_json::to_vec(&LocalPassword {
            schema: 1,
            password_hash: hash,
        })?,
    )
}

pub fn first_account_needed(state: &AppState) -> bool {
    state.config.auth_mode == AuthMode::LocalPassword
        && state.config.web_transport == WebTransport::LocalHttp
        && !state.home.join("secrets/local-password.json").exists()
        && state.home.join("secrets/first-account.json").is_file()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirstAccount {
    setup_token: String,
    password: String,
    #[serde(default)]
    start_at_signin: bool,
}

pub async fn first_account(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<FirstAccount>,
) -> ApiResult<Response> {
    same_origin(&state, &headers)?;
    if !first_account_needed(&state) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This installation already has an account, or was not opened for first setup".into(),
        ));
    }
    let path = state.home.join("secrets/first-account.json");
    if path.is_symlink() || !std::fs::metadata(&path).is_ok_and(|m| m.len() <= 4096) {
        return Err(ApiError::forbidden());
    }
    let setup: Value =
        serde_json::from_slice(&std::fs::read(path)?).map_err(|_| ApiError::forbidden())?;
    if setup["installation_id"] != state.config.id
        || setup["expires_at"].as_i64().unwrap_or(0) <= now_ms()
        || !fixed_hex(&input.setup_token)
        || !setup["token"]
            .as_str()
            .is_some_and(|t| constant_equal(t, &input.setup_token))
    {
        return Err(ApiError::forbidden());
    }
    validate_local_password(&input.password).map_err(|e| ApiError::bad(e.to_string()))?;
    let permit = state.login_gate.clone().try_acquire_owned().map_err(|_| {
        ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Account setup is already running".into(),
        )
    })?;
    let home = state.home.clone();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        initialize_local_password(&home, &input.password)
    })
    .await
    .map_err(|_| ApiError::bad("Account setup did not complete; reopen the app to inspect it"))?
    .map_err(|_| {
        ApiError(
            StatusCode::CONFLICT,
            "The account could not be created. Existing credentials have been preserved".into(),
        )
    })?;
    if input.start_at_signin {
        // Failure never removes an account that has already been created.
        let home = state.home.clone();
        let _ = tokio::task::spawn_blocking(move || crate::desktop::set_startup(&home, true)).await;
    }
    signed_in(&state, &state.config.owner, "local-setup")
}

fn local_password_matches(home: &Path, password: &str) -> bool {
    let path = home.join("secrets/local-password.json");
    // A corrupt or edited credential never selects a fallback authenticator or
    // supplies unbounded Argon2 work. Schema 1 has one exact hashing profile.
    if path.is_symlink() || !std::fs::metadata(&path).is_ok_and(|m| m.len() <= 4096) {
        return false;
    }
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let Ok(saved) = serde_json::from_slice::<LocalPassword>(&bytes) else {
        return false;
    };
    saved.schema == 1
        && amplifier_native_platform::local_auth::verify_password(&saved.password_hash, password)
}

#[cfg(target_os = "linux")]
fn pam_matches(service: &str, username: &str, password: String) -> bool {
    let conversation = PasswordConversation {
        password,
        answered: false,
    };
    let Ok(mut context) = pam_client::Context::new(service, Some(username), conversation) else {
        return false;
    };
    context.authenticate(pam_client::Flag::NONE).is_ok()
        && context.acct_mgmt(pam_client::Flag::NONE).is_ok()
}
#[cfg(not(target_os = "linux"))]
fn pam_matches(_service: &str, _username: &str, _password: String) -> bool {
    false
}

pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalProofQuery {
    nonce: String,
}
fn fixed_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
/// The native client proves a loopback listener before sending its credential.
/// This does not authenticate a caller or grant management access.
pub async fn local_proof(
    State(state): State<Arc<AppState>>,
    Query(query): Query<LocalProofQuery>,
) -> ApiResult<Json<Value>> {
    if state.config.web_transport != WebTransport::LocalHttp {
        return Err(ApiError::missing());
    }
    if !fixed_hex(&query.nonce) {
        return Err(ApiError::bad(
            "Use a 256-bit lowercase hexadecimal challenge",
        ));
    }
    let credential = std::fs::read_to_string(state.home.join("secrets/control-token"))?;
    let proof = amplifier_native_platform::local_auth::listener_proof(
        "persona-workshop.local-proof/1",
        &state.config.id,
        &state.config.public_url,
        &query.nonce,
        credential.trim(),
    )
    .map_err(|_| ApiError::bad("Local installation proof is unavailable"))?;
    Ok(Json(
        json!({"schema":1,"id":state.config.id,"origin":state.config.public_url,"nonce":query.nonce,"proof":proof}),
    ))
}
pub fn verify_local_proof(config: &Config, nonce: &str, credential: &str, value: &Value) -> bool {
    if config.web_transport != WebTransport::LocalHttp
        || !fixed_hex(nonce)
        || value["schema"] != 1
        || value["id"] != config.id
        || value["origin"] != config.public_url
        || value["nonce"] != nonce
    {
        return false;
    }
    let Some(proof) = value["proof"].as_str().filter(|p| fixed_hex(p)) else {
        return false;
    };
    amplifier_native_platform::local_auth::verify_listener_proof(
        "persona-workshop.local-proof/1",
        &config.id,
        &config.public_url,
        nonce,
        credential,
        proof,
    )
    .is_ok()
}
pub fn cookie_name(state: &AppState) -> String {
    format!("twn_{}", state.config.id.replace('-', ""))
}
fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        // HTTP/2 may send each cookie in its own field. Other apps on the
        // same hostname (including different ports) can precede our cookie.
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|v| {
            let (k, v) = v.trim().split_once('=')?;
            (k == name).then(|| v.to_owned())
        })
}
fn constant_equal(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |sum, (a, b)| sum | (a ^ b))
            == 0
}
#[derive(Clone)]
pub struct Principal {
    pub actor: String,
    pub csrf: Option<String>,
    pub session_hash: Option<String>,
}

pub fn check(
    state: &AppState,
    headers: &HeaderMap,
    scope: &str,
    mutation: bool,
) -> ApiResult<Principal> {
    if let Some(value) = headers.get(header::AUTHORIZATION) {
        let token = value
            .to_str()
            .ok()
            .and_then(|s| s.strip_prefix("Bearer "))
            .ok_or_else(ApiError::unauthorized)?;
        let row: Option<(String, String)> = state
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT id,scopes FROM tokens WHERE hash=?1 AND expires_at>?2",
                params![digest(token), now_ms()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (id, scopes) = row.ok_or_else(ApiError::unauthorized)?;
        let scopes: Vec<String> = serde_json::from_str(&scopes).unwrap_or_default();
        if !scopes.iter().any(|s| s == "admin" || s == scope) {
            return Err(ApiError::forbidden());
        }
        return Ok(Principal {
            actor: format!("token:{id}"),
            csrf: None,
            session_hash: None,
        });
    }
    let token = cookie_value(headers, &cookie_name(state)).ok_or_else(ApiError::unauthorized)?;
    let hash = digest(&token);
    let row: Option<(String, String)> = state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT owner,csrf FROM browser_sessions WHERE hash=?1 AND expires_at>?2",
            params![hash, now_ms()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (owner, csrf) = row.ok_or_else(ApiError::unauthorized)?;
    if mutation {
        same_origin(state, headers)?;
        if !headers
            .get("x-csrf-token")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| constant_equal(v, &csrf))
        {
            return Err(ApiError::forbidden());
        }
    }
    Ok(Principal {
        actor: owner,
        csrf: Some(csrf),
        session_hash: Some(hash),
    })
}
pub fn same_origin(state: &AppState, headers: &HeaderMap) -> ApiResult<()> {
    if let Some(origin) = headers.get(header::ORIGIN) {
        let host = headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(ApiError::forbidden)?;
        let expected = if state.config.web_transport == WebTransport::LocalHttp {
            state.config.public_url.clone()
        } else {
            format!("https://{host}")
        };
        if origin.to_str().ok() != Some(expected.as_str()) {
            return Err(ApiError::forbidden());
        }
    }
    Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Login {
    username: String,
    password: String,
}
pub async fn login(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(login): Json<Login>,
) -> ApiResult<Response> {
    same_origin(&state, &headers)?;
    if login.username.len() > 100 || login.password.len() > 1024 || login.password.is_empty() {
        return Err(ApiError::unauthorized());
    }
    let permit = state.login_gate.clone().try_acquire_owned().map_err(|_| {
        ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Please wait before trying to sign in again".into(),
        )
    })?;
    let config = state.config.clone();
    {
        let db = state.db.lock().unwrap();
        let locked: Option<i64> = db
            .query_row(
                "SELECT locked_until FROM login_attempts WHERE key='owner'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if locked.is_some_and(|until| until > now_ms()) {
            return Err(ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "Please wait before trying to sign in again".into(),
            ));
        }
        db.execute(
            "DELETE FROM browser_sessions WHERE expires_at<=?1",
            [now_ms()],
        )?;
    }
    let owner = config.owner.clone();
    let method = config.auth_mode;
    let service = config.pam_service;
    let home = state.home.clone();
    let success = if login.username == owner {
        let worker = tokio::task::spawn_blocking(move || {
            // The permit stays with a timed-out verifier, bounding blocked workers too.
            let _permit = permit;
            match method {
                AuthMode::Pam => pam_matches(&service, &login.username, login.password),
                AuthMode::LocalPassword => local_password_matches(&home, &login.password),
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(20), worker)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or(false)
    } else {
        false
    };
    if !success {
        state.db.lock().unwrap().execute("INSERT INTO login_attempts(key,failures,locked_until) VALUES('owner',1,0) ON CONFLICT(key) DO UPDATE SET failures=CASE WHEN locked_until>0 AND locked_until<?1 THEN 1 ELSE failures+1 END, locked_until=CASE WHEN failures>=4 THEN ?1+60000 ELSE 0 END",[now_ms()])?;
        state.audit("auth.failed", None, json!({"method":method.as_str()}))?;
        return Err(ApiError::unauthorized());
    }
    signed_in(&state, &owner, method.as_str())
}

/// Used only after authentication or capability-bound, create-only local setup.
pub fn signed_in(state: &AppState, owner: &str, method: &str) -> ApiResult<Response> {
    if owner != state.config.owner {
        return Err(ApiError::forbidden());
    }
    let token = random_token();
    let csrf = random_token();
    {
        let db = state.db.lock().unwrap();
        db.execute("DELETE FROM login_attempts WHERE key='owner'", [])?;
        db.execute(
            "INSERT INTO browser_sessions(hash,owner,csrf,expires_at) VALUES(?1,?2,?3,?4)",
            params![digest(&token), owner, csrf, now_ms() + 8 * 3600 * 1000],
        )?;
    }
    state.audit(
        "auth.signed_in",
        None,
        json!({"actor":owner,"method":method}),
    )?;
    let cookie = format!(
        "{}={}; {}HttpOnly; SameSite=Strict; Path=/; Max-Age=28800",
        cookie_name(state),
        token,
        secure_cookie(state),
    );
    Ok((
        [(header::SET_COOKIE, cookie)],
        Json(json!({"authenticated":true,"owner":owner,"csrf":csrf})),
    )
        .into_response())
}
pub async fn session(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    match check(&state, &headers, "observe", false) {
        Ok(p) => Ok(Json(
            json!({"authenticated":true,"owner":state.config.owner,"csrf":p.csrf,"auth_mode":state.config.auth_mode,"first_account_needed":first_account_needed(&state)}),
        )),
        Err(e) if e.0 == StatusCode::UNAUTHORIZED => Ok(Json(
            json!({"authenticated":false,"owner":state.config.owner,"csrf":null,"auth_mode":state.config.auth_mode,"first_account_needed":first_account_needed(&state)}),
        )),
        Err(e) => Err(e),
    }
}
pub async fn logout(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Response> {
    let p = check(&state, &headers, "observe", true)?;
    if let Some(hash) = p.session_hash {
        state
            .db
            .lock()
            .unwrap()
            .execute("DELETE FROM browser_sessions WHERE hash=?1", [hash])?;
    }
    state.audit("auth.signed_out", None, json!({"actor":p.actor}))?;
    Ok((
        [(
            header::SET_COOKIE,
            format!(
                "{}=; {}HttpOnly; SameSite=Strict; Path=/; Max-Age=0",
                cookie_name(&state),
                secure_cookie(&state),
            ),
        )],
        Json(json!({"ok":true})),
    )
        .into_response())
}

fn secure_cookie(state: &AppState) -> &'static str {
    match state.config.web_transport {
        WebTransport::Https => "Secure; ",
        WebTransport::LocalHttp => "",
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use pam_client::ConversationHandler;
    #[test]
    fn password_is_not_reused_for_another_authentication_challenge() {
        let mut c = PasswordConversation {
            password: "test".into(),
            answered: false,
        };
        assert!(c.prompt_echo_off(c"Verification code:").is_err());
        assert!(c.prompt_echo_off(c"Password:").is_ok());
        assert!(c.prompt_echo_off(c"Password:").is_err());
        assert!(c.prompt_echo_on(c"Username:").is_err());
    }
}
