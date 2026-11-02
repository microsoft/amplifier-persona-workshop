import http from "node:http";
import fs from "node:fs/promises";
import path from "node:path";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
const { chromium } = await import(
  process.env.PLAYWRIGHT_MODULE || "playwright"
);
const ui = path.resolve(path.dirname(fileURLToPath(import.meta.url)), ".."),
  output = process.env.UI_ARTIFACTS || "/tmp/persona-workshop-ui";
await fs.mkdir(output, { recursive: true });
const prefix = "/embedded/twin/",
  views = new Map(),
  projects = new Map(),
  artifacts = new Map(),
  jobs = new Map(),
  viewBodies = [],
  identity = new Map();
const agentTargets = new Map(),
  agentTargetId = "79037bb7-fdde-4773-9222-6972fa6c5533",
  baselineId = "b1e8d7bc-52a0-4a65-8254-c041b8ff3841";
let agentInstallation = { revision: 0, active: null, previous: [] };
let cookieEnabled = false,
  loggedIn = false,
  serial = 0,
  forcedConflict = false;
const newId = (k) => `${k}-${++serial}`,
  full = (p) => ({
    ...p,
    artifacts: p.artifacts.map((a) => {
      const { spans, text, ...summary } = a;
      return summary;
    }),
  }),
  change = (p) => {
    p.revision++;
    p.updated_at = new Date().toISOString();
  };
