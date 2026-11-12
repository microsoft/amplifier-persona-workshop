use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Extractor {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub cwd: String,
    #[serde(default = "timeout")]
    pub timeout_seconds: u64,
}
fn timeout() -> u64 {
    300
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthMode {
    #[default]
    Pam,
    LocalPassword,
}
impl AuthMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pam => "pam",
            Self::LocalPassword => "local-password",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WebTransport {
    #[default]
    Https,
    LocalHttp,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PdfBackend {
    #[default]
    Pdftotext,
    Pypdf,
}
impl PdfBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pdftotext => "pdftotext",
            Self::Pypdf => "pypdf",
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub id: String,
    pub owner: String,
    pub bind: String,
    pub port: u16,
    pub public_url: String,
    #[serde(default)]
    pub auth_mode: AuthMode,
    #[serde(default)]
    pub web_transport: WebTransport,
    #[serde(default)]
    pub pam_service: String,
    pub tls_cert: PathBuf,
    pub tls_key: PathBuf,
    pub tls_ca: Option<PathBuf>,
    pub extractor: Option<Extractor>,
    #[serde(default)]
    pub transcription: Option<Extractor>,
    #[serde(default)]
    pub document_python: Option<PathBuf>,
    #[serde(default)]
    pub pdf_backend: PdfBackend,
    #[serde(default)]
    pub embed_origins: Vec<String>,
}
impl Config {
    pub fn load(home: &Path) -> Result<Self> {
        let c: Self = serde_json::from_slice(&std::fs::read(home.join("config.json"))?)?;
        c.validate()?;
        Ok(c)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.owner.is_empty() && self.owner.len() < 100,
            "Choose an owner account"
        );
        match self.web_transport {
            WebTransport::Https => ensure!(
                self.public_url.is_empty() || valid_origin(&self.public_url),
                "Use an HTTPS address without a path"
            ),
            WebTransport::LocalHttp => ensure!(
                self.bind == "127.0.0.1"
                    && self.port != 0
                    && self.auth_mode == AuthMode::LocalPassword
                    && [
                        format!("http://127.0.0.1:{}", self.port),
                        format!("http://localhost:{}", self.port)
                    ]
                    .contains(&self.public_url)
                    && self.embed_origins.is_empty(),
                "Local HTTP needs a loopback-only address, matching origin and local password"
            ),
        }
        ensure!(
            self.document_python
                .as_ref()
                .is_none_or(|p| p.is_absolute()),
            "Document interpreter must be an absolute private path"
        );
        ensure!(
            self.pdf_backend != PdfBackend::Pypdf || self.document_python.is_some(),
            "The bundled PDF converter requires its private interpreter"
        );
        ensure!(
            self.embed_origins.iter().all(|s| valid_origin(s)),
            "Embedding origins must be HTTPS origins"
        );
        for e in self.extractor.iter().chain(self.transcription.iter()) {
            ensure!(
                !e.command.is_empty() && Path::new(&e.cwd).is_absolute(),
                "Configure an executable and absolute working folder"
            );
            ensure!(
                (10..=1800).contains(&e.timeout_seconds),
                "Extractor timeout must be 10–1800 seconds"
            );
        }
        Ok(())
    }
    pub fn save(&self, home: &Path) -> Result<()> {
        self.validate()?;
        private_write(&home.join("config.json"), &serde_json::to_vec_pretty(self)?)
    }
}
pub fn valid_origin(value: &str) -> bool {
    reqwest::Url::parse(value).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
            && u.query().is_none()
            && u.fragment().is_none()
            && u.path() == "/"
    })
}
pub fn private_dir(path: &Path) -> Result<()> {
    amplifier_native_platform::private_dir(path)
}
pub fn private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    amplifier_native_platform::private_write(path, bytes)
}
pub fn private_create(path: &Path, bytes: &[u8]) -> Result<()> {
    amplifier_native_platform::private_create(path, bytes)
}
