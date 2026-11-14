//! Blinded comparisons keep candidate mappings and reference answers off the model path.
use crate::{
    auth,
    error::{ApiError, ApiResult},
    model::{Job, Project, Revision, id},
    profiles,
    server::get_revision,
    state::{AppState, now_ms},
    worker,
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use rand::seq::SliceRandom;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc};
use tokio::sync::watch;

#[derive(Clone, Serialize, Deserialize)]
struct Candidate {
    label: String,
    revision_id: String,
    variant: String,
    response: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct Comparison {
    id: String,
    project_id: String,
    job_id: String,
    created_at: i64,
    scenario: String,
    holdout_ids: Vec<String>,
    holdout_hashes: Vec<String>,
    candidates: Vec<Candidate>,
    rating: Option<Value>,
    revealed_at: Option<i64>,
}
fn text(v: &Value, key: &str, max: usize) -> ApiResult<String> {
    let s = v[key].as_str().unwrap_or("").trim();
    if s.is_empty() || s.len() > max {
        return Err(ApiError::bad(format!("Enter a valid {key}")));
    }
    Ok(s.into())
}
fn read(s: &AppState, project: &str, cid: &str) -> ApiResult<Comparison> {
    let c: Comparison = serde_json::from_value(s.record("comparisons", cid)?)
        .map_err(|_| ApiError::bad("Invalid comparison"))?;
    if c.project_id != project {
        return Err(ApiError::missing());
    }
    Ok(c)
}
fn save_feedback(s: &AppState, before: &Value, c: &Comparison) -> ApiResult<()> {
    // Concurrent reveal/rate requests must not rewrite a revealed result as blind.
    let changed = s.db.lock().unwrap().execute(
        "UPDATE comparisons SET value=?1 WHERE id=?2 AND project_id=?3 AND value=?4",
        params![json!(c).to_string(), c.id, c.project_id, before.to_string()],
    )?;
    if changed != 1 {
        return Err(ApiError(
            axum::http::StatusCode::CONFLICT,
            "This comparison changed. Refresh before saving feedback.".into(),
        ));
    }
    Ok(())
}
fn holdouts(p: &Project, ids: &[String], revisions: &[Revision]) -> ApiResult<Vec<String>> {
    if ids.len() > 8 || ids.iter().collect::<BTreeSet<_>>().len() != ids.len() {
        return Err(ApiError::bad(
            "Choose up to eight distinct held-back sources",
        ));
    }
    ids.iter().map(|aid| {
        let a = p.artifacts.iter().find(|a| &a.id == aid && a.holdout && !a.deleted)
            .ok_or_else(|| ApiError::bad("Reference sources must still be held back from profile creation"))?;
        // Reject any source used by either historical revision, even if it has since
        // been moved to the held-back collection. Hashes also catch duplicate imports.
        if revisions.iter().any(|r| r.manifest["source_ids"].as_array().is_some_and(|used| used.iter().any(|id| {
            p.artifacts.iter().any(|old| Some(old.id.as_str()) == id.as_str() && (old.id == a.id || old.sha256 == a.sha256))
        }))) { return Err(ApiError::bad("A comparison reference was used to create a selected profile. Choose fresh held-back evidence.")); }
            let mut selectable = a.clone();
            selectable.holdout = false;
            if !a.spans.iter().any(|span| crate::evidence::eligible(&selectable, span)) {
                return Err(ApiError::bad("Identify the subject's speaker in each held-back reference first"));
            }
            Ok(auth::digest(&json!([a.sha256,a.authors,a.selected_span_ids,a.collections]).to_string()))
    }).collect()
}
fn current(s: &AppState, c: &Comparison) -> ApiResult<()> {
    let p = s.project(&c.project_id)?;
    let revisions = c
        .candidates
        .iter()
        .map(|a| get_revision(s, &c.project_id, &a.revision_id))
        .collect::<ApiResult<Vec<_>>>()?;
    if holdouts(&p, &c.holdout_ids, &revisions)? != c.holdout_hashes {
        return Err(ApiError::bad(
            "Reference evidence changed; run a new comparison",
        ));
    }
    Ok(())
}
fn public(s: &AppState, c: &Comparison) -> ApiResult<Value> {
    let job = s.record("jobs", &c.job_id)?;
    let valid = current(s, c).is_ok();
    let references = if valid {
        let p = s.project(&c.project_id)?;
        c.holdout_ids.iter().filter_map(|id| p.artifacts.iter().find(|a| &a.id == id)).map(|a| {
            // Only explicitly attributed excerpts; never substitute a peer's speech.
            let mut eligible = a.clone(); eligible.holdout = false;
            json!({"id":a.id,"name":a.name,"spans":a.spans.iter().filter(|span| crate::evidence::eligible(&eligible,span)).collect::<Vec<_>>()})
        }).collect::<Vec<_>>()
    } else {
        vec![]
    };
    Ok(
        json!({"id":c.id,"project_id":c.project_id,"job_id":c.job_id,"created_at":c.created_at,"scenario":c.scenario,
        "status":job["status"],"note":job["note"],"valid":valid,"reference_mode":if c.holdout_ids.is_empty(){"fresh_scenario"}else{"held_out_evidence"},
        "references":references,"rating":c.rating,"revealed_at":c.revealed_at,"calibrated":false,
        "candidates":c.candidates.iter().map(|a| {
            let mut v=json!({"label":a.label,"response":if valid {a.response.as_str()}else{""}});
            if c.revealed_at.is_some() {v["revision_id"]=json!(a.revision_id);v["variant"]=json!(a.variant);}
            v
        }).collect::<Vec<_>>() }),
    )
}
pub async fn list(
    State(s): State<Arc<AppState>>,
    h: HeaderMap,
    Path(pid): Path<String>,
) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "observe", false)?;
    s.project(&pid)?;
    let items = s
        .records("comparisons", &pid)?
        .into_iter()
        .map(|v| {
            let c: Comparison =
                serde_json::from_value(v).map_err(|_| ApiError::bad("Invalid comparison"))?;
            public(&s, &c)
        })
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(Json(json!({"items":items})))
}
pub async fn get(
    State(s): State<Arc<AppState>>,
    h: HeaderMap,
    Path((pid, cid)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "observe", false)?;
    Ok(Json(public(&s, &read(&s, &pid, &cid)?)?))
}
pub async fn start(
    State(s): State<Arc<AppState>>,
    h: HeaderMap,
    Path(pid): Path<String>,
    Json(v): Json<Value>,
) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "build", true)?;
    let p = s.project(&pid)?;
    if v["expected_revision"] != p.revision {
        return Err(ApiError(
            axum::http::StatusCode::CONFLICT,
            "Refresh this project before comparing".into(),
        ));
    }
    let extractor = s
        .config
        .extractor
        .clone()
        .ok_or_else(|| ApiError::bad("Connect an analysis endpoint first"))?;
    let scenario = text(&v, "scenario", 12000)?;
    let inputs = v["candidates"]
        .as_array()
        .filter(|a| (2..=4).contains(&a.len()))
        .ok_or_else(|| ApiError::bad("Compare two to four profile versions or variants"))?;
    let mut candidates = vec![];
    let mut revisions = vec![];
    let mut prompts = vec![];
    let mut unique = BTreeSet::new();
    for input in inputs {
        let rid = text(input, "revision_id", 100)?;
        let variant = text(input, "variant", 30)?;
        if !unique.insert((rid.clone(), variant.clone())) {
            return Err(ApiError::bad(
                "Choose different versions or profile variants",
            ));
        }
        let r = get_revision(&s, &pid, &rid)?;
        prompts.push(profiles::evaluation_prompt(&r, &scenario, &variant)?);
        revisions.push(r);
        candidates.push(Candidate {
            label: String::new(),
            revision_id: rid,
            variant,
            response: String::new(),
        });
    }
    let holdout_ids: Vec<String> =
        serde_json::from_value(v.get("holdout_ids").cloned().unwrap_or(json!([])))
            .map_err(|_| ApiError::bad("Use a list of held-back source IDs"))?;
    let holdout_hashes = holdouts(&p, &holdout_ids, &revisions)?;
    let mut paired = candidates.into_iter().zip(prompts).collect::<Vec<_>>();
    paired.shuffle(&mut rand::rngs::OsRng);
    for (i, (c, _)) in paired.iter_mut().enumerate() {
        c.label = ((b'A' + i as u8) as char).to_string();
    }
    let job = Job {
        id: id(),
        project_id: pid.clone(),
        kind: "comparison".into(),
        status: "queued".into(),
        stage: "queued".into(),
        note: "Preparing blind responses".into(),
        created_at: now_ms(),
        updated_at: now_ms(),
        source_revision: p.revision,
        result_id: None,
        submission: None,
    };
    let c = Comparison {
        id: id(),
        project_id: pid.clone(),
        job_id: job.id.clone(),
        created_at: now_ms(),
        scenario,
        holdout_ids,
        holdout_hashes,
        candidates: paired.iter().map(|(c, _)| c.clone()).collect(),
        rating: None,
        revealed_at: None,
    };
    s.enqueue(&job)?;
    s.save_record("comparisons", &c.id, &pid, &json!(c))?;
    s.audit(
        "comparison_started",
        Some(&pid),
        json!({"comparison_id":c.id,"job_id":job.id}),
    )?;
    let response = public(&s, &c)?;
    let (tx, mut rx) = watch::channel(false);
    s.running.lock().unwrap().insert(job.id.clone(), tx);
    tokio::spawn(async move {
        let mut job = job;
        let mut c = c;
        let work = async {
            let _permit = s
                .work_gate
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| ApiError::bad("Worker unavailable"))?;
            job.status = "running".into();
            job.stage = "simulating".into();
            job.note = "Generating separate responses with the same analysis endpoint".into();
            s.save_record("jobs", &job.id, &pid, &json!(job))?;
            for (i, (_, prompt)) in paired.iter().enumerate() {
                // Fresh ACP session each time; no reference answer, other candidate,
                // feedback, or source artifact is ever included in these prompts.
                let response = worker::run(&extractor, prompt).await.map_err(|_| {
                    ApiError::bad(
                        "Comparison generation failed; check the analysis endpoint and try again",
                    )
                })?;
                current(&s, &c)?;
                c.candidates[i].response = response;
            }
            current(&s, &c)?;
            s.save_record("comparisons", &c.id, &pid, &json!(c))?;
            Ok::<_, ApiError>(())
        };
        match tokio::select! {r=work=>Some(r),_=rx.changed()=>None} {
            Some(Ok(())) => {
                job.status = "complete".into();
                job.note = "Rate the responses before revealing their profiles".into();
                job.result_id = Some(c.id.clone());
            }
            Some(Err(e)) => {
                job.status = "failed".into();
                job.note = e.1;
            }
            None => {
                job.status = "cancelled".into();
                job.note = "Comparison cancelled".into();
            }
        }
        job.stage = job.status.clone();
        job.updated_at = now_ms();
        let _ = s.save_record("jobs", &job.id, &pid, &json!(job));
        s.running.lock().unwrap().remove(&job.id);
    });
    Ok(Json(response))
}
pub async fn rate(
    State(s): State<Arc<AppState>>,
    h: HeaderMap,
    Path((pid, cid)): Path<(String, String)>,
    Json(v): Json<Value>,
) -> ApiResult<Json<Value>> {
    let who = auth::check(&s, &h, "edit", true)?;
    let mut c = read(&s, &pid, &cid)?;
    let before = json!(c);
    current(&s, &c)?;
    if s.record("jobs", &c.job_id)?["status"] != "complete" || c.revealed_at.is_some() {
        return Err(ApiError::bad(
            "Rate completed responses before revealing their profiles",
        ));
    }
    let ratings = v["ratings"]
        .as_object()
        .ok_or_else(|| ApiError::bad("Rate each response"))?;
    if ratings.len() != c.candidates.len() {
        return Err(ApiError::bad("Rate each response before revealing"));
    }
    for candidate in &c.candidates {
        let r = ratings
            .get(&candidate.label)
            .and_then(Value::as_object)
            .ok_or_else(|| ApiError::bad("Rate each response label"))?;
        if !["communication_fit", "working_fit"].iter().all(|k| {
            r.get(*k)
                .and_then(Value::as_u64)
                .is_some_and(|n| (1..=5).contains(&n))
        }) || r
            .keys()
            .any(|k| !["communication_fit", "working_fit", "notes"].contains(&k.as_str()))
            || r.get("notes")
                .is_some_and(|v| v.as_str().is_none_or(|s| s.len() > 4000))
        {
            return Err(ApiError::bad(
                "Give communication and working style ratings from 1 to 5, with optional notes",
            ));
        }
    }
    let preference = text(&v, "preference", 20)?;
    if !["tie", "neither"].contains(&preference.as_str())
        && !c.candidates.iter().any(|a| a.label == preference)
    {
        return Err(ApiError::bad("Choose a response, tie, or neither"));
    }
    let perspective = v["perspective"].as_str().unwrap_or("reviewer");
    if !["self", "peer", "reviewer"].contains(&perspective) {
        return Err(ApiError::bad("Choose self, peer, or reviewer feedback"));
    }
    c.rating = Some(
        json!({"ratings":ratings,"preference":preference,"perspective":perspective,"reviewer":who.actor,"recorded_at":now_ms(),"blind":true}),
    );
    save_feedback(&s, &before, &c)?;
    s.audit(
        "comparison_rated",
        Some(&pid),
        json!({"comparison_id":cid,"reviewer":who.actor}),
    )?;
    Ok(Json(public(&s, &c)?))
}
pub async fn reveal(
    State(s): State<Arc<AppState>>,
    h: HeaderMap,
    Path((pid, cid)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "edit", true)?;
    let mut c = read(&s, &pid, &cid)?;
    let before = json!(c);
    current(&s, &c)?;
    if c.rating.is_none() {
        return Err(ApiError::bad(
            "Save feedback for every response before revealing",
        ));
    }
    c.revealed_at.get_or_insert_with(now_ms);
    save_feedback(&s, &before, &c)?;
    Ok(Json(public(&s, &c)?))
}
pub async fn diff(
    State(s): State<Arc<AppState>>,
    h: HeaderMap,
    Path((pid, from, to)): Path<(String, String, String)>,
) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "observe", false)?;
    let a = get_revision(&s, &pid, &from)?;
    let b = get_revision(&s, &pid, &to)?;
    let files=["communication.md","working-style.md"].iter().map(|name|{
        let old=a.files.get(*name).map(String::as_str).unwrap_or("");let new=b.files.get(*name).map(String::as_str).unwrap_or("");
        let left=old.split("\n\n").map(str::trim).filter(|x|!x.is_empty()).collect::<BTreeSet<_>>();
        let right=new.split("\n\n").map(str::trim).filter(|x|!x.is_empty()).collect::<BTreeSet<_>>();
        json!({"file":name,"before":old,"after":new,"changed":old!=new,"removed_paragraphs":left.difference(&right).collect::<Vec<_>>(),"added_paragraphs":right.difference(&left).collect::<Vec<_>>()})
    }).collect::<Vec<_>>();
    Ok(Json(
        json!({"from":from,"to":to,"files":files,"format":"paragraph-diff/1"}),
    ))
}
