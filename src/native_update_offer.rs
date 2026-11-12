//! A separately opened, complete download can update its existing installation.
//! Browser input selects only a request identity, never a file or command.
use crate::{auth, config::Config, desktop, native_update};
use amplifier_native_platform as platform;
use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub fn different_download(home: &Path) -> Result<bool> {
    let marker = home.join("native-installation.json");
    match std::fs::symlink_metadata(&marker) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        other => {
            other?;
        }
    }
    let marker: Value = serde_json::from_slice(&std::fs::read(marker)?)?;
    let config = Config::load(home)?;
    ensure!(
        marker["id"] == config.id,
        "Native installation identity changed"
    );
    if marker["update_owner"] != "installation"
        || home.join("saved-connections.json").exists()
        || home.join("updates/registered-service.json").exists()
    {
        return Ok(false);
    }
    let installed = platform::installed_binary(&desktop::APPLICATION, home);
    let current = std::env::current_exe()?;
    if current.canonicalize()? == installed.canonicalize()? {
        return Ok(false);
    }
    let resources = desktop::runtime_source(&current)?;
    let digest = |path: &Path| -> Result<String> {
        Ok(format!("{:x}", Sha256::digest(std::fs::read(path)?)))
    };
    let image = digest(&current)?;
    let runtime = digest(&resources.join("native-bundle.json"))?;
    // After a swap, the separately opened download can equal installed bytes
    // while its verification/recovery is unresolved. Keep that saved offer.
    // The retained installed executable returned above keeps ordinary reopen.
    Ok(
        pending_download(native_update::active(home)?.as_ref(), &image, &runtime)
            || image != digest(&installed)?
            || runtime != digest(&home.join("runtime/native-bundle.json"))?,
    )
}

fn pending_download(plan: Option<&Value>, image: &str, runtime: &str) -> bool {
    plan.is_some_and(|plan| {
        plan["source"] == "local_download"
            && plan["binary_sha256"] == image
            && plan["runtime_manifest_sha256"] == runtime
            && !["succeeded", "rolled_back", "failed"]
                .contains(&plan["state"].as_str().unwrap_or(""))
    })
}

struct Offer {
    home: PathBuf,
    origin: String,
    token: String,
    installation_url: String,
    prepared: Mutex<Option<std::result::Result<String, String>>>,
    reconciled: Mutex<Option<(String, String)>>,
    closed: tokio::sync::Notify,
}

pub async fn serve(home: PathBuf, open_browser: bool) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let state = Arc::new(Offer {
        installation_url: Config::load(&home)?.public_url,
        home,
        origin: format!("http://127.0.0.1:{}", listener.local_addr()?.port()),
        token: auth::random_token(),
        prepared: Mutex::new(None),
        reconciled: Mutex::new(None),
        closed: tokio::sync::Notify::new(),
    });
    let preparation = state.clone();
    tokio::task::spawn_blocking(move || {
        let result = (|| -> Result<String> {
            let value = native_update::prepare_local(&preparation.home, &std::env::current_exe()?)?;
            Ok(value["id"]
                .as_str()
                .context("Preparation did not return an update identity")?
                .to_owned())
        })();
        *preparation.prepared.lock().unwrap() = Some(result.map_err(|e| e.to_string()));
    });
    let url = format!("{}/#setup={}", state.origin, state.token);
    if open_browser {
        platform::open_browser(&url)?;
    } else {
        println!("{}", json!({"state":"preparing_update","url":url}));
    }
    let shutdown = state.clone();
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async move { shutdown.closed.notified().await })
        .await?;
    Ok(())
}

fn router(state: Arc<Offer>) -> Router {
    Router::new().fallback(any(request)).with_state(state)
}
fn same_origin(headers: &HeaderMap, origin: &str) -> bool {
    headers.get(header::HOST).and_then(|v| v.to_str().ok()) == origin.strip_prefix("http://")
        && headers
            .get(header::ORIGIN)
            .is_none_or(|v| v.to_str().ok() == Some(origin))
        && headers
            .get("sec-fetch-site")
            .is_none_or(|v| matches!(v.to_str(), Ok("same-origin" | "none")))
}
fn asset(kind: &'static str, body: &'static str) -> Response {
    ([
        (header::CONTENT_TYPE, kind),
        (header::CACHE_CONTROL, "no-store"),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::CONTENT_SECURITY_POLICY, "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'"),
    ], body).into_response()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Command {
    request_id: uuid::Uuid,
}

