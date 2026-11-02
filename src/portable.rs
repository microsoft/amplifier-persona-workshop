//! Workspace exports. Persona instructions stay separate from provenance and identity.
use crate::{
    auth,
    error::{ApiError, ApiResult},
    model::Revision,
    server::get_revision,
    state::AppState,
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, io::Write, sync::Arc};

pub const BEGIN: &str = "<!-- persona-workshop:begin -->";
pub const END: &str = "<!-- persona-workshop:end -->";
pub const MAX_EXISTING_BYTES: usize = 256 * 1024;

fn loader() -> String {
    format!(
        "{BEGIN}\n## Persona guidance\n\nBefore responding or planning work, read [communication.md](persona/communication.md) and [working-style.md](persona/working-style.md), relative to this AGENTS.md. Apply the communication instructions to phrasing and tone, and the working-style instructions to decisions and collaboration.\n{END}"
    )
}

/// Replace only our complete marked section. Everything outside it is byte-preserved.
/// An incomplete or repeated section is ambiguous and must be repaired explicitly.
pub fn merge_agents(existing: &str) -> ApiResult<String> {
    if existing.len() > MAX_EXISTING_BYTES || existing.contains('\0') {
        return Err(ApiError::bad("Choose a UTF-8 AGENTS.md file up to 256 KB"));
    }
    let begin: Vec<_> = existing.match_indices(BEGIN).map(|(i, _)| i).collect();
    let end: Vec<_> = existing.match_indices(END).map(|(i, _)| i).collect();
    let newline = if existing.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let block = loader().replace('\n', newline);
    match (begin.as_slice(), end.as_slice()) {
        ([], []) => {
            let separator =
                if existing.is_empty() || existing.ends_with(&format!("{newline}{newline}")) {
                    String::new()
                } else if existing.ends_with('\n') {
                    newline.to_owned()
                } else {
                    format!("{newline}{newline}")
                };
            Ok(format!("{existing}{separator}{block}{newline}"))
        }
        ([start], [finish]) if start < finish => {
            let marker_line = |offset: usize, marker: &str| {
                (offset == 0 || existing[..offset].ends_with('\n'))
                    && (existing[offset + marker.len()..].is_empty()
                        || existing[offset + marker.len()..].starts_with('\n')
                        || existing[offset + marker.len()..].starts_with("\r\n"))
            };
            if !marker_line(*start, BEGIN) || !marker_line(*finish, END) {
                return Err(ApiError::bad(
                    "Persona Workshop markers must occupy their own lines",
                ));
            }
            Ok(format!(
                "{}{block}{}",
                &existing[..*start],
                &existing[*finish + END.len()..]
            ))
        }
        _ => Err(ApiError::bad(
            "AGENTS.md contains incomplete or repeated Persona Workshop markers. Repair that section before merging.",
        )),
    }
}

fn instruction_files(r: &Revision) -> ApiResult<(&str, &str)> {
    if r.manifest["instruction_format"] != 2 {
        return Err(ApiError::bad(
            "Rebuild this older profile before exporting workspace instructions",
        ));
    }
    let get = |name: &str| {
        let text = r
            .files
            .get(name)
            .ok_or_else(|| ApiError::bad("Profile file is missing"))?;
        if r.manifest["files"][name] != auth::digest(text) {
            return Err(ApiError::bad("Profile content does not match its manifest"));
        }
        Ok(text.as_str())
    };
    Ok((get("communication.md")?, get("working-style.md")?))
}

