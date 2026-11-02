# Portable persona exports

The same saved profile version can be applied to a Constellation instance or used in another agent's workspace. The exported instructions remain the entity-specific communication and working guidance. Source attribution, confidence, review information and package metadata stay in separate JSON files. Private identity bindings and raw evidence are not exported.

## Formats

| Format | Entry point | Files an agent needs |
| --- | --- | --- |
| Native | `.twin.json` package or unpacked profile directory | `communication.md` and `working-style.md`, loaded by the consumer |
| AGENTS.md | `AGENTS.md` at the workspace root | The loader explicitly asks the agent to read `persona/communication.md` and `persona/working-style.md` relative to it |
| SOUL.md | `SOUL.md` at the OpenClaw agent workspace root | Both profiles are in this one file; there are no required external includes |

The [AGENTS.md convention](https://agents.md/) uses ordinary Markdown and directory-scoped instructions; it does not define an automatic include syntax. The generated loader therefore uses a direct read instruction and relative Markdown links. A host must support AGENTS.md discovery and reading those workspace files. A closer nested instruction file may affect which instructions the host loads.

[OpenClaw's workspace documentation](https://docs.openclaw.ai/concepts/agent-workspace) names `SOUL.md` as a directly loaded persona file. Its [SOUL.md guidance](https://docs.openclaw.ai/concepts/soul) describes the personality/tone role. The export is self-contained rather than assuming arbitrary neighboring profile files enter the bootstrap context. Host size limits and session/subagent loading rules still apply. Export compatibility describes these file conventions; it does not establish identical behavior across models or hosts.

An AGENTS.md ZIP contains the entry point, the two profiles under `persona/`, and metadata under `persona/metadata/`. A SOUL.md ZIP contains the entry point and the same separate metadata directory. The original native package is unchanged.

## Preserving existing instructions

Select **Profiles → Export… → AGENTS.md**, then enable **Keep an existing AGENTS.md** and choose or paste the file. Download and extract the resulting ZIP into the intended workspace. The merge preserves all bytes outside these complete marker lines:

```markdown
<!-- persona-workshop:begin -->
... generated loader ...
<!-- persona-workshop:end -->
```

An existing marked section is replaced, so repeated exports are idempotent. CRLF line endings are retained. Multiple, reversed, incomplete, or non-line markers are rejected instead of guessing which content to replace. The file must be UTF-8, at most 256 KB. The merge does not change the selected source file on your computer, store it in project records, or place its text in shared display state. Existing user-supplied content is preserved as supplied and may contain private material; the generated profile's privacy filtering does not rewrite that existing content.

The CLI's `export --format agents --agents-file PATH --output NEW_DIRECTORY` produces the same merged workspace, leaving `PATH` unchanged. Omit `--agents-file` for a new loader. `--format soul` creates the OpenClaw export. `--format native` is the backwards-compatible default. Export always requires a new output directory and verifies the returned file digests.

## HTTP and view control

For project `P` and saved revision `R`, with an `export`-scoped token:

- `GET /api/v1/projects/P/revisions/R/exports/agents` returns a JSON workspace package; substitute `soul` for the other format.
- `GET .../exports/agents/download` returns a ZIP attachment; substitute `soul` as needed.
- `POST .../exports/agents` with `{"existing_agents":"...existing file text..."}` returns a merged JSON package. `POST .../exports/agents/download` returns its ZIP. Cookie-authenticated POSTs require the normal CSRF token.
- `GET .../export` retains the native package contract.

The workspace package schema is `persona-workshop.export/1`. It carries a `files` mapping and manifest with the entry point, source revision and SHA-256 file digests. The manifest is also included as `persona/metadata/manifest.json`; the original native manifest is `source-manifest.json` in that directory. `privacy_scope` is `generated_profile_content_only`, and `existing_agents_included` reports whether the supplied AGENTS.md contributed content. No project or profile mutation occurs during export.

Every endpoint rejects invalidated revisions and older report-format profiles that need rebuilding. Authentication and export scope apply equally to the JSON and ZIP routes.

The semantic modal is `profile.export`. Its public draft fields are `export_format` (`agents`, `soul`, `native`) and `export_merge` (boolean). The browser actions are `export.open`, `export.existing` with `{text}`, and `export.download`. They are exposed through `window.personaWorkshop.act`; `actions()` lists them. The existing file text is passed directly, never through shared drafts. A separate agent can perform the transformation entirely through HTTP and control the display through the existing view API.

## Validation boundaries

Rust tests extract actual ZIPs and resolve the AGENTS.md relative references, check that SOUL.md contains both profiles without external files, preserve instruction text and code examples, and exercise repeated and malformed merges. HTTP tests check authorization, privacy/version invalidation and absence of merge input from persistent records. Browser checks drive the export controls against the real temporary HTTPS service and compare the resulting files with actual CLI exports. These checks validate file and integration behavior, not a live OpenClaw model session or persona fidelity.
