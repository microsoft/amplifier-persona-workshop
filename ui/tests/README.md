# Persona Workshop browser checks

The web client uses local HTML/CSS/JavaScript, relative service URLs, and no external fonts or asset servers. It works standalone or beneath an embedding prefix with `?embed=1`.

From this directory:

```sh
npm ci --ignore-scripts
npm test
```

The fixture test runs Chromium against an in-memory synthetic service. It exercises sign-in cookie failure, project creation, text-safe file upload, exact speaker mapping, source reuse, held-back evidence, automatic source-to-profile creation and cancellation, evidence review, owner-specified rules, versioned exports, private identity separation, scenario variants and ratings, programmatic display control and human takeover, reattachment, revision conflicts, invalidated exports, unavailable history selection, externally started jobs, and embedded mobile/200% zoom-equivalent layouts. Fixture transcripts and identities are fictional. It does not claim to test an actual PAM installation or model quality.

To exercise the actual Rust HTTPS service, parser, state store, and ACP process:

```sh
cargo build --locked --manifest-path ../../Cargo.toml
node real-service.mjs
```

This second check creates a private temporary installation and a deterministic tool-free ACP extractor, authenticates the browser using that installation's generated management credential, and removes the installation afterward. It tests automatic source-to-profile creation, existing-profile reference attribution, and four separately queued evaluation variants. Before opening the browser, OpenSSL verifies the generated certificate chain and IP identity, and the actual CLI connects with CA verification enabled. The temporary browser context ignores certificate errors because the disposable CA is not installed into the machine/browser trust store; the earlier checks validate trust independently. The test does not read any person's artifacts, connect a real model, or change machine authentication.

`TWIN_BINARY` can select another built service. `PLAYWRIGHT_MODULE` and `CHROMIUM_EXECUTABLE` can select existing browser tooling. Screenshots go to `/tmp/twin-workbench-ui` by default; override with `UI_ARTIFACTS`. Screenshots from these tests contain only synthetic data.

## Default workflow

Sources → Profiles → Try it are the three primary destinations. A completed build saves a profile version automatically and opens it. Individual instruction review, linked provenance, history, and package export remain available under Details; comparison variants and rating controls are optional. Ordinary use does not require a review ceremony or a choice of evaluation variants. Existing profiles can be imported as reference guidance, separately from authored writing samples.

Version selection pins the chosen history entry. If source changes invalidate it, its protected files are not fetched; the UI explains that it is unavailable and offers the latest version. Title-only files get a short empty-state explanation in the UI without changing the downloadable Markdown.

**Export…** on Profiles opens the workspace export dialog. Real-service checks
download AGENTS.md and SOUL.md ZIPs, verify the relative-file and self-contained
loading layouts, merge the same AGENTS.md twice without duplicating its section,
and compare the result with a CLI export. Existing document text remains outside
shared display state and the source file is unchanged.

## Semantic browser controls

Every interface action has a stable `data-action` identifier. The in-page interface exposes:

- `window.twinWorkbench.actions()` — available semantic action names.
- `window.twinWorkbench.snapshot()` — display state, busy/error state, project identifiers and job progress; excludes private input buffers.
- `window.twinWorkbench.act(name, payload)` — invokes the same action as the UI. Inspect `snapshot().error` afterward for an actionable failure.
- `window.twinWorkbench.setDraft(key, value)` — changes an allowed public form draft and synchronizes it to the service.
- `window.twinWorkbench.control(values)` — updates semantic display fields using the revisioned view API.
- `window.twinWorkbench.importSource({name,kind,text,collections,holdout})` — explicit private-source submission; raw text never enters the view draft.

Sign-in and private identity values can be passed directly to `act('auth.login', {username,password})` and `act('identity.save', {json})`; do not put these into shared draft fields. The service's regular authenticated APIs remain the authority for all project changes. View control provides navigation, selected items, filters, public drafts, dialogs, semantic focus targets, and scroll coordinates. Private identity is not fetched merely because a remote view command opens its dialog.

A separate controller can discover a view through the service's view API and set its state with `POST api/v1/views/{id}/commands`. Human interactions use the same commands and mark the controller as `human`; rendering a remotely requested focus or scroll does not itself take over. The browser acknowledges the rendered revision. Display state is restored within the same browser tab and mounting path; `?view=ID` can explicitly attach to an existing display.

Example command body:

```json
{
  "expected_revision": 8,
  "controller": "voice-assistant",
  "commands": [
    {"op":"set","field":"screen","value":"evaluate"},
    {"op":"set","field":"focus","value":"draft.scenario"},
    {"op":"set","field":"scroll","value":{"x":0,"y":0}}
  ]
}
```

`controller` is part of the command envelope, not a `set` operation. Use the current project and display revisions from the APIs instead of guessing them.

Optional display sections are controlled through the same view draft: `source_details`, `source_options`, `profile_details`, `compare_profiles`, `review_details`, `observation_details`, and `evaluation_details`. The `profile_pinned` boolean distinguishes explicit history selection from the default newest-valid profile. `act("details.toggle", {key})` toggles a section; `act("profile.latest")` returns to the latest valid revision. Section placement uses the optional `observation_section` draft; section/scope are not required for an instruction edit.


Version 0.4 adds a real-service blind-comparison run with randomized response
labels, actual actor feedback, reveal locking, a narrow comparison layout, and
live HTML conversion/held-back import. The responses and feedback in this test
are deliberately synthetic; they do not rate any real person.
