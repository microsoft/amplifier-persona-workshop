use crate::{
    auth,
    config::{Config, private_dir, private_write},
    error::{ApiError, ApiResult},
    evidence,
    model::*,
};
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::sync::{Semaphore, watch};

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
pub struct AppState {
    pub home: PathBuf,
    pub config: Config,
    pub db: Mutex<Connection>,
    pub login_gate: Arc<Semaphore>,
    pub work_gate: Arc<Semaphore>,
    pub running: Mutex<HashMap<String, watch::Sender<bool>>>,
}
impl AppState {
    pub fn open(home: PathBuf) -> Result<Arc<Self>> {
        private_dir(&home)?;
        private_dir(&home.join("secrets"))?;
        private_dir(&home.join("workspace"))?;
        let config = Config::load(&home)?;
        let db = Connection::open(home.join("state.sqlite3"))?;
        db.execute_batch("PRAGMA journal_mode=WAL;PRAGMA foreign_keys=ON;PRAGMA busy_timeout=5000;
    CREATE TABLE IF NOT EXISTS projects(id TEXT PRIMARY KEY,value TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS jobs(id TEXT PRIMARY KEY,project_id TEXT NOT NULL,value TEXT NOT NULL);
    DROP INDEX IF EXISTS one_active_job;
    CREATE UNIQUE INDEX IF NOT EXISTS one_active_build ON jobs(project_id) WHERE json_extract(value,'$.kind')='build' AND json_extract(value,'$.status') IN ('queued','running');
    CREATE TABLE IF NOT EXISTS revisions(id TEXT PRIMARY KEY,project_id TEXT NOT NULL,value TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS evaluations(id TEXT PRIMARY KEY,project_id TEXT NOT NULL,value TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS comparisons(id TEXT PRIMARY KEY,project_id TEXT NOT NULL,value TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS agent_targets(id TEXT PRIMARY KEY,value TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS tokens(hash TEXT PRIMARY KEY,id TEXT NOT NULL,scopes TEXT NOT NULL,expires_at INTEGER NOT NULL);
    CREATE TABLE IF NOT EXISTS browser_sessions(hash TEXT PRIMARY KEY,owner TEXT NOT NULL,csrf TEXT NOT NULL,expires_at INTEGER NOT NULL);
    CREATE TABLE IF NOT EXISTS login_attempts(key TEXT PRIMARY KEY,failures INTEGER NOT NULL,locked_until INTEGER NOT NULL);
    CREATE TABLE IF NOT EXISTS views(id TEXT PRIMARY KEY,value TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS audit(seq INTEGER PRIMARY KEY AUTOINCREMENT,at INTEGER NOT NULL,kind TEXT NOT NULL,project_id TEXT,data TEXT NOT NULL);")?;
        db.execute("UPDATE jobs SET value=json_set(value,'$.status','interrupted','$.stage','interrupted','$.note','The service restarted. Start a new build to continue.') WHERE json_extract(value,'$.status') IN ('queued','running')",[])?;
        let path = home.join("secrets/control-token");
        let token = if path.exists() {
            std::fs::read_to_string(&path)?.trim().to_owned()
        } else {
            let token = auth::random_token();
            private_write(&path, token.as_bytes())?;
            token
        };
        anyhow::ensure!(token.len() >= 32, "Invalid control credential");
        db.execute("DELETE FROM tokens WHERE id='local-control'", [])?;
        db.execute(
            "INSERT INTO tokens VALUES(?1,'local-control','[\"admin\"]',?2)",
            params![
                auth::digest(&token),
                now_ms() + 100_i64 * 366 * 86400 * 1000
            ],
        )?;
        Ok(Arc::new(Self {
            home,
            config,
            db: Mutex::new(db),
            login_gate: Arc::new(Semaphore::new(2)),
            work_gate: Arc::new(Semaphore::new(2)),
            running: Mutex::new(HashMap::new()),
        }))
    }
    pub fn audit(&self, kind: &str, project: Option<&str>, data: Value) -> Result<()> {
        self.db.lock().unwrap().execute(
            "INSERT INTO audit(at,kind,project_id,data) VALUES(?1,?2,?3,?4)",
            params![now_ms(), kind, project, data.to_string()],
        )?;
        Ok(())
    }
    pub fn project(&self, id: &str) -> ApiResult<Project> {
        let raw: Option<String> = self
            .db
            .lock()
            .unwrap()
            .query_row("SELECT value FROM projects WHERE id=?1", [id], |r| r.get(0))
            .optional()?;
        serde_json::from_str(&raw.ok_or_else(ApiError::missing)?)
            .map_err(|_| ApiError::bad("Invalid project record"))
    }
    pub fn create(
        &self,
        alias: String,
        purpose: String,
        aliases: Vec<String>,
        subject_type: String,
    ) -> ApiResult<Project> {
        validate_project(&alias, &purpose, &aliases)?;
        if !["person", "fictional", "composite"].contains(&subject_type.as_str()) {
            return Err(ApiError::bad("Choose person, fictional, or composite"));
        }
        let p = Project {
            id: id(),
            alias,
            purpose,
            aliases,
            subject_type,
            revision: 1,
            created_at: now_ms(),
            updated_at: now_ms(),
            artifacts: vec![],
            observations: vec![],
            identity: Default::default(),
        };
        self.db.lock().unwrap().execute(
            "INSERT INTO projects VALUES(?1,?2)",
            params![p.id, serde_json::to_string(&p).unwrap()],
        )?;
        self.audit("project_created", Some(&p.id), json!({}))?;
        Ok(p)
    }
    pub fn change<F>(&self, id: &str, expected: u64, f: F) -> ApiResult<Project>
    where
        F: FnOnce(&mut Project) -> ApiResult<()>,
    {
        self.change_record(id, expected, None, f)
    }
    pub fn change_record<F>(
        &self,
        id: &str,
        expected: u64,
        record: Option<(&str, &Value)>,
        f: F,
    ) -> ApiResult<Project>
    where
        F: FnOnce(&mut Project) -> ApiResult<()>,
    {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let raw: Option<String> = tx
            .query_row("SELECT value FROM projects WHERE id=?1", [id], |r| r.get(0))
            .optional()?;
        let mut p: Project = serde_json::from_str(&raw.ok_or_else(ApiError::missing)?)
            .map_err(|_| ApiError::bad("Invalid project record"))?;
        if p.revision != expected {
            return Err(ApiError(
                axum::http::StatusCode::CONFLICT,
                "This project changed. Refresh before saving.".into(),
            ));
        }
        f(&mut p)?;
        p.revision += 1;
        p.updated_at = now_ms();
        tx.execute(
            "UPDATE projects SET value=?2 WHERE id=?1",
            params![id, serde_json::to_string(&p).unwrap()],
        )?;
        if let Some((table, value)) = record {
            if !["revisions", "evaluations", "jobs", "comparisons"].contains(&table) {
                return Err(ApiError::bad("Invalid collection"));
            }
            let rid = value["id"]
                .as_str()
                .ok_or_else(|| ApiError::bad("Missing record identity"))?;
            tx.execute(&format!("INSERT INTO {table}(id,project_id,value) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET value=excluded.value"), params![rid, id, value.to_string()])?;
        }
        tx.commit()?;
        Ok(p)
    }
    pub fn enqueue(&self, job: &crate::model::Job) -> ApiResult<()> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let revision: i64 = tx.query_row(
            "SELECT json_extract(value,'$.revision') FROM projects WHERE id=?1",
            [&job.project_id],
            |r| r.get(0),
        )?;
        let (active, builds): (i64, i64) = tx.query_row("SELECT count(*), coalesce(sum(json_extract(value,'$.kind')='build'),0) FROM jobs WHERE project_id=?1 AND json_extract(value,'$.status') IN ('queued','running')", [&job.project_id], |r| Ok((r.get(0)?,r.get(1)?)))?;
        if revision != job.source_revision as i64
            || builds > 0
            || (job.kind == "build" && active > 0)
            || active >= 16
        {
            return Err(ApiError(axum::http::StatusCode::CONFLICT, "This project changed or already has incompatible work queued. Refresh and let the active work finish.".into()));
        }
        tx.execute(
            "INSERT INTO jobs VALUES(?1,?2,?3)",
            params![job.id, job.project_id, serde_json::to_string(job).unwrap()],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn save_build(
        &self,
        project: &str,
        expected: u64,
        observations: Vec<Observation>,
        mut completed_job: Job,
    ) -> ApiResult<Revision> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let raw: String =
            tx.query_row("SELECT value FROM projects WHERE id=?1", [project], |r| {
                r.get(0)
            })?;
        let mut p: Project =
            serde_json::from_str(&raw).map_err(|_| ApiError::bad("Invalid project"))?;
        if p.revision != expected {
            return Err(ApiError(
                axum::http::StatusCode::CONFLICT,
                "This project changed during the build. Your changes were kept; build again."
                    .into(),
            ));
        }
        // Rebuild pending inference, retaining deliberate edits and decisions.
        p.observations
            .retain(|o| o.source == "owner" || o.status != "pending" || o.reviewer.is_some());
        for o in observations {
            if !p
                .observations
                .iter()
                .any(|old| old.collection == o.collection && old.statement == o.statement)
            {
                p.observations.push(o);
            }
        }
        crate::profiles::refresh_validity(&mut p);
        p.revision += 1;
        p.updated_at = now_ms();
        let revision = crate::profiles::compile(&p)?;
        completed_job.result_id = Some(revision.id.clone());
        completed_job.note = "Profiles are ready".into();
        tx.execute(
            "UPDATE projects SET value=?2 WHERE id=?1",
            params![project, serde_json::to_string(&p).unwrap()],
        )?;
        tx.execute(
            "INSERT INTO revisions VALUES(?1,?2,?3)",
            params![
                revision.id,
                project,
                serde_json::to_string(&revision).unwrap()
            ],
        )?;
        tx.execute(
            "UPDATE jobs SET value=?2 WHERE id=?1",
            params![
                completed_job.id,
                serde_json::to_string(&completed_job).unwrap()
            ],
        )?;
        tx.commit()?;
        Ok(revision)
    }
    pub fn save_evaluation(
        &self,
        project: &str,
        revision: &crate::model::Revision,
        evaluation: &crate::model::Evaluation,
        completed_job: &crate::model::Job,
    ) -> ApiResult<()> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let raw: String =
            tx.query_row("SELECT value FROM projects WHERE id=?1", [project], |r| {
                r.get(0)
            })?;
        let mut p: Project =
            serde_json::from_str(&raw).map_err(|_| ApiError::bad("Invalid project"))?;
        let valid: bool = tx.query_row(
            "SELECT json_extract(value,'$.valid') FROM revisions WHERE id=?1 AND project_id=?2",
            params![revision.id, project],
            |r| r.get(0),
        )?;
        if !valid || !crate::profiles::revision_current(&p, &revision.manifest) {
            return Err(ApiError(axum::http::StatusCode::CONFLICT, "Evidence or private identity settings changed during this simulation. Create replacement profiles.".into()));
        }
        p.revision += 1;
        p.updated_at = now_ms();
        tx.execute(
            "UPDATE projects SET value=?2 WHERE id=?1",
            params![project, serde_json::to_string(&p).unwrap()],
        )?;
        tx.execute(
            "INSERT INTO evaluations VALUES(?1,?2,?3)",
            params![
                evaluation.id,
                project,
                serde_json::to_string(evaluation).unwrap()
            ],
        )?;
        tx.execute(
            "UPDATE jobs SET value=?2 WHERE id=?1",
            params![
                completed_job.id,
                serde_json::to_string(completed_job).unwrap()
            ],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn records(&self, table: &str, project: &str) -> ApiResult<Vec<Value>> {
        if !["jobs", "revisions", "evaluations", "comparisons"].contains(&table) {
            return Err(ApiError::bad("Invalid collection"));
        }
        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare(&format!(
            "SELECT value FROM {table} WHERE project_id=?1 ORDER BY rowid DESC LIMIT 100"
        ))?;
        Ok(stmt
            .query_map([project], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .iter()
            .filter_map(|s| serde_json::from_str(s).ok())
            .collect())
    }
    pub fn save_record(
        &self,
        table: &str,
        id: &str,
        project: &str,
        value: &Value,
    ) -> ApiResult<()> {
        if !["jobs", "revisions", "evaluations", "comparisons"].contains(&table) {
            return Err(ApiError::bad("Invalid collection"));
        }
        self.db.lock().unwrap().execute(&format!("INSERT INTO {table}(id,project_id,value) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET value=excluded.value"),params![id,project,value.to_string()])?;
        Ok(())
    }
    pub fn record(&self, table: &str, id: &str) -> ApiResult<Value> {
        if !["jobs", "revisions", "evaluations", "comparisons"].contains(&table) {
            return Err(ApiError::bad("Invalid collection"));
        }
        let raw: Option<String> = self
            .db
            .lock()
            .unwrap()
            .query_row(
                &format!("SELECT value FROM {table} WHERE id=?1"),
                [id],
                |r| r.get(0),
            )
            .optional()?;
        serde_json::from_str(&raw.ok_or_else(ApiError::missing)?)
            .map_err(|_| ApiError::bad("Invalid record"))
    }
    pub fn list(&self) -> ApiResult<Vec<Value>> {
        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare(
            "SELECT value FROM projects ORDER BY json_extract(value,'$.updated_at') DESC LIMIT 200",
        )?;
        let all = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(all.iter().filter_map(|s|serde_json::from_str::<Project>(s).ok()).map(|p|json!({"id":p.id,"alias":p.alias,"purpose":p.purpose,"revision":p.revision,"updated_at":p.updated_at,"stats":stats(&p)})).collect())
    }
    pub fn snapshot(&self, p: &Project) -> ApiResult<Value> {
        let builds = self.records("jobs", &p.id)?;
        let revisions = self
            .records("revisions", &p.id)?
            .into_iter()
            .map(|mut r| {
                r.as_object_mut().unwrap().remove("files");
                r
            })
            .collect::<Vec<_>>();
        Ok(
            json!({"id":p.id,"alias":p.alias,"purpose":p.purpose,"aliases":p.aliases,"subject_type":p.subject_type,"revision":p.revision,"created_at":p.created_at,"updated_at":p.updated_at,"artifacts":p.artifacts.iter().map(evidence::summary).collect::<Vec<_>>(),"observations":p.observations,"builds":builds,"revisions":revisions,"evaluations":self.records("evaluations",&p.id)?,"identity_present":!p.identity.is_empty(),"stats":stats(p)}),
        )
    }
    pub fn invalidate(&self, p: &Project) -> ApiResult<()> {
        for mut r in self.records("revisions", &p.id)? {
            if !crate::profiles::revision_current(p, &r["manifest"]) {
                r["valid"] = json!(false);
                r["status"] = json!("invalidated");
                let id = r["id"].as_str().unwrap().to_owned();
                self.save_record("revisions", &id, &p.id, &r)?;
            }
        }
        Ok(())
    }
}
pub fn stats(p: &Project) -> Value {
    let mut communication = 0;
    let mut working = 0;
    let mut missing = 0;
    for a in p.artifacts.iter().filter(|a| !a.deleted) {
        let n = a.spans.iter().filter(|s| evidence::eligible(a, s)).count();
        if n == 0 && !a.holdout {
            missing += 1;
        }
        if a.collections.iter().any(|c| c == "communication") {
            communication += n;
        }
        if a.collections.iter().any(|c| c == "working") {
            working += n;
        }
    }
    json!({"sources":p.artifacts.iter().filter(|a|!a.deleted).count(),"communication_spans":communication,"working_spans":working,"attribution_needed":missing,"pending":p.observations.iter().filter(|o|o.valid&&o.status=="pending").count(),"accepted":p.observations.iter().filter(|o|o.valid&&o.status=="accepted").count(),"calibrated":false})
}
pub fn validate_project(alias: &str, purpose: &str, aliases: &[String]) -> ApiResult<()> {
    if alias.trim().is_empty()
        || alias.len() > 160
        || purpose.len() > 4000
        || aliases.len() > 50
        || aliases.iter().any(|s| s.trim().is_empty() || s.len() > 160)
    {
        return Err(ApiError::bad("Use a short display name and author labels"));
    }
    Ok(())
}
