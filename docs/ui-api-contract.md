# Service and reference-client API contract

For clients operating Persona Workshop's evidence, profiles, revisions and jobs.
These are the concrete service and reference-client interfaces. The existing view
fields do not prescribe the layout of future products. The [contract map](contracts.md)
locates profile-content, package and shared family commitments without copying
them into this API definition.

All paths are relative to the app root. The service is HTTPS/PAM, single owner.
`GET auth/session` returns `{authenticated, owner, csrf}`; `POST auth/login`
accepts `{username,password}`; `POST auth/logout` clears the cookie. Include
`X-CSRF-Token` on mutations. API failures are `{error: "human readable message"}`.
UI is embedded with `?embed=1`; keep asset and API URLs relative.

## Projects and evidence

- `GET api/v1/projects` → `{items:[{id,alias,purpose,revision,updated_at,stats}]}`.
- `POST api/v1/projects` `{alias,purpose,aliases:["Alex","Alex Example"],subject_type:"person"}` → full project.
- `GET api/v1/projects/{id}` → project: `{id,alias,purpose,aliases,subject_type,revision,artifacts,observations,builds,revisions,evaluations,identity_present,stats}`.
- `PUT api/v1/projects/{id}` `{expected_revision,alias?,purpose?,aliases?}` → full project.
- `POST api/v1/projects/{id}/artifacts` `{expected_revision,name,kind,text,collections?,authors?,holdout:false}` → full project.
  Kinds: `meeting`, `chat`, `ai_chat`, `notes`, `profile`. AI chat defaults to `working`, other kinds to `communication`. Existing `profile` sources use role `reference` and the automatically selected speaker `Reference profile`; they contribute supplied guidance, not authored prose samples.
  Upload text/Markdown or normalized JSON chat content as text. Max 2 MB per source.
  Each artifact summary: `{id,name,kind,sha256,collections,authors,speakers,span_count,selected_count,holdout,deleted}`.
- `GET api/v1/projects/{id}/artifacts/{aid}` → full artifact including `spans:[{id,speaker,role,text,line_start,line_end,quoted}]`.
- `PUT api/v1/projects/{id}/artifacts/{aid}` `{expected_revision,authors?,collections?,selected_span_ids?:null|[ids],holdout?}` → project. Exact speaker labels select authorship. Null selects all attributed, non-assistant/non-tool, nonquoted spans.
- `DELETE api/v1/projects/{id}/artifacts/{aid}` `{expected_revision}` → project. Derived revisions become invalid; never export stale evidence silently.

## Candidate and review

- `POST api/v1/projects/{id}/builds` `{expected_revision}` → job `{id,project_id,status,stage,note,created_at}` immediately. Uses configured ACP extractor; imported content is data, not authority.
  A completed build atomically saves observations and a compiled profile revision. `job.result_id` is that revision's ID. The UI can open it directly; a separate compile step is not required. Owner rules, reviewed observations and manually edited pending observations survive rebuilds.
