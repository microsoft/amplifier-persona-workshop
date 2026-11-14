use crate::{
    error::{ApiError, ApiResult},
    model::*,
    profiles,
    state::{AppState, now_ms},
    worker,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::watch;

pub struct EvaluationInput {
    pub revision: Revision,
    pub scenario: String,
    pub variant: String,
}
pub fn start(
    state: Arc<AppState>,
    p: Project,
    expected: u64,
    evaluation: Option<EvaluationInput>,
    submission: Option<JobSubmission>,
) -> ApiResult<Value> {
    if expected != p.revision {
        return Err(ApiError(
            axum::http::StatusCode::CONFLICT,
            "This project changed. Refresh first.".into(),
        ));
    }
    let extractor = state
        .config
        .extractor
        .clone()
        .ok_or_else(|| ApiError::bad("Configure an ACP analysis endpoint before building"))?;
    let prompt = if let Some(e) = &evaluation {
        profiles::evaluation_prompt(&e.revision, &e.scenario, &e.variant)?
    } else {
        profiles::extraction_prompt(&p)?
    };
    let job = Job {
        id: id(),
        project_id: p.id.clone(),
        kind: if evaluation.is_some() {
            "evaluation"
        } else {
            "build"
        }
        .into(),
        status: "queued".into(),
        stage: "queued".into(),
        note: "Waiting for the analysis worker".into(),
        created_at: now_ms(),
        updated_at: now_ms(),
        source_revision: p.revision,
        result_id: None,
        submission,
    };
    let response = json!(job);
    if let Some(prior) =
        state.enqueue_job(&job, evaluation.as_ref().map(|input| &input.revision))?
    {
        return Ok(json!(prior));
    }
    let (cancel, mut rx) = watch::channel(false);
    state.running.lock().unwrap().insert(job.id.clone(), cancel);
    state.audit(
        "analysis_queued",
        Some(&p.id),
        json!({"job_id":job.id,"kind":job.kind}),
    )?;
    tokio::spawn(async move {
        let mut job = job;
        let work = async {
            let _permit = state
                .work_gate
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| ApiError::bad("Worker unavailable"))?;
            job.status = "running".into();
            job.stage = if evaluation.is_some() {
                "simulating"
            } else {
                "extracting"
            }
            .into();
            job.note = "Analyzing the selected evidence through ACP".into();
            job.updated_at = now_ms();
            state.save_record("jobs", &job.id, &p.id, &json!(job))?;
            let result=worker::run(&extractor,&prompt).await.map_err(|_|ApiError::bad("The ACP analysis did not complete. Check the endpoint configuration and account, then start a new run."))?;
            let mut completed = job.clone();
            completed.status = "complete".into();
            completed.stage = "complete".into();
            completed.note = "Ready for review".into();
            completed.updated_at = now_ms();
            if let Some(e) = evaluation {
                let ev = Evaluation {
                    id: id(),
                    revision_id: e.revision.id.clone(),
                    variant: e.variant,
                    scenario: e.scenario,
                    response: result,
                    created_at: now_ms(),
                    rating: None,
                };
                completed.result_id = Some(ev.id.clone());
                state.save_evaluation(&p.id, &e.revision, &ev, &completed)?;
                Ok::<_, ApiError>(Some(ev.id))
            } else {
                let observations = profiles::parse_observations(&p, &result)?;
                let revision = state.save_build(&p.id, p.revision, observations, completed)?;
                Ok(Some(revision.id))
            }
        };
        let result = tokio::select! {r=work=>Some(r),_=rx.changed()=>None};
        match result {
            None => {
                job.status = "cancelled".into();
                job.note = "Cancelled; no candidate was applied".into();
            }
            Some(Ok(id)) => {
                job.status = "complete".into();
                job.note = if job.kind == "build" {
                    "Profiles are ready"
                } else {
                    "Response is ready"
                }
                .into();
                job.result_id = id;
            }
            Some(Err(e)) => {
                job.status = if e.0 == axum::http::StatusCode::CONFLICT {
                    "stale"
                } else {
                    "failed"
                }
                .into();
                job.note = e.1;
            }
        }
        job.stage = job.status.clone();
        job.updated_at = now_ms();
        let _ = state.save_record("jobs", &job.id, &p.id, &json!(job));
        let _ = state.audit(
            "analysis_finished",
            Some(&p.id),
            json!({"job_id":job.id,"kind":job.kind,"status":job.status}),
        );
        state.running.lock().unwrap().remove(&job.id);
    });
    Ok(response)
}
pub fn cancel(state: &AppState, id: &str) -> ApiResult<Value> {
    let job = state.record("jobs", id)?;
    if let Some(tx) = state.running.lock().unwrap().get(id) {
        let _ = tx.send(true);
    }
    Ok(job)
}
