# Configuration

`config.json` belongs to the private installation. `persona-workshop --home PATH config get` reads it, and `config set --file CONFIG.json` validates a replacement. Restart the service to apply changes. Installation ID and owner cannot be changed by that command. Configuration may contain provider environment values; keep it private.

The extractor file passed to the installer has this shape:

```json
{
  "command": "/absolute/path/to/acp-analysis-agent",
  "args": [],
  "cwd": "/absolute/path/to/empty-analysis-workspace",
  "env": {},
  "timeout_seconds": 600
}
```

The command speaks ACP v1 over stdin/stdout. It must support initialize, session/new and session/prompt. Configure its account separately; never put credentials in source artifacts, profile packages or browser view state. The service gives it the selected attributed text, not source paths, speaker identity bindings or the rest of the project. Restrict native tools at the endpoint too: empty ACP client capabilities only constrain client-provided tools.

For the existing HTTPS configuration, `bind` and `port` set the listening address. Port 0 selects a free port; `runtime.json` reports the actual port after startup. A fixed port is preferable behind a reverse proxy. `public_url` is the trusted HTTPS origin. `embed_origins` is an explicit list of allowed HTTPS frame origins; empty means same-origin only. The UI uses relative routes, secure cookies and same-origin CSRF checks.

## Existing agent connections

From **Profiles → Use on agent…**, connect with the existing agent service's
HTTPS management origin and a dedicated token with `observe` and `profiles`
access. This is separate from its ACP connection or Slack adapter. Tailscale
Serve addresses with a trusted certificate need no extra certificate input.
For an installation using a local certificate, expand **Local certificate** and
paste its downloaded PEM root certificate. Verification of that agent's hostname
is always required; the workshop does not offer an insecure-certificate bypass.

Saved target metadata lives in the installation database. Its credential and
optional root certificate live in `secrets/agent-target-UUID.json`, mode 0600.
No restart is required. Reconnecting the same instance updates its access token;
forgetting a connection removes local access while leaving the agent running.
Profile copies and restore history belong to the agent. Future workshop builds
do not change those copies until an operator explicitly applies a saved version.
The executable and service are named `persona-workshop`. New installations use
`~/.local/share/persona-workshop/default`. The installer recognizes the previous
default installation, reuses its existing folder, configuration and projects,
and replaces its running service. Pass `--home` to select an existing custom
installation. Cookie and portable package formats remain compatible.

Normalized chat import accepts an array or `{"messages":[...]}` with records such as:

```json
{"messages":[
  {"role":"user","author":"Subject label","text":"An original authored message."},
  {"role":"assistant","author":"Assistant","text":"Excluded generated response."},
  {"role":"user","author":"Subject label","text":"Copied third-party text.","quoted":true}
]}
```

Role comparisons are case-insensitive. Speaker labels are exact matches. Meeting text accepts `[0:01:20] Speaker Name: ...`; supported chat exports also accept `[12:30] **Speaker Name**` followed by blockquoted message text. In that export format a second `>` remains a quotation. Preview all imported spans and correct selection before building. Plain notes require an explicitly selected author label.

A source can be marked held out. It remains inspectable but cannot support extraction or a compiled claim. Blind comparisons can show explicitly attributed held-out passages to the reviewer without sending them to either generation. Select a fresh scenario yourself; the service does not automatically turn a meeting into a benchmark. Sources used by a selected profile cannot be reused as held-out references.

## Optional isolated Codex analysis endpoint

`scripts/codex-analysis.sh` is a Linux deployment example for upstream `codex-acp`, not required by the workbench. Its private folder contains `runtime/node`, `runtime/acp`, `runtime/codex`, an independently authenticated `codex-home`, and an empty `workspace`. Use an absolute path to the script as `command`, the private folder as its only argument, and that folder's workspace as `cwd`.

The wrapper uses Bubblewrap to expose only system libraries, certificate/DNS files, that runtime, the dedicated model account/history folder, and the analysis workspace. It does not mount the operator's source tree, home directory, evidence store, Slack credentials or application sockets. The outer process retains network access to reach the model provider; upstream Codex starts in read-only mode. Configure native tools and extensions off in its dedicated config as well. Callback denial or cancellation alone is not an execution isolation boundary.

The initial verified deployment used Node 22.23.2, Codex CLI 0.165.0 and upstream `@agentclientprotocol/codex-acp` 2.5.0. Authentication and billing belong to the configured model account; a separate installation folder does not imply separate billing or provider identity. Provider sessions may retain the selected evidence supplied in prompts; clearing a workbench artifact does not delete provider records or previously downloaded packages.

## Documents and recordings