- `GET api/v1/jobs/{id}` → job; status `queued`, `running`, `complete`, `failed`, `cancelled`, `stale`, or `interrupted`.
- `POST api/v1/jobs/{id}/cancel` `{}` → job.
- An observation: `{id,collection,section,statement,scope,confidence:{attribution,support,generalization,reason},evidence_ids:[spanIds],conflicts:[spanIds],status:"pending"|"accepted"|"rejected",source:"inferred"|"owner",valid:true}`. `statement` contains up to 3000 characters of runtime guidance; scope/confidence are metadata.
- Sections: communication `voice`, `phrasing`, `structure`, `tone`, `context`, `editing`; working `framing`, `decisions`, `execution`, `collaboration`, `feedback`, `tradeoffs`. Missing/empty section is general guidance for legacy compatibility.
- `PUT api/v1/projects/{id}/observations/{oid}` `{expected_revision,status?,section?,statement?,scope?,confidence?}` → project.
- `POST api/v1/projects/{id}/observations` `{expected_revision,collection,statement,section?,scope?}` → project. Clearly marked owner-specified, not model evidence. Omitted scope defaults to `Selected contexts`.
- `GET api/v1/projects/{id}/evidence/{span_id}` → `{artifact:{id,name,kind},span}`.
- `POST api/v1/projects/{id}/revisions` `{expected_revision}` → revision `{id,project_id,created_at,status,valid,manifest,files}`. Saves a new version after optional edits. Includes pending guidance and excludes rejected/invalid rules. Markdown contains only grouped instructions; scope/confidence/review details remain in JSON. Files is an object with Markdown/JSON contents as strings.
- `GET api/v1/projects/{id}/revisions/{rid}` → revision.
- `GET api/v1/projects/{id}/revisions/{rid}/export` → downloadable `.twin.json` portable package. Also offer individual `communication.md` and `working-style.md` from returned files; CLI can unpack the same package. Default package excludes identity and raw evidence.
- `GET api/v1/projects/{id}/revisions/{rid}/exports/{format}` → `persona-workshop.export/1` JSON package, where format is `agents` or `soul`. Append `/download` for a ZIP. `POST` to either `agents` route with `{existing_agents:"..."}` produces a merged package without saving the supplied text. All routes require `export`; cookie POSTs require CSRF. See [portable exports](portable-exports.md).
- `GET/PUT api/v1/projects/{id}/identity` → `{bindings:{...}}`, PUT adds `expected_revision`. Keep behind explicit Private identity section; don't place fields into view state.

## Evaluation

- `POST api/v1/projects/{id}/evaluations` `{revision_id,scenario,variant:"both"|"communication"|"working"|"baseline"}` → job; response is simulated and visibly labeled, never real person approval.
- Project `evaluations` contains `{id,revision_id,variant,scenario,response,created_at,rating:null|object}`.
- `PUT api/v1/projects/{id}/evaluations/{eid}` `{expected_revision,rating:{communication_fit?,working_fit?,notes}}` → project. Labels 1–5; no overall fidelity percentage.

## Apply to an existing agent

Persona Workshop connects to an agent's **management API**, separately from ACP.
These routes require workshop `admin` access (the signed-in owner has this).
The remote token needs `observe` and `profiles` scopes, or admin, on an
agent-instance service that supports versioned profile packages.

- `GET api/v1/agent-targets` → `{items:[{id,url,name,agent_id,created_at}]}`. No credentials are returned.
- `POST api/v1/agent-targets` `{url,token,ca_pem?:""}` → `{target,installation}`. Verifies HTTPS, remote instance identity and package-management support before saving the connection. Connecting never applies a profile. Reconnecting the same instance replaces its connection credential and keeps its target ID. `url` is an HTTPS origin without a path, query, userinfo or fragment. Optional `ca_pem` adds the agent's PEM root certificate; hostname verification remains enabled. Redirects are rejected and credentials are never forwarded.
- `GET api/v1/agent-targets/{id}` → `{target,installation}`. Reads live status and checks that the URL still identifies the same agent.
- `DELETE api/v1/agent-targets/{id}` → `{ok:true}`. Removes this connection and its local credential. It does not alter the remote agent or its installed profiles.
- `POST api/v1/projects/{id}/revisions/{rid}/apply` `{target_id,expected_revision}` → `{target,installation}`. Sends the saved, currently valid portable package to the agent. Here `expected_revision` is the **remote installation generation**, obtained from `installation.revision`, not the project or profile revision. The remote agent atomically installs the pair and owns its history. No credentials, identity bindings or raw evidence enter the package.
- `POST api/v1/agent-targets/{id}/restore` `{expected_revision,id:<snapshot UUID>}` → `{target,installation}`. Restores a snapshot from the agent's own history. `expected_revision` again refers to the remote installation generation. This does not rebuild a workshop profile or change source material.

`installation` is `{revision:<number>,active:null|snapshot,previous:[snapshot...]}`;
each snapshot includes `id`, `source` (package schema, subject key and profile
revision), file hashes, creation time and kind. A baseline/manual snapshot may
have `source:null`. The agent can use a different generation after manual edits,
another workshop or another management client. A stale generation returns 409;
refresh, review, and explicitly retry instead of silently overwriting it.