pub fn package(r: &Revision, format: &str, existing: Option<&str>) -> ApiResult<Value> {
    let (communication, working) = instruction_files(r)?;
    if existing.is_some() && format != "agents" {
        return Err(ApiError::bad(
            "An existing AGENTS.md can only be used with the AGENTS.md export",
        ));
    }
    let mut files: BTreeMap<String, String> = BTreeMap::new();
    match format {
        "agents" => {
            files.insert("AGENTS.md".into(), merge_agents(existing.unwrap_or(""))?);
            files.insert("persona/communication.md".into(), communication.into());
            files.insert("persona/working-style.md".into(), working.into());
        }
        "soul" => {
            // OpenClaw loads SOUL.md directly. No links to files that may not be loaded.
            files.insert(
                "SOUL.md".into(),
                format!("{}\n\n{}\n", communication.trim(), working.trim()),
            );
        }
        _ => {
            return Err(ApiError::bad(
                "Choose agents or soul as the workspace export format",
            ));
        }
    }
    for name in ["claims.json", "evaluation-summary.json"] {
        if let Some(text) = r.files.get(name) {
            files.insert(format!("persona/metadata/{name}"), text.clone());
        }
    }
    files.insert(
        "persona/metadata/source-manifest.json".into(),
        serde_json::to_string_pretty(&r.manifest).unwrap(),
    );
    let hashes: BTreeMap<_, _> = files
        .iter()
        .map(|(name, text)| (name.clone(), auth::digest(text)))
        .collect();
    let manifest = json!({
        "schema":"persona-workshop.export/1", "format":format,
        "subject_key":r.manifest["subject_key"], "revision":r.id,
        "files":hashes, "identity_included":false, "raw_evidence_included":false,
        "existing_agents_included":existing.is_some(),
        "privacy_scope":"generated_profile_content_only",
        "entrypoint":if format == "agents" { "AGENTS.md" } else { "SOUL.md" }
    });
    files.insert(
        "persona/metadata/manifest.json".into(),
        serde_json::to_string_pretty(&manifest).unwrap(),
    );
    Ok(json!({"schema":"persona-workshop.export/1","manifest":manifest,"files":files}))
}

pub fn zip_package(package: &Value) -> ApiResult<Vec<u8>> {
    let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .unix_permissions(0o600);
    for (name, content) in package["files"]
        .as_object()
        .ok_or_else(|| ApiError::bad("Invalid workspace package"))?
    {
        archive
            .start_file(name, options)
            .map_err(|_| ApiError::bad("Could not create workspace download"))?;
        archive
            .write_all(
                content
                    .as_str()
                    .ok_or_else(|| ApiError::bad("Invalid workspace file"))?
                    .as_bytes(),
            )
            .map_err(|_| ApiError::bad("Could not create workspace download"))?;
    }
    Ok(archive
        .finish()
        .map_err(|_| ApiError::bad("Could not finish workspace download"))?
        .into_inner())
}

type S = State<Arc<AppState>>;
type ExportPath = Path<(String, String, String)>;

pub async fn get(
    State(s): S,
    h: HeaderMap,
    Path((id, rid, format)): ExportPath,
) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "export", false)?;
    Ok(Json(package(&get_revision(&s, &id, &rid)?, &format, None)?))
}

pub async fn merge(
    State(s): S,
    h: HeaderMap,
    Path((id, rid, format)): ExportPath,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    // This is a transformation, not a stored edit. The supplied text never enters records,
    // audit events, shared views, or the native profile package.
    auth::check(&s, &h, "export", true)?;
    let text = body["existing_agents"]
        .as_str()
        .ok_or_else(|| ApiError::bad("Supply existing_agents as UTF-8 text"))?;
    Ok(Json(package(
        &get_revision(&s, &id, &rid)?,
        &format,
        Some(text),
    )?))
}

fn download_response(rid: &str, format: &str, package: &Value) -> ApiResult<Response> {
    Ok((
        [
            (header::CONTENT_TYPE, "application/zip".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"persona-{format}-{rid}.zip\""),
            ),
        ],
        zip_package(package)?,
    )
        .into_response())
}

pub async fn download(
    State(s): S,
    h: HeaderMap,
    Path((id, rid, format)): ExportPath,
) -> ApiResult<Response> {
    auth::check(&s, &h, "export", false)?;
    let r = get_revision(&s, &id, &rid)?;
    download_response(&r.id, &format, &package(&r, &format, None)?)
}

pub async fn merge_download(
    State(s): S,
    h: HeaderMap,
    Path((id, rid, format)): ExportPath,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    auth::check(&s, &h, "export", true)?;
    let r = get_revision(&s, &id, &rid)?;
    let text = body["existing_agents"]
        .as_str()
        .ok_or_else(|| ApiError::bad("Supply existing_agents as UTF-8 text"))?;
    download_response(&r.id, &format, &package(&r, &format, Some(text))?)
}
