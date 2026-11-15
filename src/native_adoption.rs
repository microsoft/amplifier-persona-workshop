//! Explicit adoption of an existing Linux user service. The maintenance owner
//! stops the old service; this module never invents an old admission protocol.
//! Native package/update machinery owns the replacement after stopped-state proof.
use crate::{
    config::{AuthMode, Config, WebTransport},
    native_update,
};
use amplifier_native_platform::{self as platform, installation as manager, process};
use anyhow::{Context, Result, ensure};
use rusqlite::OpenFlags;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn read(path: &Path) -> Result<Value> {
    let m = fs::symlink_metadata(path)?;
    ensure!(
        m.is_file() && !m.file_type().is_symlink() && m.len() < 4 * 1024 * 1024,
        "Invalid adoption record"
    );
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
fn digest(path: &Path) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(fs::read(path)?)))
}
fn save(path: &Path, value: &Value) -> Result<()> {
    platform::private_write_data(path, &serde_json::to_vec_pretty(value)?)
}

pub fn owner(home: &Path) -> Result<Value> {
    let native = home.join("native-installation.json");
    if native.exists() {
        return read(&native);
    }
    let pending = read(&home.join("native-adoption.json"))?;
    let active = native_update::active(home)?.context("No adoption operation")?;
    ensure!(
        active["id"] == pending["plan_id"] && active["legacy_adoption"] == true,
        "Legacy adoption no longer owns this operation"
    );
    Ok(pending["owner"].clone())
}
pub fn legacy(home: &Path) -> Result<Option<Value>> {
    if !home.join("native-installation.json").exists()
        && !home.join("native-adoption.json").exists()
    {
        return Ok(None);
    }
    let record = owner(home)?;
    if record["legacy"].is_null() {
        return Ok(None);
    }
    let profile = &record["legacy"];
    let cfg = Config::load(home)?;
    ensure!(
        cfg!(target_os = "linux")
            && cfg.auth_mode == AuthMode::Pam
            && cfg.web_transport == WebTransport::Https,
        "Legacy adoption preserves Linux PAM and HTTPS"
    );
    ensure!(
        record["schema"] == 1
            && record["id"] == cfg.id
            && record["update_owner"] == "installation"
            && profile["schema"] == 1
            && profile["home"] == fs::canonicalize(home)?.to_string_lossy().as_ref(),
        "Legacy installation identity changed"
    );
    let binary = PathBuf::from(
        profile["binary"]
            .as_str()
            .context("Missing legacy executable")?,
    );
    ensure!(
        binary.is_absolute()
            && binary.parent() == Some(home.join("bin").as_path())
            && !binary.is_symlink(),
        "Legacy executable must remain in this installation's bin directory"
    );
    let _: manager::Service = serde_json::from_value(profile["service"].clone())?;
    Ok(Some(profile.clone()))
}
pub fn installed_binary(home: &Path) -> Result<PathBuf> {
    Ok(if let Some(profile) = legacy(home)? {
        PathBuf::from(profile["binary"].as_str().unwrap())
    } else {
        platform::installed_binary(&crate::desktop::APPLICATION, home)
    })
}
pub fn verify_profile(home: &Path, profile: &Value, snapshot: &Value) -> Result<()> {
    ensure!(
        profile["home"] == fs::canonicalize(home)?.to_string_lossy().as_ref(),
        "Legacy home changed"
    );
    for key in [
        "service",
        "definition",
        "effective_definition",
        "enabled",
        "masked",
        "start_allowed",
    ] {
        ensure!(
            profile["baseline"][key] == snapshot[key],
            "Original service definition or startup preference changed"
        );
    }
    ensure!(
        snapshot["definition_matches"] == true
            && snapshot["masked"] == false
            && snapshot["start_allowed"] == true,
        "Original service cannot be started by its owner"
    );
    Ok(())
}
fn original_cgroup(snapshot: &Value) -> Result<String> {
    let pid = snapshot["native"]["MainPID"]
        .as_str()
        .context("Missing original process")?
        .parse::<u32>()?;
    let before = process::identity(pid)?.context("Original process disappeared")?;
    ensure!(
        snapshot["processes"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["identity"] == before)),
        "Service generation changed during cgroup observation"
    );
    let groups = fs::read_to_string(format!("/proc/{pid}/cgroup"))?;
    ensure!(
        process::matches(&before)?,
        "Original process generation changed"
    );
    let group = groups
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .context("Existing-service adoption requires unified cgroup observations")?;
    ensure!(
        group.starts_with('/') && !group.split('/').any(|x| x == "..") && !group.contains('\\'),
        "Invalid service cgroup"
    );
    Ok(group.to_owned())
}
fn group_empty(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    ensure!(!path.is_symlink(), "Invalid service cgroup");
    ensure!(
        fs::read_to_string(path.join("cgroup.procs"))?
            .trim()
            .is_empty(),
        "Original service still has child processes"
    );
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            group_empty(&entry.path())?;
        }
    }
    Ok(())
}
pub fn stopped(home: &Path, profile: &Value) -> Result<()> {
    let service: manager::Service = serde_json::from_value(profile["service"].clone())?;
    let snapshot = manager::snapshot(&service)?;
    verify_profile(home, profile, &snapshot)?;
    ensure!(
        snapshot["active"] == false
            && snapshot["transitioning"] == false
            && snapshot["processes"].as_array().is_some_and(Vec::is_empty),
        "Stop the original service after its work settles before adopting it"
    );
    for p in profile["baseline"]["processes"]
        .as_array()
        .context("Missing original process observations")?
    {
        ensure!(
            !process::matches(&p["identity"])?,
            "Original process generation still exists"
        );
    }
    let group = profile["control_group"]
        .as_str()
        .context("Missing original cgroup")?;
    ensure!(
        group.starts_with('/') && !group.split('/').any(|x| x == "..") && !group.contains('\\'),
        "Invalid service cgroup path"
    );
    group_empty(&Path::new("/sys/fs/cgroup").join(group.trim_start_matches('/')))?;
    if let Some(port) = profile["listener_port"].as_u64() {
        ensure!(
            port > 0 && port <= u16::MAX as u64,
            "Invalid original listener"
        );
        let _vacant = std::net::TcpListener::bind(("127.0.0.1", port as u16))
            .context("The original listener is still occupied")?;
    }
    Ok(())
}
pub fn settled_jobs(home: &Path) -> Result<()> {
    // Never open AppState: doing so changes retained active jobs to interrupted.
    let db = rusqlite::Connection::open_with_flags(
        home.join("state.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let pending: i64 = db.query_row(
        "SELECT count(*) FROM jobs WHERE json_extract(value,'$.status') IN ('queued','running')",
        [],
        |r| r.get(0),
    )?;
    ensure!(
        pending == 0,
        "Retained work is queued or running; inspect its effects before adoption"
    );
    Ok(())
}

/// Capture the actual old service before maintenance. This never stops it or
/// writes a native baseline. A retry of the same request returns the same plan.
pub fn prepare(home: &Path, name: &str, binary: &Path, request: &str) -> Result<Value> {
    ensure!(
        cfg!(target_os = "linux"),
        "Existing-service adoption currently supports Linux user services"
    );
    let home = fs::canonicalize(home)?;
    let cfg = Config::load(&home)?;
    ensure!(
        cfg.auth_mode == AuthMode::Pam && cfg.web_transport == WebTransport::Https,
        "Use existing-service adoption only for PAM/HTTPS homes"
    );
    let request = uuid::Uuid::parse_str(request)?.to_string();
    let current = std::env::current_exe()?;
    let resources = crate::desktop::runtime_source(&current)?;
    let manifest = read(&resources.join("native-bundle.json"))?;
    ensure!(
        manifest["version"] == env!("CARGO_PKG_VERSION"),
        "Complete candidate version changed"
    );
    let candidate_hash = digest(&current)?;
    let manifest_hash = digest(&resources.join("native-bundle.json"))?;
    let root = home.join("native-updates");
    crate::native_update_service::ensure_directory(&root)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("queue.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock).context("Another update is being prepared")?;
    let record = home.join("native-adoption.json");
    let active = native_update::active(&home)?;
    let mut prior_origin = None;
    let retained = if record.exists() {
        let old = read(&record)?;
        ensure!(
            old["owner"]["legacy"]["service"]["name"] == name
                && old["owner"]["legacy"]["binary"] == binary.to_string_lossy().as_ref(),
            "A different original service is retained"
        );
        if old["plan_id"] == request {
            ensure!(
                old["binary_sha256"] == candidate_hash
                    && old["runtime_manifest_sha256"] == manifest_hash,
                "The same request cannot select a different complete download"
            );
            if active
                .as_ref()
                .is_some_and(|a| a["id"] == request && a["legacy_adoption"] == true)
            {
                return Ok(old);
            }
            ensure!(
                active
                    .as_ref()
                    .is_none_or(|a| a["id"] == old["previous_plan_id"]
                        && ["failed", "rolled_back"].contains(&a["state"].as_str().unwrap_or(""))),
                "A different operation owns this installation"
            );
            Some(old)
        } else {
            let previous = active
                .as_ref()
                .context("Finish the retained preparation before selecting another download")?;
            ensure!(
                previous["id"] == old["plan_id"]
                    && previous["legacy_adoption"] == true
                    && ["failed", "rolled_back"]
                        .contains(&previous["state"].as_str().unwrap_or(""))
                    && !home.join("native-installation.json").exists(),
                "The retained adoption has not settled"
            );
            crate::native_update_service::settle_previous(&home)?;
            prior_origin = Some(old);
            None
        }
    } else {
        ensure!(
            active.is_none(),
            "Existing update ownership must be retained"
        );
        None
    };
    ensure!(
        !home.join("native-installation.json").exists(),
        "Existing native ownership must be retained"
    );
    ensure!(
        binary.is_absolute()
            && binary.parent() == Some(home.join("bin").as_path())
            && binary.is_file()
            && !binary.is_symlink(),
        "Choose the existing ordinary executable in this installation's bin directory"
    );
    let definitions = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap()).join(".config"))
        .join("systemd/user");
    let service = manager::Service {
        manager: manager::ServiceManager::SystemdUser,
        name: name.into(),
        definition_path: Some(definitions.join(name)),
    };
    let snapshot = manager::snapshot(&service)?;
    let pending = if let Some(old) = retained {
        verify_profile(&home, &old["owner"]["legacy"], &snapshot)?;
        ensure!(
            old["owner"]["legacy"]["original_sha256"] == digest(binary)?,
            "Original executable changed"
        );
        old
    } else {
        let profile = if let Some(old) = &prior_origin {
            verify_profile(&home, &old["owner"]["legacy"], &snapshot)?;
            ensure!(
                old["owner"]["legacy"]["original_sha256"] == digest(binary)?,
                "Original executable has not been restored"
            );
            if snapshot["active"] == true {
                let mut profile = old["owner"]["legacy"].clone();
                profile["baseline"] = snapshot.clone();
                profile["control_group"] = json!(original_cgroup(&snapshot)?);
                profile["listener_port"] = read(&home.join("runtime.json"))?["port"].clone();
                profile
            } else {
                old["owner"]["legacy"].clone()
            }
        } else {
            ensure!(
                snapshot["definition_matches"] == true
                    && snapshot["native"]["DropInPaths"] == ""
                    && snapshot["masked"] == false
                    && snapshot["start_allowed"] == true
                    && snapshot["active"] == true,
                "Capture the original running, loaded, unmasked user service before maintenance"
            );
            let expected = format!(
                "argv[]={} --home {} serve ;",
                binary.display(),
                home.display()
            );
            let exec = snapshot["native"]["ExecStart"]
                .as_str()
                .context("Missing effective executable")?;
            ensure!(
                exec.matches("argv[]=").count() == 1 && exec.contains(&expected),
                "Original service must directly run this executable and home"
            );
            let runtime = read(&home.join("runtime.json"))?;
            ensure!(
                runtime["id"] == cfg.id,
                "Original listener belongs to another installation"
            );
            json!({"schema":1,"home":home,"binary":binary,"service":service,"baseline":snapshot,
            "control_group":original_cgroup(&snapshot)?,"original_sha256":digest(binary)?,"listener_port":runtime["port"]})
        };
        let owner = json!({"schema":1,"id":cfg.id,"update_owner":"installation","legacy":profile});
        let pending = json!({"schema":1,"plan_id":request,"owner":owner,"origin":"observed_legacy_service","baseline_committed":false,
            "previous_plan_id":prior_origin.as_ref().map(|p|p["plan_id"].clone()),
            "binary_sha256":candidate_hash,"runtime_manifest_sha256":manifest_hash,"config_sha256":digest(&home.join("config.json"))?});
        if prior_origin.is_some() {
            save(&record, &pending)?;
        } else {
            platform::private_create(&record, &serde_json::to_vec_pretty(&pending)?)?;
        }
        pending
    };
    ensure!(
        pending["config_sha256"] == digest(&home.join("config.json"))?,
        "Configuration changed during preparation"
    );
    let folder = root.join(&request);
    crate::native_update_service::ensure_directory(&folder)?;
    let retained_origin = folder.join("legacy-origin.json");
    if retained_origin.exists() {
        ensure!(
            read(&retained_origin)? == pending,
            "Retained legacy origin changed"
        );
    } else {
        platform::private_create(&retained_origin, &serde_json::to_vec_pretty(&pending)?)?;
    }
    let candidate = folder.join("candidate");
    // Only this unpublished plan's staging tree can be discarded after an
    // interrupted preparation; no service or installed bytes have changed.
    if candidate.exists() {
        ensure!(
            !candidate.is_symlink(),
            "Invalid retained candidate directory"
        );
        fs::remove_dir_all(&candidate)?;
    }
    crate::native_update_service::ensure_directory(&candidate)?;
    crate::desktop::copy_download_support(&current, &candidate)?;
    platform::private_create(&candidate.join("persona-workshop"), &fs::read(&current)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            candidate.join("persona-workshop"),
            fs::Permissions::from_mode(0o700),
        )?;
    }
    platform::private_create(
        &candidate.join("native-bundle.json"),
        &fs::read(candidate.join("runtime/native-bundle.json"))?,
    )?;
    let plan = json!({"schema":1,"id":request,"state":"ready","action":"apply","source":"local_download","legacy_adoption":true,
        "entry":"persona-workshop","binary_sha256":digest(&current)?,"runtime_manifest_sha256":digest(&resources.join("native-bundle.json"))?,
        "installed_sha256":digest(binary)?,"config_sha256":digest(&home.join("config.json"))?,"version":env!("CARGO_PKG_VERSION"),
        "release_url":"","created_at":crate::state::now_ms(),"updated_at":crate::state::now_ms(),"up_to_date":false});
    save(&folder.join("plan.json"), &plan)?;
    save(&root.join("active.json"), &json!({"id":request}))?;
    Ok(pending)
}