const compile = (p) => {
  const r = {
    id: newId("revision"),
    project_id: p.id,
    created_at: new Date().toISOString(),
    status: "draft",
    valid: true,
    manifest: {
      uncalibrated: true,
      identity_included: false,
      evidence_included: false,
    },
    files: {
      "communication.md":
        "## Voice\n\nStart with a concrete example from the shared task before generalizing. Keep the opening brief, then build the explanation in connected paragraphs that give people enough detail to act.\n\n## Phrasing\n\nUse direct, conversational language. Ask a specific question when something is unclear, and explain what the answer will change. Avoid praise or ceremony that delays the useful point.\n\n## Context\n\nWhen a teammate is exploring, leave room for alternatives. When they need a decision, make the recommendation explicit and connect it to the agreed goal.",
      "working-style.md":
        "## Decisions\n\nPropose a reversible experiment when the team is uncertain. State what it would test, what would change your mind, and what you will do after the result.\n\n## Collaboration\n\nKeep ownership clear while leaving room for peers to challenge the approach. Surface a disagreement early and resolve it with a concrete example, rather than turning the discussion into a debate about abstractions.",
    },
  };
  p.revisions.push(r);
  return r;
};
const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, "http://localhost"),
    send = (data, status = 200, headers = {}) => {
      res.writeHead(status, {
        "Content-Type": "application/json",
        "Cache-Control": "no-store",
        ...headers,
      });
      res.end(JSON.stringify(data));
    };
  try {
    if (!url.pathname.startsWith(prefix))
      return send({ error: "Incorrect mount prefix" }, 404);
    const ep = url.pathname.slice(prefix.length);
    if (["", "index.html", "app.js", "app.css", "markdown.js"].includes(ep)) {
      const name = ep || "index.html";
      res.writeHead(200, {
        "Content-Type": name.endsWith(".js")
          ? "text/javascript"
          : name.endsWith(".css")
            ? "text/css"
            : "text/html",
      });
      let content=await fs.readFile(path.join(ui, name),'utf8');
      if(name==='markdown.js')content+=await fs.readFile(path.join(ui,'markdown-renderer.js'),'utf8');
      if(name==='app.css')content+='\n'+await fs.readFile(path.join(ui,'markdown.css'),'utf8');
      res.end(content);
      return;
    }
    let raw = "";
    for await (const chunk of req) raw += chunk;
    const data = raw ? JSON.parse(raw) : {},
      authenticated =
        loggedIn && (req.headers.cookie || "").includes("tw_fixture=session");
    if (ep === "auth/session")
      return send({
        authenticated,
        owner: "fixture-owner",
        csrf: authenticated ? "fixture-csrf" : null,
      });
    if (ep === "auth/login") {
      assert.equal(data.username, "fixture-owner");
      assert.equal(data.password, "synthetic-password");
      loggedIn = true;
      return send(
        { authenticated: true, csrf: "fixture-csrf" },
        200,
        cookieEnabled
          ? {
              "Set-Cookie": `tw_fixture=session; Path=${prefix}; HttpOnly; SameSite=Strict`,
            }
          : {},
      );
    }
    if (!authenticated) return send({ error: "Sign in" }, 401);
    if (req.method !== "GET" && req.headers["x-csrf-token"] !== "fixture-csrf")
      return send({ error: "CSRF missing" }, 403);
    if (ep === "auth/logout") {
      loggedIn = false;
      return send({ ok: true });
    }
    if (ep === "api/v1/status")
      return send({
        extractor_ready: true,
        extractor: { ready: true, note: "Synthetic fixture extractor" },
      });
    if (ep === "api/v1/activity")
      return send({
        items: [
          {
            kind: "source.added",
            time: Date.now(),
            data: { note: "Source added" },
          },
          { kind: "view.ack", time: Date.now() },
        ],
      });
    if (ep === "api/v1/descriptor")
      return send({ name: "Persona Workshop", version: "fixture" });
    if (ep === "api/v1/agent-targets") {
      if (req.method === "GET") return send({ items: [...agentTargets.values()] });
      assert.equal(data.token, "PRIVATE_AGENT_CREDENTIAL");
      const target = { id: agentTargetId, name: "Alex (simulation)", agent_id: "fixture-agent", url: data.url };
      agentTargets.set(target.id, target);
      return send({ target, installation: agentInstallation });
    }
    if (ep.startsWith("api/v1/agent-targets/")) {
      const [id, action] = ep.slice("api/v1/agent-targets/".length).split("/");
      const target = agentTargets.get(id);
      if (!target) return send({error:"Connection missing"},404);
      if (req.method === "DELETE") { agentTargets.delete(id); return send({ok:true}); }
      if (action === "restore") {
        if (data.expected_revision !== agentInstallation.revision) return send({error:"The agent's installed profile changed. Refresh its status and review before applying again."},409);
        const next = agentInstallation.previous.find(x => x.id === data.id);
        assert(next);
        agentInstallation = { revision: agentInstallation.revision + 1, active: next, previous: [agentInstallation.active, ...agentInstallation.previous.filter(x=>x.id!==data.id)].filter(Boolean) };
      }
      return send({ target, installation: agentInstallation });
    }
    if (ep === "api/v1/views") {
      const v = {
        id: newId("view"),
        revision: 1,
        screen: "projects",
        project_id: null,
        artifact_id: null,
        observation_id: null,
        profile_revision_id: null,
        tab: "communication",
        filter: "all",
        modal: null,
        draft: {},
        focus: null,
        scroll: null,
        controller: "human",
        rendered_revision: 0,
      };
      views.set(v.id, v);
      return send(v);
    }
    if (ep.startsWith("api/v1/views/")) {
      const [, id, action] = ep.slice("api/v1/".length).split("/"),
        v = views.get(id);
      if (!v) return send({ error: "View missing" }, 404);
      if (action === "commands") {
        viewBodies.push(data);
        if (data.expected_revision !== v.revision)
          return send({ error: "View changed" }, 409);
        for (const c of data.commands) {
          assert.equal(c.op, "set");
          assert(
            !["source_text", "identity_json", "password"].some((k) =>
              JSON.stringify(c).includes(`"${k}"`),
            ),
          );
          v[c.field] = c.value;
        }
        v.controller = data.controller;
        v.revision++;
      }
      if (action === "ack") v.rendered_revision = data.revision;
      return send(action === "ack" ? { ok: true } : v);
    }
    if (ep === "api/v1/projects") {
      if (req.method === "GET")
        return send({ items: [...projects.values()].map(full) });
      const p = {
        id: newId("project"),
        revision: 1,
        alias: data.alias,
        purpose: data.purpose,
        aliases: data.aliases,
        subject_type: data.subject_type,
        updated_at: new Date().toISOString(),
        artifacts: [],
        observations: [],
        builds: [],
        revisions: [],
        evaluations: [],
        identity_present: false,
        stats: {},
      };
      projects.set(p.id, p);
      return send(full(p));
    }
    if (ep.startsWith("api/v1/jobs/")) {
      const [, id, action] = ep.slice("api/v1/".length).split("/"),
        j = jobs.get(id);
      if (action === "cancel") {
        j.status = "cancelled";
        j.note = "Cancelled before extraction completed";
        return send(j);
      }
      if (!["complete", "cancelled"].includes(j.status) && ++j.polls >= 2) {
        const p = projects.get(j.project_id);
        j.status = "complete";
        j.stage = "Complete";
        if (j.kind === "build") {
          p.observations = [
            {
              id: "obs-communication",
              collection: "communication",
              section: "voice",
              statement: "Uses concrete examples before broad advice.",
              scope: "In the selected product discussion",
              confidence: {
                attribution: "high",
                support: "moderate",
                generalization: "low",
                reason: "One selected conversation.",
              },
              evidence_ids: [p.artifacts[0].spans[0].id],
              conflicts: [],
              status: "pending",
              source: "inferred",
              valid: true,
            },
            {
              id: "obs-working",
              collection: "working",
              section: "decisions",
              statement: "Asks for a reversible experiment.",
              scope: "When uncertainty blocks a decision",
              confidence: {
                attribution: "high",
                support: "moderate",
                generalization: "low",
              },
              evidence_ids: [p.artifacts[0].spans[0].id],
              conflicts: [],
              status: "pending",
              source: "inferred",
              valid: true,
            },
          ];
          j.result_id = compile(p).id;
        } else
          p.evaluations.push({
            id: newId("evaluation"),
            revision_id: j.revision_id,
            variant: j.variant,
            scenario: j.scenario,
            response:
              "Let’s try the smallest reversible test, then inspect what changed. I would first isolate the assumption that could change our release decision. Once that test finishes, we can decide whether the remaining uncertainty is acceptable.",
            created_at: new Date().toISOString(),
            rating: null,
          });
        change(p);
      }
      return send(j);
    }
    if (ep.startsWith("api/v1/projects/")) {
      const [pid, kind, id, action] = ep
          .slice("api/v1/projects/".length)
          .split("/"),
        p = projects.get(pid);
      if (!p) return send({ error: "Project missing" }, 404);
      if (req.method !== "GET" && kind !== "evaluations" && action !== "apply") {
        if (data.expected_revision !== p.revision || forcedConflict) {
          forcedConflict = false;
          change(p);
          return send({ error: "Project changed in another window" }, 409);
        }
      }
      if (!kind) {
        if (req.method === "PUT") {
          Object.assign(
            p,
            Object.fromEntries(
              Object.entries(data).filter(([k]) => k !== "expected_revision"),
            ),
          );
          change(p);
        }
        return send(full(p));
      }
      if (kind === "artifacts") {
        if (!id) {
          const aid = newId("source"),
            a = {
              id: aid,
              name: data.name,
              kind: data.kind,
              text: data.text,
              sha256: "synthetic-hash",
              collections: data.collections,
              authors: data.authors,
              speakers: ["Alex", "Teammate"],
              span_count: 4,
              selected_count: 1,
              holdout: data.holdout,
              deleted: false,
              selected_span_ids: null,
              spans: [
                {
                  id: `${aid}-1`,
                  speaker: "Alex",
                  role: "user",
                  text: data.text,
                  line_start: 1,
                  line_end: 2,
                  quoted: false,
                },
                {
                  id: `${aid}-2`,
                  speaker: "Teammate",
                  role: "user",
                  text: "A different participant speaking.",
                  line_start: 3,
                  line_end: 3,
                  quoted: false,
                },
                {
                  id: `${aid}-3`,
                  speaker: "Alex",
                  role: "assistant",
                  text: "Assistant output, not Alex evidence.",
                  line_start: 4,
                  line_end: 4,
                  quoted: false,
                },
                {
                  id: `${aid}-4`,
                  speaker: "Alex",
                  role: "user",
                  text: "A quoted statement from someone else.",
                  line_start: 5,
                  line_end: 5,
                  quoted: true,
                },
              ],
            };
          if (data.kind === "profile") {
            a.authors = ["Reference profile"];
            a.speakers = ["Reference profile"];
            a.span_count = 1;
            a.spans = [
              {
                id: `${aid}-1`,
                speaker: "Reference profile",
                role: "reference",
                text: data.text,
                line_start: 1,
                line_end: 1,
                quoted: false,
              },
            ];
          }
          p.artifacts.push(a);
          artifacts.set(aid, a);
          change(p);
          return send(full(p));
        }
        const a = artifacts.get(id);
        if (req.method === "GET") return send(a);
        if (req.method === "PUT") {
          Object.assign(
            a,
            Object.fromEntries(
              Object.entries(data).filter(([k]) => k !== "expected_revision"),
            ),
          );
          a.selected_count =
            a.authors.includes("Alex") ||
            a.authors.includes("Reference profile")
              ? 1
              : 0;
        }
        if (req.method === "DELETE") {
          a.deleted = true;
          p.revisions.forEach((r) => (r.valid = false));
          p.observations.forEach((o) => (o.valid = false));
        }
        change(p);
        return send(full(p));
      }
      if (kind === "builds") {
        const j = {
          id: newId("job"),
          project_id: p.id,
          status: "running",
          stage: "Reading selected evidence",
          note: "Synthetic extraction in progress",
          created_at: new Date().toISOString(),
          kind: "build",
          polls: 0,
        };
        jobs.set(j.id, j);
        p.builds.push(j);
        return send(j);
      }
      if (kind === "observations") {
        if (!id)
          p.observations.push({
            id: newId("observation"),
            collection: data.collection,
            statement: data.statement,
            scope: data.scope,
            confidence: {},
            evidence_ids: [],
            conflicts: [],
            source: "owner",
            status: "accepted",
            valid: true,
          });
        else
          Object.assign(
            p.observations.find((o) => o.id === id),
            Object.fromEntries(
              Object.entries(data).filter(([k]) => k !== "expected_revision"),
            ),
          );
        change(p);
        return send(full(p));
      }
      if (kind === "evidence") {
        const a = p.artifacts.find((a) => a.spans.some((s) => s.id === id));
        return send({
          artifact: { id: a.id, name: a.name, kind: a.kind },
          span: a.spans.find((s) => s.id === id),
        });
      }
      if (kind === "revisions") {
        if (!id) {
          const r = compile(p);
          change(p);
          return send(r);
        }
        const r = p.revisions.find((r) => r.id === id);
        if (action === "apply") {
          if (data.expected_revision !== agentInstallation.revision) return send({error:"The agent's installed profile changed. Refresh its status and review before applying again."},409);
          assert(r.valid);
          agentInstallation = { revision: agentInstallation.revision + 1,
            active: { id: "a72917f3-c846-4efe-83a5-13501de0df59", source:{subject_key:p.id,revision:r.id} },
            previous:[agentInstallation.active || {id:baselineId,kind:"baseline",source:null}, ...agentInstallation.previous] };
          return send({ target: agentTargets.get(data.target_id), installation: agentInstallation });
        }
        if (action === "export") {
          if (!r.valid)
            return send({ error: "Stale profile cannot be exported" }, 409);
          return send({ format: "twin", manifest: r.manifest, files: r.files });
        }
        if (!r.valid)
          return send(
            { error: "This revision is unavailable after source changes" },
            409,
          );
        return send(r);
      }
      if (kind === "evaluations") {
        if (!id) {
          const j = {
            id: newId("job"),
            project_id: p.id,
            revision_id: data.revision_id,
            scenario: data.scenario,
            variant: data.variant,
            status: "running",
            stage: "Trying scenario",
            note: "Synthetic simulation in progress",
            kind: "evaluation",
            polls: 0,
          };
          jobs.set(j.id, j);
          return send(j);
        }
        if (data.expected_revision !== p.revision)
          return send({ error: "Project changed" }, 409);
        p.evaluations.find((e) => e.id === id).rating = data.rating;
        change(p);
        return send(full(p));
      }
      if (kind === "identity") {
        if (req.method === "PUT") {
          identity.set(p.id, data.bindings);
          p.identity_present = Object.keys(data.bindings).length > 0;
          change(p);
        }
        return send({ bindings: identity.get(p.id) || {} });
      }
    }
    return send({ error: `Unknown fixture route: ${ep}` }, 404);
  } catch (e) {
    process.stderr.write(`FIXTURE ${e.stack}\n`);
    send({ error: e.message }, 500);
  }
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const origin = `http://127.0.0.1:${server.address().port}`;
const browser = await chromium.launch({
  headless: true,
  ...(process.env.CHROMIUM_EXECUTABLE
    ? { executablePath: process.env.CHROMIUM_EXECUTABLE }
    : {}),
  args: ["--no-sandbox"],
});
const context = await browser.newContext({
    viewport: { width: 1440, height: 1050 },
  }),
  page = await context.newPage(),
  errors = [];