async fn request(State(s): State<Arc<Offer>>, req: Request) -> Response {
    if !same_origin(req.headers(), &s.origin) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if req.method() == "GET" {
        match req.uri().path() {
            "/" => return asset("text/html; charset=utf-8", HTML),
            "/offer.css" => return asset("text/css; charset=utf-8", CSS),
            "/offer.js" => return asset("text/javascript; charset=utf-8", JS),
            _ => (),
        }
    }
    if req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        != Some(format!("Bearer {}", s.token).as_str())
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let method = req.method().as_str().to_owned();
    let path = req.uri().path().to_owned();
    let result = async {
        if method == "POST" && path == "/close" {
            s.closed.notify_one();
            return Ok(json!({"closed":true}));
        }
        let prepared = s.prepared.lock().unwrap().clone();
        if method == "GET" && path == "/status" {
            return status(&s, prepared);
        }
        ensure!(
            method == "POST" && matches!(path.as_str(), "/apply" | "/recover"),
            "Unknown update operation"
        );
        let plan = prepared
            .context("The download is still being prepared")?
            .map_err(anyhow::Error::msg)?;
        let bytes = to_bytes(req.into_body(), 1024)
            .await
            .context("Invalid update request")?;
        let command: Command = serde_json::from_slice(&bytes)?;
        let home = s.home.clone();
        let operation = tokio::task::spawn_blocking(move || {
            if path == "/recover" {
                native_update::recover_local(&home, &plan, &command.request_id.to_string())
            } else {
                native_update::apply_local(&home, &plan, &command.request_id.to_string())
            }
        })
        .await
        .context("The update response was interrupted; inspect its saved status");
        // Inspect the saved outcome even if the dispatch acknowledgement was
        // lost; observation never repeats the operation.
        let visible = status(&s, s.prepared.lock().unwrap().clone());
        operation??;
        visible
    }
    .await;
    let mut response = match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => (
            StatusCode::CONFLICT,
            Json(json!({"error":error.to_string()})),
        )
            .into_response(),
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}
fn status(s: &Offer, prepared: Option<std::result::Result<String, String>>) -> Result<Value> {
    let Some(prepared) = prepared else {
        return Ok(json!({"state":"preparing","version":env!("CARGO_PKG_VERSION")}));
    };
    let plan = prepared.map_err(anyhow::Error::msg)?;
    let status = native_update::status(&s.home);
    ensure!(
        status["journal_error"].is_null(),
        "{}",
        status["journal_error"]
            .as_str()
            .unwrap_or("Update state needs inspection")
    );
    let current = &status["current"];
    ensure!(
        current["id"] == plan && current["source"] == "local_download",
        "Another update owns the saved state; this offer has not changed it"
    );
    if let Some(request) = current["request_id"].as_str()
        && uuid::Uuid::parse_str(request).is_ok()
    {
        let key = (plan, request.to_owned());
        let mut previous = s.reconciled.lock().unwrap();
        if previous.as_ref() != Some(&key) {
            *previous = Some(key);
            // One bounded observer per saved request also covers explicit
            // rollback from the new app to an older app without an observer.
            // Its existing retirement journal preserves ambiguous outcomes.
            tokio::spawn(crate::native_update_service::reconcile(s.home.clone()));
        }
    }
    // The page does not need private paths, configurations or the full journal.
    let mut visible = json!({"url":s.installation_url});
    for key in [
        "id",
        "state",
        "version",
        "reason",
        "error",
        "native_error",
        "worker_active",
        "recovery_available",
        "up_to_date",
        "request_id",
    ] {
        if current[key].is_null() || current[key].is_string() || current[key].is_boolean() {
            visible[key] = current[key].clone();
        }
    }
    Ok(visible)
}

