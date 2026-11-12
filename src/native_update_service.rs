//! Fixed native update participants. These are application-owned profiles, not
//! arbitrary commands, new startup preferences, or another update scheduler.
use crate::{config::Config, desktop::APPLICATION, native_update};
use amplifier_native_platform::{self as platform, installation as manager, process, registration};
use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Execution {
    service: manager::Service,
    profile: Option<registration::Registration>,
    baseline: Value,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    schema: u32,
    installation_id: String,
    plan_id: String,
    request_id: String,
    worker: Execution,
    application: Option<Execution>,
    worker_sha256: String,
    python_sha256: String,
    control_sha256: String,
    original_application: Value,
}

fn read(path: &Path) -> Result<Value> {
    let m = fs::symlink_metadata(path)?;
    ensure!(
        m.is_file() && !m.file_type().is_symlink() && m.len() < 4 * 1024 * 1024,
        "Update manager record is not an ordinary bounded file"
    );
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
fn write(path: &Path, value: &impl Serialize) -> Result<()> {
    platform::private_write_data(path, &serde_json::to_vec_pretty(value)?)
}
pub(crate) fn ensure_directory(path: &Path) -> Result<()> {
    match platform::private_create_dir(path) {
        Ok(()) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(error.into()),
    }
    platform::validate_private_directory(path)
}

fn hash(path: &Path) -> Result<String> {
    let m = fs::symlink_metadata(path)?;
    ensure!(
        m.is_file() && !m.file_type().is_symlink(),
        "Expected an ordinary update file"
    );
    Ok(format!("{:x}", Sha256::digest(fs::read(path)?)))
}
fn id(value: &str) -> Result<&str> {
    ensure!(
        uuid::Uuid::parse_str(value)?.to_string() == value,
        "Invalid update identity"
    );
    Ok(value)
}
fn job(home: &Path, plan: &str) -> Result<Value> {
    id(plan)?;
    let j = native_update::active(home)?.context("No update is selected")?;
    ensure!(j["id"] == plan, "A different update is selected");
    Ok(j)
}
fn folder(home: &Path, plan: &str) -> Result<PathBuf> {
    Ok(home.join("native-updates").join(id(plan)?))
}
fn dispatch(home: &Path, plan: &str, request: &str) -> Result<PathBuf> {
    Ok(folder(home, plan)?.join("dispatch").join(id(request)?))
}
fn binding(home: &Path, j: &Value) -> Result<Binding> {
    let plan = j["id"].as_str().context("Missing update identity")?;
    let request = j
        .get("worker_request_id")
        .unwrap_or(&j["request_id"])
        .as_str()
        .context("Missing request identity")?;
    let b: Binding =
        serde_json::from_value(read(&dispatch(home, plan, request)?.join("manager.json"))?)?;
    ensure!(
        b.schema == 1
            && b.plan_id == plan
            && b.request_id == request
            && b.installation_id == Config::load(home)?.id,
        "Update manager binding changed"
    );
    Ok(b)
}
fn lock(home: &Path) -> Result<fs::File> {
    let path = home.join("native-updates/manager.lock");
    platform::private_create(&path, b"").or_else(|e| {
        if e.downcast_ref::<std::io::Error>()
            .is_some_and(|x| x.kind() == std::io::ErrorKind::AlreadyExists)
        {
            Ok(())
        } else {
            Err(e)
        }
    })?;
    ensure!(
        !path.is_symlink(),
        "Update manager lock is not an ordinary file"
    );
    let file = fs::OpenOptions::new().read(true).write(true).open(path)?;
    file.try_lock_exclusive()
        .context("Another native update operation is active")?;
    Ok(file)
}
fn service(home: &Path, label: String) -> Result<manager::Service> {
    #[cfg(unix)]
    let _ = home;
    #[cfg(target_os = "linux")]
    return Ok(manager::Service {
        manager: manager::ServiceManager::SystemdUser,
        definition_path: Some(
            platform::user_home()?
                .join(".config/systemd/user")
                .join(format!("{label}.service")),
        ),
        name: format!("{label}.service"),
    });
    #[cfg(target_os = "macos")]
    return Ok(manager::Service {
        manager: manager::ServiceManager::LaunchAgent,
        definition_path: Some(
            platform::user_home()?
                .join("Library/LaunchAgents")
                .join(format!("{label}.plist")),
        ),
        name: label,
    });
    #[cfg(windows)]
    return Ok(manager::Service {
        manager: manager::ServiceManager::ScheduledTask,
        definition_path: Some(
            home.join("native-updates/definitions")
                .join(format!("{label}.xml")),
        ),
        name: label,
    });
}
fn inactive(s: &Value) -> bool {
    s["active"] == false
        && s["transitioning"] == false
        && s["processes"].as_array().is_some_and(Vec::is_empty)
        && s["native"]["instances"]
            .as_array()
            .is_none_or(Vec::is_empty)
}
fn same_definition(before: &Value, now: &Value) -> bool {
    [
        "service",
        "definition",
        "effective_definition",
        "enabled",
        "masked",
        "start_allowed",
    ]
    .iter()
    .all(|key| before[*key] == now[*key])
}
fn verify_profile(home: &Path, execution: &Execution, observation: &Value) -> Result<()> {
    ensure!(
        same_definition(&execution.baseline, observation),
        "Application manager definition or startup preference changed"
    );
    ensure!(
        observation["start_allowed"] == true && observation["masked"] == false,
        "This native registration is independently disabled or masked"
    );
    if let Some(profile) = &execution.profile {
        ensure!(
            registration::profile_matches(&execution.service, profile, observation)?,
            "Native profile changed"
        );
    } else {
        verify_startup(home, &execution.service, observation)?;
    }
    Ok(())
}
fn verify_startup(home: &Path, svc: &manager::Service, observation: &Value) -> Result<()> {
    #[cfg(target_os = "linux")]
    let _ = svc;
    let binary = platform::installed_binary(&APPLICATION, home);
    #[cfg(target_os = "linux")]
    {
        let expected = platform::systemd_unit(&APPLICATION, &binary, home)?;
        ensure!(
            observation["definition"]["bytes"] == json!(expected.as_bytes())
                && observation["native"]["DropInPaths"] == "",
            "Existing Persona startup was changed independently"
        );
    }
    #[cfg(target_os = "macos")]
    {
        let expected = platform::launch_agent(&APPLICATION, &svc.name, &binary, home)?;
        ensure!(
            observation["definition"]["bytes"] == json!(expected.as_bytes()),
            "Existing Persona startup was changed independently"
        );
    }
    #[cfg(windows)]
    {
        ensure!(
            platform::task_matches(
                &APPLICATION,
                observation["native"]["xml"]
                    .as_str()
                    .context("Missing task definition")?,
                &binary,
                home,
                &platform::current_user_sid()?
            )?,
            "Existing Persona startup was changed independently"
        );
    }
    ensure!(
        observation["definition_matches"] == true,
        "Native registration does not match its definition"
    );
    Ok(())
}
fn registered(
    svc: &manager::Service,
    profile: &registration::Registration,
    attempt: &Path,
) -> Result<Execution> {
    if !attempt.exists() {
        platform::private_create(
            attempt,
            &serde_json::to_vec_pretty(&json!({"state":"issued","service":svc,"profile":profile}))?,
        )?;
        let outcome = match registration::register(svc, profile) {
            Ok(value) => serde_json::to_value(value)?,
            Err(error) => json!({"status":"unknown","message":error.to_string()}),
        };
        write(
            attempt,
            &json!({"state":outcome["status"],"service":svc,"profile":profile,"outcome":outcome}),
        )?;
    } else {
        let prior = read(attempt)?;
        ensure!(
            prior["service"] == serde_json::to_value(svc)?
                && prior["profile"] == serde_json::to_value(profile)?,
            "A different native registration attempt is retained"
        );
        // An interrupted registration can be reconciled by observing the exact
        // profile. Absence or error never causes another registration attempt.
    }
    // An exact no-autostart LaunchAgent is intentionally an unloaded plist.
    // Snapshot/profile proof reconciles that state without calling register
    // again; a missing definition or missing Windows task still fails here.
    let baseline = manager::snapshot(svc)?;
    ensure!(
        registration::profile_matches(svc, profile, &baseline)?,
        "Registered profile changed"
    );
    Ok(Execution {
        service: svc.clone(),
        profile: Some(profile.clone()),
        baseline,
    })
}
fn application(home: &Path, installation_id: &str) -> Result<Execution> {
    let startup = service(home, format!("persona-workshop-{installation_id}"))?;
    // Original Windows startup XML is merely a retained export; the native task
    // is authoritative. A missing task is distinct from a failed observation.
    #[cfg(windows)]
    let startup = manager::Service {
        definition_path: None,
        ..startup
    };
    let exists = manager::manager_registration_exists(&startup)?;
    #[cfg(unix)]
    let exists = exists || startup.definition_path.as_ref().is_some_and(|p| p.exists());
    if exists {
        let baseline = manager::snapshot(&startup)?;
        let execution = Execution {
            service: startup,
            profile: None,
            baseline,
        };
        verify_profile(home, &execution, &execution.baseline)?;
        return Ok(execution);
    }
    #[cfg(unix)]
    ensure!(
        !startup.definition_path.as_ref().unwrap().exists(),
        "An unloaded startup definition remains; inspect it before updating"
    );
    let svc = service(home, format!("persona-workshop-runtime-{installation_id}"))?;
    let profile = registration::Registration {
        executable: platform::installed_binary(&APPLICATION, home),
        args: vec![
            "--home".into(),
            home.to_string_lossy().into_owned(),
            "serve".into(),
        ],
        cwd: home.to_owned(),
        description: "Persona Workshop (opened by this installation)".into(),
        autostart: false,
    };
    Ok(Execution {
        service: svc,
        profile: Some(profile),
        baseline: Value::Null,
    })
}

/// Prepare only fixed profiles and immutable local worker bytes. Registration
/// does not start the worker or alter the existing sign-in registration.
pub fn prepare(home: &Path, plan: &str) -> Result<()> {
    let _guard = lock(home)?;
    let j = job(home, plan)?;
    let request = j["request_id"].as_str().context("Missing update request")?;
    let root = folder(home, plan)?;
    let d = dispatch(home, plan, request)?;
    ensure_directory(&root.join("dispatch"))?;
    ensure_directory(&d)?;
    ensure_directory(&home.join("native-updates/definitions"))?;
    let cfg = Config::load(home)?;
    let control = d.join(if cfg!(windows) {
        "manager-control.exe"
    } else {
        "manager-control"
    });
    let current = std::env::current_exe()?;
    if !control.exists() {
        platform::private_create(&control, &fs::read(&current)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&control, fs::Permissions::from_mode(0o700))?;
        }
    }
    ensure!(
        hash(&control)? == hash(&current)?,
        "This update retains a different native controller"
    );
    let python = root.join("worker-runtime/python").join(if cfg!(windows) {
        "python.exe"
    } else {
        "bin/python3"
    });
    let script = root.join("worker.py");
    let worker_service = service(home, format!("persona-update-{}-{request}", cfg.id))?;
    let profile = registration::Registration {
        executable: python.clone(),
        args: vec![
            "-I".into(),
            "-B".into(),
            "-X".into(),
            "utf8".into(),
            script.to_string_lossy().into_owned(),
            home.to_string_lossy().into_owned(),
            plan.into(),
            request.into(),
        ],
        cwd: home.to_owned(),
        description: "Persona Workshop update operation".into(),
        autostart: false,
    };
    let path = d.join("manager.json");
    if path.exists() {
        let captured = binding(home, &j)?;
        ensure!(
            captured.worker_sha256 == hash(&script)?
                && captured.python_sha256 == hash(&python)?
                && captured.control_sha256 == hash(&control)?,
            "The captured native update files changed"
        );
        return Ok(());
    }
    let intent_path = d.join("manager-intent.json");
    let mut b: Binding = if intent_path.exists() {
        serde_json::from_value(read(&intent_path)?)?
    } else {
        let app = if j["action"] == "check" {
            None
        } else {
            Some(application(home, &cfg.id)?)
        };
        let worker = Execution {
            service: worker_service,
            profile: Some(profile),
            baseline: Value::Null,
        };
        let status = tokio::runtime::Handle::try_current()?
            .block_on(crate::desktop::local_api(home, "GET", "status", None));
        let original_application = match status {
            Ok(status) => {
                ensure!(
                    status["id"] == cfg.id,
                    "A different application answered before update preparation"
                );
                process::identity(u32::try_from(
                    status["pid"]
                        .as_u64()
                        .context("Missing application process")?,
                )?)?
                .context("Original application generation disappeared")?
            }
            Err(error) if j["critical_recovery"] == true => {
                let previous = j["previous_request_id"]
                    .as_str()
                    .context("Recovery has no captured prior application")?;
                let retained: Binding = serde_json::from_value(read(
                    &dispatch(home, plan, previous)?.join("manager.json"),
                )?)?;
                ensure!(
                    retained.schema == 1
                        && retained.plan_id == plan
                        && retained.request_id == previous
                        && retained.installation_id == cfg.id,
                    "Recovery application identity changed: {error}"
                );
                // The original process identity remains attributable even when
                // the listener is gone. Stop/settlement still require that exact
                // generation or the prior immutable native launch claim.
                retained.original_application
            }
            Err(error) => return Err(error),
        };
        let planned = Binding {
            schema: 1,
            installation_id: cfg.id.clone(),
            plan_id: plan.into(),
            request_id: request.into(),
            worker,
            application: app,
            worker_sha256: hash(&script)?,
            python_sha256: hash(&python)?,
            control_sha256: hash(&control)?,
            original_application,
        };
        // Immutable provenance exists before either native definition can be
        // created. Interrupted registration never loses the original profile.
        platform::private_create(&intent_path, &serde_json::to_vec_pretty(&planned)?)?;
        planned
    };
    ensure!(
        b.installation_id == cfg.id
            && b.plan_id == plan
            && b.request_id == request
            && b.worker_sha256 == hash(&script)?
            && b.python_sha256 == hash(&python)?
            && b.control_sha256 == hash(&control)?,
        "The retained registration intent changed"
    );
    if let Some(app) = b.application.as_mut() {
        if app.baseline.is_null() {
            *app = registered(
                &app.service,
                app.profile.as_ref().context("Missing fixed app profile")?,
                &d.join("registration-runtime.json"),
            )?;
        } else {
            verify_profile(home, app, &manager::snapshot(&app.service)?)?;
        }
    }
    b.worker = registered(
        &b.worker.service,
        b.worker
            .profile
            .as_ref()
            .context("Missing fixed worker profile")?,
        &d.join("registration-worker.json"),
    )?;
    ensure!(
        inactive(&b.worker.baseline),
        "This update worker already has an unsettled lifetime"
    );
    platform::private_create(&path, &serde_json::to_vec_pretty(&b)?)?;
    Ok(())
}

