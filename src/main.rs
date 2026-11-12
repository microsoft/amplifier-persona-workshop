#![cfg_attr(windows, windows_subsystem = "windows")]
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use clap::{Parser, Subcommand};
use persona_workshop::{
    config::{self, Config, Extractor},
    server,
    state::AppState,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    version,
    about = "Build evidence-backed communication and working profiles for labeled simulations"
)]
struct Cli {
    #[arg(long)]
    home: Option<PathBuf>,
    #[arg(long)]
    no_browser: bool,
    #[command(subcommand)]
    action: Option<Action>,
}
#[derive(Subcommand)]
enum Action {
    Init {
        #[arg(long)]
        owner: Option<String>,
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        #[arg(long, default_value_t = 0)]
        port: u16,
        #[arg(long, default_value = "")]
        public_url: String,
        #[arg(long)]
        extractor_config: Option<PathBuf>,
    },
    Serve,
    Status,
    Api {
        method: String,
        path: String,
        #[arg(long)]
        body_file: Option<PathBuf>,
    },
    Import {
        #[arg(long)]
        project: String,
        #[arg(long)]
        file: PathBuf,
        #[arg(long, default_value = "notes")]
        kind: String,
        #[arg(long)]
        collection: Vec<String>,
        #[arg(long)]
        author: Vec<String>,
        #[arg(long)]
        holdout: bool,
    },
    Export {
        #[arg(long)]
        project: String,
        #[arg(long)]
        revision: String,
        #[arg(long)]
        output: PathBuf,
        /// Native profile package, an AGENTS.md workspace, or a self-contained SOUL.md.
        #[arg(long, default_value = "native", value_parser = ["native", "agents", "soul"])]
        format: String,
        /// Preserve this existing AGENTS.md in the exported workspace; the input stays unchanged.
        #[arg(long)]
        agents_file: Option<PathBuf>,
    },
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}
#[derive(Subcommand)]
enum ConfigAction {
    Get,
    Set {
        #[arg(long)]
        file: PathBuf,
    },
}
#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let show_error = cli.action.is_none() && !cli.no_browser;
    if let Err(error) = run(cli).await {
        if show_error {
            amplifier_native_platform::show_launch_error(
                &persona_workshop::desktop::APPLICATION,
                &error.to_string(),
            );
        }
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}
async fn run(cli: Cli) -> Result<()> {
    let home = match cli.home {
        Some(home) => home,
        None => persona_workshop::desktop::default_home()?,
    };
    let Some(action) = cli.action else {
        return persona_workshop::desktop::launch(home, !cli.no_browser).await;
    };
    match action {
        Action::Init {
            owner,
            bind,
            port,
            public_url,
            extractor_config,
        } => {
            ensure!(
                !home.join("config.json").exists(),
                "This installation already exists"
            );
            config::private_dir(&home)?;
            let home = std::fs::canonicalize(home)?;
            let (tls_cert, tls_key, tls_ca) = generate_tls(&home)?;
            let extractor = extractor_config
                .map(|p| -> Result<Extractor> { Ok(serde_json::from_slice(&std::fs::read(p)?)?) })
                .transpose()?;
            let c = Config {
                id: uuid::Uuid::new_v4().to_string(),
                owner: owner
                    .or_else(|| std::env::var("USER").ok())
                    .context("Supply --owner")?,
                bind,
                port,
                public_url,
                pam_service: "persona-workshop".into(),
                auth_mode: Default::default(),
                web_transport: Default::default(),
                document_python: None,
                pdf_backend: Default::default(),
                tls_cert,
                tls_key,
                tls_ca,
                extractor,
                transcription: None,
                embed_origins: vec![],
            };
            c.save(&home)?;
            AppState::open(home.clone())?;
            println!(
                "{}",
                json!({"id":c.id,"home":home,"next":"Install the PAM service and run serve; choose an ACP analysis endpoint to build profiles"})
            );
        }
        Action::Serve => server::serve(home).await?,
        Action::Status => {
            println!(
                "{}",
                String::from_utf8(std::fs::read(home.join("runtime.json"))?)?
            );
        }
        Action::Api {
            method,
            path,
            body_file,
        } => {
            let body = body_file
                .map(|p| -> Result<Value> { Ok(serde_json::from_slice(&std::fs::read(p)?)?) })
                .transpose()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&api(&home, &method, &path, body).await?)?
            );
        }
        Action::Import {
            project,
            file,
            kind,
            collection,
            author,
            holdout,
        } => {
            uuid::Uuid::parse_str(&project)?;
            let p = api(&home, "GET", &format!("/api/v1/projects/{project}"), None).await?;
            ensure!(
                std::fs::metadata(&file)?.len() <= 20 * 1024 * 1024,
                "Files must be 20 MB or smaller"
            );
            let mut body = json!({"expected_revision":p["revision"],"filename":file.file_name().context("Source needs a filename")?.to_string_lossy(),"kind":kind,"content_base64":STANDARD.encode(std::fs::read(&file)?),"holdout":holdout});
            if !collection.is_empty() {
                body["collections"] = json!(collection);
            }
            if !author.is_empty() {
                body["authors"] = json!(author);
            }
            let value = api(
                &home,
                "POST",
                &format!("/api/v1/projects/{project}/file-import"),
                Some(body),
            )
            .await?;
            println!(
                "{}",
                json!({"id":value["id"],"revision":value["revision"],"stats":value["stats"]})
            );
        }
        Action::Export {
            project,
            revision,
            output,
            format,
            agents_file,
        } => {
            uuid::Uuid::parse_str(&project)?;
            uuid::Uuid::parse_str(&revision)?;
            ensure!(
                !output.exists(),
                "Choose a new output directory; existing files will not be overwritten"
            );
            ensure!(
                agents_file.is_none() || format == "agents",
                "Use --agents-file with --format agents"
            );
            let existing = agents_file
                .map(|path| -> Result<String> {
                    ensure!(
                        std::fs::metadata(&path)?.len()
                            <= persona_workshop::portable::MAX_EXISTING_BYTES as u64,
                        "AGENTS.md must be 256 KB or smaller"
                    );
                    Ok(std::fs::read_to_string(path)?)
                })
                .transpose()?;
            let route = if format == "native" {
                format!("/api/v1/projects/{project}/revisions/{revision}/export")
            } else {
                format!("/api/v1/projects/{project}/revisions/{revision}/exports/{format}")
            };
            let package = api(
                &home,
                if existing.is_some() { "POST" } else { "GET" },
                &route,
                existing.map(|text| json!({"existing_agents":text})),
            )
            .await?;
            let files = package["files"]
                .as_object()
                .context("Invalid profile package")?;
            for (name, value) in files {
                ensure!(
                    [
                        "communication.md",
                        "working-style.md",
                        "claims.json",
                        "manifest.json",
                        "evaluation-summary.json",
                        "AGENTS.md",
                        "SOUL.md",
                        "persona/communication.md",
                        "persona/working-style.md",
                        "persona/metadata/manifest.json",
                        "persona/metadata/source-manifest.json",
                        "persona/metadata/claims.json",
                        "persona/metadata/evaluation-summary.json"
                    ]
                    .contains(&name.as_str()),
                    "Unexpected package file"
                );
                let text = value.as_str().context("Invalid package content")?;
                if name != "manifest.json" && name != "persona/metadata/manifest.json" {
                    ensure!(
                        package["manifest"]["files"][name] == persona_workshop::auth::digest(text),
                        "Package digest mismatch"
                    );
                }
            }
            config::private_dir(&output)?;
            for (name, value) in files {
                if let Some(parent) = output.join(name).parent() {
                    config::private_dir(parent)?;
                }
                config::private_write(&output.join(name), value.as_str().unwrap().as_bytes())?;
            }
            println!(
                "Exported {format} profiles and separate provenance to {}",
                output.display()
            );
        }
        Action::Config { action } => match action {
            ConfigAction::Get => {
                println!("{}", serde_json::to_string_pretty(&Config::load(&home)?)?)
            }
            ConfigAction::Set { file } => {
                let c: Config = serde_json::from_slice(&std::fs::read(file)?)?;
                let old = Config::load(&home)?;
                ensure!(
                    c.id == old.id && c.owner == old.owner,
                    "Installation identity and owner are immutable"
                );
                c.save(&home)?;
                println!("Saved. Restart this workbench to apply the configuration.");
            }
        },
    }
    Ok(())
}
async fn api(home: &Path, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
    ensure!(
        path.starts_with("/api/v1/") && !path.contains(['\r', '\n']),
        "Use a local /api/v1/ path"
    );
    if Config::load(home)?.web_transport == config::WebTransport::LocalHttp {
        return persona_workshop::desktop::local_api(
            home,
            method,
            path.trim_start_matches("/api/v1/"),
            body,
        )
        .await;
    }
    let c = Config::load(home)?;
    let runtime: Value = serde_json::from_slice(&std::fs::read(home.join("runtime.json"))?)?;
    let mut client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());
    if let Some(ca) = c.tls_ca {
        client = client.add_root_certificate(reqwest::Certificate::from_pem(&std::fs::read(ca)?)?);
    }
    let client = client.build()?;
    let token = std::fs::read_to_string(home.join("secrets/control-token"))?;
    let url = format!(
        "https://127.0.0.1:{}{path}",
        runtime["port"].as_u64().context("Missing listening port")?
    );
    let mut request = client
        .request(method.parse()?, url)
        .bearer_auth(token.trim());
    if let Some(v) = body {
        request = request.json(&v);
    }
    let response = request.send().await?;
    let status = response.status();
    let result: Value = response.json().await?;
    ensure!(
        status.is_success(),
        "{}",
        result["error"].as_str().unwrap_or("Request failed")
    );
    Ok(result)
}