Remote credentials and optional CA certificates live in private server files,
never project records, exports, activity, shared display state or browser storage.
Remote errors are translated locally rather than reflecting remote response text.
The ordinary CLI `api` command exposes every route with `--body-file` for private
connect/apply inputs. The agent continues working if the workshop is closed.

## Programmatic view control

- `POST api/v1/views` `{}` → `{id,revision,screen:"projects",project_id:null,artifact_id:null,observation_id:null,profile_revision_id:null,tab:"communication",filter:"all",modal:null,draft:{},focus:null,scroll:null,controller:"human",rendered_revision:0}`.
- `GET api/v1/views/{id}` → same; browser polls at 1 s while visible.
- `POST api/v1/views/{id}/commands` `{expected_revision,controller,commands:[{op:"set",field:"screen",value:"review"},...]}` → same. Allowed fields above (except id/revision/rendered_revision), screens `projects`, `evidence`, `review`, `profiles`, `evaluate`, `settings`; atomic revision conflict 409. Draft excludes passwords/raw artifacts/private identity.
- `POST api/v1/views/{id}/ack` `{revision}` → `{ok:true}` after render. Human interactions use same commands with controller `human`, taking control at current revision.
- `GET api/v1/descriptor`, `GET api/v1/status`, `GET api/v1/activity` provide discovery, configured extractor readiness, and meaningful job/source/profile events. View changes/acks are excluded from activity.

The primary path is Sources → Profiles → Try it. Rule review, provenance,
version history, comparison variants and service details are optional.
All disclosures and selections use typed shared view state, including draft
booleans `source_details`, `source_options`, `profile_details`, `compare_profiles`,
`review_details`, `observation_details`, `evaluation_details`, `activity_details`,
`profile_pinned` (explicit history selection), and the string `observation_section`.

Agent workflow modals are `agent.apply` and `agent.connect`. Shared drafts include
`agent_target_id`, `agent_restore_id` (empty or UUID), and booleans
`agent_history`, `agent_advanced`, `agent_connection`. Connection-entry fields
`agent_url`, `agent_token`, and `agent_ca` remain local, private inputs; they cannot
be placed in shared drafts. Automation connects through the management API and
then selects the saved target in the display, or calls
`window.personaWorkshop.act("agent.connect", {url,token,ca_pem})` from a same-origin
host. The public UI actions are `agent.open`, `agent.new`, `agent.connect`,
`agent.refresh`, `agent.apply`, `agent.restore`, and `agent.forget`. Use `actions()`
for discovery. `window.twinWorkbench` remains an alias for compatibility.

The export dialog uses modal `profile.export`, public draft `export_format`
(`native`, `agents`, `soul`) and boolean `export_merge`. Browser actions are
`export.open`, `export.existing` with `{text}`, and `export.download`. Existing
AGENTS.md text is a transient private input, excluded from views and snapshots;
the HTTP merge route lets an agent perform the same action directly.

## Blind comparisons and revision changes

- `POST api/v1/projects/{id}/comparisons` requires `build`, with
  `{expected_revision,scenario,holdout_ids?:[],candidates:[{revision_id,variant},...]}`.
  Choose 2–4 distinct version/variant pairs from this project. The response has
  `id`, `job_id`, `status`, and randomized response labels A–D. Poll the normal
  job API or comparison route. Generation uses fresh ACP sessions through the
  same configured endpoint, without candidate cross-talk or reference answers.
- `GET api/v1/projects/{id}/comparisons` and `GET .../comparisons/{cid}` require
  `observe`. Candidate objects contain `label` and `response`; `revision_id` and
  `variant` remain absent until reveal. Job/activity/project snapshots do not
  disclose this mapping. Review references contain only selected attributed
  passages from held-back sources. Results become unavailable when profile or
  reference evidence is invalidated/deleted.