pub fn start_worker(home: &Path, plan: &str) -> Result<Value> {
    let _guard = lock(home)?;
    let j = job(home, plan)?;
    let b = binding(home, &j)?;
    let root = folder(home, plan)?;
    let d = dispatch(home, plan, &b.request_id)?;
    let receipt = d.join("worker-start.json");
    ensure!(
        !receipt.exists(),
        "This worker already has a recorded dispatch; reconcile it without replay"
    );
    ensure!(
        hash(&root.join("worker.py"))? == b.worker_sha256
            && hash(&b.worker.profile.as_ref().unwrap().executable)? == b.python_sha256,
        "The fixed update worker bytes changed"
    );
    let current = manager::snapshot(&b.worker.service)?;
    verify_profile(home, &b.worker, &current)?;
    ensure!(inactive(&current), "Native worker is not settled inactive");
    write(
        &receipt,
        &json!({"state":"issued","request_id":b.request_id,"before":current}),
    )?;
    let outcome =
        manager::request_state(&b.worker.service, manager::ServiceAction::Start, &current);
    let value = match outcome {
        Ok(value) => serde_json::to_value(value)?,
        Err(error) => json!({"status":"unknown","message":error.to_string()}),
    };
    write(
        &receipt,
        &json!({"state":value["status"],"request_id":b.request_id,"before":current,"outcome":value}),
    )?;
    Ok(value)
}