const HTML: &str = r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Update Persona Workshop</title><link rel="stylesheet" href="/offer.css"><script defer src="/offer.js"></script></head><body><main><p class="brand">Persona Workshop</p><h1>Bring your workshop up to date.</h1><p class="intro">Use this download to update your existing workshop. Your account, profiles and evidence stay in place.</p><section aria-live="polite" aria-atomic="true"><h2 id="heading">Preparing your download</h2><p id="detail">Checking the application and its included files. Your workshop remains available.</p><p id="error" role="alert" hidden></p></section><div class="actions"><button id="apply" hidden>Update Persona Workshop</button><button id="recover" hidden>Recover interrupted update</button><a id="open" hidden>Open Persona Workshop</a><button id="close" class="quiet">Close this window</button></div><p id="note" class="note">Updates wait for active work to settle.</p></main></body></html>"#;
const CSS: &str = r#":root{font-family:Inter,ui-sans-serif,system-ui,sans-serif;color:#243936;background:#f5f7f3;font-synthesis:none}*{box-sizing:border-box}body{margin:0}main{max-width:640px;margin:clamp(32px,12vh,112px) auto;padding:0 24px 40px}.brand{color:#176b64;font-weight:700;font-size:15px;margin:0 0 36px}h1{font-size:clamp(30px,6vw,42px);line-height:1.12;letter-spacing:-.03em;margin:0 0 20px;max-width:520px}.intro{font-size:18px;line-height:1.65;max-width:520px}section{border-top:1px solid #dce5de;margin-top:32px;padding-top:24px}h2{font-size:20px;line-height:1.35;margin:0 0 10px}section p{line-height:1.65;margin:0 0 12px}#error{color:#a13c35;overflow-wrap:anywhere}.actions{display:flex;gap:12px;align-items:center;flex-wrap:wrap;margin-top:24px}button,a{font:inherit;line-height:1.4;font-weight:600;border:1px solid #176b64;background:#176b64;color:white;padding:12px 16px;border-radius:6px;text-decoration:none;cursor:pointer;min-height:48px}button.quiet{border-color:transparent;color:#176b64;background:transparent;padding-left:0}button:disabled{opacity:.6;cursor:wait}:focus-visible{outline:3px solid #173d41;outline-offset:4px}.note{font-size:14px;line-height:1.6;color:#677974;margin-top:24px}[hidden]{display:none!important}@media(max-width:440px){main{padding:0 20px 32px}.brand{margin-bottom:28px}.actions{align-items:stretch;flex-direction:column}.actions a{text-align:center}.quiet{text-align:left}}"#;
const JS: &str = r#"'use strict';
const $=id=>document.getElementById(id), key='persona-update-offer';
const fragment=new URLSearchParams(location.hash.slice(1)).get('setup');
if(fragment){sessionStorage.setItem(key,fragment);history.replaceState(null,'',location.pathname);}
const token=sessionStorage.getItem(key);let busy=false,closed=false,timer;
const pendingKey=key+'-request';
function error(message){$('error').textContent=message||'';$('error').hidden=!message;}
async function api(path,body){const r=await fetch(path,{method:body?'POST':'GET',headers:{Authorization:'Bearer '+token,'Content-Type':'application/json'},body:body?JSON.stringify(body):undefined});const data=await r.json();if(!r.ok)throw Error(data.error||'This update window could not verify the request. Reopen the same download.');return data;}
const text={preparing:['Preparing your download','Checking the application and its included files. Your workshop remains available.'],ready:['Ready to update','Install version VERSION. Your workshop will briefly close and reopen.'],deferred:['Your workshop is still busy','No files have been replaced. Let active work finish, then try again.'],queued:['Update requested','Waiting for the managed update to begin.'],draining:['Finishing active work','Your workshop will close when its current work has settled.'],stopping:['Closing the workshop','The update is waiting for the existing application to stop.'],swapping:['Installing the update','Your saved profiles and evidence are staying in place.'],verifying:['Checking the updated workshop','The new application is opening and checking its saved installation.'],rolling_back:['Restoring the previous version','Keeping the saved workshop data while restoring the application.'],succeeded:['Your workshop is up to date','The new version has opened successfully.'],rolled_back:['The previous version was restored','The update did not complete. Your saved workshop data was kept.'],failed:['The update did not complete','The saved result is below. You can close this window; no automatic retry will be made.'],recovery_required:['The update needs recovery','Its outcome is not yet confirmed. Use the saved recovery action when it becomes available.']};
function pendingRequest(){try{return JSON.parse(sessionStorage.getItem(pendingKey)||'null');}catch{return null;}}
function show(s){let pending=pendingRequest();if(pending&&s.request_id===pending.request_id){sessionStorage.removeItem(pendingKey);pending=null;}const t=text[s.state]||['Checking update status','Waiting for the saved update result.'];$('heading').textContent=t[0];$('detail').textContent=t[1].replace('VERSION',s.version||'this download');error(s.native_error||s.error||s.reason||(pending&&['ready','deferred'].includes(s.state)?'The previous request was not confirmed. Checking status does not send it again. Retry keeps the same request identity.':''));$('apply').textContent=pending?.action==='apply'?'Retry the same update request':'Update Persona Workshop';$('apply').hidden=!['ready','deferred'].includes(s.state)||s.up_to_date===true;$('recover').hidden=s.recovery_available!==true;$('open').hidden=!['succeeded','rolled_back'].includes(s.state);if(!$('open').hidden)$('open').href=s.url;$('apply').disabled=busy;$('recover').disabled=busy;if(s.up_to_date===true&&s.state==='ready'){$('heading').textContent='This version is already installed';$('detail').textContent='Your workshop already has this application version.';$('open').href=s.url;$('open').hidden=false;}}
async function poll(){if(closed)return;try{show(await api('/status'));}catch(e){error(e.message);}finally{if(!closed)timer=setTimeout(poll,1500);}}
async function act(action){if(busy)return;busy=true;$('apply').disabled=$('recover').disabled=true;error('');let pending;try{pending=JSON.parse(sessionStorage.getItem(pendingKey)||'null');}catch{}if(!pending||pending.action!==action){pending={action,request_id:crypto.randomUUID()};sessionStorage.setItem(pendingKey,JSON.stringify(pending));}try{show(await api('/'+action,{request_id:pending.request_id}));sessionStorage.removeItem(pendingKey);}catch(e){error(e.message+' The result may still be pending. Checking saved status; no second request has been sent.');}finally{busy=false;}}
$('apply').onclick=()=>act('apply');$('recover').onclick=()=>act('recover');$('close').onclick=async()=>{try{await api('/close',{});closed=true;clearTimeout(timer);$('apply').hidden=$('recover').hidden=$('close').hidden=true;$('heading').textContent='You can close this tab';$('detail').textContent='Any update already started continues independently. Reopen the same download to check its result.';error('');}catch(e){error(e.message);}};
if(token)poll();else{error('Open this window from the Persona Workshop download to continue.');$('close').hidden=true;}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use tower::ServiceExt;
    #[test]
    fn interrupted_swapped_download_keeps_its_offer_without_adopting_another_plan() {
        let mut plan = json!({"source":"local_download","binary_sha256":"same-installed-image","runtime_manifest_sha256":"same-installed-runtime","state":"verifying"});
        assert!(pending_download(
            Some(&plan),
            "same-installed-image",
            "same-installed-runtime"
        ));
        assert!(!pending_download(
            Some(&plan),
            "same-installed-image",
            "different-runtime"
        ));
        assert!(!pending_download(
            Some(&plan),
            "different-image",
            "same-installed-runtime"
        ));
        plan["state"] = json!("succeeded");
        assert!(!pending_download(
            Some(&plan),
            "same-installed-image",
            "same-installed-runtime"
        ));
        assert!(!pending_download(
            None,
            "same-installed-image",
            "same-installed-runtime"
        ));
    }
    fn state(home: PathBuf) -> Arc<Offer> {
        Arc::new(Offer {
            home,
            origin: "http://127.0.0.1:23456".into(),
            token: "test-capability".into(),
            installation_url: "http://127.0.0.1:23457".into(),
            prepared: Mutex::new(None),
            reconciled: Mutex::new(None),
            closed: tokio::sync::Notify::new(),
        })
    }
    #[tokio::test]
    async fn origin_and_capability_are_required_before_any_operation() {
        let root = tempfile::tempdir().unwrap();
        let s = state(root.path().into());
        for (host, origin, site, auth, expected) in [
            (
                "evil.test",
                "http://127.0.0.1:23456",
                "same-origin",
                true,
                403,
            ),
            (
                "127.0.0.1:23456",
                "http://evil.test",
                "cross-site",
                true,
                403,
            ),
            (
                "127.0.0.1:23456",
                "http://127.0.0.1:23456",
                "same-site",
                true,
                403,
            ),
            (
                "127.0.0.1:23456",
                "http://127.0.0.1:23456",
                "same-origin",
                false,
                401,
            ),
        ] {
            let mut r = Request::builder()
                .method("POST")
                .uri("/apply")
                .header("host", host)
                .header("origin", origin)
                .header("sec-fetch-site", site);
            if auth {
                r = r.header("authorization", "Bearer test-capability");
            }
            let response = router(s.clone())
                .oneshot(r.body(Body::from("{}")).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status().as_u16(), expected);
        }
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
    #[tokio::test]
    async fn bounded_command_rejects_browser_supplied_paths_and_extra_fields() {
        let root = tempfile::tempdir().unwrap();
        let s = state(root.path().into());
        *s.prepared.lock().unwrap() = Some(Ok(uuid::Uuid::new_v4().to_string()));
        for body in [
            json!({"request_id":uuid::Uuid::new_v4(),"binary":"arbitrary"}).to_string(),
            json!({"request_id":"invalid"}).to_string(),
            " ".repeat(1025),
        ] {
            let r = Request::builder()
                .method("POST")
                .uri("/apply")
                .header("host", "127.0.0.1:23456")
                .header("authorization", "Bearer test-capability")
                .body(Body::from(body))
                .unwrap();
            assert_eq!(
                router(s.clone()).oneshot(r).await.unwrap().status(),
                StatusCode::CONFLICT
            );
        }
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
    #[tokio::test]
    async fn public_page_has_no_capability_and_preparation_status_is_authenticated() {
        let root = tempfile::tempdir().unwrap();
        let s = state(root.path().into());
        let r = Request::builder()
            .uri("/")
            .header("host", "127.0.0.1:23456")
            .body(Body::empty())
            .unwrap();
        let response = router(s.clone()).oneshot(r).await.unwrap();
        assert!(
            !response.headers()[header::CONTENT_SECURITY_POLICY]
                .to_str()
                .unwrap()
                .contains("unsafe-inline")
        );
        let bytes = to_bytes(response.into_body(), 10000).await.unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("test-capability"));
        assert_eq!(status(&s, None).unwrap()["state"], "preparing");
    }
}