pub fn apply(home: &Path, plan: &str, request: &str) -> Result<Value> {
    let profile = legacy(home)?.context("No legacy adoption plan")?;
    let current = native_update::active(home)?.context("No adoption plan")?;
    ensure!(
        current["id"] == plan && current["legacy_adoption"] == true,
        "A different update owns this home"
    );
    if current["state"] != "ready" {
        return Ok(current);
    }
    stopped(home, &profile)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(home.join("service.lock"))?;
    fs2::FileExt::try_lock_exclusive(&file).context("Original service is still running")?;
    settled_jobs(home)?;
    stopped(home, &profile)?;
    drop(file); // Worker repeats stopped/job checks under the same lock before exchange.
    native_update::apply_local(home, plan, request)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspecting_legacy_jobs_never_reconciles_or_restarts_them() {
        let dir = tempfile::tempdir().unwrap();
        let db = rusqlite::Connection::open(dir.path().join("state.sqlite3")).unwrap();
        db.execute("CREATE TABLE jobs(id TEXT,value TEXT)", [])
            .unwrap();
        for status in ["complete", "interrupted", "stale", "unknown"] {
            db.execute(
                "INSERT INTO jobs VALUES(?1,?2)",
                [
                    status,
                    &json!({"status":status,"note":"original"}).to_string(),
                ],
            )
            .unwrap();
        }
        settled_jobs(dir.path()).unwrap();
        for status in ["queued", "running"] {
            let original = json!({"status":status,"note":"keep unknown"}).to_string();
            db.execute("INSERT INTO jobs VALUES('pending',?1)", [&original])
                .unwrap();
            assert!(settled_jobs(dir.path()).is_err());
            let retained: String = db
                .query_row("SELECT value FROM jobs WHERE id='pending'", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(retained, original);
            db.execute("DELETE FROM jobs WHERE id='pending'", [])
                .unwrap();
        }
    }

    #[test]
    fn changed_original_service_or_startup_preference_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let baseline = json!({"service":{"name":"original.service"},"definition":{"sha256":"original"},
            "effective_definition":{"sha256":"effective"},"enabled":true,"masked":false,"start_allowed":true,"definition_matches":true});
        let profile = json!({"home":fs::canonicalize(dir.path()).unwrap(),"baseline":baseline});
        verify_profile(dir.path(), &profile, &baseline).unwrap();
        for (key, change) in [
            ("definition", json!({"sha256":"newer"})),
            ("enabled", json!(false)),
            ("masked", json!(true)),
            ("service", json!({"name":"another.service"})),
        ] {
            let mut changed = baseline.clone();
            changed[key] = change;
            assert!(verify_profile(dir.path(), &profile, &changed).is_err());
        }
    }

    #[test]
    fn remaining_cgroup_children_prevent_stopped_proof() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("cgroup.procs"), b"").unwrap();
        fs::create_dir(dir.path().join("child")).unwrap();
        fs::write(dir.path().join("child/cgroup.procs"), b"123\n").unwrap();
        assert!(group_empty(dir.path()).is_err());
        fs::write(dir.path().join("child/cgroup.procs"), b"").unwrap();
        group_empty(dir.path()).unwrap();
    }
}