fn ancestry(current: &Value, ancestor: &Value) -> Result<bool> {
    let mut chain = vec![current.clone()];
    for _ in 0..32 {
        let item = chain.last().unwrap();
        ensure!(
            process::matches(item)?,
            "Application process generation changed"
        );
        if item == ancestor {
            return Ok(true);
        }
        let Some(parent) = process::parent_identity(item)? else {
            return Ok(false);
        };
        ensure!(!chain.contains(&parent), "Cyclic native process ancestry");
        chain.push(parent);
    }
    anyhow::bail!("Application process ancestry exceeded its bound")
}
fn membership(current: &Value, snapshot: &Value) -> Result<Value> {
    let processes = snapshot["processes"]
        .as_array()
        .context("Missing native processes")?;
    for observed in processes {
        if !ancestry(current, &observed["identity"])? {
            continue;
        }
        if observed["role"] == "scheduled_task_engine" {
            let guid = observed["instance_guid"]
                .as_str()
                .context("Missing task instance")?;
            let instances = snapshot["native"]["instances"]
                .as_array()
                .context("Missing task instances")?;
            let instance = instances
                .iter()
                .find(|v| v["instance_guid"] == guid)
                .context("Task instance disappeared")?;
            ensure!(
                instance["engine_instance_guids"] == json!([guid]),
                "Task engine is shared; application ownership is unproved"
            );
        }
        return Ok(json!({"application":current,"manager_process":observed}));
    }
    anyhow::bail!("Application is not owned by the captured native registration")
}