- `PUT .../comparisons/{cid}/rating` requires `edit`:
  `{ratings:{A:{communication_fit:1..5,working_fit:1..5,notes?:""},B:{...}},preference:"A"|"B"|"C"|"D"|"tie"|"neither",perspective:"self"|"peer"|"reviewer"}`.
  Every response must be rated. The service records the actual authenticated
  actor, time and blind status. Perspective is the reviewer's declaration, not
  proof of identity. No aggregate fidelity score is produced.
- `POST .../comparisons/{cid}/reveal` requires `edit` and saved feedback.
  Revealed ratings are locked; start a new comparison for a new blind judgment.
  Concurrent feedback and reveal cannot turn an informed rating back into a blind one.
- `GET api/v1/projects/{id}/revision-diff/{from}/{to}` requires `observe`.
  Returns changed flags, added/removed paragraphs and full before/after text for
  the two runtime Markdown files. Paragraph lists ignore ordering and duplicates;
  full text preserves those differences. It does not change either revision.

The service remains owner-authenticated. Have the subject review the blind
responses with the operator or through separately authorized automation; do not
share the installation control token. No invitation is sent automatically.
Feedback is stored separately from source artifacts and runtime instructions.

View state adds `compare_blind`, `comparison_revision_a`,
`comparison_revision_b`, `comparison_variant_a`, `comparison_variant_b`,
`comparison_holdout`, `comparison_perspective`, `comparison_references`,
`revision_diff`, and `diff_full` drafts. `comparison_rating_{comparisonUUID}`
holds response-specific fields such as `A_communication_fit`, `A_working_fit`,
`A_notes`, and `preference`. Semantic actions are `comparison.open`,
`comparison.close`, `comparison.run`, `comparison.rate`/`comparison.reveal`
(with `{id}`), and `revision.diff`. All have the same browser and API behavior.

## Rich file import

- `GET api/v1/import-capabilities` requires `observe` and returns supported
  formats, file-size limits, and whether transcription is configured.
- `POST api/v1/projects/{id}/file-import` requires `edit`, with
  `{expected_revision,filename,name?,content_base64,kind?,collections?,authors?,holdout?}`.
  The filename has no folder components. It accepts UTF-8 text/Markdown/JSON,
  EML/HTML/DOCX/ODT/PDF, diarized transcript JSON, and configured recording types.
  File bytes and provider credentials never go into shared display state.
  Returns the normal project snapshot; open the new artifact to review the text
  and speaker mapping before using it. Import is atomic on the project revision.
- The CLI's `import --file PATH` performs the same operation. Browser automation
  can call `act("source.create", {file:{name,content_base64}})` after preparing the
  ordinary public source draft. Raw file bytes stay in the private submission.

New artifact kinds are `email`, `document`, and `recording`. Recording imports
ignore proposed author defaults until an explicit artifact speaker selection is
saved, even when the model uses a familiar person's name as a diarization label.
See configuration for the replaceable transcription command contract.

## Native local account and startup

`GET /auth/session` also reports `auth_mode` and `first_account_needed`.
For a fresh native installation only, `POST /auth/first-account` accepts
`{setup_token, password, start_at_signin}` and returns the normal signed-in cookie
and CSRF response. Its private, expiring, installation-bound capability comes from
opening the local app; it is not an account-reset API. Wrong/expired capabilities
are refused, and an existing credential is never replaced. Startup failure leaves
the newly created account usable and records the failure.

Authenticated admin `GET /api/v1/startup` reads the last current-user startup
result; `POST /api/v1/startup` accepts `{enabled}`. Existing PAM deployments remain
owner-managed. The local-only `GET /api/v1/local-proof?nonce=…` is a bounded
installation proof for the native launcher/CLI, not authentication or access.
HTTPS deployments do not expose it. Shared cryptographic primitives live in the
[umbrella native-platform crate](https://github.com/microsoft/amplifier-constellation/tree/main/crates/native-platform);
Persona Workshop owns the HTTP, session, cookie and permission behavior.
