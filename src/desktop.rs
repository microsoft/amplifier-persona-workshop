//! Installation-owned native launch; no analysis engine or provider is selected.
use crate::{
    auth,
    config::{AuthMode, Config, PdfBackend, WebTransport},
    state::AppState,
};
use amplifier_native_platform::{self as platform, Application};
use anyhow::{Context, Result, ensure};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, header},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

pub const APPLICATION: Application = Application {
    display_name: "Persona Workshop",
    startup_description: "Persona Workshop",
    executable_name: "persona-workshop",
    startup_prefix: "persona-workshop",
    data_directory: "persona-workshop",
    background_command: "serve",
};
pub fn default_home() -> Result<PathBuf> {
    let current = platform::default_home(&APPLICATION, "default")?;
    #[cfg(target_os = "linux")]
    if !current.exists() {
        let legacy = current
            .parent()
            .and_then(|p| p.parent())
            .context("Missing application data folder")?
            .join("twin-workbench/default");
        if legacy.join("config.json").is_file() {
            return Ok(legacy);
        }
    }
    Ok(current)
}
pub fn create(home: &Path, port: u16) -> Result<()> {
    ensure!(
        !home.exists(),
        "This folder already exists. Its files were preserved; choose a new folder."
    );
    let parent = home.parent().context("Missing installation parent")?;
    std::fs::create_dir_all(parent)?;
    let stage = parent.join(format!(".persona-setup-{}", uuid::Uuid::new_v4()));
    platform::private_dir(&stage)?;
    platform::private_dir(&stage.join("secrets"))?;
    install_support(&stage)?;
    let config = Config {
        id: uuid::Uuid::new_v4().to_string(),
        owner: std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_else(|_| "owner".into()),
        bind: "127.0.0.1".into(),
        port,
        public_url: format!("http://127.0.0.1:{port}"),
        auth_mode: AuthMode::LocalPassword,
        web_transport: WebTransport::LocalHttp,
        pam_service: String::new(),
        tls_cert: PathBuf::new(),
        tls_key: PathBuf::new(),
        tls_ca: None,
        extractor: None,
        transcription: None,
        embed_origins: vec![],
        document_python: Some(python_path(home)),
        pdf_backend: PdfBackend::Pypdf,
    };
    config.save(&stage)?;
    platform::private_create(
        &stage.join("native-installation.json"),
        &serde_json::to_vec(&json!({"schema":1,"id":config.id,"update_owner":"installation"}))?,
    )?;
    platform::copy_self(&APPLICATION, &stage)?;
    platform::private_create(
        &stage.join("secrets/first-account.json"),
        &serde_json::to_vec(
            &json!({"installation_id":config.id,"token":auth::random_token(),"expires_at":crate::state::now_ms()+24*3600*1000_i64}),
        )?,
    )?;
    drop(AppState::open(stage.clone())?);
    platform::publish_directory_if_absent(&stage, home)?;
    Ok(())
}
pub async fn launch(home: PathBuf, open_browser: bool) -> Result<()> {
    if !home.join("config.json").exists() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let target = home.clone();
        tokio::task::spawn_blocking(move || create(&target, port)).await??;
        return crate::server::serve_with_browser(home, open_browser, Some(listener)).await;
    }
    let config = Config::load(&home)?;
    if crate::native_update_offer::different_download(&home)? {
        // A reopened interrupted download must retain recovery even when the
        // old listener has stopped. Preparation binds it to the same plan.
        let pending = crate::native_update::active(&home)?.is_some_and(|j| {
            j["source"] == "local_download"
                && !["succeeded", "rolled_back", "failed"]
                    .contains(&j["state"].as_str().unwrap_or(""))
        });
        if !pending {
            let status = local_api(&home, "GET", "status", None).await.context(
                "Open the installed Persona Workshop first, then reopen this download to update it",
            )?;
            ensure!(
                status["id"] == config.id,
                "A different installation answered"
            );
        }
        return crate::native_update_offer::serve(home, open_browser).await;
    }
    if let Ok(status) = local_api(&home, "GET", "status", None).await {
        ensure!(
            status["id"] == config.id,
            "A different installation answered at the saved address"
        );
        return ready(&home, &config, open_browser);
    }
    crate::server::serve_with_browser(home, open_browser, None).await
}
pub fn ready(home: &Path, config: &Config, open_browser: bool) -> Result<()> {
    let runtime: Value = serde_json::from_slice(&std::fs::read(home.join("runtime.json"))?)?;
    let mut url = if config.public_url.is_empty() {
        runtime["url"]
            .as_str()
            .context("Missing local address")?
            .to_owned()
    } else {
        config.public_url.clone()
    };
    if config.web_transport == WebTransport::LocalHttp
        && !home.join("secrets/local-password.json").exists()
    {
        let setup: Value =
            serde_json::from_slice(&std::fs::read(home.join("secrets/first-account.json"))?)?;
        ensure!(
            setup["installation_id"] == config.id,
            "Setup capability belongs to another installation"
        );
        let setup = if setup["expires_at"].as_i64().unwrap_or(0) <= crate::state::now_ms() {
            let refreshed = json!({"installation_id":config.id,"token":auth::random_token(),"expires_at":crate::state::now_ms()+24*3600*1000_i64});
            platform::private_write(
                &home.join("secrets/first-account.json"),
                &serde_json::to_vec(&refreshed)?,
            )?;
            refreshed
        } else {
            setup
        };
        let token = setup["token"]
            .as_str()
            .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .context("Invalid local setup capability")?;
        url.push_str(&format!("/#setup={token}"));
    }
    if open_browser {
        if url.starts_with("https:") {
            platform::open_https_browser(&url)?;
        } else {
            platform::open_browser(&url)?;
        }
    } else {
        println!(
            "{}",
            json!({"ready":true,"home":home,"url":url,"id":config.id})
        );
    }
    Ok(())
}
pub fn set_startup(home: &Path, enabled: bool) -> Result<()> {
    let config = Config::load(home)?;
    ensure!(
        config.auth_mode == AuthMode::LocalPassword,
        "Startup for this existing installation remains owner-managed"
    );
    let id = uuid::Uuid::parse_str(&config.id)?;
    let result = if enabled {
        platform::install_startup(&APPLICATION, home, &id)
    } else {
        platform::remove_startup(&APPLICATION, home, &id)
    };
    let receipt = json!({"requested":enabled,"configured":result.is_ok().then_some(enabled),"error":result.as_ref().err().map(ToString::to_string)});
    platform::private_write(
        &home.join("startup.json"),
        &serde_json::to_vec_pretty(&receipt)?,
    )?;
    result
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Startup {
    enabled: bool,
}
pub async fn startup_get(
    State(state): State<Arc<crate::state::AppState>>,
    headers: HeaderMap,
) -> crate::error::ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", false)?;
    let status = std::fs::read(state.home.join("startup.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .unwrap_or(json!({"configured":false}));
    Ok(Json(status))
}
pub async fn startup_set(
    State(state): State<Arc<crate::state::AppState>>,
    headers: HeaderMap,
    Json(input): Json<Startup>,
) -> crate::error::ApiResult<Json<Value>> {
    auth::check(&state, &headers, "admin", true)?;
    set_startup(&state.home, input.enabled)?;
    Ok(Json(json!({"configured":input.enabled})))
}

pub async fn local_api(
    home: &Path,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value> {
    ensure!(
        !path.starts_with('/') && !path.contains(".."),
        "Use a relative API path"
    );
    let c = Config::load(home)?;
    let token = std::fs::read_to_string(home.join("secrets/control-token"))?;
    if c.web_transport == WebTransport::LocalHttp {
        return tokio::time::timeout(
            Duration::from_secs(15),
            proved_local_api(&c, token.trim(), method, path, body),
        )
        .await
        .context("Local Persona Workshop did not respond in time")?;
    }
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15));
    let ca = std::fs::read(c.tls_ca.as_ref().unwrap_or(&c.tls_cert))?;
    builder = builder.add_root_certificate(reqwest::Certificate::from_pem(&ca)?);
    let runtime: Value = serde_json::from_slice(&std::fs::read(home.join("runtime.json"))?)?;
    let port = runtime["port"].as_u64().context("Missing listening port")?;
    let base = format!("https://127.0.0.1:{port}");
    let mut request = builder
        .build()?
        .request(method.parse()?, format!("{base}/api/v1/{path}"))
        .bearer_auth(token.trim());
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await?;
    let status = response.status();
    let result: serde_json::Value = response.json().await?;
    ensure!(
        status.is_success(),
        "{}",
        result["error"].as_str().unwrap_or("Request failed")
    );
    Ok(result)
}

// Keep proof and authenticated request on one connection. A client pool could
// reconnect to a different process after proof and disclose the credential.
async fn proved_local_api(
    config: &Config,
    token: &str,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value> {
    use axum::body::Bytes;
    use http_body_util::{BodyExt, Full, Limited};
    use hyper_util::rt::TokioIo;

    struct Connection(tokio::task::JoinHandle<()>);
    impl Drop for Connection {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", config.port)).await?;
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake::<_, Full<Bytes>>(TokioIo::new(stream)).await?;
    let _connection = Connection(tokio::spawn(async move {
        let _ = connection.await;
    }));
    let host = config
        .public_url
        .strip_prefix("http://")
        .context("Invalid local origin")?;
    let nonce = auth::random_token();
    let proof_request = hyper::Request::builder()
        .method("GET")
        .uri(format!("/api/v1/local-proof?nonce={nonce}"))
        .header(header::HOST, host)
        .body(Full::new(Bytes::new()))?;
    let response = sender.send_request(proof_request).await?;
    ensure!(
        response.status().is_success(),
        "Local installation proof was refused"
    );
    let bytes = Limited::new(response.into_body(), 4096)
        .collect()
        .await
        .map_err(|_| anyhow::anyhow!("Invalid local installation proof response"))?
        .to_bytes();
    let proof: serde_json::Value = serde_json::from_slice(&bytes)?;
    ensure!(
        auth::verify_local_proof(config, &nonce, token, &proof),
        "A different installation answered at the saved address"
    );
    let mut request = hyper::Request::builder()
        .method(method)
        .uri(format!("/api/v1/{path}"))
        .header(header::HOST, host)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let bytes = if let Some(body) = body {
        request = request.header(header::CONTENT_TYPE, "application/json");
        serde_json::to_vec(&body)?
    } else {
        Vec::new()
    };
    let response = sender
        .send_request(request.body(Full::new(Bytes::from(bytes)))?)
        .await?;
    let status = response.status();
    let bytes = Limited::new(response.into_body(), 16 * 1024 * 1024)
        .collect()
        .await
        .map_err(|_| anyhow::anyhow!("Invalid or excessive local API response"))?
        .to_bytes();
    let result: serde_json::Value = serde_json::from_slice(&bytes)?;
    ensure!(
        status.is_success(),
        "{}",
        result["error"].as_str().unwrap_or("Request failed")
    );
    Ok(result)
}

/// Bundled setup dependencies belong to this installation, never the system PATH.
pub fn python_path(home: &Path) -> PathBuf {
    home.join("runtime/python").join(if cfg!(windows) {
        "python.exe"
    } else {
        "bin/python3"
    })
}
fn install_support(home: &Path) -> Result<()> {
    copy_download_support(&std::env::current_exe()?, home)
}

/// Inspect the complete download without changing an existing installation.
/// The caller chooses only its own executable, never a path from the browser.
pub fn runtime_source(binary: &Path) -> Result<PathBuf> {
    let parent = binary.parent().context("No application folder")?;
    let bundle = if cfg!(target_os = "macos") && parent.ends_with("MacOS") {
        parent
            .parent()
            .context("No application resources")?
            .join("Resources")
    } else {
        parent.to_owned()
    };
    let (manifest, _) = download_manifest(&bundle)?;
    for (name, expected) in manifest["python_files"].as_object().unwrap() {
        let _ = download_member(&bundle, name, expected)?;
    }
    let python = bundle.join("runtime/python").join(if cfg!(windows) {
        "python.exe"
    } else {
        "bin/python3"
    });
    ensure!(python.is_file(), "Private Python executable missing");
    Ok(bundle)
}
fn download_manifest(bundle: &Path) -> Result<(Value, Vec<u8>)> {
    let path = bundle.join("native-bundle.json");
    let metadata = std::fs::symlink_metadata(&path)
        .context("Open the complete Persona Workshop download, with its private runtime folder")?;
    ensure!(
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && metadata.len() <= 4 * 1024 * 1024,
        "Missing or invalid native bundle manifest"
    );
    let raw = std::fs::read(path)?;
    let manifest: Value = serde_json::from_slice(&raw)?;
    ensure!(
        manifest["schema"] == 1 && manifest["component"] == "persona-workshop",
        "Invalid native bundle manifest"
    );
    let files = manifest["python_files"]
        .as_object()
        .context("Missing private runtime inventory")?;
    ensure!(
        !files.is_empty() && files.len() < 20000,
        "Invalid private runtime inventory"
    );
    Ok((manifest, raw))
}
fn download_member(
    bundle: &Path,
    name: &str,
    expected: &Value,
) -> Result<(Vec<u8>, std::fs::Permissions)> {
    use sha2::{Digest, Sha256};
    let path = Path::new(name);
    ensure!(
        path.components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
            && !name.contains(['\\', ':']),
        "Invalid runtime member"
    );
    let source = bundle.join("runtime/python").join(path);
    let metadata = std::fs::symlink_metadata(&source)?;
    ensure!(
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && metadata.len() <= 256 * 1024 * 1024,
        "Invalid private runtime member"
    );
    let bytes = std::fs::read(&source)?;
    ensure!(
        expected.as_str() == Some(format!("{:x}", Sha256::digest(&bytes)).as_str()),
        "Private runtime file failed verification"
    );
    Ok((bytes, metadata.permissions()))
}
fn private_runtime_parent(root: &Path, relative: &Path) -> Result<()> {
    crate::native_update_service::ensure_directory(root)?;
    let mut next = root.to_owned();
    for part in relative.components() {
        let std::path::Component::Normal(part) = part else {
            anyhow::bail!("Invalid runtime directory");
        };
        next.push(part);
        crate::native_update_service::ensure_directory(&next)?;
    }
    Ok(())
}
/// Copy the verified download into a fresh destination's private runtime.
pub fn copy_download_support(binary: &Path, destination_home: &Path) -> Result<()> {
    let bundle = runtime_source(binary)?;
    let (manifest, raw) = download_manifest(&bundle)?;
    let target = destination_home.join("runtime/python");
    private_runtime_parent(destination_home, Path::new("runtime/python"))?;
    for (name, expected) in manifest["python_files"].as_object().unwrap() {
        let (bytes, permissions) = download_member(&bundle, name, expected)?;
        let relative = Path::new(name);
        private_runtime_parent(
            &target,
            relative.parent().context("Missing runtime parent")?,
        )?;
        let output = target.join(relative);
        platform::private_create(&output, &bytes)?;
        #[cfg(unix)]
        std::fs::set_permissions(&output, permissions)?;
        #[cfg(not(unix))]
        let _ = permissions;
    }
    ensure!(
        python_path(destination_home).is_file(),
        "Private Python executable missing"
    );
    platform::private_create(&destination_home.join("runtime/native-bundle.json"), &raw)?;
    Ok(())
}

/// Copy only verified installation-owned runtime files for the independent updater.
pub fn copy_runtime(source: &Path, destination: &Path) -> Result<()> {
    use sha2::{Digest, Sha256};
    ensure!(!destination.exists(), "Worker runtime already exists");
    let raw = std::fs::read(source.join("native-bundle.json"))?;
    ensure!(
        raw.len() <= 4 * 1024 * 1024,
        "Runtime inventory exceeds bound"
    );
    let manifest: Value = serde_json::from_slice(&raw)?;
    ensure!(
        manifest["component"] == "persona-workshop",
        "Unexpected private runtime"
    );
    let files = manifest["python_files"]
        .as_object()
        .context("Missing private runtime inventory")?;
    ensure!(
        !files.is_empty() && files.len() <= 20000,
        "Invalid private runtime inventory"
    );
    platform::private_dir(destination)?;
    for (name, expected) in files {
        let path = Path::new(name);
        ensure!(
            path.components()
                .all(|c| matches!(c, std::path::Component::Normal(_)))
                && !name.contains(['\\', ':']),
            "Unsafe private runtime member"
        );
        let input = source.join("python").join(path);
        let m = std::fs::symlink_metadata(&input)?;
        ensure!(
            m.is_file() && m.len() <= 256 * 1024 * 1024,
            "Invalid private runtime file"
        );
        let bytes = std::fs::read(&input)?;
        ensure!(
            expected.as_str() == Some(format!("{:x}", Sha256::digest(&bytes)).as_str()),
            "Private runtime checksum mismatch"
        );
        let output = destination.join("python").join(path);
        std::fs::create_dir_all(output.parent().context("Missing file parent")?)?;
        std::fs::write(&output, bytes)?;
        std::fs::set_permissions(&output, m.permissions())?;
    }
    platform::private_write(&destination.join("native-bundle.json"), &raw)?;
    Ok(())
}

#[cfg(test)]
mod download_tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn bundle(root: &Path) -> PathBuf {
        let binary = root.join(if cfg!(windows) {
            "persona-workshop.exe"
        } else {
            "persona-workshop"
        });
        let name = if cfg!(windows) {
            "python.exe"
        } else {
            "bin/python3"
        };
        let python = root.join("runtime/python").join(name);
        std::fs::create_dir_all(python.parent().unwrap()).unwrap();
        std::fs::write(&binary, b"fixture application").unwrap();
        std::fs::write(python, b"fixture interpreter").unwrap();
        std::fs::write(
            root.join("native-bundle.json"),
            serde_json::to_vec(&json!({
                "schema":1,"component":"persona-workshop","version":env!("CARGO_PKG_VERSION"),
                "python_files":{name:format!("{:x}",Sha256::digest(b"fixture interpreter"))}
            }))
            .unwrap(),
        )
        .unwrap();
        binary
    }
    #[test]
    fn complete_download_is_copied_to_fresh_private_runtime_without_overwrite() {
        let temp = tempfile::tempdir().unwrap();
        let binary = bundle(&temp.path().join("download"));
        let home = temp.path().join("destination");
        assert_eq!(runtime_source(&binary).unwrap(), binary.parent().unwrap());
        copy_download_support(&binary, &home).unwrap();
        assert_eq!(
            std::fs::read(python_path(&home)).unwrap(),
            b"fixture interpreter"
        );
        let manifest = std::fs::read(home.join("runtime/native-bundle.json")).unwrap();
        assert_eq!(
            manifest,
            std::fs::read(binary.parent().unwrap().join("native-bundle.json")).unwrap()
        );
        assert!(copy_download_support(&binary, &home).is_err());
        assert_eq!(
            std::fs::read(home.join("runtime/native-bundle.json")).unwrap(),
            manifest
        );
    }
    #[test]
    fn tampered_download_is_refused_before_destination_creation() {
        let temp = tempfile::tempdir().unwrap();
        let binary = bundle(&temp.path().join("download"));
        std::fs::write(
            python_path(binary.parent().unwrap()),
            b"changed interpreter",
        )
        .unwrap();
        let home = temp.path().join("must-not-exist");
        assert!(copy_download_support(&binary, &home).is_err());
        assert!(!home.exists());
    }
    #[test]
    fn unsafe_inventory_member_never_reads_or_copies_outside_bundle() {
        let temp = tempfile::tempdir().unwrap();
        let binary = bundle(&temp.path().join("download"));
        let manifest = binary.parent().unwrap().join("native-bundle.json");
        let mut value: Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
        value["python_files"] = json!({"../outside":"untrusted"});
        std::fs::write(manifest, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(runtime_source(&binary).is_err());
    }
}