fn launch_intent(home: &Path, plan: &str, nonce: &str) -> Result<(Value, Binding, Execution)> {
    id(nonce)?;
    let j = job(home, plan)?;
    let mode = match j["state"].as_str() {
        Some("verifying") => "candidate",
        Some("rolling_back") => "rollback",
        _ => anyhow::bail!("Update launch was superseded"),
    };
    ensure!(
        j["launch"]["nonce"] == nonce && j["launch"]["mode"] == mode,
        "Update launch was superseded"
    );
    let b = binding(home, &j)?;
    let app = b
        .application
        .clone()
        .context("This operation has no application launch")?;
    Ok((j, b, app))
}

fn generation(snapshot: &Value) -> Option<Value> {
    let processes = snapshot["processes"].as_array()?;
    if snapshot["active"] != true || snapshot["transitioning"] != false || processes.is_empty() {
        return None;
    }
    Some(
        json!({"processes":processes,"invocation_id":snapshot["native"]["InvocationID"],
                "task_instances":snapshot["native"]["instances"]}),
    )
}
fn previous_dispatch_can_advance(
    dispatch: &str,
    has_claim: bool,
    has_observed_generation: bool,
) -> bool {
    dispatch == "refused"
        || (dispatch == "accepted" && has_observed_generation)
        || (matches!(dispatch, "issued" | "accepted" | "unknown") && has_claim)
}
fn settled_launch(root: &Path, intent: &Value, current: &Value) -> Result<()> {
    let nonce = id(intent["nonce"]
        .as_str()
        .context("Missing prior launch identity")?)?;
    let path = root.join(format!("manager-claim-{nonce}.json"));
    let observed = intent.get("observed_generation").filter(|v| !v.is_null());
    ensure!(
        previous_dispatch_can_advance(
            intent["dispatch"].as_str().unwrap_or(""),
            path.exists(),
            observed.is_some()
        ),
        "A prior native launch was never bound to an observed generation; its outcome remains unresolved"
    );
    if path.exists() {
        let claim = read(&path)?;
        ensure!(
            !process::matches(&claim["claim"]["application"])?,
            "A prior application generation still exists"
        );
        ensure!(
            generation_settled(
                &generation(&claim["snapshot"]).context("Missing claimed generation")?,
                current
            )?,
            "The previous claimed native generation has not settled"
        );
    } else if let Some(observed) = observed {
        ensure!(
            generation_settled(observed, current)?,
            "The previous native generation has not settled"
        );
    }
    Ok(())
}
fn generation_settled(observed: &Value, current: &Value) -> Result<bool> {
    if !inactive(current) {
        return Ok(false);
    }
    for p in observed["processes"]
        .as_array()
        .context("Missing captured generation")?
    {
        if p["role"] == "scheduled_task_engine" {
            let guid = p["instance_guid"]
                .as_str()
                .context("Missing captured task instance")?;
            if current["native"]["instances"]
                .as_array()
                .is_some_and(|xs| xs.iter().any(|x| x["instance_guid"] == guid))
            {
                return Ok(false);
            }
        } else if process::matches(&p["identity"])? {
            return Ok(false);
        }
    }
    Ok(true)
}

