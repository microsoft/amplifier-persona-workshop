# Persona Workshop

Part of [Amplifier Constellation](https://github.com/microsoft/amplifier-constellation).

Create an inspectable simulation profile from selected evidence, with separate communication and working-style files. This is a standalone Rust service with a browser UI, CLI, HTTP API, and semantic view controls. It can apply saved profile versions to an existing agent through that agent’s management API. Agent provisioning and Slack connections remain separate.

Import text/Markdown meeting transcripts, chats, notes, normalized JSON chat records, an existing profile, EML email, HTML, DOCX, ODT, or text-based PDF. Diarized transcript JSON is supported directly; raw recordings use an operator-configured transcription provider. A configured ACP endpoint extracts person-specific guidance and creates two portable Markdown files: **how they communicate** and **how they think and work**. Try them in a scenario, correct the guidance, or inspect its sources. Review metadata stays separate from the instructions an agent loads.

The service, executable and new installation folders use **Persona Workshop**. Existing installations can be upgraded in place; saved projects and the `twin-profile/1` export format remain compatible.

## Install

Linux with PAM and systemd is supported by this installer. Rust and build dependencies are installed if missing. The UI works in a browser on other operating systems; native Windows/macOS services are not implemented in this first version.

```bash
git clone https://github.com/microsoft/amplifier-persona-workshop.git
cd amplifier-persona-workshop
./install.sh --extractor-config /absolute/path/to/analysis-acp.json
```

The installer creates an independent private installation, chooses an available port, installs the owner-only PAM login service, starts a user service, and opens the browser when a desktop is available. It always prints the URL. Existing installations retain their data/configuration. Use `--home` for another installation, `--bind 0.0.0.0` for LAN access, `--port` for a fixed port, or `--public-url https://machine.example:port` behind a trusted HTTPS proxy. Certificate trust is required for the generated local HTTPS endpoint; setting a public URL does not provision DNS or a trusted certificate.

Sign in with the installation owner's Linux username/password. TLS is required. For Tailscale Serve, forward a dedicated HTTPS port to the local HTTPS endpoint and use the machine's `*.ts.net` DNS name. Keep existing Serve routes intact; do not enable Funnel for private evidence.

An analysis endpoint is optional for importing, attribution, manual observations, profile compilation, and export. Automated extraction and scenario generation require one. See [configuration](docs/configuration.md).

## Workflow

1. Create a subject and identify the speaker labels used in its sources.
2. Add authored human communication to **Communication** and decision/work evidence to **Thinking and working**. AI chats default to the latter. A source can be explicitly reused in both.
3. Review attribution and select excerpts. Assistant/tool turns, quoted material, unselected speakers and held-out sources are excluded.
4. Choose **Create profiles**. The completed build saves both files automatically. Read or download them directly; use profile details to inspect sources, edit individual instructions, or exclude a rule. Rebuilding preserves your edited rules and review decisions.
5. **Try it** with a situation you know. Both profiles are used by default; optional comparison runs isolate communication, working style, and the unprofiled baseline. Choose **Compare blind** to compare versions or variants behind randomized A/B labels. Optionally select a held-back reference for the reviewer; that material never enters response generation. Save both communication and working-style feedback before revealing the profiles. Instruction diffs show what changed between versions. Generated responses are never automatically imported as real-person evidence.

6. Choose **Use on agent…**, select an existing connection or connect with its management address and access token, and click **Apply profiles**. The agent keeps its own installed copy; drafts never update a running agent automatically. Previous installations can be restored. **Export…** offers an AGENTS.md workspace ZIP, a self-contained SOUL.md ZIP for OpenClaw, or the native `.twin.json` package. Individual profile files remain downloadable.

An **Existing profile** source contributes prior guidance, not a sample of the subject's prose. It is kept distinct from authored material. The extractor looks for concrete phrasing, cadence, structure and context in communication; decision criteria, priorities, tradeoffs and execution preferences in working style. It does not fill sparse evidence with generic advice. See the [profile content contract](docs/profile-content.md).

The [v1 package schema](schemas/twin-profile.schema.json) describes the portable boundary. A package contains `communication.md`, `working-style.md`, `claims.json`, `evaluation-summary.json`, and a versioned manifest with file digests and opaque source references. Markdown contains instructions only; the JSON files hold evidence, review and version information. New profiles declare `instruction_format: 2`; historical versions retain their original contents. Packages exclude raw artifacts and private identity bindings. Known identity strings are scrubbed; this is not a general anonymizer, and distinctive behavior can remain identifying. Source removal/reassignment, holdout changes and private-identity changes invalidate dependent revisions. Already downloaded copies cannot be recalled.

## CLI and automation

```bash
INSTANCE="$HOME/.local/share/persona-workshop/default"
APP="$INSTANCE/bin/persona-workshop"
"$APP" --home "$INSTANCE" status
"$APP" --home "$INSTANCE" api GET /api/v1/projects
"$APP" --home "$INSTANCE" api POST /api/v1/projects --body-file project.json
"$APP" --home "$INSTANCE" import --project PROJECT_UUID --file meeting.md --kind meeting --author 'Speaker Name'
"$APP" --home "$INSTANCE" export --project PROJECT_UUID --revision REVISION_UUID --output ./new-profile-folder
"$APP" --home "$INSTANCE" export --project PROJECT_UUID --revision REVISION_UUID --format agents --agents-file ./existing/AGENTS.md --output ./new-workspace-export
"$APP" --home "$INSTANCE" export --project PROJECT_UUID --revision REVISION_UUID --format soul --output ./new-openclaw-export
```

The CLI authenticates locally with a private installation control credential, never a user's password. Every UI operation uses the same API. See the [API and view-control contract](docs/ui-api-contract.md), including optimistic revisions, cancelable jobs, private identity access, and view commands for navigation, selection, drafts, focus and scroll. Programmatic uploads send text or base64 file contents through the API; they do not operate a native file picker.

Workspace exports keep provenance in `persona/metadata/`. The AGENTS.md loader references `persona/communication.md` and `persona/working-style.md`; extract the ZIP at the workspace root and keep those relative paths together. If the workspace already has an AGENTS.md, choose **Keep an existing AGENTS.md**: only the marked Persona Workshop section is added or replaced. All other content stays unchanged, and repeating the export does not duplicate the section. The existing file is used only to create your download; the workshop does not save it. CLI `--agents-file` also leaves the source file untouched. SOUL.md contains both instruction profiles directly; combine it with any existing SOUL.md before replacing that file. See [portable exports](docs/portable-exports.md) for host loading details and automation.

The UI supports `?embed=1`; configure allowed HTTPS frame origins explicitly. Same-origin embedding can call `window.personaWorkshop`; cross-origin hosts can use the authenticated semantic HTTP API. Authentication/session provisioning is the host's responsibility. A future fleet UI can integrate these contracts without depending on this service's internals.

## Boundaries and current limits

- One private service can hold several subjects. ACP is the replaceable extraction/simulation boundary, not a profile-installation standard.
- Text-only analysis supplies no ACP filesystem/terminal/MCP capabilities, denies approval callbacks, and aborts tool-using responses. An endpoint can still have its own tools: deploy it with independent execution restrictions. Do not point untrusted evidence at an unrestricted agent.
- Build results use source revisions; stale work cannot overwrite newer edits. Evaluation jobs use immutable profile revisions and can queue in parallel. Two analyses run at once. Jobs interrupted by restart are visibly marked; they do not silently resume.
- First-version attribution is explicit, not an automated authorship guarantee. Pasted AI output without labels needs manual exclusion. Exact duplicate artifacts are rejected; near-duplicate detection is not implemented.
- The optional management connector targets services supporting the agent-instance versioned profile-package API. It requires a separate management credential; a generic ACP-only endpoint can still consume exported files, but does not automatically gain management operations. Apply/restore updates both profile files together and leaves private identity, tools, permissions and experience adapters alone.
- Recording import requires a configured transcription/diarization provider; this release does not install a speech model or supply provider credentials. PDF extraction needs `pdftotext`; scanned PDFs need OCR first. Python 3 handles EML/HTML/DOCX/ODT locally. Speaker labels from recordings are always unassigned until explicit identification. Automatic source connectors and fleet provisioning remain separate.
- Blind comparisons collect authenticated reviewer feedback, not a measured fidelity score. Random labels conceal the assigned profile, but responses can contain clues. A single review does not establish calibration; run multiple fresh situations and collect feedback from the actual subject. Feedback stays outside runtime instruction files and never silently edits an agent.
- Evidence stays in the private installation, outside this repository. Selected evidence is sent to the operator-configured model endpoint for analysis. Provider account/retention policies apply.

## Development

```bash
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo build --release --locked
```

Tests use synthetic sources and a deterministic ACP fixture; no model account is needed. Browser checks live in `ui/tests/` with their own dependency setup. Real-model trials complement these protocol tests but do not establish human fidelity.

Management clients can obtain a named installation credential through `POST /api/v1/tokens` using an administrator credential. Supply `scopes` (for example `["observe"]`) and `expires_in_seconds`. Values are returned once; `GET /api/v1/tokens` lists metadata and `DELETE /api/v1/tokens/{id}` revokes access. Observation access does not allow edits, builds, exports or private identity reads.

## Contributing

> [!NOTE]
> This project is not currently accepting external contributions, but we're actively working toward opening this up. We value community input and look forward to collaborating in the future. For now, feel free to fork and experiment!

Most contributions require you to agree to a
Contributor License Agreement (CLA) declaring that you have the right to, and actually do, grant us
the rights to use your contribution. For details, visit [Contributor License Agreements](https://cla.opensource.microsoft.com).

When you submit a pull request, a CLA bot will automatically determine whether you need to provide
a CLA and decorate the PR appropriately (e.g., status check, comment). Simply follow the instructions
provided by the bot. You will only need to do this once across all repos using our CLA.

This project has adopted the [Microsoft Open Source Code of Conduct](https://opensource.microsoft.com/codeofconduct/).
For more information see the [Code of Conduct FAQ](https://opensource.microsoft.com/codeofconduct/faq/) or
contact [opencode@microsoft.com](mailto:opencode@microsoft.com) with any additional questions or comments.

## Trademarks

This project may contain trademarks or logos for projects, products, or services. Authorized use of Microsoft
trademarks or logos is subject to and must follow
[Microsoft's Trademark & Brand Guidelines](https://www.microsoft.com/legal/intellectualproperty/trademarks/usage/general).
Use of Microsoft trademarks or logos in modified versions of this project must not cause confusion or imply Microsoft sponsorship.
Any use of third-party trademarks or logos are subject to those third-party's policies.
