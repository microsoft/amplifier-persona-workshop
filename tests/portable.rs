//! File-format compatibility checks, not model impersonation/fidelity claims.
use serde_json::json;
use std::{collections::BTreeMap, io::Read};
use twin_workbench::{auth::digest, model::Revision, portable};

fn revision() -> Revision {
    let files = BTreeMap::from([
        ("communication.md".into(), "# Communication\n\n## Phrasing\n\nUse a brief ‘Yeah, that tracks’ when an idea lands, then name the part that clicked.\n".into()),
        ("working-style.md".into(), "# Thinking and working\n\n## Decisions\n\nSketch the user experience before deciding which implementation to reuse.\n\n```sh\n# Keep this example unchanged.\n```\n".into()),
        ("claims.json".into(), "[{\"scope\":\"Peer chats\",\"support\":\"limited\"}]".into()),
        ("evaluation-summary.json".into(), "{\"calibrated\":false}".into()),
    ]);
    let hashes: BTreeMap<_, _> = files
        .iter()
        .map(|(k, v): (&String, &String)| (k.clone(), digest(v)))
        .collect();
    Revision {
        id: "revision-id".into(),
        project_id: "subject-key".into(),
        created_at: 0,
        status: "draft".into(),
        valid: true,
        manifest: json!({"instruction_format":2,"subject_key":"subject-key","files":hashes}),
        files,
    }
}

#[test]
fn agents_workspace_loads_both_instruction_files_from_entrypoint_relative_paths() {
    let export = portable::package(&revision(), "agents", None).unwrap();
    let bytes = portable::zip_package(&export).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let home = tempfile::tempdir().unwrap();
    zip.extract(home.path()).unwrap();
    let entrypoint = std::fs::read_to_string(home.path().join("AGENTS.md")).unwrap();
    // AGENTS.md is plain Markdown, without a standardized include directive. The
    // loader explicitly asks the agent to read the referenced relative files.
    assert!(entrypoint.contains("Before responding or planning work, read"));
    for file in ["communication.md", "working-style.md"] {
        assert!(entrypoint.contains(&format!("[{file}](persona/{file})")));
        let loaded = std::fs::read_to_string(home.path().join("persona").join(file)).unwrap();
        assert_eq!(loaded, revision().files[file]);
        assert!(!loaded.contains("calibrated"));
        assert!(!loaded.contains("support"));
    }
    assert!(home.path().join("persona/metadata/claims.json").is_file());
}

#[test]
fn soul_is_directly_loadable_without_reading_any_other_file() {
    let r = revision();
    let export = portable::package(&r, "soul", None).unwrap();
    let bytes = portable::zip_package(&export).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    // OpenClaw's workspace bootstrap discovers root SOUL.md directly; it doesn't
    // automatically treat arbitrary persona files as bootstrap instructions.
    let mut soul = String::new();
    zip.by_name("SOUL.md")
        .unwrap()
        .read_to_string(&mut soul)
        .unwrap();
    for file in ["communication.md", "working-style.md"] {
        assert!(soul.contains(r.files[file].trim()));
    }
    assert!(!soul.contains("persona/"));
    assert!(!soul.contains("calibrated"));
    assert!(!soul.contains("Peer chats"));
    assert_eq!(export["manifest"]["entrypoint"], "SOUL.md");
    assert_eq!(export["manifest"]["files"]["SOUL.md"], digest(&soul));
}

#[test]
fn repeated_agents_exports_preserve_owners_instructions_and_line_endings() {
    for original in [
        "",
        "# Existing\n\nKeep my test commands.\n",
        "# Existing\r\nKeep my test commands.\r\n",
        "# Existing without a final newline",
    ] {
        let merged = portable::merge_agents(original).unwrap();
        assert!(merged.starts_with(original));
        assert_eq!(portable::merge_agents(&merged).unwrap(), merged);
        assert_eq!(merged.matches(portable::BEGIN).count(), 1);
    }
    let before = "# Existing\r\n\r\n";
    let after = "\r\n\r\n## Tests\r\nRun the test runner.\r\n";
    let old = format!(
        "{before}{}\r\nOld instructions\r\n{}{after}",
        portable::BEGIN,
        portable::END
    );
    let next = portable::merge_agents(&old).unwrap();
    assert!(next.starts_with(before));
    assert!(next.ends_with(after));
    assert!(!next.contains("Old instructions"));
    assert_eq!(portable::merge_agents(&next).unwrap(), next);
}

#[test]
fn ambiguous_markers_and_old_report_profiles_are_rejected() {
    for bad in [
        portable::BEGIN.to_owned(),
        portable::END.to_owned(),
        format!("{}\n{}", portable::END, portable::BEGIN),
        format!(
            "{}\n{}\n{}",
            portable::BEGIN,
            portable::BEGIN,
            portable::END
        ),
        format!("Example: {}\n{}", portable::BEGIN, portable::END),
        "x".repeat(portable::MAX_EXISTING_BYTES + 1),
    ] {
        assert!(portable::merge_agents(&bad).is_err());
    }
    let mut r = revision();
    r.manifest["instruction_format"] = json!(1);
    assert!(portable::package(&r, "agents", None).is_err());
    r.manifest["instruction_format"] = json!(2);
    r.files.insert(
        "communication.md".into(),
        "Unexpected altered content".into(),
    );
    assert!(portable::package(&r, "soul", None).is_err());
}