pub fn start_application(home: &Path, plan: &str, nonce: &str) -> Result<Value> {
    let _guard = lock(home)?;
    let (j, b, app) = launch_intent(home, plan, nonce)?;
    let root = folder(home, plan)?;
    let path = root.join("manager-launch.json");
    let current = manager::snapshot(&app.service)?;
    verify_profile(home, &app, &current)?;
    ensure!(
        inactive(&current),
        "A previous native application lifetime is not settled"
    );
    if path.exists() {
        let previous = read(&path)?;
        ensure!(
            previous["nonce"] != nonce,
            "This launch was already dispatched; do not replay it"
        );
        settled_launch(&root, &previous, &current)?;
    }
    let expected = j["launch"]["binary_sha256"]
        .as_str()
        .context("Missing launch digest")?;
    ensure!(
        hash(&platform::installed_binary(&APPLICATION, home))? == expected,
        "Application image changed"
    );
    ensure!(
        job(home, plan)? == j,
        "Update launch changed during image verification"
    );
    write(
        &path,
        &json!({"schema":1,"plan":plan,"request":b.request_id,"nonce":nonce,
        "before":current,"dispatch":"issued","service":app.service,"image_sha256":expected}),
    )?;
    let outcome = manager::request_state(&app.service, manager::ServiceAction::Start, &current);
    let value = match outcome {
        Ok(v) => serde_json::to_value(v)?,
        Err(e) => json!({"status":"unknown","message":e.to_string()}),
    };
    let mut record = read(&path)?;
    ensure!(record["nonce"] == nonce, "Native launch record changed");
    record["dispatch"] = value["status"].clone();
    record["outcome"] = value.clone();
    record["observed_generation"] = Value::Null;
    write(&path, &record)?;
    if value["status"] == "accepted" {
        // A single acknowledged Start can transition asynchronously. Capture
        // its first observed native generation here, before health checks, so
        // an older rollback binary needs no new in-process hook. This window
        // never sends Start again and cannot be reopened after an unbound exit.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            let observation = manager::snapshot(&app.service)?;
            verify_profile(home, &app, &observation)?;
            if let Some(generation) = generation(&observation) {
                record["observed_generation"] = generation;
                write(&path, &record)?;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    // No observation remains explicitly unbound. A later inactive snapshot or
    // unrelated health response cannot manufacture an invocation identity.
    Ok(value)
}

/// Called under service.lock before opening application state. A managed start
/// consumes only its current pre-start inactive intent; no environment is added
/// to or definition rewritten for an existing startup registration.
pub fn validate_managed_launch(home: &Path) -> Result<bool> {
    let Some(j) = native_update::active(home)? else {
        return Ok(false);
    };
    if !matches!(j["state"].as_str(), Some("verifying" | "rolling_back")) {
        return Ok(false);
    }
    let Some(nonce) = j["launch"]["nonce"].as_str() else {
        return Ok(false);
    };
    let plan = j["id"].as_str().context("Missing update plan")?;
    let path = folder(home, plan)?.join("manager-launch.json");
    if !path.exists() {
        return Ok(false);
    }
    let (current_job, _, app) = launch_intent(home, plan, nonce)?;
    let before_bytes = fs::read(&path)?;
    let intent: Value = serde_json::from_slice(&before_bytes)?;
    ensure!(
        intent["nonce"] == nonce
            && intent["plan"] == plan
            && inactive(&intent["before"])
            && matches!(
                intent["dispatch"].as_str(),
                Some("issued" | "accepted" | "unknown")
            ),
        "Managed update launch was superseded or refused"
    );
    ensure!(
        hash(&std::env::current_exe()?)?
            == intent["image_sha256"]
                .as_str()
                .context("Missing image digest")?,
        "Managed update executable changed"
    );
    let process =
        process::identity(std::process::id())?.context("Application generation missing")?;
    let snapshot = manager::snapshot(&app.service)?;
    verify_profile(home, &app, &snapshot)?;
    let claim = membership(&process, &snapshot)?;
    // The immutable claim prevents a delayed previous process from consuming a
    // later intent, including Task Scheduler engines shared by unrelated tasks.
    let claim_path = folder(home, plan)?.join(format!("manager-claim-{nonce}.json"));
    let claim_record = json!({"nonce":nonce,"plan":plan,"claim":claim,"snapshot":snapshot});
    let after_intent = read(&path)?;
    ensure!(
        native_update::active(home)?.as_ref() == Some(&current_job)
            && [
                "schema",
                "plan",
                "request",
                "nonce",
                "before",
                "service",
                "image_sha256"
            ]
            .iter()
            .all(|key| after_intent[*key] == intent[*key])
            && matches!(
                after_intent["dispatch"].as_str(),
                Some("issued" | "accepted" | "unknown")
            ),
        "Managed launch changed during ownership verification"
    );
    if claim_path.exists() {
        ensure!(
            read(&claim_path)? == claim_record,
            "This native launch already belongs to another generation"
        );
    } else {
        platform::private_create(&claim_path, &serde_json::to_vec_pretty(&claim_record)?)?;
    }
    Ok(true)
}

pub fn observe_application(home: &Path, plan: &str, nonce: &str, pid: u32) -> Result<Value> {
    let (_, _, app) = launch_intent(home, plan, nonce)?;
    let snapshot = manager::snapshot(&app.service)?;
    verify_profile(home, &app, &snapshot)?;
    let identity = process::identity(pid)?.context("Application generation disappeared")?;
    let claim = membership(&identity, &snapshot)?;
    let root = folder(home, plan)?;
    let intent = read(&root.join("manager-launch.json"))?;
    ensure!(
        intent["nonce"] == nonce,
        "A different launch owns this native generation"
    );
    let path = root.join(format!("manager-claim-{}.json", id(nonce)?));
    if path.exists() {
        let retained = read(&path)?;
        ensure!(
            retained["nonce"] == nonce && retained["claim"] == claim,
            "The original launch claim belongs to a different native generation"
        );
    } else {
        // Older rollback binaries lack the new in-process claim hook. Their
        // authenticated health PID must match the generation actually observed
        // by the exact earlier native Start, never merely the same service name.
        ensure!(
            !intent["observed_generation"].is_null()
                && generation(&snapshot).as_ref() == Some(&intent["observed_generation"]),
            "The application launch was not bound to this native generation"
        );
        platform::private_create(
            &path,
            &serde_json::to_vec_pretty(
                &json!({"nonce":nonce,"plan":plan,"claim":claim,"snapshot":snapshot}),
            )?,
        )?;
    }
    Ok(json!({"snapshot":snapshot,"claim":claim}))
}

pub fn authorize_stop(home: &Path, plan: &str, pid: u32) -> Result<Value> {
    let j = job(home, plan)?;
    let b = binding(home, &j)?;
    if let Some(app) = &b.application {
        verify_profile(home, app, &manager::snapshot(&app.service)?)?;
    }
    let expected = if b.original_application["pid"] == pid
        && process::matches(&b.original_application)?
    {
        b.original_application
    } else if let Some(nonce) = j["launch"]["nonce"].as_str() {
        read(&folder(home, plan)?.join(format!("manager-claim-{}.json", id(nonce)?)))?["claim"]["application"].clone()
    } else {
        b.original_application
    };
    ensure!(
        expected["pid"] == pid && process::matches(&expected)?,
        "This update does not own the responding application generation"
    );
    Ok(json!({"identity":expected}))
}

pub fn stop_application(home: &Path, plan: &str, nonce: &str) -> Result<Value> {
    let _guard = lock(home)?;
    let j = job(home, plan)?;
    ensure!(
        matches!(j["state"].as_str(), Some("stopping" | "rolling_back")),
        "Application stop is not authorized by this update"
    );
    let b = binding(home, &j)?;
    let app = b
        .application
        .context("No captured application registration")?;
    ensure!(
        j["launch"]["nonce"] == nonce,
        "A different launch owns this stop"
    );
    let claim = read(&folder(home, plan)?.join(format!("manager-claim-{}.json", id(nonce)?)))?;
    let current = manager::snapshot(&app.service)?;
    verify_profile(home, &app, &current)?;
    if inactive(&current) {
        ensure!(
            !process::matches(&claim["claim"]["application"])?,
            "The claimed application still exists outside the observed manager state"
        );
        return Ok(json!({"status":"inactive"}));
    }
    ensure!(
        membership(&claim["claim"]["application"], &current)? == claim["claim"],
        "A different native application generation is running"
    );
    let path = folder(home, plan)?.join(format!("manager-stop-{}.json", id(nonce)?));
    if path.exists() {
        return Ok(
            json!({"status":"unknown","message":"The earlier native stop has not settled; it was not repeated"}),
        );
    }
    // This is used only for the exact failed replacement generation retained in
    // this plan. Record before dispatch; an ambiguous stop is never resent.
    platform::private_create(
        &path,
        &serde_json::to_vec_pretty(
            &json!({"state":"issued","nonce":nonce,"before":current,"claim":claim}),
        )?,
    )?;
    let outcome = match manager::request_state(&app.service, manager::ServiceAction::Stop, &current)
    {
        Ok(value) => serde_json::to_value(value)?,
        Err(error) => json!({"status":"unknown","message":error.to_string()}),
    };
    write(
        &path,
        &json!({"state":outcome["status"],"nonce":nonce,"before":current,"claim":claim,"outcome":outcome}),
    )?;
    Ok(outcome)
}

pub fn wait_application_stopped(home: &Path, plan: &str) -> Result<()> {
    let j = job(home, plan)?;
    let b = binding(home, &j)?;
    let app = b.application.context("Missing application registration")?;
    for _ in 0..40 {
        let s = manager::snapshot(&app.service)?;
        verify_profile(home, &app, &s)?;
        if inactive(&s) {
            ensure!(
                !process::matches(&b.original_application)?,
                "The original application generation has not exited"
            );
            let root = folder(home, plan)?;
            let path = root.join("manager-launch.json");
            if path.exists() {
                let intent = read(&path)?;
                if let Some(nonce) = j["launch"]["nonce"].as_str() {
                    ensure!(
                        intent["nonce"] == nonce,
                        "A different native launch remains unresolved"
                    );
                }
                settled_launch(&root, &intent, &s)?;
            }
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    anyhow::bail!("Native application lifetime remains unresolved")
}

pub fn cleanup(home: &Path, plan: &str, include_runtime: bool) -> Result<Value> {
    let _guard = lock(home)?;
    let j = job(home, plan)?;
    ensure!(
        !matches!(
            j["state"].as_str(),
            Some(
                "queued"
                    | "checking"
                    | "draining"
                    | "stopping"
                    | "swapping"
                    | "verifying"
                    | "rolling_back"
                    | "recovery_required"
            )
        ),
        "This operation still needs its native profiles for reconciliation"
    );
    let b = binding(home, &j)?;
    let mut retired = Vec::new();
    let mut waiting = false;
    let mut unresolved = false;
    for (kind, execution) in std::iter::once(("worker", &b.worker)).chain(
        b.application
            .as_ref()
            .filter(|a| include_runtime && a.profile.is_some())
            .map(|a| ("runtime", a)),
    ) {
        let profile = execution
            .profile
            .as_ref()
            .context("Only exact temporary profiles can be retired")?;
        let path = dispatch(home, plan, &b.request_id)?.join(format!("retirement-{kind}.json"));
        let absence = registration::is_retired(&execution.service);
        let outcome = if matches!(absence, Ok(true)) {
            json!({"status":"retired","removed":false})
        } else if path.exists() {
            // Any recorded call may have reached the native manager. Subsequent
            // opens only observe absence; they never retry an ambiguous delete.
            let prior = read(&path)?;
            ensure!(
                prior["service"] == serde_json::to_value(&execution.service)?
                    && prior["profile"] == serde_json::to_value(profile)?
                    && inactive(&prior["before"]),
                "Retirement ownership changed"
            );
            unresolved = true;
            json!({"status":"unknown","message":"Prior retirement remains unconfirmed","attempt":prior})
        } else {
            // Without a prior settled retirement attempt, inspection errors
            // still leave lifetime unknown. An inert profile refusal below is
            // a cleanup result, never a new prerequisite for another update.
            absence?;
            let current = manager::snapshot(&execution.service)?;
            if !inactive(&current) {
                waiting = true;
                retired.push(json!({"kind":kind,"status":"waiting_for_exit"}));
                continue;
            }
            if let Err(error) = verify_profile(home, execution, &current) {
                let outcome = json!({"status":"refused","code":"profile_changed","message":error.to_string()});
                platform::private_create(
                    &path,
                    &serde_json::to_vec_pretty(
                        &json!({"state":"refused","phase":"before_dispatch",
                    "service":execution.service,"profile":profile,"before":current,"outcome":outcome}),
                    )?,
                )?;
                unresolved = true;
                retired.push(json!({"kind":kind,"service":execution.service,"outcome":outcome}));
                continue;
            }
            platform::private_create(
                &path,
                &serde_json::to_vec_pretty(&json!({
                    "state":"issued","service":execution.service,"profile":profile,"before":current
                }))?,
            )?;
            let outcome = match registration::retire(&execution.service, profile, &current) {
                Ok(value) => serde_json::to_value(value)?,
                Err(error) => json!({"status":"unknown","message":error.to_string()}),
            };
            write(
                &path,
                &json!({"state":outcome["status"],"service":execution.service,
                "profile":profile,"before":current,"outcome":outcome}),
            )?;
            unresolved |= outcome["status"] != "retired";
            outcome
        };
        retired.push(json!({"kind":kind,"service":execution.service,"outcome":outcome}));
    }
    let result = json!({"retired":retired,"waiting_for_exit":waiting,"unresolved":unresolved});
    write(
        &dispatch(home, plan, &b.request_id)?.join("retirement.json"),
        &result,
    )?;
    Ok(result)
}

/// Settle the previous terminal operation before its current request pointer is
/// replaced. Cleanup uncertainty is retained separately and does not prevent an
/// independent update once the actual participant is observed inactive.
pub(crate) fn settle_previous(home: &Path) -> Result<()> {
    let Some(j) = native_update::active(home)? else {
        return Ok(());
    };
    if !matches!(
        j["state"].as_str(),
        Some("ready" | "failed" | "deferred" | "succeeded" | "rolled_back")
    ) {
        return Ok(());
    }
    let Some(request) = j
        .get("worker_request_id")
        .unwrap_or(&j["request_id"])
        .as_str()
    else {
        return Ok(());
    };
    let plan = j["id"].as_str().context("Missing previous plan")?;
    if !dispatch(home, plan, request)?.join("manager.json").exists() {
        return Ok(());
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let result = cleanup(home, plan, false)?;
        if result["waiting_for_exit"] == false {
            return Ok(());
        }
        ensure!(
            std::time::Instant::now() < deadline,
            "The previous update worker is still exiting; its original request was preserved"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// One bounded reconciliation attached to an existing update. It never starts,
/// retries, or cancels work. A native ambiguity leaves the retained profile.
pub async fn reconcile(home: PathBuf) {
    let Ok(Some(original)) = native_update::active(&home) else {
        return;
    };
    let Some(plan) = original["id"].as_str().map(str::to_owned) else {
        return;
    };
    let request = original["request_id"].clone();
    for _ in 0..120 {
        let Ok(Some(current)) = native_update::active(&home) else {
            return;
        };
        if current["id"] != plan || current["request_id"] != request {
            return;
        }
        if matches!(
            current["state"].as_str(),
            Some("ready" | "succeeded" | "rolled_back" | "failed" | "deferred")
        ) {
            let selected_home = home.clone();
            let selected_plan = plan.clone();
            match tokio::task::spawn_blocking(move || {
                cleanup(&selected_home, &selected_plan, false)
            })
            .await
            {
                Ok(Ok(value))
                    if value["waiting_for_exit"] == true && value["unresolved"] == false => {}
                _ => return,
            }
            // Only a pre-dispatch, still-active observation keeps waiting. An
            // attempted retirement (including unknown) is never sent twice.
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

pub fn command(
    home: &Path,
    plan: &str,
    operation: &str,
    nonce: Option<&str>,
    pid: Option<u32>,
) -> Result<Value> {
    match operation {
        "prepare" => {
            prepare(home, plan)?;
            Ok(json!({"prepared":true}))
        }
        "start" => start_application(home, plan, nonce.context("Missing launch nonce")?),
        "observe" => observe_application(
            home,
            plan,
            nonce.context("Missing launch nonce")?,
            pid.context("Missing app process")?,
        ),
        "authorize-stop" => authorize_stop(home, plan, pid.context("Missing application PID")?),
        "stop" => stop_application(home, plan, nonce.context("Missing launch nonce")?),
        "stopped" => {
            wait_application_stopped(home, plan)?;
            Ok(json!({"inactive":true}))
        }
        "retire-worker" => cleanup(home, plan, false),
        "retire-runtime" => cleanup(home, plan, true),
        _ => anyhow::bail!("Unknown fixed update operation"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_but_unobserved_start_cannot_consume_a_new_nonce() {
        let temporary = tempfile::tempdir().unwrap();
        let first = uuid::Uuid::new_v4().to_string();
        let intent = json!({"nonce":first,"dispatch":"accepted","observed_generation":null});
        let path = temporary.path().join("manager-launch.json");
        write(&path, &intent).unwrap();
        let before = fs::read(&path).unwrap();
        let inactive =
            json!({"active":false,"transitioning":false,"processes":[],"native":{"instances":[]}});
        assert!(settled_launch(temporary.path(), &intent, &inactive).is_err());
        // A temporarily empty native observation does not authorize overwriting
        // N1; its delayed invocation can never claim a synthetic N2.
        assert_eq!(fs::read(&path).unwrap(), before);
        for dispatch in ["issued", "unknown"] {
            let mut pending = intent.clone();
            pending["dispatch"] = json!(dispatch);
            assert!(settled_launch(temporary.path(), &pending, &inactive).is_err());
        }
        let mut refused = intent;
        refused["dispatch"] = json!("refused");
        settled_launch(temporary.path(), &refused, &inactive).unwrap();
    }

    #[test]
    fn a_live_claim_cannot_be_replaced_even_when_service_appears_inactive() {
        let temporary = tempfile::tempdir().unwrap();
        let nonce = uuid::Uuid::new_v4().to_string();
        let identity = process::identity(std::process::id()).unwrap().unwrap();
        write(
            &temporary.path().join(format!("manager-claim-{nonce}.json")),
            &json!({"claim":{"application":identity}}),
        )
        .unwrap();
        let intent = json!({"nonce":nonce,"dispatch":"accepted"});
        let inactive =
            json!({"active":false,"transitioning":false,"processes":[],"native":{"instances":[]}});
        assert!(settled_launch(temporary.path(), &intent, &inactive).is_err());
    }

    #[test]
    fn a_same_name_service_with_disabled_or_drifted_definition_is_refused() {
        let before = json!({"service":{"name":"original"},"definition":{"bytes":[1]},
                           "effective_definition":{"bytes":[1]},"enabled":true,"masked":false,"start_allowed":true});
        for (key, replacement) in [
            ("enabled", json!(false)),
            ("masked", json!(true)),
            ("start_allowed", json!(false)),
            ("definition", json!({"bytes":[2]})),
            ("effective_definition", json!({"bytes":[2]})),
        ] {
            let mut after = before.clone();
            after[key] = replacement;
            assert!(!same_definition(&before, &after), "{key}");
        }
    }

    #[test]
    fn private_directory_creation_does_not_repair_existing_permissions() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("owned");
        ensure_directory(&path).unwrap();
        ensure_directory(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            assert!(ensure_directory(&path).is_err());
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o755
            );
        }
    }
}