page.on("pageerror", (e) => errors.push(e.message));
let passed = 0;
const ok = (m) => {
    passed++;
    console.log(`PASS ${m}`);
  },
  idle = () =>
    page.waitForFunction(() => window.personaWorkshop?.snapshot().busy === false),
  click = async (action) => {
    await page.locator(`[data-action="${action}"]`).first().click();
    await idle();
  },
  draft = async (k, v) => page.locator(`[data-draft="${k}"]`).fill(v);
try {
  await page.goto(origin + prefix);
  await page.locator('[data-private="password"]').waitFor();
  assert.equal(
    await page.locator('[data-private="username"]').inputValue(),
    "fixture-owner",
  );
  await page.locator('[data-private="password"]').fill("synthetic-password");
  await page.locator('button[type="submit"]').click();
  await page
    .locator(".banner.error")
    .filter({ hasText: /could not restore your sign-in session/ })
    .waitFor();
  await idle();
  ok("Missing session cookie produces an actionable sign-in error");
  cookieEnabled = true;
  await page.locator('[data-private="password"]').fill("synthetic-password");
  await page.locator('button[type="submit"]').click();
  await page.getByRole("heading", { name: "Projects" }).waitFor();
  await idle();
  await page.screenshot({
    path: path.join(output, "projects.png"),
    fullPage: true,
  });
  ok("PAM flow opens a private workspace beneath an embedded URL prefix");
  await click("project.new");
  await draft("project_alias", "Alex · product collaboration");
  await draft("project_aliases", "Alex");
  await draft("project_purpose", "Synthetic product discussion experiments");
  await click("project.create");
  await page.getByRole("heading", { name: "Sources" }).waitFor();
  assert.equal(projects.size, 1);
  ok("Project creation leads directly to two evidence collections");
  await click("source.new");
  const raw =
    "Alex: Try a reversible experiment before committing.\n<script>window.injected=true</script>\nPRIVATE_SYNTHETIC_RAW";
  await page.locator('[data-private="source_file"]').setInputFiles({
    name: "Synthetic planning.md",
    mimeType: "text/markdown",
    buffer: Buffer.from(raw),
  });
  await page.waitForFunction(() =>
    document
      .querySelector('[data-private="source_text"]')
      ?.value.includes("PRIVATE_SYNTHETIC_RAW"),
  );
  await page.locator('[data-draft="source_working"]').check();
  await click("source.create");
  await page
    .getByRole("heading", { name: "Synthetic planning.md", exact: true })
    .waitFor();
  assert.equal(await page.evaluate(() => window.injected), undefined);
  assert.equal(await page.locator(".span-item.excluded").count(), 3);
  await page.locator('[data-author="Alex"]').uncheck();
  await page.waitForFunction(
    () => document.querySelectorAll(".span-item.excluded").length === 4,
  );
  await page.locator('[data-author="Alex"]').check();
  await click("source.save");
  assert.deepEqual([...projects.values()][0].artifacts[0].collections, [
    "communication",
    "working",
  ]);
  ok(
    "Import is text-safe; reuse and exact speaker choices preserve default exclusion of quoted and assistant content",
  );
  await click("source.new");
  await page.locator('[data-draft="source_kind"]').selectOption("ai_chat");
  assert.equal(
    await page.locator('[data-draft="source_working"]').isChecked(),
    true,
  );
  assert.equal(
    await page.locator('[data-draft="source_communication"]').isChecked(),
    false,
  );
  await draft("source_name", "Held back AI discussion");
  await page
    .locator('[data-private="source_text"]')
    .fill("Alex: What would we learn from the smallest test?");
  await page.locator('[data-key="source_options"]').click();
  await idle();
  await page.locator('[data-draft="source_holdout"]').check();
  await click("source.create");
  await click("source.save");
  assert.equal([...projects.values()][0].artifacts[1].holdout, true);
  ok("AI chats default to working style; held-back evidence is explicit");
  await page.screenshot({
    path: path.join(output, "sources.png"),
    fullPage: true,
  });
  await click("build.start");
  await page.locator(".job").waitFor();
  await page.waitForFunction(
    () =>
      window.personaWorkshop.snapshot().jobs.some((j) => j.status === "complete"),
    null,
    { timeout: 12000 },
  );
  await page.getByRole("heading", { name: "Profiles", exact: true }).waitFor();
  await page.locator(".profile-document").waitFor();
  assert.equal(
    await page.locator('nav[aria-label="Project"] button').count(),
    3,
  );
  assert.equal(await page.locator('[data-action="profile.export"]').count(), 0);
  assert.equal(
    await page.locator('[data-action="profile.download"]').count(),
    1,
  );
  assert.equal(await page.locator(".profile-document h2").count(), 3);
  await page.screenshot({
    path: path.join(output, "profiles.png"),
    fullPage: true,
  });
  await page.locator('[data-key="profile_details"]').click();
  await idle();
  await click("screen.review");
  await page.locator(".observation").waitFor();
  assert.equal(await page.locator('[data-action="evidence.open"]').count(), 0);
  await page.locator('[data-key="review_details"]').click();
  await idle();
  await page.screenshot({
    path: path.join(output, "review.png"),
    fullPage: true,
  });
  await click("evidence.open");
  await page.locator('[role="dialog"] .span-text').waitFor();
  assert(
    (await page.locator('[role="dialog"] .span-text').innerText()).includes(
      "PRIVATE_SYNTHETIC_RAW",
    ),
  );
  await click("modal.close");
  await click("observation.edit");
  await draft(
    "observation_statement",
    "Prefers a concrete example in this product discussion.",
  );
  await click("observation.save");
  await click("observation.accept");
  assert.equal([...projects.values()][0].observations[0].status, "accepted");
  ok(
    "Async builds, linked evidence, edits, and acceptance complete the review workflow",
  );
  await click("observation.new");
  await draft(
    "observation_statement",
    "Ask for clarification when the evidence is missing.",
  );
  assert.equal(
    await page.locator('[data-draft="observation_scope"]').count(),
    0,
  );
  await click("observation.create");
  assert(
    [...projects.values()][0].observations.some((o) => o.source === "owner"),
  );
  ok("Owner instructions remain distinct from inferred observations");
  await click("profile.create");
  await page.getByRole("heading", { name: "Profiles" }).waitFor();
  const download = page.waitForEvent("download");
  await click("profile.export");
  const file = await download,
    contents = JSON.parse(await fs.readFile(await file.path(), "utf8"));
  assert(contents.files["communication.md"]);
  assert(contents.files["working-style.md"]);
  assert(!JSON.stringify(contents).includes("PRIVATE_SYNTHETIC_RAW"));
  await page.locator('[data-action="tab.select"][data-tab="working"]').click();
  await idle();
  assert(
    (await page.locator(".profile-document").innerText()).includes(
      "reversible experiment",
    ),
  );
  await page.screenshot({
    path: path.join(output, "profile-details.png"),
    fullPage: true,
  });
  ok("Portable export preserves two profile files while omitting raw evidence");
  assert.equal(await page.title(), "Persona Workshop");
  assert.equal(await page.evaluate(() => window.personaWorkshop === window.twinWorkbench), true);
  await click("agent.open");
  await page.getByRole("heading", { name: "Use on agent", exact: true }).waitFor();
  await click("agent.new");
  await page.locator('[data-private="agent_url"]').fill("https://agent-fixture.test:18449");
  await page.locator('[data-private="agent_token"]').fill("PRIVATE_AGENT_CREDENTIAL");
  await click("agent.connect");
  await page.getByText("No package installed", {exact:true}).waitFor();
  assert.equal(agentInstallation.revision, 0, "Connecting does not apply profiles");
  assert(!JSON.stringify(viewBodies).includes("PRIVATE_AGENT_CREDENTIAL"));
  await page.screenshot({ path:path.join(output,"use-on-agent-preview.png"),fullPage:true });
  await click("agent.apply");
  await page.getByText("This version is installed", {exact:true}).waitFor();
  assert.equal(agentInstallation.revision, 1);
  assert.equal(await page.locator('[data-action="agent.apply"]').isDisabled(),true);
  await page.locator('[data-key="agent_history"]').click();
  await idle();
  await page.locator('[data-draft="agent_restore_id"]').selectOption(baselineId);
  await page.waitForFunction(()=>!document.querySelector('[data-action="agent.restore"]')?.disabled);
  await click("agent.restore");
  await page.getByText("Original agent profiles",{exact:true}).first().waitFor();
  assert.equal(agentInstallation.revision,2);
  assert.equal(agentInstallation.active.kind,"baseline");
  // A separate management client changes the generation after the preview.
  agentInstallation.revision++;
  await click("agent.apply");
  await page.locator('[role="dialog"] .banner.error').filter({hasText:/installed profile changed/}).waitFor();
  assert.equal(agentInstallation.revision,3);
  await page.evaluate(() => window.personaWorkshop.act("message.dismiss"));
  await idle();
  await click("agent.apply");
  await page.getByText("This version is installed",{exact:true}).waitFor();
  await page.setViewportSize({width:390,height:844});
  await page.screenshot({ path:path.join(output,"use-on-agent-390.png"),fullPage:true });
  assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
  await page.setViewportSize({width:1440,height:1050});
  await click("modal.close");
  await click("agent.open");
  await page.getByText("This version is installed",{exact:true}).waitFor();
  assert.equal(await page.locator('[data-private="agent_token"]').count(),0);
  await page.locator('[data-key="agent_connection"]').click();
  await idle();
  await click("agent.forget");
  assert.equal(agentTargets.size,0);
  assert.equal(agentInstallation.revision,4,"Forgetting the connection leaves the agent alone");
  await click("modal.close");
  ok("Persona Workshop connects once, previews and applies saved profiles, restores history, handles conflicts and retains no credentials in shared views");
  const historyProject = [...projects.values()][0],
    oldest = historyProject.revisions[0],
    newest = historyProject.revisions.at(-1);
  oldest.valid = false;
  change(historyProject);
  await page.evaluate(() => window.personaWorkshop.act("refresh"));
  await idle();
  await page
    .locator('[data-field="profile_revision_id"]')
    .selectOption(oldest.id);
  await idle();
  await page
    .getByText("This version is unavailable.", { exact: false })
    .waitFor();
  assert.equal(
    await page.locator('[data-action="profile.download"]').isDisabled(),
    true,
  );
  assert.equal(
    await page.evaluate(
      () => window.personaWorkshop.snapshot().view.draft.profile_pinned,
    ),
    true,
  );
  await page.reload();
  await page.waitForFunction(
    () => window.personaWorkshop?.snapshot().view?.draft.profile_pinned,
  );
  assert.equal(
    await page.locator('[data-field="profile_revision_id"]').inputValue(),
    oldest.id,
  );
  assert.equal(
    await page.evaluate(() => window.personaWorkshop.snapshot().error),
    "",
  );
  await click("profile.latest");
  assert.equal(
    await page.locator('[data-field="profile_revision_id"]').inputValue(),
    newest.id,
  );
  const originalText = newest.files["working-style.md"];
  newest.files["working-style.md"] = "# Working style\n";
  await page.reload();
  await page.locator(".empty-profile").waitFor();
  assert(
    (await page.locator(".empty-profile").innerText()).includes(
      "No instructions yet",
    ),
  );
  newest.files["working-style.md"] = originalText;
  await page.reload();
  await page.locator(".profile-document h2").first().waitFor();
  ok(
    "Unavailable pinned history is explicit and safe; latest selection and empty-file explanations remain useful",
  );

  await click("screen.settings");
  assert.equal(await page.getByText("View ack", { exact: true }).count(), 0);
  await click("identity.open");
  await page
    .locator('[data-private="identity_json"]')
    .fill('{"account":"PRIVATE_SYNTHETIC_IDENTITY"}');
  await click("identity.save");
  assert.equal([...identity.values()][0].account, "PRIVATE_SYNTHETIC_IDENTITY");
  assert(!JSON.stringify(viewBodies).includes("PRIVATE_SYNTHETIC_IDENTITY"));
  assert(!JSON.stringify(viewBodies).includes("PRIVATE_SYNTHETIC_RAW"));
  assert(!JSON.stringify(viewBodies).includes("synthetic-password"));
  ok(
    "Private identity, passwords, and raw artifacts never enter shared display state",
  );
  await click("screen.evaluate");
  await draft(
    "scenario",
    "A teammate wants to ship before an uncertain test finishes. What do you suggest?",
  );
  assert.equal(
    await page.locator('[data-draft="variant_baseline"]').count(),
    0,
  );
  await click("evaluation.run");
  await page.waitForFunction(
    () => document.querySelectorAll(".evaluation").length === 1,
    null,
    { timeout: 15000 },
  );
  assert.equal([...projects.values()][0].evaluations[0].variant, "both");
  assert.equal(
    await page.locator('[data-action="evaluation.rate"]').count(),
    0,
  );
  await page.screenshot({
    path: path.join(output, "try-it.png"),
    fullPage: true,
  });
  await page.locator('[data-key="compare_profiles"]').click();
  await idle();
  await page.locator('[data-draft="variant_both"]').uncheck();
  await page.locator('[data-draft="variant_baseline"]').check();
  await click("evaluation.run");
  await page.waitForFunction(
    () => document.querySelectorAll(".evaluation").length === 2,
    null,
    { timeout: 15000 },
  );
  await page.locator('[data-key="evaluation_details"]').click();
  await idle();
  const eid = [...projects.values()][0].evaluations[0].id;
  await page
    .locator(`[data-rating="${eid}"][data-rating-field="communication_fit"]`)
    .selectOption("4");
  await page
    .locator(`[data-rating="${eid}"][data-rating-field="working_fit"]`)
    .selectOption("3");
  await page
    .locator(`[data-rating="${eid}"][data-rating-field="notes"]`)
    .fill("Useful concrete proposal; too formal for this scenario.");
  await page
    .locator(`[data-action="evaluation.rate"][data-id="${eid}"]`)
    .click();
  await idle();
  assert.equal(
    [...projects.values()][0].evaluations[0].rating.communication_fit,
    4,
  );
  await page.screenshot({
    path: path.join(output, "experiments.png"),
    fullPage: true,
  });
  ok(
    "Separate simulation variants receive distinct communication and working-style judgments",
  );
  const evaluatedRevision = [...projects.values()][0].evaluations[0]
    .revision_id;
  await page.evaluate(() => window.personaWorkshop.act("profile.create"));
  await idle();
  await click("screen.evaluate");
  assert.equal(await page.locator(".evaluation").count(), 0);
  assert.equal([...projects.values()][0].evaluations.length, 2);
  await click("screen.profiles");
  await page
    .locator('[data-field="profile_revision_id"]')
    .selectOption(evaluatedRevision);
  await idle();
  await click("screen.evaluate");
  assert.equal(await page.locator(".evaluation").count(), 2);
  ok(
    "Responses are scoped to the selected profile version while historical results remain available",
  );
  await page.waitForTimeout(1200);
  const v = [...views.values()][0];
  v.screen = "evaluate";
  v.controller = "voice-agent";
  v.draft = {
    ...v.draft,
    scenario: "A remotely guided scenario",
    compare_profiles: false,
    evaluation_details: false,
  };
  v.focus = "draft.scenario";
  v.scroll = { x: 0, y: 0 };
  v.revision++;
  await page
    .waitForFunction(
      () =>
        document.querySelector('[data-draft="scenario"]')?.value ===
        "A remotely guided scenario",
      null,
      { timeout: 5000 },
    )
    .catch(async (e) => {
      console.log(
        "REMOTE DEBUG",
        JSON.stringify({
          server: v,
          browser: await page.evaluate(() => window.personaWorkshop.snapshot()),
          hidden: await page.evaluate(() => document.hidden),
        }),
      );
      throw e;
    });
  assert.equal(
    await page
      .locator('[data-draft="scenario"]')
      .evaluate((e) => e === document.activeElement),
    true,
  );
  assert.equal(
    await page.locator('[data-draft="variant_baseline"]').count(),
    0,
  );
  assert.equal(
    await page.locator('[data-action="evaluation.rate"]').count(),
    0,
  );
  await page.waitForTimeout(700);
  assert.equal(v.controller, "voice-agent");
  await page.locator('[data-draft="scenario"]').fill("A human refinement");
  await page.waitForTimeout(800);
  assert.equal(v.controller, "human");
  assert.equal(v.draft.scenario, "A human refinement");
  assert(v.rendered_revision > 0);
  ok("Remote draft/focus changes do not steal control; human input takes over");
  await page.reload();
  await page.waitForFunction(
    () => window.personaWorkshop?.snapshot().view?.screen === "evaluate",
  );
  assert.equal(
    await page.locator('[data-draft="scenario"]').inputValue(),
    "A human refinement",
  );
  ok("A reattached display restores its public unsent draft");
  await click("screen.settings");
  await draft("settings_purpose", "A changed purpose");
  forcedConflict = true;
  await click("project.save");
  await page
    .locator(".banner.error")
    .filter({ hasText: /latest project has been reloaded/ })
    .waitFor();
  assert.equal(
    [...projects.values()][0].purpose,
    "Synthetic product discussion experiments",
  );
  await click("message.dismiss");
  ok("Revision conflicts reload safely without overwriting another edit");
  await click("screen.evidence");
  await click("build.start");
  await click("job.cancel");
  assert([...jobs.values()].some((j) => j.status === "cancelled"));
  ok("A running candidate can be cancelled");
  const externalProject = [...projects.values()][0],
    externalJob = {
      id: newId("job"),
      project_id: externalProject.id,
      status: "running",
      stage: "Creating profiles",
      note: "Started through the service API",
      created_at: new Date().toISOString(),
      kind: "build",
      polls: 0,
    };
  jobs.set(externalJob.id, externalJob);
  externalProject.builds.push(externalJob);
  await page.waitForFunction(
    (id) => window.personaWorkshop.snapshot().jobs.some((j) => j.id === id),
    externalJob.id,
    { timeout: 10000 },
  );
  await page.waitForFunction(
    (id) =>
      window.personaWorkshop.snapshot().view.screen === "profiles" &&
      window.personaWorkshop
        .snapshot()
        .jobs.some((j) => j.id === id && j.status === "complete"),
    externalJob.id,
    { timeout: 12000 },
  );
  ok(
    "Jobs started outside the browser are discovered and completed profiles open automatically",
  );
  await click("screen.evidence");
  await click("source.open");
  await click("source.remove_confirm");
  await click("source.remove");
  await click("screen.profiles");
  await page
    .getByText("Sources or instructions changed.", { exact: false })
    .waitFor();
  assert.equal(
    await page.locator('[data-action="profile.export"]').isDisabled(),
    true,
  );
  ok("Removed evidence visibly invalidates profile exports");
  await page.setViewportSize({ width: 720, height: 550 });
  await page.waitForTimeout(300);
  assert(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth + 1,
    ),
  );
  assert(await page.locator(".profile-document").isVisible());
  await page.screenshot({
    path: path.join(output, "profiles-200-percent-layout.png"),
    fullPage: true,
  });
  ok(
    "Two-times browser zoom equivalent viewport keeps the full document and controls readable",
  );
  await page.goto(origin + prefix + "?embed=1");
  await page.waitForFunction(() => window.personaWorkshop?.snapshot().view);
  assert.equal(await page.locator(".sidebar").isVisible(), false);
  assert.equal(await page.locator(".topbar").isVisible(), false);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.waitForTimeout(400);
  assert(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth + 1,
    ),
  );
  await page.screenshot({
    path: path.join(output, "embedded-mobile.png"),
    fullPage: true,
  });
  ok(
    "Embedded and mobile layouts preserve the workflow without horizontal overflow",
  );
  assert.deepEqual(errors, []);
  assert(!JSON.stringify(viewBodies).includes("PRIVATE_SYNTHETIC_RAW"));
  ok("No browser runtime errors or raw-data leaks");
  console.log(`${passed} browser scenarios passed. Screenshots: ${output}`);
} finally {
  await browser.close();
  await new Promise((r) => server.close(r));
}
