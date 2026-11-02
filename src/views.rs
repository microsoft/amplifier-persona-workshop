use crate::{
    error::{ApiError, ApiResult},
    model::id,
    state::AppState,
};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
pub fn create(state: &AppState) -> ApiResult<Value> {
    let v = json!({"id":id(),"revision":1,"screen":"projects","project_id":null,"artifact_id":null,"observation_id":null,"profile_revision_id":null,"tab":"communication","filter":"all","modal":null,"draft":{},"focus":null,"scroll":null,"controller":"human","rendered_revision":0});
    state.db.lock().unwrap().execute(
        "INSERT INTO views VALUES(?1,?2)",
        params![v["id"].as_str(), v.to_string()],
    )?;
    Ok(v)
}
pub fn read(state: &AppState, id: &str) -> ApiResult<Value> {
    let s: Option<String> = state
        .db
        .lock()
        .unwrap()
        .query_row("SELECT value FROM views WHERE id=?1", [id], |r| r.get(0))
        .optional()?;
    serde_json::from_str(&s.ok_or_else(ApiError::missing)?)
        .map_err(|_| ApiError::bad("Invalid view record"))
}
fn short_text(v: &Value, max: usize) -> bool {
    v.as_str().is_some_and(|s| s.len() <= max)
}
fn text_list(v: &Value, max_items: usize, max_chars: usize) -> bool {
    v.as_array()
        .is_some_and(|a| a.len() <= max_items && a.iter().all(|v| short_text(v, max_chars)))
}
fn safe_draft(v: &Value) -> bool {
    let Some(map) = v.as_object() else {
        return false;
    };
    map.iter().all(|(key, value)| match key.as_str() {
        "project_alias"
        | "project_aliases"
        | "project_purpose"
        | "source_name"
        | "artifact_extra_authors"
        | "observation_statement"
        | "observation_scope"
        | "evidence_id"
        | "settings_alias"
        | "settings_aliases"
        | "settings_purpose" => short_text(value, 4000),
        "agent_target_id" | "agent_restore_id" => value
            .as_str()
            .is_some_and(|s| s.is_empty() || uuid::Uuid::parse_str(s).is_ok()),
        "scenario" => short_text(value, 12000),
        "export_format" => value
            .as_str()
            .is_some_and(|s| ["native", "agents", "soul"].contains(&s)),
        "export_merge" => value.is_boolean(),
        "comparison_revision_a"
        | "comparison_revision_b"
        | "comparison_holdout"
        | "diff_from"
        | "diff_to" => value
            .as_str()
            .is_some_and(|s| s.is_empty() || uuid::Uuid::parse_str(s).is_ok()),
        "comparison_variant_a" | "comparison_variant_b" => value
            .as_str()
            .is_some_and(|s| ["both", "communication", "working", "baseline"].contains(&s)),
        "comparison_perspective" => value
            .as_str()
            .is_some_and(|s| ["self", "peer", "reviewer"].contains(&s)),
        "compare_blind" | "comparison_references" | "revision_diff" | "diff_full" => {
            value.is_boolean()
        }
        "observation_section" => value.as_str().is_some_and(|s| {
            crate::profiles::sections("communication")
                .iter()
                .chain(crate::profiles::sections("working"))
                .any(|(key, _)| *key == s)
        }),
        "source_kind" => value.as_str().is_some_and(|s| {
            [
                "meeting",
                "chat",
                "ai_chat",
                "notes",
                "profile",
                "email",
                "document",
                "recording",
            ]
            .contains(&s)
        }),
        "manual_collection" => value
            .as_str()
            .is_some_and(|s| ["communication", "working"].contains(&s)),
        "source_communication"
        | "source_working"
        | "source_holdout"
        | "artifact_communication"
        | "artifact_working"
        | "artifact_holdout"
        | "span_selection"
        | "variant_both"
        | "variant_communication"
        | "variant_working"
        | "variant_baseline"
        | "source_details"
        | "source_options"
        | "profile_details"
        | "profile_pinned"
        | "compare_profiles"
        | "review_details"
        | "observation_details"
        | "evaluation_details"
        | "activity_details"
        | "service_details" => value.is_boolean(),
        "agent_advanced" | "agent_history" | "agent_connection" => value.is_boolean(),
        "artifact_authors" => text_list(value, 100, 160),
        "artifact_span_ids" => value.is_null() || text_list(value, 5000, 100),
        key if key
            .strip_prefix("rating_")
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok()) =>
        {
            value.as_object().is_some_and(|r| {
                r.iter().all(|(key, v)| match key.as_str() {
                    "communication_fit" | "working_fit" => {
                        v.is_null()
                            || v.as_u64().is_some_and(|n| (1..=5).contains(&n))
                            || v.as_str()
                                .is_some_and(|s| ["", "1", "2", "3", "4", "5"].contains(&s))
                    }
                    "notes" => short_text(v, 4000),
                    _ => false,
                })
            })
        }
        key if key
            .strip_prefix("comparison_rating_")
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok()) =>
        {
            value.as_object().is_some_and(|map| {
                map.len() <= 13
                    && map.iter().all(|(key, v)| {
                        if key == "preference" {
                            return v.as_str().is_some_and(|s| {
                                ["", "A", "B", "C", "D", "tie", "neither"].contains(&s)
                            });
                        }
                        let Some((label, field)) = key.split_once('_') else {
                            return false;
                        };
                        ["A", "B", "C", "D"].contains(&label)
                            && match field {
                                "communication_fit" | "working_fit" => {
                                    v.as_str()
                                        .is_some_and(|s| ["", "1", "2", "3", "4", "5"].contains(&s))
                                        || v.as_u64().is_some_and(|n| (1..=5).contains(&n))
                                }
                                "notes" => short_text(v, 2000),
                                _ => false,
                            }
                    })
            })
        }
        _ => false,
    })
}
fn valid_field(key: &str, v: &Value) -> bool {
    match key {
        "screen" => v.as_str().is_some_and(|s| {
            [
                "projects", "evidence", "review", "profiles", "evaluate", "settings",
            ]
            .contains(&s)
        }),
        "tab" => v
            .as_str()
            .is_some_and(|s| ["communication", "working"].contains(&s)),
        "project_id" | "artifact_id" | "observation_id" | "profile_revision_id" => {
            v.is_null() || v.as_str().is_some_and(|s| uuid::Uuid::parse_str(s).is_ok())
        }
        "filter" | "focus" => v.is_null() || short_text(v, 300),
        "modal" => {
            v.is_null()
                || v.as_str().is_some_and(|s| {
                    [
                        "project.new",
                        "source.new",
                        "source",
                        "source.remove",
                        "observation.new",
                        "observation.edit",
                        "evidence",
                        "identity",
                        "agent.apply",
                        "agent.connect",
                        "profile.export",
                    ]
                    .contains(&s)
                })
        }
        "draft" => safe_draft(v),
        "scroll" => {
            v.is_null()
                || v.as_object().is_some_and(|m| {
                    m.len() == 2
                        && ["x", "y"].iter().all(|key| {
                            m.get(*key)
                                .and_then(Value::as_i64)
                                .is_some_and(|n| (0..=10_000_000).contains(&n))
                        })
                })
        }
        _ => false,
    }
}
pub fn commands(state: &AppState, id: &str, body: &Value) -> ApiResult<Value> {
    let mut db = state.db.lock().unwrap();
    let tx = db.transaction()?;
    let raw: Option<String> = tx
        .query_row("SELECT value FROM views WHERE id=?1", [id], |r| r.get(0))
        .optional()?;
    let mut v: Value = serde_json::from_str(&raw.ok_or_else(ApiError::missing)?).unwrap();
    if body["expected_revision"] != v["revision"] {
        return Err(ApiError(
            axum::http::StatusCode::CONFLICT,
            "The visible view changed; refresh its revision".into(),
        ));
    }
    let commands = body["commands"]
        .as_array()
        .ok_or_else(|| ApiError::bad("Supply semantic view commands"))?;
    if commands.len() > 30 {
        return Err(ApiError::bad("Too many view commands"));
    }
    for c in commands {
        let key = c["field"].as_str().unwrap_or("");
        let value = &c["value"];
        if c["op"] != "set"
            || ![
                "screen",
                "project_id",
                "artifact_id",
                "observation_id",
                "profile_revision_id",
                "tab",
                "filter",
                "modal",
                "draft",
                "focus",
                "scroll",
            ]
            .contains(&key)
        {
            return Err(ApiError::bad("Unsupported view command"));
        }
        if !valid_field(key, value) {
            return Err(ApiError::bad(
                "Invalid view field. Use typed navigation and form state; private source text, identity and credentials do not belong in view state.",
            ));
        }
        if value.to_string().len() > 16_000 {
            return Err(ApiError::bad("View field too large"));
        }
        v[key] = value.clone();
    }
    let controller = body["controller"].as_str().unwrap_or("programmatic");
    if controller.len() > 100 {
        return Err(ApiError::bad("Controller label too long"));
    }
    v["controller"] = json!(controller);
    v["revision"] = json!(v["revision"].as_u64().unwrap() + 1);
    tx.execute(
        "UPDATE views SET value=?2 WHERE id=?1",
        params![id, v.to_string()],
    )?;
    tx.commit()?;
    Ok(v)
}
pub fn ack(state: &AppState, id: &str, revision: u64) -> ApiResult<Value> {
    let mut db = state.db.lock().unwrap();
    let tx = db.transaction()?;
    let raw: Option<String> = tx
        .query_row("SELECT value FROM views WHERE id=?1", [id], |r| r.get(0))
        .optional()?;
    let mut v: Value = serde_json::from_str(&raw.ok_or_else(ApiError::missing)?).unwrap();
    if revision > v["revision"].as_u64().unwrap() {
        return Err(ApiError::bad("Cannot acknowledge a future view"));
    }
    let prior = v["rendered_revision"].as_u64().unwrap_or(0);
    v["rendered_revision"] = json!(prior.max(revision));
    tx.execute(
        "UPDATE views SET value=?2 WHERE id=?1",
        params![id, v.to_string()],
    )?;
    tx.commit()?;
    Ok(json!({"ok":true}))
}
