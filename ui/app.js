/* Credentials and raw imports are local; semantic display state is shared. */
"use strict";
(() => {
  const app = document.querySelector("#app"),
    announcer = document.querySelector("#announcer"),
    query = new URLSearchParams(location.search);
  const setupStorageKey = "persona-workshop-first-account";
  const setupFragment = new URLSearchParams(location.hash.slice(1));
  if (setupFragment.has("setup")) {
    const capability = setupFragment.get("setup");
    if (/^[a-f0-9]{64}$/.test(capability || "")) sessionStorage.setItem(setupStorageKey, capability);
    history.replaceState(null, "", location.pathname + location.search);
  }
  const storageKey = `persona-workshop-view:${new URL(".", location.href).pathname}`;
  // Keep the selected view when upgrading an existing browser installation.
  const previousStorageKey = `twin-workbench-view:${new URL(".", location.href).pathname}`;
  if (!sessionStorage.getItem(storageKey) && sessionStorage.getItem(previousStorageKey)) {
    sessionStorage.setItem(storageKey, sessionStorage.getItem(previousStorageKey));
  }
  sessionStorage.removeItem(previousStorageKey);
  document.body.classList.toggle("embed", query.get("embed") === "1");
  const S = {
    auth: null,
    view: null,
    projects: [],
    project: null,
    artifact: null,
    evidence: null,
    comparisons: [],
    revisionDiff: null,
    importCapabilities: null,
    profile: null,
    agentTargets: [],
    agentPreview: null,
    agentTargetsLoaded: false,
    status: {},
    activity: [],
    jobs: new Map(),
    error: "",
    notice: "",
    busy: false,
    opening: false,
    private: {},
    queued: Promise.resolve(),
    syncTimer: null,
    pollBusy: false,
    remoteFocus: null,
  };
  const screens = {
      projects: "Projects",
      evidence: "Sources",
      review: "Profile details",
      profiles: "Profiles",
      evaluate: "Try it",
      settings: "Settings",
    },
    icons = {
      projects: "▦",
      evidence: "▤",
      review: "✓",
      profiles: "◫",
      evaluate: "◇",
      settings: "⚙",
    },
    kinds = {
      meeting: "Meeting transcript",
      chat: "Messages & conversations",
      ai_chat: "AI conversation",
      notes: "Notes & writing",
      profile: "Existing profile",
      email: "Email",
      document: "Document",
      recording: "Diarized recording",
    },
    variants = {
      both: "Both profiles",
      communication: "Communication only",
      working: "Working style only",
      baseline: "Baseline · no profile",
    },
    terminal = new Set([
      "complete",
      "failed",
      "cancelled",
      "stale",
      "interrupted",
    ]);
  const sections = {
    communication: {
      voice: "Voice",
      phrasing: "Phrasing",
      structure: "Structure",
      tone: "Tone",
      context: "Context",
      editing: "Editing",
    },
    working: {
      framing: "Framing",
      decisions: "Decisions",
      execution: "Execution",
      collaboration: "Collaboration",
      feedback: "Feedback",
      tradeoffs: "Tradeoffs",
    },
  };
  const d = () => S.view?.draft || {},
    list = (v) => (Array.isArray(v) ? v : []),
    esc = (v) =>
      String(v ?? "").replace(
        /[&<>"']/g,
        (c) =>
          ({
            "&": "&amp;",
            "<": "&lt;",
            ">": "&gt;",
            '"': "&quot;",
            "'": "&#39;",
          })[c],
      ),
    label = (v) =>
      String(v ?? "")
        .replace(/[_\.]/g, " ")
        .replace(/^./, (c) => c.toUpperCase());
  const stamp = (v) => {
    if (!v) return "";
    const t = new Date(typeof v === "number" && v < 1e12 ? v * 1000 : v);
    return Number.isNaN(+t)
      ? ""
      : new Intl.DateTimeFormat(undefined, {
          month: "short",
          day: "numeric",
          hour: "numeric",
          minute: "2-digit",
        }).format(t);
  };
  const badge = (text, style = "") =>
      `<span class="badge ${style}">${esc(text)}</span>`,
    btn = (action, text, cls = "", data = "", disabled = false) =>
      `<button type="button" class="button ${cls}" data-action="${action}" ${data} ${disabled || S.busy ? "disabled" : ""}>${text}</button>`,
    link = (action, text, data = "") =>
      `<button type="button" class="text-button" data-action="${action}" ${data} ${S.busy ? "disabled" : ""}>${text}</button>`;
  function field(name, title, help = "", o = {}) {
    const value = o.value ?? d()[name] ?? "",
      control = o.private ? `data-private="${name}"` : `data-draft="${name}"`;
    return `<label class="field ${o.full ? "full" : ""}" for="field-${name}"><span>${esc(title)}</span>${o.area ? `<textarea class="input" id="field-${name}" ${control} rows="${o.rows || 3}" placeholder="${esc(o.placeholder || "")}">${esc(value)}</textarea>` : `<input class="input" id="field-${name}" ${control} type="${o.type || "text"}" value="${esc(value)}" placeholder="${esc(o.placeholder || "")}" ${o.required ? "required" : ""} ${o.autocomplete ? `autocomplete="${o.autocomplete}"` : ""}>`}${help ? `<p class="help">${esc(help)}</p>` : ""}</label>`;
  }
  const select = (name, title, values, value) =>
    `<label class="field"><span>${esc(title)}</span><select class="input" data-draft="${name}" aria-label="${esc(title)}">${Object.entries(
      values,
    )
      .map(
        ([k, v]) =>
          `<option value="${k}" ${String(value) === k ? "selected" : ""}>${esc(v)}</option>`,
      )
      .join("")}</select></label>`;
  const check = (name, title, help = "", checked = false) =>
    `<label class="check"><input type="checkbox" data-draft="${name}" ${checked ? "checked" : ""}><span><strong>${esc(title)}</strong>${help ? `<small>${esc(help)}</small>` : ""}</span></label>`;
  const path = (tail) =>
      `api/v1/projects/${encodeURIComponent(S.project.id)}${tail || ""}`,
    sources = () => list(S.project?.artifacts).filter((a) => !a.deleted),
    observations = () => list(S.project?.observations),
    jobsRunning = () =>
      [...S.jobs.values()].filter(
        (j) => j.project_id === S.project?.id && !terminal.has(j.status),
      ),
    versions = () =>
      [...list(S.project?.revisions)].sort(
        (a, b) => epoch(a.created_at) - epoch(b.created_at),
      ),
    latest = () => versions().at(-1),
    ready = () =>
      S.status.extractor_ready ?? S.status.extractor?.ready ?? false;
  class ApiError extends Error {
    constructor(message, status) {
      super(message);
      this.status = status;
    }
  }
  function epoch(value) {
    return +new Date(
      typeof value === "number" && value < 1e12 ? value * 1000 : value || 0,
    );
  }
  async function api(url, o = {}) {
    const method = o.method || "GET",
      headers = {
        ...(o.body !== undefined ? { "Content-Type": "application/json" } : {}),
        ...(method !== "GET" && S.auth?.csrf
          ? { "X-CSRF-Token": S.auth.csrf }
          : {}),
      };
    const r = await fetch(url, {
      method,
      headers,
      credentials: "same-origin",
      cache: "no-store",
      ...(o.body !== undefined ? { body: JSON.stringify(o.body) } : {}),
    });
    if (!r.ok) {
      let message = `The service could not complete this request (${r.status}).`;
      try {
        message = (await r.json()).error || message;
      } catch (_) {}
      if (r.status === 401 && !url.startsWith("auth/")) {
        S.auth = { authenticated: false, owner: S.auth?.owner, auth_mode: S.auth?.auth_mode };
        S.private = {};
        S.view = null;
        render();
      }
      throw new ApiError(message, r.status);
    }
    return o.raw ? r : r.status === 204 ? {} : r.json();
  }
  function announce(message) {
    announcer.textContent = "";
    requestAnimationFrame(() => {
      announcer.textContent = message;
    });
  }
  function notify(message) {
    S.notice = message;
    announce(message);
    clearTimeout(S.noticeTimer);
    S.noticeTimer = setTimeout(() => {
      S.notice = "";
      render();
    }, 3500);
  }
  async function guarded(fn) {
    if (S.busy) return;
    S.busy = true;
    S.error = "";
    render();
    try {
      await fn();
    } catch (e) {
      S.error = e.message || "Something went wrong. Please try again.";
      announce(S.error);
      if (e.status === 409 && S.project) {
        try {
          await loadProject(S.project.id);
        } catch (_) {}
        S.error +=
          " The latest project has been reloaded. Review it before trying again.";
      }
    } finally {
      S.busy = false;
      render();
    }
  }
  async function loadProject(id) {
    S.project = await api(`api/v1/projects/${encodeURIComponent(id)}`);
    for (const j of list(S.project.builds))
      if (j.id && !terminal.has(j.status))
        S.jobs.set(j.id, { ...j, project_id: id });
    const i = S.projects.findIndex((p) => p.id === id);
    if (i >= 0) S.projects[i] = S.project;
    return S.project;
  }
  async function resources() {
    if (!S.view) return;
    const id = S.view.project_id;
    if (id && S.project?.id !== id) {
      S.artifact = null;
      S.profile = null;
      await loadProject(id);
    }
    if (!id) {
      S.project = null;
      S.artifact = null;
      S.profile = null;
    }
    if (
      S.view.artifact_id &&
      S.project &&
      S.artifact?.id !== S.view.artifact_id
    )
      S.artifact = await api(
        path(`/artifacts/${encodeURIComponent(S.view.artifact_id)}`),
      );
    if (
      S.view.modal === "evidence" &&
      d().evidence_id &&
      S.evidence?.span?.id !== d().evidence_id
    )
      S.evidence = await api(
        path(`/evidence/${encodeURIComponent(d().evidence_id)}`),
      );
    if (S.view.screen === "profiles" && S.project) {
      const selected = versions().find(
        (r) => r.id === S.view.profile_revision_id,
      );
      const current = [...versions()].reverse().find((r) => r.valid !== false);
      const rid = S.view.draft?.profile_pinned
        ? selected?.id || current?.id
        : current?.id || selected?.id || latest()?.id;
      if (rid && S.view.profile_revision_id !== rid)
        control(
          { profile_revision_id: rid },
          { controller: S.view.controller },
        ).catch(() => {});
      if (
        S.profile &&
        versions().find((r) => r.id === S.profile.id)?.valid !== S.profile.valid
      )
        S.profile = null;
      if (rid && S.profile?.id !== rid) {
        const summary = versions().find((r) => r.id === rid);
        S.profile =
          summary?.valid === false
            ? { ...summary, files: {} }
            : await api(path(`/revisions/${encodeURIComponent(rid)}`));
      }
    }
    if (S.project && S.view.screen === "evaluate" && d().compare_blind) {
      S.comparisons = list((await api(path("/comparisons"))).items);
      if (d().revision_diff && d().comparison_revision_a && d().comparison_revision_b) {
        S.revisionDiff = await api(path(`/revision-diff/${encodeURIComponent(d().comparison_revision_a)}/${encodeURIComponent(d().comparison_revision_b)}`));
      } else S.revisionDiff = null;
    }
    if (S.view.modal === "agent.apply") {
      if (!S.agentTargetsLoaded) await loadAgentTargets();
      if (d().agent_target_id && S.agentPreview?.target?.id !== d().agent_target_id)
        await loadAgentStatus(d().agent_target_id);
    }
  }
  async function loadAgentTargets() {
    S.agentTargets = list((await api("api/v1/agent-targets")).items);
    S.agentTargetsLoaded = true;
  }
  async function loadAgentStatus(id = d().agent_target_id) {
    S.agentPreview = null;
    if (id) S.agentPreview = await api(`api/v1/agent-targets/${encodeURIComponent(id)}`);
    return S.agentPreview;
  }
  async function ack() {
    if (S.view)
      await api(`api/v1/views/${encodeURIComponent(S.view.id)}/ack`, {
        method: "POST",
        body: { revision: S.view.revision },
      }).catch(() => {});
  }
  function control(values, { paint = false, controller = "human" } = {}) {
    if (!S.view) return Promise.resolve();
    const optimistic = { ...S.view, ...values, controller };
    S.view = optimistic;
    if (paint) render();
    const operation = async () => {
      const current = await api(
        `api/v1/views/${encodeURIComponent(optimistic.id)}`,
      );
      const sent = await api(
        `api/v1/views/${encodeURIComponent(optimistic.id)}/commands`,
        {
          method: "POST",
          body: {
            expected_revision: current.revision,
            controller,
            commands: Object.entries(values)
              .filter(([field]) => field !== "controller")
              .map(([field, value]) => ({ op: "set", field, value })),
          },
        },
      );
      if (S.view?.id === sent.id) {
        const newer = {};
        for (const key of Object.keys(S.view))
          if (
            !["id", "revision", "rendered_revision"].includes(key) &&
            S.view[key] !== optimistic[key]
          )
            newer[key] = S.view[key];
        S.view = { ...sent, ...newer };
        await resources();
        if (paint) render();
        await ack();
      }
      return sent;
    };
    const result = S.queued.then(operation);
    S.queued = result.catch((e) => {
      S.error = `Display sync: ${e.message}`;
      render();
    });
    return result;
  }
  function setDraft(name, value) {
    if (
      /^(password|username|source_text|identity_json|agent_token|agent_ca|agent_url|raw|credentials|auth)/i.test(
        name,
      )
    )
      throw Error(
        "Private input belongs in the local form, not shared view state.",
      );
    if (!S.view) return Promise.resolve();
    S.view = { ...S.view, draft: { ...d(), [name]: value } };
    clearTimeout(S.syncTimer);
    S.syncTimer = setTimeout(
      () => control({ draft: { ...d() } }).catch(() => {}),
      260,
    );
    return Promise.resolve();
  }
  async function flush() {
    clearTimeout(S.syncTimer);
    if (S.view) await control({ draft: { ...d() } });
  }
  async function navigate(screen, extra = {}) {
    await flush();
    await control(
      {
        screen,
        modal: null,
        artifact_id: null,
        observation_id: null,
        focus: null,
        scroll: { x: 0, y: 0 },
        ...extra,
      },
      { paint: true },
    );
    window.scrollTo(0, 0);
  }
  async function openModal(modal, extra = {}) {
    await flush();
    await control({ modal, ...extra }, { paint: true });
  }
  async function refresh() {
    const [p, s, a] = await Promise.all([
      api("api/v1/projects"),
      api("api/v1/status"),
      api("api/v1/activity"),
    ]);
    S.projects = list(p.items);
    S.status = s;
    S.activity = list(a.items || a.events);
    if (S.project) await loadProject(S.project.id);
    await resources();
  }
  async function openWorkspace() {
    S.opening = true;
    render();
    try {
      await refresh();
      const remembered =
        query.get("view") || sessionStorage.getItem(storageKey);
      if (remembered) {
        try {
          S.view = await api(`api/v1/views/${encodeURIComponent(remembered)}`);
        } catch (_) {}
      }
      if (!S.view)
        S.view = await api("api/v1/views", { method: "POST", body: {} });
      sessionStorage.setItem(storageKey, S.view.id);
      await resources();
      S.error = "";
      await ack();
    } catch (e) {
      S.error = e.message;
    } finally {
      S.opening = false;
      render();
    }
  }
  function messages() {
    return `${S.error ? `<div class="banner error" role="alert"><p>${esc(S.error)}</p>${link("message.dismiss", "Dismiss")}</div>` : ""}${S.notice ? `<div class="toast" role="status">${esc(S.notice)}${link("notice.dismiss", "×", 'aria-label="Dismiss notification"')}</div>` : ""}`;
  }
  const heading = (title, description = "", actions = "") =>
    `<div class="page-heading"><div><h1>${esc(title)}</h1>${description ? `<p>${esc(description)}</p>` : ""}</div>${actions ? `<div class="actions">${actions}</div>` : ""}</div>`;
  const empty = (title, description, actions = "", compact = false) =>
    `<div class="empty ${compact ? "compact" : ""}"><h2>${esc(title)}</h2>${description ? `<p>${esc(description)}</p>` : ""}${actions ? `<div class="actions">${actions}</div>` : ""}</div>`;
  const details = (key, title, content) =>
    `<div class="optional-details"><button type="button" class="details-toggle" data-action="details.toggle" data-key="${key}" aria-expanded="${!!d()[key]}"><span aria-hidden="true">${d()[key] ? "−" : "+"}</span>${esc(title)}</button>${d()[key] ? `<div class="details-content">${content}</div>` : ""}</div>`;
  function jobCards() {
    return [...S.jobs.values()]
      .filter((j) => j.project_id === S.project?.id)
      .map((j) => {
        const done = terminal.has(j.status);
        if (done && j.status === "complete") return "";
        return `<div class="job ${done ? "done" : ""} ${j.status === "failed" ? "failed" : ""}" role="status"><div class="job-icon" aria-hidden="true">${done ? "!" : ""}</div><div><strong>${esc(done ? label(j.status) : label(j.stage || j.status))}</strong><p>${esc(j.note || (done ? `Job ${j.status}.` : "Working…"))}</p></div>${done ? link("job.dismiss", "Dismiss", `data-id="${esc(j.id)}"`) : link("job.cancel", "Cancel", `data-id="${esc(j.id)}"`)}</div>`;
      })
      .join("");
  }
  function login() {
    const opening = S.auth?.authenticated, first = S.auth?.first_account_needed;
    const setup = sessionStorage.getItem(setupStorageKey);
    const content = opening
      ? `<h1>${S.opening ? "Opening…" : "You’re signed in"}</h1><p>${S.opening ? "Loading your workspace." : "The workspace could not open. Try again."}</p>${btn("auth.resume", "Open workspace", "", "", S.opening)}`
      : first
        ? `<h1>Set up Persona Workshop</h1>${setup ? `<p>Choose a password for this workshop. Your computer’s password stays separate.</p><form data-form="auth.first-account">${field("password", "New password", "Use at least 12 characters.", {private:true,type:"password",autocomplete:"new-password",required:true,value:S.private.password || ""})}<label class="check"><input type="checkbox" data-private="start_at_signin" ${S.private.start_at_signin ? "checked" : ""}><span>Open in the background when I sign in</span></label><button type="submit" class="button" ${S.busy ? "disabled" : ""}>${S.busy ? "Creating account…" : "Create account"}</button></form>` : '<p>Open the Persona Workshop app on this computer to finish setup.</p>'}`
        : `<h1>Sign in</h1><p>${S.auth?.auth_mode === "local-password" ? "Use the password you chose for this workshop." : "Use your account on this machine."}</p><form data-form="auth.login">${field("username", "Username", "", { value: S.private.username ?? S.auth?.owner ?? "", private: true, autocomplete: "username", required: true })}${field("password", "Password", "", { private: true, type: "password", autocomplete: "current-password", required: true, value: S.private.password || "" })}<button type="submit" class="button" ${S.busy ? "disabled" : ""}>${S.busy ? "Signing in…" : "Sign in"}</button></form>`;
    return `<main id="main" class="login-simple"><div class="login-form"><div class="brand"><span class="mark" aria-hidden="true">pw.</span><strong>Persona Workshop</strong></div>${messages()}${content}</div></main>`;
  }
  function projectsScreen() {
    const cards = S.projects
      .map(
        (p) =>
          `<article class="card project-card"><h2>${esc(p.alias)}</h2><p>${esc(p.purpose || "")}</p><div class="project-foot"><span>${esc(stamp(p.updated_at))}</span>${btn("project.open", "Open", "secondary small", `data-id="${esc(p.id)}"`)}</div></article>`,
      )
      .join("");
    return (
      heading(
        "Projects",
        "Choose a person to work on.",
        btn("project.new", "New project"),
      ) +
      (cards
        ? `<div class="project-grid">${cards}</div>`
        : empty(
            "Create your first project",
            "Add someone’s writing and working notes to create their two profiles.",
            btn("project.new", "New project"),
          ))
    );
  }
  function evidenceScreen() {
    const all = sources(),
      building = jobsRunning().length > 0;
    const collection = (key, title, description) => {
      const chosen = all.filter((a) => list(a.collections).includes(key));
      return `<section class="collection-summary"><div><h2>${title}</h2><p>${description}</p></div><strong>${chosen.length}<span>${chosen.length === 1 ? "source" : "sources"}</span></strong></section>`;
    };
    const rows = all
      .map(
        (a) =>
          `<article class="source-row"><div class="source-main"><div><button class="source-name" data-action="source.open" data-id="${esc(a.id)}">${esc(a.name)}</button>${d().source_details ? `<p class="help">${esc(kinds[a.kind] || label(a.kind))} · ${a.selected_count || 0} selected passages${list(a.authors).length ? ` · ${esc(a.authors.join(", "))}` : ""}</p>` : ""}</div><div class="chips">${list(
            a.collections,
          )
            .map((c) =>
              badge(c === "working" ? "Working style" : "Communication"),
            )
            .join(
              "",
            )}${a.holdout ? badge("Held back", "neutral") : ""}${!a.holdout && !a.selected_count ? badge("Choose speaker", "warning") : ""}</div></div>${link("source.open", "Edit", `data-id="${esc(a.id)}"`)}</article>`,
      )
      .join("");
    return (
      heading(
        "Sources",
        "Add writing, conversations, or notes.",
        btn("source.new", "Add source"),
      ) +
      jobCards() +
      `<div class="collection-summaries">${collection("communication", "Communication", "How they sound.")}${collection("working", "Working style", "How they think and work.")}</div><section class="card source-card"><div class="card-head"><h2>${all.length ? `${all.length} ${all.length === 1 ? "source" : "sources"}` : "Source material"}</h2>${all.length ? link("details.toggle", d().source_details ? "Hide details" : "Show details", 'data-key="source_details"') : ""}</div>${rows || empty("Add a source", "Use a transcript, chat export, piece of writing, or existing profile.", btn("source.new", "Add source", "secondary"), true)}<div class="form-footer"><small>${ready() ? (all.length ? "Sources can inform one or both profiles." : "") : esc(S.status.extractor?.note || "Connect an extractor to create profiles.")}</small>${btn("build.start", building ? "Creating profiles…" : latest() ? "Update profiles" : "Create profiles", "", "", building || !all.some((a) => !a.holdout && a.selected_count > 0) || !ready())}</div></section>`
    );
  }
  function tabs(active) {
    return [
      ["communication", "Communication"],
      ["working", "Working style"],
    ]
      .map(
        ([key, title]) =>
          `<button type="button" role="tab" aria-selected="${active === key}" class="tab ${active === key ? "active" : ""}" data-action="tab.select" data-tab="${key}">${title}</button>`,
      )
      .join("");
  }
  function confidence(o) {
    const c = o.confidence || {},
      show = (v) => v ?? "Unspecified";
    return `<div class="confidence"><span>Attribution <strong>${esc(show(c.attribution))}</strong></span><span>Evidence support <strong>${esc(show(c.support))}</strong></span><span>Generalization <strong>${esc(show(c.generalization))}</strong></span></div>${c.reason ? `<p class="help">${esc(c.reason)}</p>` : ""}`;
  }
  function reviewScreen() {
    const tab = S.view.tab === "working" ? "working" : "communication",
      filter = d().review_details ? S.view.filter || "all" : "all",
      all = observations().filter((o) => o.collection === tab),
      shown = all.filter(
        (o) =>
          filter === "all" ||
          (filter === "invalid" ? o.valid === false : o.status === filter),
      );
    const cards = shown
      .map(
        (o) =>
          `<article class="card observation"><div class="observation-top"><span class="rule-section">${esc(sections[tab]?.[o.section] || "General")}${!d().review_details && (o.status === "rejected" || o.valid === false) ? badge(o.valid === false ? "Source changed" : "Excluded", "neutral") : ""}</span>${link("observation.edit", "Edit", `data-id="${esc(o.id)}"`)}</div><div class="statement markdown">${markdown(o.statement)}</div>${
            d().review_details
              ? `<div class="rule-details"><div class="chips">${badge(o.valid === false ? "Source changed" : label(o.status), o.valid === false ? "bad" : o.status === "accepted" ? "good" : "neutral")}${o.source === "owner" ? badge("Your instruction", "neutral") : ""}</div>${o.scope ? `<p class="help">${esc(o.scope)}</p>` : ""}${o.source === "owner" ? "" : confidence(o)}<div class="observation-foot"><div class="evidence-links">${list(
                  o.evidence_ids,
                )
                  .map((id, i) =>
                    link(
                      "evidence.open",
                      `Source ${i + 1} ↗`,
                      `data-id="${esc(id)}"`,
                    ),
                  )
                  .join("")}${list(o.conflicts)
                  .map((id, i) =>
                    link(
                      "evidence.open",
                      `Conflicting source ${i + 1} ↗`,
                      `data-id="${esc(id)}"`,
                    ),
                  )
                  .join(
                    "",
                  )}</div><div class="actions">${btn("observation.reject", "Exclude", "secondary small", `data-id="${esc(o.id)}"`, o.status === "rejected")}${btn("observation.accept", "Accept", "secondary small", `data-id="${esc(o.id)}"`, o.status === "accepted" || o.valid === false)}</div></div></div>`
              : ""
          }</article>`,
      )
      .join("");
    return (
      heading(
        "Profile details",
        "Edit individual instructions or inspect their sources.",
        `${btn("screen.profiles", "Back to profiles", "secondary")}${btn("observation.new", "Add instruction", "secondary")}`,
      ) +
      jobCards() +
      `<div class="filters"><div class="tabs" role="tablist" aria-label="Instruction collection">${tabs(tab)}</div>${link("details.toggle", d().review_details ? "Hide sources & review" : "Sources & review", 'data-key="review_details"')}${
        d().review_details
          ? `<select class="input" data-field="filter" aria-label="Observation status">${Object.entries(
              {
                all: "All instructions",
                pending: "Not reviewed",
                accepted: "Accepted",
                rejected: "Excluded",
                invalid: "Source changed",
              },
            )
              .map(
                ([k, v]) =>
                  `<option value="${k}" ${filter === k ? "selected" : ""}>${v}</option>`,
              )
              .join("")}</select>`
          : ""
      }</div>${cards || empty("No instructions yet", "Create profiles from your sources first.", btn("screen.evidence", "Go to sources", "secondary"))}${observations().length ? `<div class="review-footer"><small>Save your edits to update the profile files.</small>${btn("profile.create", "Save profiles", "", "", jobsRunning().length > 0)}</div>` : ""}`
    );
  }
  function markdown(text) { return renderMarkdown(text); }
  function profilesScreen() {
    const p = S.profile,
      all = versions(),
      tab = S.view.tab === "working" ? "working" : "communication",
      filename = tab === "working" ? "working-style.md" : "communication.md";
    const documentText = p?.files?.[filename] || "";
    const hasGuidance = documentText
      .split("\n")
      .some((line) => line.trim() && !/^#{1,6}\s/.test(line.trim()));
    const extra = p
      ? `<div class="profile-detail-grid"><label class="field"><span>Version</span><select class="input" data-field="profile_revision_id" aria-label="Profile version">${all.map((r, i) => `<option value="${esc(r.id)}" ${p.id === r.id ? "selected" : ""}>${i + 1} · ${esc(stamp(r.created_at))}</option>`).join("")}</select></label><div><span class="field-label">Instructions & sources</span>${btn("screen.review", "Review or edit instructions", "secondary")}</div><div><span class="field-label">Portable package</span>${btn("profile.export", "Download package", "secondary", "", p.valid === false)}<p class="help">Both profiles and their manifest; no raw sources or private identity.</p></div><div><span class="field-label">Regenerate</span>${btn("build.start", "Update from sources", "secondary", "", jobsRunning().length > 0 || !ready())}</div></div>`
      : "";
    return (
      heading(
        "Profiles",
        "",
        p ? `${btn("screen.evaluate", "Try it", "secondary", "", p.valid === false)}${btn("export.open", "Export…", "secondary", "", p.valid === false)}${btn("agent.open", "Use on agent…", "", "", p.valid === false)}` : "",
      ) +
      jobCards() +
      (!p
        ? empty(
            "Create your profiles",
            "Add source material, then create communication and working-style profiles.",
            sources().some((a) => !a.holdout && a.selected_count > 0)
              ? btn(
                  "build.start",
                  "Create profiles",
                  "",
                  "",
                  jobsRunning().length > 0 || !ready(),
                )
              : btn("screen.evidence", "Add sources", "secondary"),
          )
        : `${p.valid === false ? `<div class="banner warning"><p>Sources or instructions changed. This version is unavailable.</p>${versions().some((r) => r.valid !== false) ? btn("profile.latest", "Use latest version", "secondary small") : ""}</div>` : ""}<div class="profile-toolbar"><div class="tabs" role="tablist" aria-label="Profile file">${tabs(tab)}</div><div class="actions">${btn("profile.download", `Download ${filename}`, "secondary", `data-file="${filename}"`, p.valid === false)}</div></div><article class="card profile-document"><div class="markdown">${p.valid === false ? '<p class="empty-profile">Create an updated profile from the current sources, or select another version.</p>' : hasGuidance ? markdown(documentText) : '<p class="empty-profile">No instructions yet. Add more sources for this profile, then update it.</p>'}</div></article>${details("profile_details", "Details", extra)}`)
    );
  }
  function ratingSelect(id, key, title, value) {
    return `<label class="field"><span>${title}</span><select class="input" data-rating="${esc(id)}" data-rating-field="${key}" aria-label="${title} for ${esc(id)}"><option value="">Not rated</option>${[1, 2, 3, 4, 5].map((n) => `<option value="${n}" ${Number(value) === n ? "selected" : ""}>${n} · ${["", "Poor fit", "Some fit", "Mixed", "Good fit", "Strong fit"][n]}</option>`).join("")}</select></label>`;
  }
  function evaluateScreen() {
    if (d().compare_blind) return comparisonScreen();
    const p =
        list(S.project.revisions).find(
          (r) => r.id === S.view.profile_revision_id,
        ) || latest(),
      all = list(S.project.evaluations).filter((e) => e.revision_id === p?.id);
    const cards = [...all]
      .sort((a, b) => epoch(b.created_at) - epoch(a.created_at))
      .map((e) => {
        const r = d()[`rating_${e.id}`] || e.rating || {};
        return `<article class="card evaluation"><div class="card-head"><div class="result-label"><strong>Simulated response</strong>${all.some((x) => x.variant !== "both") ? `<span>${esc(variants[e.variant] || label(e.variant))}</span>` : ""}</div></div><div class="scenario">${esc(e.scenario)}</div><div class="response markdown">${markdown(e.response)}</div>${d().evaluation_details ? `<div class="rating"><div class="rating-grid">${ratingSelect(e.id, "communication_fit", "Communication fit", r.communication_fit)}${ratingSelect(e.id, "working_fit", "Working style fit", r.working_fit)}</div><label class="field"><span>What matched or missed?</span><textarea class="input" data-rating="${esc(e.id)}" data-rating-field="notes" aria-label="Review notes for ${esc(variants[e.variant] || e.variant)}">${esc(r.notes || "")}</textarea></label>${btn("evaluation.rate", e.rating ? "Update feedback" : "Save feedback", "secondary small", `data-id="${esc(e.id)}"`)}</div>` : ""}</article>`;
      })
      .join("");
    const compare = Object.entries(variants)
      .map(([k, v]) =>
        check(`variant_${k}`, v, "", d()[`variant_${k}`] ?? k === "both"),
      )
      .join("");
    return (
      heading(
        "Try it",
        "Describe a situation and see how the profiles shape a response.",
        btn("comparison.open", "Compare blind", "secondary"),
      ) +
      jobCards() +
      `<section class="card scenario-card">${field("scenario", "Scenario", "", { area: true, rows: 4, placeholder: "A teammate wants to ship today, but one important test is unfinished. How would they respond?" })}<div class="scenario-actions">${link("details.toggle", d().compare_profiles ? "Hide comparison options" : "Compare profiles", 'data-key="compare_profiles"')}${btn("evaluation.run", d().compare_profiles ? "Run comparison" : "Try it", "", "", !p || p.valid === false || !ready() || jobsRunning().length > 0)}</div>${d().compare_profiles ? `<div class="compare-options"><p class="help">Try profiles separately, together, or without either.</p><div class="check-grid">${compare}</div></div>` : ""}${!p ? `<p class="help">Create profiles first.</p>` : p.valid === false ? `<p class="help">Update the profile before trying it.</p>` : ""}</section>${cards ? `<div class="results-heading"><h2>Responses for this version</h2>${link("details.toggle", d().evaluation_details ? "Hide feedback" : "Add feedback", 'data-key="evaluation_details"')}</div>${cards}` : ""}`
    );
  }
  function comparisonScreen() {
    const choices = Object.fromEntries(versions().map((r, i) => ({r,i})).filter(({r}) => r.valid !== false).map(({r, i}) => [r.id, `Version ${i + 1} · ${stamp(r.created_at)}`]));
    const first = d().comparison_revision_a || latest()?.id || "";
    const second = d().comparison_revision_b || first;
    const held = Object.fromEntries(sources().filter(a => a.holdout).map(a => [a.id, a.name]));
    const score = (c, label, key, title) => {
      const value = d()[`comparison_rating_${c.id}`]?.[`${label}_${key}`] ?? c.rating?.ratings?.[label]?.[key] ?? "";
      return `<label class="field"><span>${esc(title)}</span><select class="input" data-comparison="${c.id}" data-comparison-field="${label}_${key}" ${c.revealed_at ? "disabled" : ""}><option value="">Choose a rating</option>${[1,2,3,4,5].map(n => `<option value="${n}" ${Number(value) === n ? "selected" : ""}>${n} · ${["","Poor fit","Some fit","Mixed","Good fit","Strong fit"][n]}</option>`).join("")}</select></label>`;
    };
    const cards = S.comparisons.map(c => `<section class="card comparison-result"><div class="card-head"><h2>${c.revealed_at ? "Reviewed comparison" : "Blind comparison"}</h2>${badge(c.status)}</div><p class="scenario">${esc(c.scenario)}</p>${!c.valid ? '<p class="banner warning">Sources changed. These responses are unavailable; start a fresh comparison.</p>' : c.status !== "complete" ? `<p>${esc(c.note)}</p>` : `<div class="comparison-grid">${c.candidates.map(a => `<article><h3>Response ${esc(a.label)}</h3>${c.revealed_at ? `<p class="help">${esc(variants[a.variant])} · ${esc(choices[a.revision_id] || a.revision_id)}</p>` : ""}<div class="markdown">${markdown(a.response)}</div><div class="rating-grid">${score(c,a.label,"communication_fit","Sounds like them")}${score(c,a.label,"working_fit","Approaches work like them")}</div><label class="field"><span>What matched or missed?</span><textarea class="input" data-comparison="${c.id}" data-comparison-field="${a.label}_notes" ${c.revealed_at ? "disabled" : ""}>${esc(d()[`comparison_rating_${c.id}`]?.[`${a.label}_notes`] ?? c.rating?.ratings?.[a.label]?.notes ?? "")}</textarea></label></article>`).join("")}</div>${c.references.length ? details("comparison_references", "Held-back references · never sent to generation", c.references.map(a => `<h3>${esc(a.name)}</h3>${a.spans.map(span => `<blockquote>${esc(span.text)}</blockquote>`).join("")}`).join("")) : '<p class="help">Fresh hypothetical scenario; no reference answer was supplied.</p>'}${c.revealed_at ? `<p class="help">Feedback recorded by ${esc(c.rating.reviewer)} · preference ${esc(c.rating.preference)}. These are that reviewer’s judgments, not a fidelity score.</p>` : `<label class="field"><span>Which response fits better overall?</span><select class="input" data-comparison="${c.id}" data-comparison-field="preference"><option value="">Choose after reading</option>${[...c.candidates.map(a => a.label),"tie","neither"].map(x => `<option value="${x}" ${(d()[`comparison_rating_${c.id}`]?.preference ?? c.rating?.preference) === x ? "selected" : ""}>${esc(x === "tie" ? "About the same" : x === "neither" ? "Neither fits" : `Response ${x}`)}</option>`).join("")}</select></label><div class="actions">${btn("comparison.rate", c.rating ? "Update feedback" : "Save feedback", "secondary", `data-id="${c.id}"`)}${btn("comparison.reveal", "Reveal profiles", "", `data-id="${c.id}"`, !c.rating)}</div>`}`}</section>`).join("");
    const diff = S.revisionDiff ? S.revisionDiff.files.map(f => `<section><h3>${esc(f.file)}</h3>${f.changed ? `<div class="comparison-grid"><div><h4>Removed or changed</h4>${f.removed_paragraphs.map(x => `<pre class="diff-removed">${esc(x)}</pre>`).join("") || '<p class="help">None</p>'}</div><div><h4>Added or changed</h4>${f.added_paragraphs.map(x => `<pre class="diff-added">${esc(x)}</pre>`).join("") || '<p class="help">None</p>'}</div></div>${details("diff_full", "Full before and after", `<div class="comparison-grid"><pre>${esc(f.before)}</pre><pre>${esc(f.after)}</pre></div>`)}` : '<p class="help">No changes.</p>'}</section>`).join("") : "";
    return heading("Compare before revealing", "Judge the responses first, then see which profile produced each one.", btn("comparison.close", "Back to Try it", "secondary")) + jobCards() + `<section class="card">${field("scenario", "New scenario", "Describe a fresh situation, without copying a reference answer.", {area:true,rows:4})}<div class="comparison-grid"><div>${select("comparison_revision_a", "First profile version", choices, first)}${select("comparison_variant_a", "First profile variant", variants, d().comparison_variant_a || "both")}</div><div>${select("comparison_revision_b", "Second profile version", choices, second)}${select("comparison_variant_b", "Second profile variant", variants, d().comparison_variant_b || "baseline")}</div></div>${select("comparison_holdout", "Reference for the reviewer (optional)", {"":"No reference · fresh scenario",...held}, d().comparison_holdout || "")}<p class="help">Held-back sources stay out of profile creation and response generation. Names of profiles are hidden until you save feedback. Responses can still contain clues.</p>${select("comparison_perspective", "Your perspective", {reviewer:"Reviewing this persona",self:"Reviewing my own persona",peer:"Reviewing someone I work with"},d().comparison_perspective || "reviewer")}<div class="actions">${btn("comparison.run", "Generate blind responses", "", "", !latest() || !ready() || jobsRunning().length > 0)}${btn("revision.diff", "See instruction changes", "secondary", "", !first || !second || first === second)}</div>${diff}</section>${cards}`;
  }
  function settingsScreen() {
    const p = S.project,
      events = S.activity
        .filter((e) => !String(e.kind || e.event || "").startsWith("view"))
        .sort(
          (a, b) =>
            epoch(b.at || b.time || b.created_at) -
            epoch(a.at || a.time || a.created_at),
        )
        .slice(0, 20);
    return (
      heading("Settings", "Project details and private identity.") +
      `<section class="card"><h2>Project details</h2><div class="form-grid">${field("settings_alias", "Project name", "", { value: d().settings_alias ?? p.alias })}${field("settings_aliases", "Speaker names or aliases", "Separate names with commas. Source-specific speaker choices remain editable.", { value: d().settings_aliases ?? list(p.aliases).join(", ") })}${field("settings_purpose", "Purpose", "What should this simulation help you explore?", { area: true, full: true, value: d().settings_purpose ?? p.purpose })}</div><div class="form-footer"><small></small>${btn("project.save", "Save details")}</div></section><section class="card"><div class="card-head"><div><h2>Private identity</h2><p>Optional account or identity bindings, stored separately from the two profiles.</p></div>${badge(p.identity_present ? "Configured" : "Not added")}</div>${btn("identity.open", p.identity_present ? "View private identity" : "Add private identity", "secondary")}<p class="help">These values are kept out of shared display state and the default exported package.</p></section><section class="card"><div class="card-head"><div><h2>Meaningful activity</h2><p>Evidence changes, builds, profile versions, and experiments.</p></div>${link("refresh", "Refresh")}</div>${events.length ? `<ul class="timeline">${events.map((e) => `<li><span class="dot" aria-hidden="true"></span><div><strong>${esc(label(e.kind || e.event))}</strong>${e.note || e.data?.note ? `<div class="muted">${esc(e.note || e.data.note)}</div>` : ""}</div><time>${esc(stamp(e.at || e.time || e.created_at))}</time></li>`).join("")}</ul>` : '<p class="help">Project activity will appear as you work.</p>'}<div class="disclosure"><button class="text-button" data-action="details.toggle" data-key="service_details" aria-expanded="${!!d().service_details}">Service & display details ${d().service_details ? "−" : "+"}</button>${d().service_details ? `<div><p class="help">Extractor: ${ready() ? "Ready" : "Not connected"}. ${esc(S.status.extractor?.note || "")}</p><p class="help">Display ID: <code>${esc(S.view.id)}</code><br>Display revision: ${S.view.revision}. Project revision: ${p.revision}.</p><p class="help">This display can be embedded with <code>?embed=1</code> and controlled through the service’s view API. Human interaction takes control.</p></div>` : ""}</div></section>`
    );
  }
  function installationLabel(snapshot) {
    if (!snapshot) return "No package installed";
    const source = snapshot.source;
    if (!source) return snapshot.kind === "manual" ? "Edited on the agent" : "Original agent profiles";
    const i = versions().findIndex((r) => r.id === source.revision);
    return source.subject_key === S.project?.id && i >= 0
      ? `${S.project.alias} · version ${i + 1}`
      : `Profile ${String(source.revision || snapshot.id).slice(0, 8)}`;
  }
  function agentApplyBody(close) {
    const p = S.profile,
      choices = Object.fromEntries(S.agentTargets.map((t) => [t.id, t.name])),
      preview = S.agentPreview?.target?.id === d().agent_target_id ? S.agentPreview : null,
      installed = preview?.installation,
      same = installed?.active?.source?.revision === p?.id && installed?.active?.source?.subject_key === S.project?.id,
      prior = list(installed?.previous),
      version = versions().findIndex((r) => r.id === p?.id) + 1;
    return `<p>Apply <strong>${esc(S.project?.alias)} · version ${version}</strong> to an independent agent. Both communication and working-style files travel together.</p>${S.agentTargets.length ? `${select("agent_target_id", "Agent", { "": "Choose an agent", ...choices }, d().agent_target_id || "")}<div class="actions">${link("agent.new", "Connect another agent")}${d().agent_target_id ? link("agent.refresh", "Refresh status") : ""}</div>` : `<p class="help">Connect an agent once using its management address and access token.</p>${btn("agent.new", "Connect an agent", "secondary")}`}${preview ? `<section class="agent-installation"><div class="card-head"><strong>${esc(preview.target.name)}</strong>${badge(same ? "This version is installed" : "Connected", same ? "good" : "neutral")}</div><p class="help agent-address">${esc(preview.target.url)}</p><dl class="agent-facts"><dt>Installed</dt><dd>${esc(installationLabel(installed.active))}</dd><dt>Activation</dt><dd>Used on the agent’s next messages. Messages already running keep their current context.</dd></dl><p class="help">The agent keeps its own copy. Future workshop edits are applied only when you choose.</p>${prior.length ? details("agent_history", "Previous installations", `${select("agent_restore_id", "Restore a previous installation", { "": "Choose a version", ...Object.fromEntries(prior.map((x) => [x.id, `${installationLabel(x)}${x.created_at ? ` · ${stamp(x.created_at)}` : ""}`])) }, d().agent_restore_id || "")}<div class="actions">${btn("agent.restore", "Restore selected version", "secondary", "", !d().agent_restore_id || !prior.some((x) => x.id === d().agent_restore_id))}</div>`) : ""}${details("agent_connection", "Connection options", `${link("agent.forget", "Forget this connection")}<p class="help">The agent and its installed profiles keep running.</p>`)}</section>` : ""}<div class="form-footer"><div class="actions">${close}${btn("agent.apply", same ? "Already applied" : "Apply profiles", "", "", !preview || !p || p.valid === false || same)}</div></div>`;
  }
  function modal() {
    const mode = S.view?.modal;
    if (!mode) return "";
    let title = "",
      description = "",
      body = "",
      wide = false;
    const close = btn("modal.close", "Cancel", "secondary");
    if (mode === "profile.export") {
      const format = d().export_format || "agents";
      title = "Use in another experience";
      body = `${select("export_format", "Format", {agents: "AGENTS.md · coding agent workspace", soul: "SOUL.md · OpenClaw workspace", native: "Constellation · profile package"}, format)}${format === "agents" ? `<p>Download a ZIP with AGENTS.md and both profile files. Extract it in the workspace your agent uses.</p>${check("export_merge", "Keep an existing AGENTS.md", "Replace only the Persona Workshop section; keep the rest of your instructions.", !!d().export_merge)}${d().export_merge ? `<div class="dropzone"><label for="existing-agents-file">Choose your existing AGENTS.md</label><input type="file" id="existing-agents-file" data-private="existing_agents_file" accept=".md,text/markdown,text/plain"></div>${field("existing_agents", "Or paste the existing file", "Used for this download only; it is not saved in your project or shared view.", {private:true,area:true,rows:6,value:S.private.existing_agents || ""})}` : ""}` : format === "soul" ? '<p>Download a ZIP with a self-contained SOUL.md and separate profile metadata. Put SOUL.md in your OpenClaw agent workspace. It contains both communication and working guidance.</p><p class="help">If you already have a SOUL.md, combine the guidance before replacing it.</p>' : '<p>Download both profiles with their manifest for a Constellation agent. You can also use “Use on agent” to apply them directly.</p>'}<p class="help">Raw sources and private identity are excluded. Provenance stays in separate JSON files.</p><div class="form-footer"><div class="actions">${close}${btn("export.download", format === "native" ? "Download package" : "Download ZIP")}</div></div>`;
    } else if (mode === "agent.apply") {
      title = "Use on agent";
      body = agentApplyBody(close);
    } else if (mode === "agent.connect") {
      title = "Connect an agent";
      description = "Use the management connection from the agent’s settings.";
      body = `${field("agent_url", "Management address", "A secure address, for example https://agent.example.test:18449", { private: true, value: S.private.agent_url || "", placeholder: "https://…", autocomplete: "off" })}${field("agent_token", "Access token", "Create a token with observe and profiles access in the agent service.", { private: true, type: "password", value: S.private.agent_token || "", autocomplete: "off" })}${details("agent_advanced", "Local certificate", field("agent_ca", "Root certificate (PEM)", "Only needed for an installation using its own certificate. Paste the root certificate downloaded from that agent.", { private: true, area: true, value: S.private.agent_ca || "", rows: 4 }))}<div class="form-footer"><div class="actions">${btn("agent.open", "Back", "secondary")}${btn("agent.connect", "Connect")}</div></div>`;
    } else if (mode === "project.new") {
      title = "New project";
      body = `${field("project_alias", "Project name", "", { placeholder: "Alex" })}${field("project_aliases", "Names used in sources", "Optional, separated by commas.", { placeholder: "Alex, Alex Morgan" })}${field("project_purpose", "Purpose", "", { area: true, placeholder: "Product planning and collaboration" })}<div class="form-footer"><div class="actions">${close}${btn("project.create", "Create project")}</div></div>`;
    } else if (mode === "source.new") {
      title = "Add source";
      description =
        "Upload text, Markdown, or a JSON chat export, or paste below.";
      body = `<div class="dropzone"><label for="source-file">Choose a file</label><input type="file" id="source-file" data-private="source_file" accept=".txt,.md,.markdown,.json,.eml,.html,.htm,.docx,.odt,.pdf,.wav,.mp3,.m4a,.ogg,.webm,.mp4"></div>${S.private.source_file ? `<p class="help">Ready to import ${esc(S.private.source_file.name)}. ${/\.(wav|mp3|m4a|ogg|webm|mp4)$/i.test(S.private.source_file.name) ? (S.importCapabilities?.transcription_ready ? "Transcription is configured; identify the speaker after import." : "A transcription provider must be configured first. You can also upload diarized transcript JSON.") : "Review the extracted text and authors after import."}</p>` : ""}${field("source_name", "Source name", "", { placeholder: "Product planning conversation" })}${field("source_text", "Content", "", { private: true, area: true, rows: 5, value: S.private.source_text || "", placeholder: "Paste writing or a transcript…" })}${select("source_kind", "Source type", kinds, d().source_kind || "meeting")}${d().source_kind === "profile" ? '<p class="help source-kind-note">Its guidance informs the new profile; its prose is not treated as a writing sample.</p>' : ""}<div class="field-label">Use for</div><div class="check-grid">${check("source_communication", "Communication", "", d().source_communication ?? true)}${check("source_working", "Working style", "", d().source_working ?? false)}</div>${details("source_options", "More options", check("source_holdout", "Hold back for evaluation", "Exclude from profile creation.", d().source_holdout || false))}<div class="form-footer"><div class="actions">${close}${btn("source.create", "Add source")}</div></div>`;
    } else if (mode === "source") {
      const a = S.artifact;
      if (!a) return "";
      wide = true;
      title = a.name;
      description = kinds[a.kind] || label(a.kind);
      const authors = d().artifact_authors ?? a.authors ?? [],
        speakers = [
          ...new Set([
            ...list(a.speakers).map((x) =>
              typeof x === "string" ? x : x.name || x.speaker || "",
            ),
            ...list(a.spans).map((x) => x.speaker || ""),
          ]),
        ].filter(Boolean),
        selected = d().artifact_span_ids;
      const excluded = (x) =>
        x.quoted ||
        ["assistant", "tool", "system"].includes(x.role) ||
        !authors.includes(x.speaker);
      const options = `${field("artifact_extra_authors", "Additional author labels", "Exact labels from the source.", { value: d().artifact_extra_authors || "", placeholder: "Optional, comma-separated" })}${check("artifact_holdout", "Hold back for evaluation", "Exclude from profile creation.", d().artifact_holdout ?? a.holdout)}${details("span_selection", "Choose individual passages", btn("source.reset_spans", "Use default selection", "secondary small"))}`;
      body = `${a.kind === "profile" ? '<p class="help">Reference guidance, attributed automatically.</p>' : `<div class="field-label">Which speaker is ${esc(S.project.alias)}?</div><div class="check-grid">${speakers.map((x) => `<label class="check"><input type="checkbox" data-author="${esc(x)}" ${authors.includes(x) ? "checked" : ""}><span>${esc(x)}</span></label>`).join("") || '<p class="help">No speaker labels found. Add a name to the source text.</p>'}</div>`}<div class="check-grid">${check("artifact_communication", "Communication", "", d().artifact_communication ?? list(a.collections).includes("communication"))}${check("artifact_working", "Working style", "", d().artifact_working ?? list(a.collections).includes("working"))}</div>${details("source_options", "More options", options)}<h3 data-space="top-24">Preview</h3><div class="span-list">${list(
        a.spans,
      )
        .map((x) => {
          const skip = excluded(x),
            enabled = Array.isArray(selected) ? selected.includes(x.id) : !skip;
          return `<div class="span-item ${skip ? "excluded" : ""}">${d().source_options && d().span_selection ? `<label class="check"><input type="checkbox" data-span="${esc(x.id)}" ${enabled ? "checked" : ""}><div>` : ""}<div class="span-meta"><strong>${esc(x.speaker || "Unattributed")}</strong>${Number.isFinite(x.start_seconds) ? `<span>${esc(x.start_seconds)}–${esc(x.end_seconds)} s</span>` : ""}${x.quoted ? badge("Quoted") : ""}${["assistant", "tool", "system"].includes(x.role) ? badge(label(x.role)) : ""}${skip ? badge("Context only", "neutral") : ""}</div><p class="span-text">${esc(x.text)}</p>${d().source_options && d().span_selection ? "</div></label>" : ""}</div>`;
        })
        .join(
          "",
        )}</div><div class="form-footer">${link("source.remove_confirm", "Remove source", `data-id="${esc(a.id)}"`)}<div class="actions">${close}${btn("source.save", "Done")}</div></div>`;
    } else if (mode === "source.remove") {
      title = "Remove source?";
      description = "Profiles based on this source will need to be updated.";
      body = `<p>${esc(S.artifact?.name || "Selected source")}</p><div class="form-footer"><div class="actions">${close}${btn("source.remove", "Remove source", "danger")}</div></div>`;
    } else if (mode === "observation.edit" || mode === "observation.new") {
      const create = mode === "observation.new",
        collection = create
          ? d().manual_collection || S.view.tab || "communication"
          : observations().find((o) => o.id === S.view.observation_id)
              ?.collection ||
            S.view.tab ||
            "communication";
      title = create ? "Add instruction" : "Edit instruction";
      const options = `${select("observation_section", "Section", { "": "General", ...sections[collection] }, d().observation_section || "")}${field("observation_scope", "Applies to", "Optional source context.", { area: true, rows: 2 })}`;
      body = `${create ? select("manual_collection", "Profile", { communication: "Communication", working: "Working style" }, collection) : ""}${field("observation_statement", "Instruction", "", { area: true, rows: 6 })}${details("observation_details", "More options", options)}<div class="form-footer"><div class="actions">${close}${btn(create ? "observation.create" : "observation.save", create ? "Add instruction" : "Save changes")}</div></div>`;
    } else if (mode === "evidence") {
      const e = S.evidence;
      if (!e) return "";
      title = e.artifact?.name || "Source passage";
      description = `${e.span?.speaker || "Unattributed"} · Lines ${e.span?.line_start ?? "?"}–${e.span?.line_end ?? "?"}`;
      body = `<div class="span-item"><p class="span-text">${esc(e.span?.text)}</p></div><div class="form-footer"><div class="actions">${btn("source.open", "Open source", "secondary", `data-id="${esc(e.artifact?.id)}"`)}${btn("modal.close", "Done")}</div></div>`;
    } else if (mode === "identity") {
      title = "Private identity";
      description =
        "Stored separately; excluded from shared display state and profile exports.";
      body = S.private.identity_json
        ? `${field("identity_json", "Identity bindings · JSON", "", { private: true, area: true, rows: 8, value: S.private.identity_json })}<div class="form-footer"><div class="actions">${close}${btn("identity.save", "Save identity")}</div></div>`
        : `<p class="help">Open explicitly to inspect or change private values.</p><div class="form-footer"><div class="actions">${close}${btn("identity.open", "Open private identity")}</div></div>`;
    } else return "";
    return `<div class="modal-backdrop"><section class="modal ${wide ? "wide" : ""}" role="dialog" aria-modal="true" aria-labelledby="modal-title" tabindex="-1"><div class="modal-head"><div><h2 id="modal-title">${esc(title)}</h2>${description ? `<p>${esc(description)}</p>` : ""}</div><button type="button" class="icon-button" data-action="modal.close" aria-label="Close dialog">×</button></div>${S.error ? `<div class="banner error" role="alert">${esc(S.error)}</div>` : ""}${body}</section></div>`;
  }
  function render() {
    document.title = S.auth?.authenticated && S.project?.alias ? `${S.project.alias}: Persona Workshop` : "Persona Workshop";
    const active = document.activeElement,
      focusKey =
        active?.dataset?.draft ||
        active?.dataset?.private ||
        active?.dataset?.control ||
        null,
      selection =
        active && typeof active.selectionStart === "number"
          ? [active.selectionStart, active.selectionEnd]
          : null,
      wasModal = !!app.querySelector('[role="dialog"]');
    if (!S.auth?.authenticated || !S.view) {
      app.innerHTML = login();
      return;
    }
    const screen = S.project
      ? screens[S.view.screen]
        ? S.view.screen
        : "evidence"
      : "projects";
    const body =
      screen === "projects"
        ? projectsScreen()
        : screen === "evidence"
          ? evidenceScreen()
          : screen === "review"
            ? reviewScreen()
            : screen === "profiles"
              ? profilesScreen()
              : screen === "evaluate"
                ? evaluateScreen()
                : settingsScreen();
    const nav = (k, text = screens[k]) =>
      `<button type="button" class="nav-button ${screen === k || (k === "profiles" && screen === "review") ? "active" : ""}" data-action="screen.${k}" ${screen === k || (k === "profiles" && screen === "review") ? 'aria-current="page"' : ""}><span class="nav-symbol" aria-hidden="true">${icons[k]}</span>${text}</button>`;
    app.innerHTML = `<div class="shell"><aside class="sidebar"><div class="brand"><span class="mark" aria-hidden="true">pw.</span><strong>Persona Workshop</strong></div>${S.project ? `<div class="project-label"><strong>${esc(S.project.alias)}</strong></div><nav aria-label="Project">${["evidence", "profiles", "evaluate"].map((k) => nav(k)).join("")}</nav>` : ""}<nav class="secondary-nav" aria-label="Workspace">${nav("projects", "All projects")}${S.project ? nav("settings") : ""}<button class="nav-button mobile-signout" data-action="auth.logout">Sign out</button></nav><div class="sidebar-bottom">${link("auth.logout", `Sign out · ${esc(S.auth.owner || "owner")}`)}</div></aside><div class="main-shell"><header class="topbar"><div class="breadcrumb">${S.project ? `${esc(S.project.alias)} <span aria-hidden="true"> / </span> ` : ""}<strong>${screens[screen]}</strong></div><div class="top-right">${S.view.controller !== "human" ? `<button class="controller" data-action="view.takeover">${esc(S.view.controller || "Agent")} is guiding · Take control</button>` : ""}<span class="owner">${esc(S.auth.owner || "")}</span></div></header><main id="main" class="content" aria-busy="${S.busy}">${messages()}${body}</main></div></div>${modal()}`;
    if (focusKey) {
      const next = [
        ...app.querySelectorAll("[data-draft],[data-private],[data-control]"),
      ].find(
        (el) =>
          (el.dataset.draft || el.dataset.private || el.dataset.control) ===
          focusKey,
      );
      if (next) {
        next.focus({ preventScroll: true });
        if (selection && typeof next.setSelectionRange === "function") {
          try {
            next.setSelectionRange(...selection);
          } catch (_) {}
        }
      }
    }
    if (S.view.modal && !wasModal && !focusKey)
      app.querySelector('[role="dialog"]')?.focus({ preventScroll: true });
  }
  function projectResult(p) {
    S.project = p;
    S.profile = null;
    const i = S.projects.findIndex((x) => x.id === p.id);
    if (i >= 0) S.projects[i] = p;
  }
  const names = (v) =>
      String(v || "")
        .split(",")
        .map((n) => n.trim())
        .filter(Boolean),
    need = (v, message) => {
      if (!v) throw Error(message);
      return v;
    };
  const actionNames = [
    "auth.login",
    "auth.first-account",
    "auth.logout",
    "auth.resume",
    "project.new",
    "project.create",
    "project.open",
    "project.save",
    "source.new",
    "source.create",
    "source.open",
    "source.save",
    "source.reset_spans",
    "source.remove_confirm",
    "source.remove",
    "build.start",
    "job.cancel",
    "job.dismiss",
    "observation.new",
    "observation.create",
    "observation.edit",
    "observation.save",
    "observation.accept",
    "observation.reject",
    "evidence.open",
    "profile.create",
    "profile.latest",
    "profile.export",
    "profile.download",
    "export.open",
    "export.existing",
    "export.download",
    "agent.open",
    "agent.new",
    "agent.connect",
    "agent.refresh",
    "agent.apply",
    "agent.restore",
    "agent.forget",
    "comparison.open",
    "comparison.close",
    "comparison.run",
    "comparison.rate",
    "comparison.reveal",
    "revision.diff",
    "evaluation.run",
    "evaluation.rate",
    "identity.open",
    "identity.save",
    "view.takeover",
    "tab.select",
    "details.toggle",
    "modal.close",
    "message.dismiss",
    "notice.dismiss",
    "refresh",
    ...Object.keys(screens).map((s) => `screen.${s}`),
  ];
  async function act(action, p = {}) {
    if (!actionNames.includes(action)) throw Error(`Unknown action: ${action}`);
    if (action === "message.dismiss") {
      S.error = "";
      render();
      return;
    }
    if (action === "notice.dismiss") {
      S.notice = "";
      render();
      return;
    }
    if (action === "job.dismiss") {
      S.jobs.delete(p.id);
      render();
      return;
    }
    if (action === "view.takeover") {
      await control({ controller: "human" }, { paint: true });
      return;
    }
    if (action === "auth.resume") {
      await openWorkspace();
      return;
    }
    if (action.startsWith("screen.")) {
      await guarded(() =>
        navigate(
          action.slice(7),
          action === "screen.projects" ? { project_id: null } : {},
        ),
      );
      return;
    }
    await guarded(async () => {
      if (action === "auth.first-account") {
        const password = S.private.password || "";
        need([...password].length >= 12, "Choose a password with at least 12 characters.");
        try {
          await api("auth/first-account", {method:"POST", body:{setup_token:sessionStorage.getItem(setupStorageKey) || "", password, start_at_signin:!!S.private.start_at_signin}});
        } catch (error) {
          S.auth = await api("auth/session");
          if (!S.auth.first_account_needed) {
            sessionStorage.removeItem(setupStorageKey);
            S.private.password = "";
            if (S.auth.authenticated) { await openWorkspace(); return; }
            throw Error("Your account is ready. Sign in with the password you just chose.");
          }
          throw error;
        }
        S.private.password = "";
        sessionStorage.removeItem(setupStorageKey);
        S.auth = await api("auth/session");
        need(S.auth.authenticated, "Your account is ready. Sign in to open your workspace.");
        const startup = await api("api/v1/startup");
        if (startup.requested && startup.configured !== true) S.notice = "Your account is ready, but starting at sign-in was not enabled. Open the app normally to use it.";
        await openWorkspace();
        return;
      }
      if (action === "auth.login") {
        const username =
            p.username ?? S.private.username ?? S.auth?.owner ?? "",
          password = p.password ?? S.private.password ?? "";
        need(username.trim(), "Enter your username.");
        need(password, "Enter your password.");
        await api("auth/login", {
          method: "POST",
          body: { username, password },
        });
        S.private.password = "";
        S.auth = await api("auth/session");
        need(
          S.auth.authenticated,
          "Sign-in succeeded, but the browser could not restore your sign-in session. Check that cookies are allowed for this address, then try again.",
        );
        await openWorkspace();
        return;
      }
      if (action === "auth.logout") {
        await api("auth/logout", { method: "POST", body: {} });
        S.auth = { authenticated: false, owner: S.auth.owner, auth_mode: S.auth.auth_mode };
        S.view = null;
        S.project = null;
        S.private = {};
        S.jobs.clear();
        sessionStorage.removeItem(storageKey);
        return;
      }
      await flush();
      if (action === "project.new")
        return openModal("project.new", {
          draft: {
            project_alias: "",
            project_aliases: "",
            project_purpose: "",
          },
        });
      if (action === "project.create") {
        const project = await api("api/v1/projects", {
          method: "POST",
          body: {
            alias: need(
              String(d().project_alias || "").trim(),
              "Give this project a name.",
            ),
            purpose: d().project_purpose || "",
            aliases: names(d().project_aliases).length
              ? names(d().project_aliases)
              : [String(d().project_alias || "").trim()],
            subject_type: "person",
          },
        });
        S.projects.push(project);
        S.project = project;
        await navigate("evidence", { project_id: project.id, draft: {} });
        notify("Project created. Add a source to begin.");
        return;
      }
      if (action === "project.open") {
        await loadProject(p.id);
        await navigate("evidence", {
          project_id: p.id,
          draft: {},
          filter: "all",
          tab: "communication",
        });
        return;
      }
      if (action === "project.save") {
        const current = S.project;
        projectResult(
          await api(path(), {
            method: "PUT",
            body: {
              expected_revision: current.revision,
              alias: need(
                String(d().settings_alias ?? current.alias).trim(),
                "A project name is required.",
              ),
              purpose: d().settings_purpose ?? current.purpose,
              aliases: names(
                d().settings_aliases ?? list(current.aliases).join(", "),
              ),
            },
          }),
        );
        notify("Project details saved.");
        return;
      }
      if (action === "source.new") {
        S.private.source_file = null;
        S.private.source_text = "";
        S.importCapabilities = await api("api/v1/import-capabilities").catch(() => null);
        return openModal("source.new", {
          draft: {
            ...d(),
            source_name: "",
            source_kind: p.collection === "working" ? "ai_chat" : "meeting",
            source_communication: p.collection !== "working",
            source_working: p.collection === "working",
            source_holdout: false,
            source_options: false,
          },
        });
      }
      if (action === "source.create") {
        const text = p.text ?? S.private.source_text ?? "";
        const uploaded = p.file || S.private.source_file;
        need(uploaded || text.trim(), "Choose a file or paste some source content.");
        if (new TextEncoder().encode(text).length > 2 * 1024 * 1024)
          throw Error(
            "This source is over 2 MB. Split it into smaller text files before adding it.",
          );
        const collections = [
          ...(d().source_communication ? ["communication"] : []),
          ...(d().source_working ? ["working"] : []),
        ];
        need(collections.length, "Choose at least one source collection.");
        const before = new Set(sources().map((a) => a.id));
        projectResult(
          await api(path(uploaded ? "/file-import" : "/artifacts"), {
            method: "POST",
            body: {
              expected_revision: S.project.revision,
              name: need(
                String(d().source_name || "").trim(),
                "Give this source a name.",
              ),
              kind: d().source_kind || "meeting",
              ...(uploaded ? { filename: uploaded.name, content_base64: uploaded.content_base64 } : { text }),
              collections,
              authors:
                d().source_kind === "notes"
                  ? [S.project.aliases?.[0] || S.project.alias]
                  : S.project.aliases || [],
              holdout: !!d().source_holdout,
            },
          }),
        );
        S.private.source_text = "";
        S.private.source_file = null;
        const added = need(
          sources().find((a) => !before.has(a.id)),
          "The source was added. Refresh the project to review it.",
        );
        await openSource(added.id);
        S.notice = "";
        return;
      }
      if (action === "source.open") return openSource(p.id);
      if (action === "source.reset_spans") {
        await control(
          { draft: { ...d(), artifact_span_ids: null } },
          { paint: true },
        );
        return;
      }
      if (action === "source.save") {
        const a = S.artifact,
          authors = [
            ...new Set([
              ...(d().artifact_authors ?? a.authors ?? []),
              ...names(d().artifact_extra_authors),
            ]),
          ],
          collections = [
            ...((d().artifact_communication ??
            list(a.collections).includes("communication"))
              ? ["communication"]
              : []),
            ...((d().artifact_working ??
            list(a.collections).includes("working"))
              ? ["working"]
              : []),
          ];
        need(collections.length, "Choose at least one collection.");
        projectResult(
          await api(path(`/artifacts/${encodeURIComponent(a.id)}`), {
            method: "PUT",
            body: {
              expected_revision: S.project.revision,
              authors,
              collections,
              selected_span_ids: d().artifact_span_ids ?? null,
              holdout: !!(d().artifact_holdout ?? a.holdout),
            },
          }),
        );
        S.artifact = null;
        await control({ modal: null, artifact_id: null }, { paint: true });
        notify("Source choices saved.");
        return;
      }
      if (action === "source.remove_confirm") return openModal("source.remove");
      if (action === "source.remove") {
        projectResult(
          await api(path(`/artifacts/${encodeURIComponent(S.artifact.id)}`), {
            method: "DELETE",
            body: { expected_revision: S.project.revision },
          }),
        );
        S.artifact = null;
        await control({ modal: null, artifact_id: null }, { paint: true });
        notify("Source removed. Dependent versions must be rebuilt.");
        return;
      }
      if (action === "build.start") {
        const j = await api(path("/builds"), {
          method: "POST",
          body: { expected_revision: S.project.revision },
        });
        S.jobs.set(j.id, j);
        S.notice = "";
        return;
      }
      if (action === "job.cancel") {
        const j = await api(`api/v1/jobs/${encodeURIComponent(p.id)}/cancel`, {
          method: "POST",
          body: {},
        });
        S.jobs.set(j.id, j);
        notify("Cancelled.");
        return;
      }
      if (action === "observation.new")
        return openModal("observation.new", {
          draft: {
            ...d(),
            observation_statement: "",
            observation_scope: "",
            observation_section: "",
            observation_details: false,
            manual_collection: S.view.tab || "communication",
          },
        });
      if (action === "observation.edit") {
        const o = need(
          observations().find((x) => x.id === p.id),
          "This observation is no longer available. Refresh the project.",
        );
        return openModal("observation.edit", {
          observation_id: o.id,
          draft: {
            ...d(),
            observation_statement: o.statement,
            observation_scope: o.scope || "",
            observation_section: o.section || "",
            observation_details: false,
          },
        });
      }
      if (action === "observation.create" || action === "observation.save") {
        const create = action === "observation.create";
        projectResult(
          await api(
            path(
              create
                ? "/observations"
                : `/observations/${encodeURIComponent(S.view.observation_id)}`,
            ),
            {
              method: create ? "POST" : "PUT",
              body: {
                expected_revision: S.project.revision,
                statement: need(
                  String(d().observation_statement || "").trim(),
                  "Write an instruction first.",
                ),
                scope:
                  String(d().observation_scope || "").trim() ||
                  "Selected contexts",
                section: d().observation_section || "",
                ...(create
                  ? { collection: d().manual_collection || "communication" }
                  : {}),
              },
            },
          ),
        );
        await control({ modal: null, observation_id: null }, { paint: true });
        notify(create ? "Instruction added." : "Instruction updated.");
        return;
      }
      if (action === "observation.accept" || action === "observation.reject") {
        projectResult(
          await api(path(`/observations/${encodeURIComponent(p.id)}`), {
            method: "PUT",
            body: {
              expected_revision: S.project.revision,
              status: action.endsWith("accept") ? "accepted" : "rejected",
            },
          }),
        );
        return;
      }
      if (action === "evidence.open")
        return openModal("evidence", { draft: { ...d(), evidence_id: p.id } });
      if (action === "profile.latest") {
        return navigate("profiles", {
          profile_revision_id: null,
          draft: { ...d(), profile_pinned: false },
        });
      }
      if (action === "profile.create") {
        const revision = await api(path("/revisions"), {
          method: "POST",
          body: { expected_revision: S.project.revision },
        });
        await loadProject(S.project.id);
        S.profile = revision;
        await navigate("profiles", {
          profile_revision_id: revision.id,
          draft: { ...d(), profile_pinned: false },
        });
        notify("Profiles saved.");
        return;
      }
      if (action === "export.open") {
        need(S.profile && S.profile.valid !== false, "Select a current profile version first.");
        delete S.private.existing_agents;
        await openModal("profile.export", {draft: {...d(), export_format: d().export_format || "agents", export_merge:false}});
        return;
      }
      if (action === "export.existing") {
        need(typeof p.text === "string" && new TextEncoder().encode(p.text).length <= 256 * 1024 && !p.text.includes("\0"), "Choose a UTF-8 AGENTS.md file up to 256 KB.");
        S.private.existing_agents = p.text;
        await control({draft:{...d(),export_format:"agents",export_merge:true}}, {paint:true});
        return;
      }
      if (action === "export.download" && (d().export_format || "agents") !== "native") {
        const r = need(S.profile, "Select a profile version first."), format = d().export_format || "agents";
        need(r.valid !== false, "Create a current profile version before exporting.");
        need(["agents", "soul"].includes(format), "Choose an export format.");
        const merge = format === "agents" && d().export_merge;
        if (merge) need(typeof S.private.existing_agents === "string", "Choose or paste your existing AGENTS.md first.");
        const response = await api(path(`/revisions/${encodeURIComponent(r.id)}/exports/${format}/download`), {
          raw:true, ...(merge ? {method:"POST",body:{existing_agents:S.private.existing_agents}} : {})
        });
        download(await response.blob(), `${safeName(S.project.alias)}-${format}.zip`);
        return;
      }
      if (action === "profile.export" || action === "export.download") {
        const r = need(S.profile, "Select a profile version first.");
        need(
          r.valid !== false,
          "This version is no longer valid. Create a new version before exporting.",
        );
        const response = await api(
          path(`/revisions/${encodeURIComponent(r.id)}/export`),
          { raw: true },
        );
        download(
          await response.blob(),
          `${safeName(S.project.alias)}.twin.json`,
        );
        return;
      }
      if (action === "profile.download") {
        need(
          S.profile && S.profile.valid !== false,
          "Create a new valid profile version before exporting.",
        );
        need(
          ["communication.md", "working-style.md"].includes(p.file),
          "Unknown profile file.",
        );
        download(
          new Blob([S.profile.files[p.file] || ""], {
            type: "text/markdown;charset=utf-8",
          }),
          p.file,
        );
        return;
      }
      if (action === "agent.open") {
        need(S.profile && S.profile.valid !== false, "Select a current profile version first.");
        clearAgentCredentials();
        await loadAgentTargets();
        const selected = S.agentTargets.some((t) => t.id === d().agent_target_id)
          ? d().agent_target_id : S.agentTargets.length === 1 ? S.agentTargets[0].id : "";
        S.agentPreview = null;
        await openModal("agent.apply", { draft: { ...d(), agent_target_id: selected, agent_restore_id: "" } });
        return;
      }
      if (action === "agent.new") {
        clearAgentCredentials();
        await openModal("agent.connect");
        return;
      }
      if (action === "agent.connect") {
        const response = await api("api/v1/agent-targets", {
          method: "POST",
          body: {
            url: String(p.url ?? S.private.agent_url ?? "").trim(),
            token: p.token ?? S.private.agent_token ?? "",
            ca_pem: p.ca_pem ?? S.private.agent_ca ?? "",
          },
        });
        clearAgentCredentials();
        await loadAgentTargets();
        S.agentPreview = response;
        await openModal("agent.apply", { draft: { ...d(), agent_target_id: response.target.id, agent_restore_id: "" } });
        notify(`${response.target.name} connected. Review, then apply your profiles.`);
        return;
      }
      if (action === "agent.refresh") {
        need(d().agent_target_id, "Choose an agent first.");
        await loadAgentStatus();
        return;
      }
      if (action === "agent.apply" || action === "agent.restore") {
        const preview = need(S.agentPreview, "Refresh the agent's status first."),
          targetId = need(d().agent_target_id, "Choose an agent first.");
        need(preview.target.id === targetId, "Refresh the selected agent's status first.");
        const restore = action === "agent.restore";
        if (restore) need(list(preview.installation.previous).some((x) => x.id === d().agent_restore_id), "Select a previous installation.");
        else need(S.profile && S.profile.valid !== false, "Choose a current profile version first.");
        try {
          S.agentPreview = await api(restore
            ? `api/v1/agent-targets/${encodeURIComponent(targetId)}/restore`
            : path(`/revisions/${encodeURIComponent(S.profile.id)}/apply`), {
            method: "POST",
            body: restore
              ? { expected_revision: preview.installation.revision, id: d().agent_restore_id }
              : { expected_revision: preview.installation.revision, target_id: targetId },
          });
        } catch (error) {
          try { await loadAgentStatus(targetId); } catch (_) { S.agentPreview = null; }
          throw new Error(error.message);
        }
        await control({ draft: { ...d(), agent_restore_id: "" } }, { paint: true });
        notify(restore ? "Previous profiles restored. The agent will use them on its next messages." : "Profiles applied. The agent will use them on its next messages.");
        return;
      }
      if (action === "agent.forget") {
        const targetId = need(d().agent_target_id, "Choose an agent first.");
        await api(`api/v1/agent-targets/${encodeURIComponent(targetId)}`, { method: "DELETE" });
        S.agentPreview = null;
        await loadAgentTargets();
        await control({ draft: { ...d(), agent_target_id: "", agent_restore_id: "", agent_connection: false } }, { paint: true });
        notify("Connection forgotten. The agent keeps its installed profiles.");
        return;
      }
      if (action === "comparison.open" || action === "comparison.close") {
        S.revisionDiff = null;
        await navigate("evaluate", {draft:{...d(),compare_blind:action === "comparison.open"}});
        return;
      }
      if (action === "revision.diff") {
        const a = need(d().comparison_revision_a || latest()?.id, "Choose the first version.");
        const b = need(d().comparison_revision_b || latest()?.id, "Choose the second version.");
        await control({draft:{...d(),comparison_revision_a:a,comparison_revision_b:b,revision_diff:true}});
        await resources();
        return;
      }
      if (action === "comparison.run") {
        const a = need(d().comparison_revision_a || latest()?.id, "Create profiles first.");
        const result = await api(path("/comparisons"), {method:"POST",body:{expected_revision:S.project.revision,scenario:need(d().scenario?.trim(),"Describe a new scenario first."),holdout_ids:d().comparison_holdout?[d().comparison_holdout]:[],candidates:[{revision_id:a,variant:d().comparison_variant_a||"both"},{revision_id:d().comparison_revision_b||a,variant:d().comparison_variant_b||"baseline"}]}});
        const job = await api(`api/v1/jobs/${encodeURIComponent(result.job_id)}`);
        S.jobs.set(job.id,job);
        await resources();
        notify("Blind comparison started.");
        return;
      }
      if (action === "comparison.rate" || action === "comparison.reveal") {
        const c = need(S.comparisons.find(x => x.id === p.id), "Refresh the comparisons first.");
        if (action === "comparison.rate") {
          const draft = d()[`comparison_rating_${c.id}`] || {}, ratings = {};
          for (const candidate of c.candidates) {
            const label = candidate.label;
            const old = c.rating?.ratings?.[label] || {};
            ratings[label] = {communication_fit:Number(draft[`${label}_communication_fit`] ?? old.communication_fit), working_fit:Number(draft[`${label}_working_fit`] ?? old.working_fit),notes:draft[`${label}_notes`] ?? old.notes ?? ""};
          }
          await api(path(`/comparisons/${c.id}/rating`),{method:"PUT",body:{ratings,preference:draft.preference ?? c.rating?.preference ?? "",perspective:d().comparison_perspective || "reviewer"}});
          notify("Your feedback was saved. You can now reveal the profiles.");
        } else await api(path(`/comparisons/${c.id}/reveal`),{method:"POST"});
        await resources();
        return;
      }
      if (action === "evaluation.run") {
        const r =
          list(S.project.revisions).find(
            (x) => x.id === S.view.profile_revision_id,
          ) || latest();
        need(
          r && r.valid !== false,
          "Create a current profile version before running an experiment.",
        );
        const scenario = need(
            String(d().scenario || "").trim(),
            "Describe a scenario first.",
          ),
          chosen = d().compare_profiles
            ? Object.keys(variants).filter(
                (k) => d()[`variant_${k}`] ?? k === "both",
              )
            : ["both"];
        need(chosen.length, "Choose at least one profile variant.");
        for (const variant of chosen) {
          const j = await api(path("/evaluations"), {
            method: "POST",
            body: { revision_id: r.id, scenario, variant },
          });
          S.jobs.set(j.id, j);
        }
        notify(
          `${chosen.length} simulation${chosen.length === 1 ? "" : "s"} started.`,
        );
        return;
      }
      if (action === "evaluation.rate") {
        const e = need(
            list(S.project.evaluations).find((x) => x.id === p.id),
            "This simulation is no longer available.",
          ),
          rating = d()[`rating_${e.id}`] || e.rating || {},
          clean = { notes: rating.notes || "" };
        for (const k of ["communication_fit", "working_fit"])
          if (
            rating[k] !== undefined &&
            rating[k] !== null &&
            rating[k] !== ""
          ) {
            const n = Number(rating[k]);
            need(
              Number.isInteger(n) && n >= 1 && n <= 5,
              "Ratings must be between 1 and 5.",
            );
            clean[k] = n;
          }
        projectResult(
          await api(path(`/evaluations/${encodeURIComponent(e.id)}`), {
            method: "PUT",
            body: { expected_revision: S.project.revision, rating: clean },
          }),
        );
        notify("Simulation review saved.");
        return;
      }
      if (action === "identity.open") {
        const identity = await api(path("/identity"));
        S.private.identity_json = JSON.stringify(
          identity.bindings || {},
          null,
          2,
        );
        return openModal("identity");
      }
      if (action === "identity.save") {
        let bindings;
        try {
          bindings = JSON.parse(p.json ?? S.private.identity_json ?? "{}");
        } catch (_) {
          throw Error("Private identity must be valid JSON.");
        }
        need(
          bindings && typeof bindings === "object" && !Array.isArray(bindings),
          "Private identity must be a JSON object.",
        );
        await api(path("/identity"), {
          method: "PUT",
          body: { expected_revision: S.project.revision, bindings },
        });
        S.private.identity_json = "";
        await loadProject(S.project.id);
        await control({ modal: null }, { paint: true });
        notify("Private identity saved separately.");
        return;
      }
      if (action === "tab.select") {
        await control({ tab: p.tab }, { paint: true });
        return;
      }
      if (action === "details.toggle") {
        await control(
          { draft: { ...d(), [p.key]: !d()[p.key] } },
          { paint: true },
        );
        return;
      }
      if (action === "modal.close") {
        delete S.private.existing_agents;
        S.private.source_text = "";
        S.private.identity_json = "";
        clearAgentCredentials();
        await control(
          { modal: null, artifact_id: null, observation_id: null },
          { paint: true },
        );
        return;
      }
      if (action === "refresh") {
        await refresh();
        notify("Workspace refreshed.");
        return;
      }
    });
  }
  function clearAgentCredentials() {
    delete S.private.agent_url;
    delete S.private.agent_token;
    delete S.private.agent_ca;
  }
  async function openSource(id) {
    S.artifact = await api(path(`/artifacts/${encodeURIComponent(id)}`));
    const a = S.artifact;
    await openModal("source", {
      artifact_id: id,
      draft: {
        ...d(),
        artifact_authors: (a.authors || []).filter((name) =>
          list(a.spans).some((span) => span.speaker === name),
        ),
        artifact_extra_authors: "",
        artifact_communication: list(a.collections).includes("communication"),
        artifact_working: list(a.collections).includes("working"),
        artifact_holdout: !!a.holdout,
        artifact_span_ids: a.selected_span_ids ?? null,
        span_selection: false,
        source_options: false,
      },
    });
  }
  function safeName(v) {
    return String(v || "twin")
      .replace(/[^a-zA-Z0-9._-]+/g, "-")
      .slice(0, 80);
  }
  function download(blob, name) {
    const url = URL.createObjectURL(blob),
      a = document.createElement("a");
    a.href = url;
    a.download = name;
    document.body.append(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }
  async function input(event) {
    const el = event.target;
    if (el.dataset.private) {
      if (el.type !== "file") {
        S.private[el.dataset.private] = el.type === "checkbox" ? el.checked : el.value;
        if (el.dataset.private === "source_text") S.private.source_file = null;
      }
      return;
    }
    if (el.dataset.draft) {
      await setDraft(
        el.dataset.draft,
        el.type === "checkbox" ? el.checked : el.value,
      );
      return;
    }
    if (el.dataset.comparison) {
      const key = `comparison_rating_${el.dataset.comparison}`;
      await setDraft(key, {...(d()[key] || {}), [el.dataset.comparisonField]: el.value});
      return;
    }
    if (el.dataset.rating) {
      const id = el.dataset.rating,
        existing =
          d()[`rating_${id}`] ||
          list(S.project.evaluations).find((e) => e.id === id)?.rating ||
          {};
      await setDraft(`rating_${id}`, {
        ...existing,
        [el.dataset.ratingField]: el.value,
      });
    }
  }
  app.addEventListener("input", input);
  app.addEventListener("change", async (event) => {
    const el = event.target;
    try {
      if (el.type === "file" && el.dataset.private === "existing_agents_file") {
        const file = el.files?.[0];
        if (!file) return;
        if (file.size > 256 * 1024) throw Error("AGENTS.md must be 256 KB or smaller.");
        const text = new TextDecoder("utf-8", {fatal:true}).decode(await file.arrayBuffer());
        await act("export.existing", {text});
        return;
      }
      if (el.type === "file" && el.dataset.private === "source_file") {
        const file = el.files?.[0];
        if (!file) return;
        if (file.size > 20 * 1024 * 1024)
          throw Error("This file is over 20 MB. Split it into smaller sources.");
        const bytes = new Uint8Array(await file.arrayBuffer());
        let raw = "";
        for (let i = 0; i < bytes.length; i += 32768) raw += String.fromCharCode(...bytes.subarray(i, i + 32768));
        const plain = /\.(txt|md|markdown)$/i.test(file.name) || (/\.json$/i.test(file.name) && !new TextDecoder().decode(bytes).includes('"segments"'));
        S.private.source_file = plain ? null : {name: file.name, content_base64: btoa(raw)};
        S.private.source_text = plain ? await file.text() : "";
        await setDraft("source_name", file.name);
        await control({ draft: { ...d() } }, { paint: true });
        return;
      }
      if (el.dataset.field) {
        await flush();
        if (el.dataset.field === "profile_revision_id") {
          await control(
            {
              profile_revision_id: el.value,
              draft: { ...d(), profile_pinned: true },
            },
            { paint: true },
          );
          return;
        }
        await control({ [el.dataset.field]: el.value }, { paint: true });
        return;
      }
      if (el.dataset.author) {
        const set = new Set(d().artifact_authors ?? S.artifact.authors ?? []);
        if (el.checked) set.add(el.dataset.author);
        else set.delete(el.dataset.author);
        await setDraft("artifact_authors", [...set]);
        await control({ draft: { ...d() } }, { paint: true });
        return;
      }
      if (el.dataset.span) {
        const fallback = list(S.artifact.spans)
            .filter(
              (s) =>
                !s.quoted &&
                !["assistant", "tool", "system"].includes(s.role) &&
                (d().artifact_authors || []).includes(s.speaker),
            )
            .map((s) => s.id),
          set = new Set(d().artifact_span_ids ?? fallback);
        if (el.checked) set.add(el.dataset.span);
        else set.delete(el.dataset.span);
        await setDraft("artifact_span_ids", [...set]);
        return;
      }
      if (el.dataset.draft === "manual_collection") {
        await control(
          {
            draft: {
              ...d(),
              manual_collection: el.value,
              observation_section: "",
            },
          },
          { paint: true },
        );
        return;
      }
      if (el.dataset.draft === "agent_target_id") {
        S.agentPreview = null;
        await guarded(() => control({ draft: { ...d(), agent_target_id: el.value, agent_restore_id: "" } }, { paint: true }));
        return;
      }
      if (el.dataset.draft === "agent_restore_id") {
        await control({ draft: { ...d(), agent_restore_id: el.value } }, { paint: true });
        return;
      }
      if (["export_format", "export_merge"].includes(el.dataset.draft)) {
        await control({draft:{...d(),[el.dataset.draft]:el.type === "checkbox" ? el.checked : el.value}}, {paint:true});
        return;
      }
      if (["comparison_revision_a", "comparison_revision_b"].includes(el.dataset.draft)) {
        S.revisionDiff = null;
        await control({draft:{...d(),[el.dataset.draft]:el.value,revision_diff:false}},{paint:true});
        return;
      }
      if (el.dataset.draft === "source_kind") {
        const ai = el.value === "ai_chat";
        await control(
          {
            draft: {
              ...d(),
              source_kind: el.value,
              source_communication: !ai,
              source_working: ai,
            },
          },
          { paint: true },
        );
        return;
      }
      if (el.type === "checkbox" || el.tagName === "SELECT") await input(event);
    } catch (e) {
      S.error = e.message;
      render();
    }
  });
  app.addEventListener("click", (event) => {
    const el = event.target.closest("[data-action]");
    if (!el || el.disabled) return;
    event.preventDefault();
    act(el.dataset.action, {
      id: el.dataset.id,
      tab: el.dataset.tab,
      key: el.dataset.key,
      file: el.dataset.file,
      collection: el.dataset.collection,
    }).catch((e) => {
      S.error = e.message;
      render();
    });
  });
  app.addEventListener("submit", (event) => {
    event.preventDefault();
    const action = event.target.dataset.form;
    if (action)
      act(action).catch((e) => {
        S.error = e.message;
        render();
      });
  });
  app.addEventListener("keydown", (event) => {
    if (
      event.target.getAttribute("role") === "tab" &&
      ["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)
    ) {
      event.preventDefault();
      const options = [
        ...event.target
          .closest('[role="tablist"]')
          .querySelectorAll('[role="tab"]'),
      ];
      const current = options.indexOf(event.target);
      const next =
        event.key === "Home"
          ? 0
          : event.key === "End"
            ? options.length - 1
            : (current +
                (event.key === "ArrowRight" ? 1 : -1) +
                options.length) %
              options.length;
      const tab = options[next].dataset.tab;
      act("tab.select", { tab }).then(() =>
        app.querySelector(`[role="tab"][data-tab="${tab}"]`)?.focus(),
      );
      return;
    }
    const dialog = app.querySelector('[role="dialog"]');
    if (!dialog) return;
    if (event.key === "Escape") {
      event.preventDefault();
      act("modal.close");
      return;
    }
    if (event.key === "Tab") {
      const items = [
          ...dialog.querySelectorAll(
            "button:not(:disabled),input:not(:disabled),textarea:not(:disabled),select:not(:disabled),a[href]",
          ),
        ],
        first = items[0],
        last = items.at(-1);
      if (
        event.shiftKey &&
        (document.activeElement === first || document.activeElement === dialog)
      ) {
        event.preventDefault();
        last?.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first?.focus();
      }
    }
  });
  let focusTimer, scrollTimer;
  app.addEventListener("focusin", (event) => {
    const el = event.target,
      target = el.dataset.draft
        ? `draft.${el.dataset.draft}`
        : el.dataset.field
          ? `field.${el.dataset.field}`
          : el.dataset.action
            ? `action.${el.dataset.action}`
            : null;
    if (S.applyingRemote || !target || !S.view || S.view.focus === target)
      return;
    clearTimeout(focusTimer);
    focusTimer = setTimeout(
      () => control({ focus: target }).catch(() => {}),
      350,
    );
  });
  window.addEventListener(
    "scroll",
    () => {
      if (S.applyingRemote || !S.view) return;
      clearTimeout(scrollTimer);
      scrollTimer = setTimeout(
        () =>
          control({
            scroll: {
              x: Math.round(window.scrollX),
              y: Math.round(window.scrollY),
            },
          }).catch(() => {}),
        500,
      );
    },
    { passive: true },
  );
  async function poll() {
    if (
      document.hidden ||
      !S.auth?.authenticated ||
      !S.view ||
      S.pollBusy ||
      S.busy
    )
      return;
    S.pollBusy = true;
    try {
      await S.queued;
      const remote = await api(`api/v1/views/${encodeURIComponent(S.view.id)}`);
      if (remote.revision > S.view.revision) {
        clearTimeout(S.syncTimer);
        clearTimeout(focusTimer);
        clearTimeout(scrollTimer);
        S.applyingRemote = true;
        if (remote.modal !== S.view.modal) {
          S.private.source_text = "";
          S.private.identity_json = "";
          delete S.private.existing_agents;
          clearAgentCredentials();
        }
        S.view = remote;
        await resources();
        render();
        const focus =
          typeof remote.focus === "string"
            ? remote.focus
            : remote.focus?.target;
        if (focus && focus !== S.remoteFocus) {
          S.remoteFocus = focus;
          const [kind, ...rest] = focus.split("."),
            key = rest.join(".");
          if (["draft", "field", "action"].includes(kind)) {
            const el = [...app.querySelectorAll(`[data-${kind}]`)].find(
              (e) => e.dataset[kind] === key,
            );
            el?.focus({ preventScroll: true });
          }
        }
        if (remote.scroll && Number.isFinite(remote.scroll.y))
          window.scrollTo({ top: remote.scroll.y, left: remote.scroll.x || 0 });
        await ack();
        requestAnimationFrame(() => {
          S.applyingRemote = false;
        });
      }
      let changed = false;
      if (S.project && Date.now() - (S.projectRefreshedAt || 0) > 5000) {
        const revision = S.project.revision;
        await loadProject(S.project.id);
        S.projectRefreshedAt = Date.now();
        if (S.project.revision !== revision) {
          S.profile = null;
          changed = true;
        }
        if (S.view.screen === "evaluate" && d().compare_blind) {
          const next = list((await api(path("/comparisons"))).items);
          if (JSON.stringify(next) !== JSON.stringify(S.comparisons)) {
            S.comparisons = next;
            changed = true;
          }
        }
      }
      for (const [id, j] of S.jobs)
        if (!terminal.has(j.status)) {
          const next = await api(`api/v1/jobs/${encodeURIComponent(id)}`);
          S.jobs.set(id, next);
          if (
            next.status !== j.status ||
            next.stage !== j.stage ||
            next.note !== j.note
          )
            changed = true;
          if (terminal.has(next.status)) {
            if (S.project?.id === next.project_id)
              await loadProject(next.project_id);
            if (
              next.status === "complete" &&
              next.kind === "build" &&
              next.result_id &&
              S.project?.id === next.project_id
            ) {
              if (
                ["evidence", "profiles"].includes(S.view.screen) &&
                !S.view.modal
              )
                await navigate("profiles", {
                  profile_revision_id: next.result_id,
                  draft: { ...d(), profile_pinned: false },
                });
              notify("Profiles ready.");
            }
            changed = true;
          }
        }
      if (changed) {
        await resources();
        render();
      }
    } catch (e) {
      S.applyingRemote = false;
      if (e.status !== 401) {
        S.error = `Connection interrupted: ${e.message}`;
        render();
      }
    } finally {
      S.pollBusy = false;
    }
  }
  window.personaWorkshop = window.twinWorkbench = Object.freeze({
    snapshot: () => ({
      authenticated: !!S.auth?.authenticated,
      busy: S.busy,
      view: S.view ? structuredClone(S.view) : null,
      project: S.project
        ? {
            id: S.project.id,
            alias: S.project.alias,
            revision: S.project.revision,
          }
        : null,
      jobs: [...S.jobs.values()].map((j) => ({
        id: j.id,
        status: j.status,
        stage: j.stage,
      })),
      error: S.error,
    }),
    actions: () => [...actionNames],
    act,
    setDraft,
    control: (values) => control(values, { paint: true }),
    importSource: async ({
      name,
      kind = "notes",
      text,
      collections = ["communication"],
      holdout = false,
    }) => {
      await act("source.new");
      await control({
        draft: {
          ...d(),
          source_name: name,
          source_kind: kind,
          source_communication: collections.includes("communication"),
          source_working: collections.includes("working"),
          source_holdout: holdout,
        },
      });
      await act("source.create", { text });
    },
  });
  (async () => {
    try {
      S.auth = await api("auth/session");
      if (!S.auth.first_account_needed) sessionStorage.removeItem(setupStorageKey);
      if (S.auth.authenticated) await openWorkspace();
      else render();
    } catch (e) {
      S.error = e.message;
      S.auth = { authenticated: false };
      render();
    }
    setInterval(poll, 1000);
  })();
})();
