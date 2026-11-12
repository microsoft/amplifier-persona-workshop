use crate::{
    auth,
    config::{self, Extractor},
    error::{ApiError, ApiResult},
    evidence,
    state::AppState,
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};

const MAX_FILE: usize = 20 * 1024 * 1024;
const CONVERTER: &str = include_str!("../scripts/import-document.py");
#[cfg(unix)]
struct ProcessGroup(u32);
#[cfg(unix)]
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        #[cfg(unix)]
        if self.0 > 0 {
            unsafe {
                libc::kill(-(self.0 as i32), libc::SIGKILL);
            }
        }
    }
}
pub async fn capabilities(State(s): State<Arc<AppState>>, h: HeaderMap) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "observe", false)?;
    Ok(Json(
        json!({"formats":["txt","md","json","eml","html","docx","odt","pdf","transcript.json"],"recording_formats":["wav","mp3","m4a","ogg","webm","mp4"],"max_file_bytes":MAX_FILE,"transcription_ready":s.config.transcription.is_some(),"speaker_identification":"Explicit speaker selection is required; diarization labels are not identities","pdf_backend":s.config.pdf_backend.as_str(),"document_python":if s.config.document_python.is_some(){"installation-private"}else{"system"},"pdf_requires":if s.config.pdf_backend==config::PdfBackend::Pypdf{"Bundled readable-text extraction; encrypted and scanned PDFs are unsupported; review layout after import"}else{"pdftotext; scanned PDFs need OCR before import"}}),
    ))
}
async fn command(c: &Extractor, input: Value) -> ApiResult<Value> {
    let mut cmd = Command::new(&c.command);
    cmd.args(&c.args)
        .current_dir(&c.cwd)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for key in [
        "PATH",
        "HOME",
        "LANG",
        "TMPDIR",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "SYSTEMROOT",
        "WINDIR",
        "USERPROFILE",
        "TEMP",
        "TMP",
    ] {
        if let Ok(v) = std::env::var(key) {
            cmd.env(key, v);
        }
    }
    cmd.envs(&c.env);
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd.spawn().map_err(|_| {
        ApiError::bad("The configured import or transcription command is unavailable")
    })?;
    #[cfg(unix)]
    let _group = ProcessGroup(child.id().unwrap_or(0));
    #[cfg(windows)]
    let _job = crate::process_tree::ProcessJob::attach(&child)?;
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap().take(3 * 1024 * 1024 + 1);
    let work = async {
        // Writing and reading run concurrently so converters cannot block on a full pipe.
        let write = async move {
            stdin.write_all(input.to_string().as_bytes()).await?;
            stdin.shutdown().await?;
            drop(stdin);
            Ok::<_, std::io::Error>(())
        };
        let read = async {
            let mut output = Vec::new();
            stdout.read_to_end(&mut output).await?;
            Ok::<_, std::io::Error>(output)
        };
        let (_, output) = tokio::try_join!(write, read)
            .map_err(|_| ApiError::bad("Import command did not complete"))?;
        if output.len() > 3 * 1024 * 1024 {
            return Err(ApiError::bad("Converted text exceeds its limit"));
        }
        let status = child
            .wait()
            .await
            .map_err(|_| ApiError::bad("Import command failed"))?;
        if !status.success() {
            return Err(ApiError::bad(
                "Cannot extract this file. Check its format and configured converter; scanned PDFs need OCR first.",
            ));
        }
        serde_json::from_slice(&output)
            .map_err(|_| ApiError::bad("Converter must return the documented transcript JSON"))
    };
    tokio::time::timeout(Duration::from_secs(c.timeout_seconds), work)
        .await
        .map_err(|_| ApiError::bad("Import timed out"))?
}
struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn recording_messages(v: &Value) -> ApiResult<Value> {
    let segments = v["segments"]
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= 5000)
        .ok_or_else(|| ApiError::bad("Transcript needs 1–5000 diarized segments"))?;
    let mut messages = vec![];
    for segment in segments {
        let speaker = segment["speaker"]
            .as_str()
            .filter(|s| !s.trim().is_empty() && s.len() <= 160)
            .ok_or_else(|| {
                ApiError::bad(
                    "Every segment needs a speaker label; identify the subject after import",
                )
            })?;
        let text = segment["text"]
            .as_str()
            .filter(|s| !s.trim().is_empty() && s.len() <= 100_000)
            .ok_or_else(|| ApiError::bad("Each transcript segment needs bounded text"))?;
        let start = segment["start"]
            .as_f64()
            .filter(|n| n.is_finite() && *n >= 0.0)
            .ok_or_else(|| ApiError::bad("Each segment needs a start time in seconds"))?;
        let end = segment["end"]
            .as_f64()
            .filter(|n| n.is_finite() && *n >= start)
            .ok_or_else(|| ApiError::bad("Each segment needs an end time after its start"))?;
        messages
            .push(json!({"author":speaker,"role":"author","text":text,"start":start,"end":end}));
    }
    Ok(json!({"messages":messages}))
}
pub async fn upload(
    State(s): State<Arc<AppState>>,
    h: HeaderMap,
    Path(pid): Path<String>,
    Json(v): Json<Value>,
) -> ApiResult<Json<Value>> {
    auth::check(&s, &h, "edit", true)?;
    let p = s.project(&pid)?;
    let expected = v["expected_revision"]
        .as_u64()
        .ok_or_else(|| ApiError::bad("Supply the current project revision"))?;
    if expected != p.revision {
        return Err(ApiError(
            axum::http::StatusCode::CONFLICT,
            "Refresh before importing".into(),
        ));
    }
    let filename = v["filename"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 240 && !s.contains(['/', '\\', '\0']))
        .ok_or_else(|| ApiError::bad("Supply the filename without folders"))?;
    let name = v["name"].as_str().unwrap_or(filename).to_owned();
    let encoded = v["content_base64"]
        .as_str()
        .ok_or_else(|| ApiError::bad("Supply base64 file content"))?;
    if encoded.len() > MAX_FILE * 4 / 3 + 8 {
        return Err(ApiError::bad("Files must be 20 MB or smaller"));
    }
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| ApiError::bad("Invalid file encoding"))?;
    if bytes.is_empty() || bytes.len() > MAX_FILE {
        return Err(ApiError::bad(
            "Files must contain data and be 20 MB or smaller",
        ));
    }
    let ext = std::path::Path::new(filename)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    let supplied_authors: Option<Vec<String>> = v
        .get("authors")
        .map(|a| serde_json::from_value(a.clone()))
        .transpose()
        .map_err(|_| ApiError::bad("Use a list of author labels"))?;
    let mut authors = supplied_authors.unwrap_or_else(|| p.aliases.clone());
    let _permit = s
        .work_gate
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ApiError::bad("Import worker unavailable"))?;
    let (kind, text) = if ["wav", "mp3", "m4a", "ogg", "webm", "mp4"].contains(&ext.as_str()) {
        let c = s.config.transcription.as_ref().ok_or_else(|| {
            ApiError::bad(
                "Configure a transcription provider first, or import its diarized transcript JSON",
            )
        })?;
        let folder = s.home.join(format!(".recording-{}", uuid::Uuid::new_v4()));
        config::private_dir(&folder)?;
        let _cleanup = Temporary(folder.clone());
        let path = folder.join(format!("media.{ext}"));
        config::private_write(&path, &bytes)?;
        let result=command(c,json!({"schema":"persona-transcription/1","media_path":path,"filename":filename,"diarization":true})).await?;
        authors.clear();
        (
            "recording".to_owned(),
            recording_messages(&result)?.to_string(),
        )
    } else if ["eml", "html", "htm", "docx", "odt", "pdf"].contains(&ext.as_str()) {
        let c = Extractor {
            command: s
                .config
                .document_python
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| "python3".into()),
            args: vec!["-I".into(), "-B".into(), "-c".into(), CONVERTER.into()],
            env: Default::default(),
            cwd: s.home.to_string_lossy().into(),
            timeout_seconds: 35,
        };
        let result=command(&c,json!({"filename":filename,"content_base64":encoded,"pdf_backend":s.config.pdf_backend.as_str(),"author":if authors.len()==1{authors[0].as_str()}else{"Unattributed"}})).await?;
        (
            result["kind"].as_str().unwrap_or("document").to_owned(),
            json!({"messages":result["messages"]}).to_string(),
        )
    } else if ["txt", "md", "markdown", "json"].contains(&ext.as_str()) {
        let text =
            String::from_utf8(bytes).map_err(|_| ApiError::bad("Text imports must be UTF-8"))?;
        if let Ok(parsed) = serde_json::from_str::<Value>(&text)
            && parsed.get("segments").is_some()
        {
            authors.clear();
            ("recording".into(), recording_messages(&parsed)?.to_string())
        } else {
            (v["kind"].as_str().unwrap_or("notes").to_owned(), text)
        }
    } else {
        return Err(ApiError::bad(
            "Choose a supported text, document, email, transcript, or recording format",
        ));
    };
    let collections = v
        .get("collections")
        .map(|a| serde_json::from_value(a.clone()))
        .transpose()
        .map_err(|_| ApiError::bad("Choose communication, working style, or both"))?;
    let a = evidence::import(name, kind, text, collections, authors, v["holdout"] == true)
        .map_err(|e| ApiError::bad(e.to_string()))?;
    let aid = a.id.clone();
    let p = s.change(&pid, expected, |p| {
        if p.artifacts.len() >= 200 {
            return Err(ApiError::bad("Project has reached its 200-source limit"));
        }
        if p.artifacts
            .iter()
            .any(|old| !old.deleted && old.sha256 == a.sha256)
        {
            return Err(ApiError::bad(
                "This source is already present; edit its collections to reuse it",
            ));
        }
        p.artifacts.push(a);
        Ok(())
    })?;
    s.audit(
        "source_imported",
        Some(&pid),
        json!({"artifact_id":aid,"format":ext}),
    )?;
    Ok(Json(s.snapshot(&p)?))
}
