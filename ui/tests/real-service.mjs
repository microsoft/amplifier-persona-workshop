// Real HTTPS service + real persisted state + a deterministic, tool-free ACP process.
// The temporary service's own management credential is supplied only by the test browser.
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import assert from "node:assert/strict";
import { spawn, execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
const { chromium } = await import(
  process.env.PLAYWRIGHT_MODULE || "playwright"
);
const root = path.resolve(
    path.dirname(fileURLToPath(import.meta.url)),
    "../..",
  ),
  binary =
    process.env.TWIN_BINARY || path.join(root, "target/debug/twin-workbench"),
  home = await fs.mkdtemp(path.join(os.tmpdir(), "twin-ui-service-"));
await fs.chmod(home, 0o700);
const output = process.env.UI_ARTIFACTS || "/tmp/twin-workbench-ui";
await fs.mkdir(output, { recursive: true });
let service, browser;
const errors = [];
let count = 0;
const ok = (s) => {
  console.log(`PASS ${s}`);
  count++;
};
try {
  const endpoint = path.join(home, "extractor.json");
  await fs.writeFile(
    endpoint,
    JSON.stringify({
      command: "python3",
      args: [path.join(root, "tests/fixture_acp.py")],
      env: { TWIN_FIXTURE_DIR: path.join(home, "fixture") },
      cwd: home,
      timeout_seconds: 15,
    }),
    { mode: 0o600 },
  );
  execFileSync(
    binary,
    [
      "--home",
      home,
      "init",
      "--owner",
      "fixture-owner",
      "--extractor-config",
      endpoint,
    ],
    { stdio: "pipe" },
  );
  service = spawn(binary, ["--home", home, "serve"], { stdio: "ignore" });
  let runtime;
  for (let i = 0; i < 100; i++) {
    try {
      runtime = JSON.parse(
        await fs.readFile(path.join(home, "runtime.json"), "utf8"),
      );
      break;
    } catch (_) {
      await new Promise((r) => setTimeout(r, 100));
    }
  }
  assert(runtime, "Service did not create runtime metadata");
  const config = JSON.parse(
    await fs.readFile(path.join(home, "config.json"), "utf8"),
  );
  execFileSync(
    "openssl",
    [
      "verify",
      "-CAfile",
      config.tls_ca,
      "-verify_ip",
      "127.0.0.1",
      config.tls_cert,
    ],
    { stdio: "pipe" },
  );
  const cliStatus = JSON.parse(
    execFileSync(binary, ["--home", home, "api", "GET", "/api/v1/status"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    }),
  );
  assert.equal(cliStatus.extractor_ready, true);
  ok(
    "Generated certificate chain and IP identity verify; CLI connects with CA trust enabled",
  );
  const token = (
      await fs.readFile(path.join(home, "secrets/control-token"), "utf8")
    ).trim(),
    origin = runtime.url;
  browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROMIUM_EXECUTABLE
      ? { executablePath: process.env.CHROMIUM_EXECUTABLE }
      : {}),
    args: ["--no-sandbox"],
  });
  const context = await browser.newContext({
      ignoreHTTPSErrors: true,
      extraHTTPHeaders: { Authorization: `Bearer ${token}` },
      viewport: { width: 1440, height: 1100 },
    }),
    page = await context.newPage();
  page.on("pageerror", (e) => errors.push(e.message));
  const idle = () =>
      page.waitForFunction(
        () => window.twinWorkbench?.snapshot().busy === false,
      ),
    act = async (action, payload = {}) => {
      // A click may paint the next dialog before its state has finished saving.
      await idle();
      await page.evaluate(
        async ({ action, payload }) =>
          window.twinWorkbench.act(action, payload),
        { action, payload },
      );
      await idle();
      const error = await page.evaluate(
        () => window.twinWorkbench.snapshot().error,
      );
      assert.equal(error, "", `${action}: ${error}`);
    },
    set = async (key, value) =>
      page.evaluate(
        ({ key, value }) => window.twinWorkbench.setDraft(key, value),
        { key, value },
      );
  await page.goto(origin);
  await page.getByRole("heading", { name: "Projects" }).waitFor();
  ok(
    "Actual TLS service accepts a scoped automation credential; browser assets and status load",
  );
  await act("project.new");
  await set("project_alias", "Avery · synthetic UI check");
  await set("project_aliases", "Avery");
  await set(
    "project_purpose",
    "Verify the evidence review workflow with fictional material",
  );
  await act("project.create");
  let project = await page.evaluate(
    () => window.twinWorkbench.snapshot().project,
  );
  assert(project.id);
  ok("Semantic UI actions create a real persisted project");
  const raw = JSON.stringify({
    messages: [
      {
        role: "user",
        author: "Avery",
        text: "Let’s compare a reversible experiment with a complete rewrite.",
      },
      { role: "user", author: "Peer", text: "What would be the first test?" },
      {
        role: "assistant",
        author: "Avery",
        text: "Generated content should never become my evidence.",
      },
      {
        role: "user",
        author: "Avery",
        text: "A concrete test would help us choose.",
      },
    ],
  });
  await page.evaluate(
    async (raw) =>
      window.twinWorkbench.importSource({
        name: "Synthetic normalized chat",
        kind: "chat",
        text: raw,
        collections: ["communication", "working"],
      }),
    raw,
  );
  await idle();
  assert.equal(
    await page.evaluate(() => window.twinWorkbench.snapshot().error),
    "",
  );
  assert.equal(await page.locator(".span-item.excluded").count(), 2);
  await act("source.save");
  ok("Normalized-chat import and speaker roles work against the actual parser");
  await page.evaluate(() =>
    window.twinWorkbench.importSource({
      name: "Existing communication profile",
      kind: "profile",
      text: "## Voice\nUse concrete examples and connected paragraphs.",
      collections: ["communication"],
    }),
  );
  await idle();
  assert.equal(await page.locator("[data-author]").count(), 0);
  assert.equal(await page.locator(".span-item.excluded").count(), 0);
  assert(
    (await page.locator(".span-meta").innerText()).includes(
      "Reference profile",
    ),
  );
  await act("source.save");
  ok(
    "Existing profiles are automatically attributed as reference guidance without another speaker-selection step",
  );

  await act("build.start");
  await page.waitForFunction(
    () =>
      window.twinWorkbench.snapshot().jobs.some((j) => j.status === "complete"),
    null,
    { timeout: 15000 },
  );
  await page.getByRole("heading", { name: "Profiles", exact: true }).waitFor();
  await page.locator(".profile-document").waitFor();
  assert(await page.locator(".profile-document").isVisible());
  await page.screenshot({
    path: path.join(output, "actual-service-profiles.png"),
    fullPage: true,
  });
  await act("screen.review");
  await act("details.toggle", { key: "review_details" });
  assert.equal(await page.locator(".observation").count(), 1);
  await act("evidence.open", {
    id: await page
      .locator('[data-action="evidence.open"]')
      .first()
      .getAttribute("data-id"),
  });
  assert(
    (await page.locator('[role="dialog"] .span-text').innerText()).includes(
      "reversible experiment",
    ),
  );
  await act("modal.close");
  const oid = await page
    .locator('[data-action="observation.accept"]')
    .first()
    .getAttribute("data-id");
  await act("observation.accept", { id: oid });
  await act("profile.create");
  assert(
    (await page.locator(".profile-document").innerText()).includes(
      "Use concise, concrete language.",
    ),
  );
  ok(
    "Actual ACP extraction creates source-linked observations and a reviewable profile version",
  );
  const download = page.waitForEvent("download");
  await act("profile.export");
  const packageFile = await download,
    packageData = JSON.parse(
      await fs.readFile(await packageFile.path(), "utf8"),
    );
  assert(packageData.files["communication.md"]);
  assert(packageData.files["working-style.md"]);
  assert(
    !JSON.stringify(packageData).includes("Generated content should never"),
  );
  ok(
    "Actual portable package contains both profiles and omits raw chat content",
  );
  const readZip = async (file) => JSON.parse(execFileSync("python3", ["-c",
    "import json,sys,zipfile; z=zipfile.ZipFile(sys.argv[1]); print(json.dumps({n:z.read(n).decode('utf-8') for n in z.namelist()}))",
    await file.path()], {encoding:"utf8"}));
  await page.getByRole("button", {name:"Export…", exact:true}).click();
  await page.getByRole("heading", {name:"Use in another experience"}).waitFor();
  const originalAgents = "# Local project\r\n\r\nRun the existing checks. PRIVATE-LOCAL-DOC-4891\r\n";
  await act("export.existing", {text:originalAgents});
  let nextDownload = page.waitForEvent("download");
  await act("export.download");
  const workspaceFiles = await readZip(await nextDownload);
  assert(workspaceFiles["AGENTS.md"].startsWith(originalAgents),
    `Existing instructions changed: ${JSON.stringify(workspaceFiles["AGENTS.md"].slice(0, 160))}`);
  assert(workspaceFiles["AGENTS.md"].includes("persona/communication.md"));
  assert.equal(workspaceFiles["persona/communication.md"], packageData.files["communication.md"]);
  const sharedView = await page.evaluate(() => window.personaWorkshop.snapshot().view);
  assert.equal(sharedView.modal, "profile.export");
  assert.equal(sharedView.draft.export_format, "agents");
  assert(!JSON.stringify(sharedView).includes("PRIVATE-LOCAL-DOC-4891"));
  await act("export.existing", {text:workspaceFiles["AGENTS.md"]});
  nextDownload = page.waitForEvent("download");
  await act("export.download");
  assert.equal((await readZip(await nextDownload))["AGENTS.md"], workspaceFiles["AGENTS.md"]);
  await page.locator('[data-draft="export_format"]').selectOption("soul");
  await page.getByText("Put SOUL.md in your OpenClaw agent workspace.", {exact:false}).waitFor();
  nextDownload = page.waitForEvent("download");
  await act("export.download");
  const soulFiles = await readZip(await nextDownload);
  assert(soulFiles["SOUL.md"].includes(packageData.files["communication.md"].trim()));
  assert(soulFiles["SOUL.md"].includes(packageData.files["working-style.md"].trim()));
  assert(!soulFiles["SOUL.md"].includes("calibrated"));
  await act("modal.close");
  const snap = await page.evaluate(() => window.personaWorkshop.snapshot());
  const existingPath = path.join(home, "existing-AGENTS.md"), exportedPath = path.join(home, "exported-workspace");
  await fs.writeFile(existingPath, originalAgents);
  execFileSync(binary, ["--home",home,"export","--project",snap.project.id,"--revision",snap.view.profile_revision_id,
    "--format","agents","--agents-file",existingPath,"--output",exportedPath], {stdio:"pipe"});
  assert.equal(await fs.readFile(existingPath,"utf8"), originalAgents);
  assert.equal(await fs.readFile(path.join(exportedPath,"AGENTS.md"),"utf8"), workspaceFiles["AGENTS.md"]);
  assert.equal(await fs.readFile(path.join(exportedPath,"persona/communication.md"),"utf8"), packageData.files["communication.md"]);
  ok("Browser and CLI export actual workspace files, preserve existing AGENTS.md idempotently, and keep private merge text out of shared state");
  await act("profile.create");
  const newest = await page.evaluate(
    () => window.twinWorkbench.snapshot().view.profile_revision_id,
  );
  await page.evaluate(() =>
    window.twinWorkbench.control({ profile_revision_id: null }),
  );
  await act("screen.profiles");
  await act("details.toggle", { key: "profile_details" });
  assert.equal(
    await page.locator('[data-field="profile_revision_id"]').inputValue(),
    newest,
  );
  ok(
    "The newest profile is selected even when server history arrives newest-first",
  );
  await act("screen.evaluate");
  await set(
    "scenario",
    "A teammate proposes a full rewrite while we have one uncertain assumption. Respond as a clearly labeled simulation.",
  );
  await set("compare_profiles", true);
  for (const key of ["both", "communication", "working", "baseline"])
    await set(`variant_${key}`, true);
  await act("evaluation.run");
  await page.waitForFunction(
    () => document.querySelectorAll(".evaluation").length === 4,
    null,
    { timeout: 20000 },
  );
  await page.screenshot({
    path: path.join(output, "actual-service-evaluations.png"),
    fullPage: true,
  });
  ok(
    "Four independently queued profile variants complete against the real service",
  );
  await act("details.toggle", { key: "evaluation_details" });
  const eid = await page
    .locator('[data-action="evaluation.rate"]')
    .first()
    .getAttribute("data-id");
  await set(`rating_${eid}`, {
    communication_fit: 4,
    working_fit: 3,
    notes: "Synthetic test review.",
  });
  await act("evaluation.rate", { id: eid });
  ok(
    "Strict persisted view schema accepts semantic rating drafts and actual evaluation reviews",
  );
  await act("comparison.open");
  await set("scenario", "A fresh fictional situation: decide which reversible experiment comes first.");
  await act("comparison.run");
  await page.waitForFunction(() => document.querySelectorAll(".comparison-result .markdown").length === 2, null, {timeout:20000});
  let comparisons = await (await page.request.get(`${origin}/api/v1/projects/${project.id}/comparisons`)).json();
  let comparison = comparisons.items[0];
  assert(!("variant" in comparison.candidates[0]));
  assert.equal(comparison.rating, null);
  const externalFeedback = await page.request.put(`${origin}/api/v1/projects/${project.id}/comparisons/${comparison.id}/rating`, {data:{ratings:{A:{communication_fit:2,working_fit:3},B:{communication_fit:3,working_fit:3}},preference:"neither",perspective:"reviewer"}});
  assert(externalFeedback.ok(), await externalFeedback.text());
  await page.waitForFunction(() => !document.querySelector('[data-action="comparison.reveal"]').disabled, null, {timeout:10000});
  const rating = {preference:"tie"};
  for (const label of ["A","B"]) {
    rating[`${label}_communication_fit`] = "3";
    rating[`${label}_working_fit`] = "4";
    rating[`${label}_notes`] = "Synthetic review for workflow verification.";
  }
  await set(`comparison_rating_${comparison.id}`, rating);
  await act("comparison.rate",{id:comparison.id});
  await act("comparison.reveal",{id:comparison.id});
  comparisons = await (await page.request.get(`${origin}/api/v1/projects/${project.id}/comparisons`)).json();
  comparison = comparisons.items[0];
  assert(comparison.revealed_at);
  assert.equal(comparison.rating.reviewer,"token:local-control");
  assert.equal(comparison.calibrated,false);
  assert(comparison.candidates.some(c => c.variant === "baseline"));
  await page.screenshot({path:path.join(output,"actual-service-blind-comparison.png"),fullPage:true});
  await page.setViewportSize({width:650,height:1000});
  assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),"Comparison must fit a narrow viewport");
  await page.screenshot({path:path.join(output,"actual-service-blind-mobile.png"),fullPage:true});
  await page.setViewportSize({width:1440,height:1100});
  ok("Blind comparison hides mapping, stores real actor feedback, reveals afterward, and fits narrow layouts");
  const versionsForDiff = (await (await page.request.get(`${origin}/api/v1/projects/${project.id}`)).json()).revisions;
  await set("comparison_revision_a",versionsForDiff[0].id);
  await set("comparison_revision_b",versionsForDiff.at(-1).id);
  await act("revision.diff");
  assert(await page.getByRole("heading",{name:"communication.md",exact:true}).count());
  assert(await page.getByRole("heading",{name:"working-style.md",exact:true}).count());
  assert.equal(await page.evaluate(() => window.personaWorkshop.snapshot().view.draft.revision_diff),true);
  ok("Revision differences are visible and their view state is available to automation");
  await act("source.new");
  await set("source_name","Synthetic HTML holdout");
  await set("source_holdout",true);
  await act("source.create",{file:{name:"holdout.html",content_base64:Buffer.from("<p>A fresh held-back reference.</p><script>Hidden text</script>").toString("base64")}});
  assert((await page.locator(".span-list").innerText()).includes("A fresh held-back reference."));
  assert(!(await page.locator(".span-list").innerText()).includes("Hidden text"));
  await act("modal.close");
  ok("Document import uses the live converter, previews authored spans, and preserves holdout choice");
  const view = await page.evaluate(() => window.twinWorkbench.snapshot().view);
  await page.waitForTimeout(1000);
  const current = await (
    await page.request.get(`${origin}/api/v1/views/${view.id}`)
  ).json();
  const response = await page.request.post(
    `${origin}/api/v1/views/${view.id}/commands`,
    {
      data: {
        expected_revision: current.revision,
        controller: "synthetic-voice-agent",
        commands: [
          { op: "set", field: "screen", value: "review" },
          { op: "set", field: "tab", value: "working" },
          {
            op: "set",
            field: "draft",
            value: { ...current.draft, review_details: false },
          },
          { op: "set", field: "scroll", value: { x: 0, y: 0 } },
        ],
      },
    },
  );
  assert(response.ok(), await response.text());
  await page.waitForFunction(
    () =>
      window.twinWorkbench.snapshot().view.screen === "review" &&
      window.twinWorkbench.snapshot().view.tab === "working",
  );
  assert(
    (await page.locator(".observation").innerText()).includes(
      "Compare evidence",
    ),
  );
  ok(
    "External service API drives the same visible review state and profile tab",
  );
  await page.screenshot({
    path: path.join(output, "actual-service-review.png"),
    fullPage: true,
  });
  assert.deepEqual(errors, []);
  console.log(`${count} actual-service browser scenarios passed.`);
} finally {
  if (browser) await browser.close();
  if (service) {
    service.kill("SIGTERM");
    await Promise.race([
      new Promise((r) => service.once("exit", r)),
      new Promise((r) => setTimeout(r, 3000)),
    ]);
    if (service.exitCode === null) service.kill("SIGKILL");
  }
  await fs.rm(home, { recursive: true, force: true });
}