File import accepts up to 20 MB, with up to 2 MB of normalized source text. The
CLI `import --file` and browser file chooser use the same `file-import` API.
Python 3 is needed for EML, HTML, DOCX and ODT; PDF extraction also needs
`pdftotext` (Ubuntu package `poppler-utils`). Extraction runs locally, with bounded
output and a timeout. It does not follow links or run document macros. HTML
scripts, styles and blockquotes are excluded. EML prefers the plain MIME body,
uses its sender label, and excludes quoted reply history. The preview remains
important for unusual exports or ambiguous attribution.

Recordings can be WAV, MP3, M4A, OGG, WebM or MP4. Configure an optional
`transcription` object in private `config.json`, with the same
`command`, `args`, `cwd`, `env`, `timeout_seconds` fields as `extractor`.
Unlike the ACP extractor, this command reads one JSON object to EOF on stdin and
writes one JSON object to stdout. Arguments are passed directly, without a shell.
Keep provider credentials in the private environment or the provider's own
credential store, not in source files or shared UI state.

Input:

```json
{"schema":"persona-transcription/1","media_path":"/private/temporary/media.wav","filename":"meeting.wav","diarization":true}
```

Output:

```json
{"segments":[
  {"speaker":"speaker_0","start":0.0,"end":3.2,"text":"An attributed contribution."},
  {"speaker":"speaker_1","start":3.5,"end":5.0,"text":"Another contribution."}
]}
```

The command may adapt a locally installed speech model or a remote provider.
It must return bounded text and speaker labels. No specific provider account is
bundled, and no model-quality claim follows from the contract tests. The command
receives the recording and decides whether data stays local or is sent to a
provider. The temporary media copy is removed after success or failure; the
workbench retains the normalized transcript. A forced host shutdown may leave a
private `.recording-*` directory for the operator to remove.

The same output JSON can be uploaded directly without a transcription provider.
**Diarization is not identification:** every recording starts with no selected
subject. In the preview, choose the speaker label that belongs to the subject,
then choose Communication, Working style, or both. These are independent choices;
reusing a recording does not merge the two output profiles.

### OpenAI recording adapter

`scripts/transcribe-openai.py` implements this contract using the audio
transcriptions API. Install a private copy with the service and add:

```json
"transcription": {
  "command": "/usr/bin/python3",
  "args": ["/private/persona/scripts/transcribe-openai.py", "--model",
    "gpt-4o-transcribe-diarize", "--key-file", "/private/persona/secrets/transcription-key"],
  "cwd": "/private/persona",
  "env": {},
  "timeout_seconds": 300
}
```

The selected provider receives the uploaded recording. This adapter uses verified
HTTPS, refuses redirects, sends a neutral filename, accepts at most 20 MB, and
requires real speaker labels and timestamps in `diarized_json`. Missing labels
are errors rather than invented identities. Keep the key owner-readable only.
The adapter/model is explicit and replaceable: the current compatibility model
`gpt-4o-transcribe-diarize` is scheduled for removal on February 26, 2027. A normal
transcription model is not an equivalent replacement unless it returns labeled
segments. See the [provider speech-to-text guide](https://developers.openai.com/api/docs/guides/speech-to-text).

A live smoke check used two synthetic macOS voices and returned five segments
with separate A/B labels. It verifies transport and this synthetic sample, not
accuracy on real meetings. Run `python3 -m unittest discover -s tests -p
'test_*.py'` for the local wire-format and document-import tests.

## Native app setup

The native package opens the existing workbench and creates an installation-local
password. It does not choose an analysis model or configure transcription. The
normal first-run link carries a short-lived setup capability in its URL fragment;
the browser removes it immediately. Creating an account is create-only. An
interrupted response is inspected through the existing session endpoint rather
than replacing credentials. Reopen the local app if an unused setup link expires.

New native configurations use `auth_mode: "local-password"` and
`web_transport: "local-http"`. HTTP binds exactly `127.0.0.1`, pins its configured
origin, rejects cross-origin requests and permits no external embedding. CLI
requests prove the saved installation's identity on the same TCP connection
before sending a management credential. Existing configurations without these
fields retain `pam`/`https`; native startup never converts an existing account.

`document_python` identifies the installation's private interpreter. Native
packages select `pdf_backend: "pypdf"` with pinned pypdf 7.0.1. This extracts
readable text, preserves importer-supplied attribution, and rejects encrypted,
scanned/empty and malformed PDFs. It does not provide OCR or guarantee the same
layout as Poppler; inspect imported text before using it as evidence. Extraction
has bounded page/stream/text work and runs in the existing timed converter process.
Legacy configurations keep `pdf_backend: "pdftotext"` and the existing system
Python/Poppler path. EML, HTML, DOCX and ODT use the same converter logic in both.

Bundled interpreter archives and distributor notices are pinned by the umbrella's
[private Python packaging support](https://github.com/microsoft/amplifier-constellation/tree/main/packaging/python).
The package manifest binds their files and the PDF wheel. Neither modifies the
system interpreter. Recordings still require an explicitly configured transcription
provider; missing capabilities are reported rather than downloading a model.
