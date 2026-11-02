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
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub id: String,
    pub owner: String,
    pub bind: String,
    pub port: u16,
    pub public_url: String,
    pub pam_service: String,
    pub tls_cert: PathBuf,
    pub tls_key: PathBuf,
    pub tls_ca: Option<PathBuf>,
    pub extractor: Option<Extractor>,
    #[serde(default)]
    pub transcription: Option<Extractor>,
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
        ensure!(
            self.public_url.is_empty() || valid_origin(&self.public_url),
            "Use an HTTPS address without a path"
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
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
pub fn private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let tmp = path.with_file_name(format!(".write-{}", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(tmp, path)?;
    Ok(())
}
