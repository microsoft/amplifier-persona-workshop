use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Span {
    pub id: String,
    pub speaker: String,
    pub role: String,
    pub text: String,
    pub line_start: usize,
    pub line_end: usize,
    pub quoted: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub sha256: String,
    pub collections: Vec<String>,
    pub authors: Vec<String>,
    pub spans: Vec<Span>,
    pub raw: String,
    pub selected_span_ids: Option<Vec<String>>,
    pub holdout: bool,
    pub deleted: bool,
    pub created_at: i64,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Confidence {
    pub attribution: String,
    pub support: String,
    pub generalization: String,
    pub reason: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Observation {
    pub id: String,
    pub collection: String,
    #[serde(default)]
    pub section: String,
    pub statement: String,
    pub scope: String,
    pub confidence: Confidence,
    pub evidence_ids: Vec<String>,
    #[serde(default)]
    pub conflicts: Vec<String>,
    pub status: String,
    pub source: String,
    pub valid: bool,
    #[serde(default)]
    pub reviewer: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub alias: String,
    pub purpose: String,
    pub aliases: Vec<String>,
    pub subject_type: String,
    pub revision: u64,
    pub created_at: i64,
    pub updated_at: i64,
    pub artifacts: Vec<Artifact>,
    pub observations: Vec<Observation>,
    pub identity: BTreeMap<String, String>,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobSubmission {
    pub request_id: String,
    pub actor: String,
    pub fingerprint: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub project_id: String,
    pub kind: String,
    pub status: String,
    pub stage: String,
    pub note: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub source_revision: u64,
    #[serde(default)]
    pub result_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submission: Option<JobSubmission>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Revision {
    pub id: String,
    pub project_id: String,
    pub created_at: i64,
    pub status: String,
    pub valid: bool,
    pub manifest: Value,
    pub files: BTreeMap<String, String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Evaluation {
    pub id: String,
    pub revision_id: String,
    pub variant: String,
    pub scenario: String,
    pub response: String,
    pub created_at: i64,
    pub rating: Option<Value>,
}