fn generate_tls(home: &Path) -> Result<(PathBuf, PathBuf, Option<PathBuf>)> {
    use rcgen::{
        BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose,
        IsCa, Issuer, KeyPair, KeyUsagePurpose,
    };
    config::private_dir(&home.join("tls"))?;
    let mut ca = CertificateParams::new(vec![])?;
    let mut issuer_name = DistinguishedName::new();
    issuer_name.push(
        DnType::CommonName,
        format!("Persona Workshop CA {}", uuid::Uuid::new_v4()),
    );
    ca.distinguished_name = issuer_name;
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let ca_key = KeyPair::generate()?;
    let ca_cert = ca.self_signed(&ca_key)?;
    config::private_write(&home.join("tls/root-ca.crt"), ca_cert.pem().as_bytes())?;
    config::private_write(
        &home.join("tls/root-ca.key"),
        ca_key.serialize_pem().as_bytes(),
    )?;
    let issuer = Issuer::new(ca, ca_key);
    let mut names = vec![
        "localhost".to_owned(),
        "127.0.0.1".to_owned(),
        "::1".to_owned(),
    ];
    if let Ok(addresses) = if_addrs::get_if_addrs() {
        for a in addresses {
            let ip = a.ip().to_string();
            if !names.contains(&ip) {
                names.push(ip);
            }
        }
    }
    let mut leaf = CertificateParams::new(names)?;
    let mut service_name = DistinguishedName::new();
    service_name.push(DnType::CommonName, "Persona Workshop service");
    leaf.distinguished_name = service_name;
    leaf.use_authority_key_identifier_extension = true;
    leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let key = KeyPair::generate()?;
    let cert = leaf.signed_by(&key, &issuer)?;
    config::private_write(&home.join("tls/cert.pem"), cert.pem().as_bytes())?;
    config::private_write(&home.join("tls/key.pem"), key.serialize_pem().as_bytes())?;
    Ok((
        home.join("tls/cert.pem"),
        home.join("tls/key.pem"),
        Some(home.join("tls/root-ca.crt")),
    ))
}
