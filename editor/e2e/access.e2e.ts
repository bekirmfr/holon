// People and access (PeoplePages.tsx): an admin defines a role and binds roles through the forms;
// a person's page shows what is bound to them and what it lets them do, and they then see the
// flow; unbinding, which says why, takes it away; History says each change, and a version what it
// changed. Someone who holds less than a binding gives is refused it in the server's words. And
// what a principal's page says it may do (access.ts givesOf) is what the server's `can` answers
// them, for each way of being bound. And the buttons follow `can` (can.tsx): a viewer's flow page
// has each that changes something disabled, saying why in the server's words, until a binding
// lets them, which the page then follows; someone who may deploy a flow but not approve what it
// asks proposes it, told why, and it waits in Approvals. And a flow whose approval no one here can
// decide says so in its consent and its review, and deploying it asks again, naming it; a step
// whoever started the run may take (`starter: true`) says so, and deploys without asking. And while
// this host has people approve an access change, an admin's binding of a role that decides waits,
// said so, listed in History and by `holon access proposals`; another admin approves it in
// Approvals, and it then holds; one declined from the command line never does.

import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Browser, Locator } from "@playwright/test";
import { givesOf, type Tagged } from "../src/access";
import type { Principal, RoleEntry } from "../src/server";
import { ADMIN, expect, HOLON, OWN_PORTS, PORTS, RUN_DIR, SAMPLE_SHIFT, SERVER, test } from "./fixtures";
import { flowRow, gotoPage, navLink, peopleTab } from "./helpers";

/** A call to the control plane as whoever `token` signs in. */
const control = (token: string, path: string, data?: unknown, method?: string) =>
  fetch(`${SERVER}${path}`, {
    method: method ?? (data === undefined ? "GET" : "POST"),
    headers: { Authorization: `Bearer ${token}`, "content-type": "application/json" },
    body: data === undefined ? undefined : JSON.stringify(data),
  });
const sure = async (res: Promise<Response>) => {
  const r = await res;
  expect(r.ok, await r.clone().text()).toBe(true);
  return r;
};
const json = async <T>(res: Promise<Response>) => (await (await sure(res)).json()) as T;

/** A person the admin adds and who joins by their link: their session's token. */
async function joined(adminToken: string, name: string, role = "viewer"): Promise<string> {
  const { invite } = await json<{ invite: string }>(control(adminToken, "/people", { name, kind: "person", role }));
  const accepted = await sure(fetch(`${SERVER}/auth/invite`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ token: invite, password: "e2e-second-password-0123" }) }));
  return ((await accepted.json()) as { token: string }).token;
}

/** Take away what `name` was given on the host when added. */
async function nothingOnTheHost(adminToken: string, name: string) {
  const held = (await json<{ bindings: { id: number; scope: unknown }[] }>(control(adminToken, `/bindings?who=${name}`))).bindings;
  for (const b of held.filter((b) => b.scope === "host")) await sure(control(adminToken, `/bindings/${b.id}?why=e2e`, undefined, "DELETE"));
}

/** Deploy `text` as `slug`, approving what it asks for. */
function deploy(adminToken: string, slug: string, text: string) {
  const file = join(tmpdir(), `${slug}.holon.ts`);
  writeFileSync(file, text);
  execFileSync(HOLON, ["deploy", file, "--server", SERVER, "--slug", slug, "--approve"], { env: { ...process.env, HOLON_TOKEN: adminToken }, stdio: "pipe" });
}

/** The editor, in a browser of its own, signed in with `token` (as signing in leaves it). */
async function editorAs(browser: Browser, token: string, name: string) {
  const context = await browser.newContext({ baseURL: `http://localhost:${PORTS.editor}` });
  await context.addInitScript((session) => localStorage.setItem("holon.session", session), JSON.stringify({ token, name, kind: "person", role: "viewer" }));
  return context.newPage();
}

test("an admin defines a role and binds through the forms; a person's page says what they may do, they see the flow, unbinding says why and takes it away, History says each change, and a binding beyond what its author holds is refused in the server's words", async ({
  page,
  browser,
  adminToken,
}) => {
  const n = Date.now().toString(36);
  const slug = `people-${n}`;
  deploy(adminToken, slug, `import { action, flow } from "holon"\n\nflow("${slug}")\n\nexport const shout = action((input) => input.toUpperCase())\n`);
  // bob joins and holds nothing; carol joins as a viewer.
  const [bob, carol] = [`e2e-bob-${n}`, `e2e-carol-${n}`];
  const [bobToken, carolToken] = [await joined(adminToken, bob), await joined(adminToken, carol)];
  await nothingOnTheHost(adminToken, bob);
  expect((await control(bobToken, "/deployments")).status).toBe(403);

  // The admin defines a role that keeps access: it reads people and roles, and binds.
  const keeper = `e2e-keeper-${n}`;
  await page.goto("/#people.roles");
  const define = page.getByRole("form", { name: "define a role" });
  await define.getByLabel("role's name").fill(keeper);
  for (const action of ["people.view", "people.assign", "role.view"]) await define.getByRole("checkbox", { name: action, exact: true }).check();
  await define.getByLabel("role's why").fill("e2e: keeps access");
  await define.getByRole("button", { name: "define role" }).click();
  await expect(page.getByRole("table", { name: "roles" }).getByRole("row").filter({ hasText: keeper })).toContainText("people view, assign · role view");

  // …and binds it to carol on this host, from the Bindings tab.
  await peopleTab(page, "Bindings");
  const form = page.getByRole("form", { name: "bind a role" });
  await form.getByLabel("bind: to whom").selectOption(`principal:${carol}`);
  await form.getByLabel("bind: role").selectOption(keeper);
  await form.getByLabel("bind: where").selectOption("host");
  await form.getByRole("button", { name: "bind", exact: true }).click();
  await expect(form.getByRole("status")).toContainText(`Bound ${keeper} to ${carol} on this host`);
  await expect(page.getByRole("table", { name: "bindings" }).getByRole("row").filter({ hasText: keeper })).toContainText(carol);

  // bob's page: nothing holds for him. The admin binds viewer on the flow there.
  await page.goto(`/#people.who.${bob}`);
  const bobs = page.getByRole("region", { name: `${bob}'s page` });
  await expect(bobs).toContainText("nothing: no binding holds for it");
  const bindBob = bobs.getByRole("form", { name: `bind a role to ${bob}` });
  await bindBob.getByLabel("bind: role").selectOption("viewer");
  await bindBob.getByLabel("bind: where").selectOption("flow");
  await bindBob.getByLabel("bind: flow").fill(slug);
  await bindBob.getByLabel("bind: why").fill("e2e: reads it");
  await bindBob.getByRole("button", { name: "bind", exact: true }).click();
  await expect(bindBob.getByRole("status")).toContainText(`Bound viewer to ${bob} on "${slug}"`);
  const bound = bobs.getByRole("table", { name: `what is bound to ${bob}` });
  await expect(bound.getByRole("row").filter({ hasText: `on "${slug}"` })).toContainText("viewer");
  const mayDo = bobs.locator('dl[aria-label="what it may do"] > div').filter({ hasText: `on ${slug}` });
  // A viewer reads a run's steps too (run.steps, R55).
  await expect(mayDo).toContainText("flow view · run view, steps, export · state view · secret list");

  // bob now sees the flow.
  const asBob = await editorAs(browser, bobToken, bob);
  await asBob.goto("/#flows");
  await expect(flowRow(asBob, slug)).toHaveCount(1);

  // Unbinding asks why, and takes it away.
  await bound.getByRole("button", { name: `unbind viewer from ${bob} on "${slug}"` }).click();
  const dialog = page.locator('[aria-modal="true"]');
  await expect(dialog.getByRole("button", { name: "Unbind" })).toBeDisabled();
  await dialog.locator("input").fill("e2e: done reading");
  await dialog.getByRole("button", { name: "Unbind" }).click();
  await expect(bobs).toContainText("nothing: no binding holds for it");
  expect((await control(bobToken, "/deployments")).status).toBe(403);
  await asBob.reload();
  await expect(flowRow(asBob, slug)).toHaveCount(0);
  await asBob.context().close();

  // History says each change, with its why; a version says what it changed.
  await page.goto("/#people.history");
  const change = page.getByRole("table", { name: "access history" }).getByRole("row").filter({ hasText: `unbound viewer from ${bob} on "${slug}"` });
  await expect(change).toContainText("e2e: done reading");
  await change.getByRole("link").click();
  await expect(page).toHaveURL(/#people\.history\.v\d+$/);
  await expect(page.getByRole("table", { name: "what it changed" }).getByRole("row").filter({ hasText: `viewer to ${bob} on "${slug}"` })).toContainText("removed");

  // carol keeps access, and holds only a viewer's on the flow: binding editor there is refused,
  // in the server's words.
  const asCarol = await editorAs(browser, carolToken, carol);
  await asCarol.goto("/");
  await gotoPage(asCarol, "people");
  await peopleTab(asCarol, "Bindings");
  const hers = asCarol.getByRole("form", { name: "bind a role" });
  await hers.getByLabel("bind: to whom").selectOption(`principal:${bob}`);
  await hers.getByLabel("bind: role").selectOption("editor");
  await hers.getByLabel("bind: flow").fill(slug);
  await hers.getByRole("button", { name: "bind", exact: true }).click();
  await expect(hers.getByRole("alert")).toContainText(`editor holds flow.edit on "${slug}", which you don't: a binding gives only what its author holds where it is made`);
  await asCarol.context().close();
});

test("what a principal's page says it may do is what the server's can answers them, for each way of being bound", async ({ adminToken }) => {
  test.setTimeout(180_000);
  const n = Date.now().toString(36);
  const tag = `e2e-${n}`;
  const [tagged, plain] = [`gives-t-${n}`, `gives-p-${n}`];
  deploy(adminToken, tagged, `import { flow, route } from "holon"\n\nflow("${tagged}", { port: ${OWN_PORTS.tagged + SAMPLE_SHIFT}, tags: ["${tag}"] })\n\nroute.get("/hi", () => "hi")\n`);
  deploy(adminToken, plain, `import { action, flow } from "holon"\n\nflow("${plain}")\n\nexport const shout = action((input) => input.toUpperCase())\n`);
  try {
    const [approver, operator, group] = [`e2e-approver-${n}`, `e2e-operator-${n}`, `e2e-g-${n}`];
    await sure(control(adminToken, `/roles/${approver}`, { allows: ["run.decide"], kinds: ["person", "agent", "service"] }, "PUT"));
    await sure(control(adminToken, `/roles/${operator}`, { allows: ["flow.deploy", "flow.lifecycle", "people.view", "host.view"], kinds: ["agent"] }, "PUT"));
    const binding = (who: string | { group: string }, role: string, scope: unknown, more: object = {}) =>
      sure(control(adminToken, "/bindings", { who: typeof who === "string" ? { principal: who } : who, role, scope, ...more }));
    const agentToken = async (name: string, kind: "agent" | "service", role: string) => {
      await sure(control(adminToken, "/people", { name, kind, role }));
      return (await json<{ token: string }>(control(adminToken, `/people/${name}/tokens`, { label: "e2e" }))).token;
    };

    // Each way of being bound: its principal, and a token of theirs.
    const tokens: Record<string, string> = { [ADMIN.name]: adminToken };
    const p = (k: string) => `e2e-${k}-${n}`;
    // A viewer whose binding ends in a moment: it holds nothing once it has.
    tokens[p("ends")] = await joined(adminToken, p("ends"));
    await nothingOnTheHost(adminToken, p("ends"));
    await binding(p("ends"), "viewer", { flows: { slug: plain } }, { expires_at: Math.floor(Date.now() / 1000) + 2 });
    // A viewer on one flow; an editor on a tag; a decider at one approval point.
    for (const [k, role, scope] of [
      ["flow", "viewer", { flows: { slug: tagged } }],
      ["tag", "editor", { flows: { tag } }],
      ["point", approver, { approval: { flow: { slug: tagged }, name: "ok" } }],
    ] as const) {
      tokens[p(k)] = await joined(adminToken, p(k));
      await nothingOnTheHost(adminToken, p(k));
      await binding(p(k), role, scope);
    }
    // A viewer on the host, and an editor on a flow through a group.
    tokens[p("member")] = await joined(adminToken, p("member"));
    await sure(control(adminToken, `/groups/${group}`, { members: [p("member")] }, "PUT"));
    await binding({ group }, "editor", { flows: { slug: plain } });
    // An agent with the agent role on the host: it saves only its own flows.
    tokens[p("agent")] = await agentToken(p("agent"), "agent", "agent");
    // An agent bound an admin's role allowing what is a person's.
    tokens[p("operator")] = await agentToken(p("operator"), "agent", "agent");
    await nothingOnTheHost(adminToken, p("operator"));
    await binding(p("operator"), operator, "host");
    // A service bound to decide, which a service never does, and a viewer on a flow.
    tokens[p("service")] = await agentToken(p("service"), "service", "viewer");
    await nothingOnTheHost(adminToken, p("service"));
    await binding(p("service"), approver, { approval: { flow: { slug: tagged }, name: "ok" } });
    await binding(p("service"), "viewer", { flows: { slug: plain } });

    // Once the short binding has ended.
    await expect
      .poll(async () => (await json<Principal[]>(control(adminToken, "/people"))).find((x) => x.name === p("ends"))?.bindings.every((b) => b.expired), { timeout: 15_000 })
      .toBe(true);
    const people = await json<Principal[]>(control(adminToken, "/people"));
    const roles = (await json<{ roles: RoleEntry[] }>(control(adminToken, "/roles"))).roles;
    const flows: Tagged[] = (await json<{ deployments: { slug: string; tags?: string[] }[] }>(control(adminToken, "/deployments"))).deployments.map((d) => ({ slug: d.slug, tags: d.tags ?? [] }));
    expect(flows.find((f) => f.slug === tagged)?.tags).toEqual([tag]);
    const catalogue = roles.find((r) => r.name === "admin")?.allows ?? [];
    const ofFlow = (a: string) => ["flow", "run", "state", "secret"].includes(a.split(".")[0]);
    const [onFlow, onHost] = [catalogue.filter(ofFlow), catalogue.filter((a) => !ofFlow(a))];

    for (const [name, token] of Object.entries(tokens)) {
      const who = people.find((x) => x.name === name);
      expect(who, name).toBeDefined();
      const gives = givesOf(who!.kind, who!.bindings, roles, flows);
      const can = async (query: string) => json<Record<string, true | string>>(control(token, `/auth/me/can?${query}`));
      const yes = (answer: Record<string, true | string>, actions: string[]) => actions.filter((a) => answer[a] === true);
      expect(yes(await can(`actions=${onHost.join(",")}`), onHost), `${name} on the host`).toEqual(gives.host);
      for (const slug of [tagged, plain]) {
        const there = gives.flows.find((f) => f.slug === slug);
        const said = onFlow.filter((a) => there?.actions.includes(a) || (a === "run.decide" && (there?.decides.length ?? 0) > 0));
        expect(yes(await can(`actions=${onFlow.join(",")}&flow=${slug}`), onFlow), `${name} on ${slug}`).toEqual(said);
        for (const point of ["ok", "other"]) {
          const decides = there?.actions.includes("run.decide") || there?.decides.includes(point) || false;
          expect((await can(`actions=run.decide&flow=${slug}&approval=${point}`))["run.decide"] === true, `${name} decides ${slug}'s "${point}"`).toBe(decides);
        }
      }
    }
  } finally {
    for (const slug of [tagged, plain]) await control(adminToken, `/deployments/${slug}`, undefined, "DELETE");
  }
});

test("a viewer's flow page has each button that changes something disabled, saying why in the server's words; bound editor there, the page follows", async ({ page, browser, adminToken }) => {
  const n = Date.now().toString(36);
  const slug = `follows-${n}`;
  deploy(adminToken, slug, `import { action, flow } from "holon"\n\nflow("${slug}")\n\nexport const shout = action((input) => input.toUpperCase())\n`);
  try {
    const vic = `e2e-vic-${n}`;
    const asVic = await editorAs(browser, await joined(adminToken, vic), vic);
    const reads = `${vic} may not do that: a viewer only reads`;
    const disabled = async (button: Locator, title: string) => {
      await expect(button).toBeDisabled();
      await expect(button).toHaveAttribute("title", title);
    };

    // The header: pausing is an editor's.
    await asVic.goto(`/#${slug}`);
    const flowPage = asVic.locator(`[aria-label="flow ${slug}"]`);
    await disabled(flowPage.getByRole("button", { name: "Pause", exact: true }), reads);
    // Status: its action is seen, not run.
    const shout = flowPage.locator('[aria-label="action shout"]');
    await disabled(shout.getByRole("button", { name: "run", exact: true }), reads);
    await expect(shout).toContainText(reads);
    // Settings: revoking, whether agents may propose, stopping it, removing it.
    await asVic.goto(`/#${slug}.settings`);
    const revokes = flowPage.getByRole("button", { name: "Revoke", exact: true });
    await expect(revokes).not.toHaveCount(0);
    for (const revoke of await revokes.all()) await disabled(revoke, reads);
    await expect(flowPage.getByRole("checkbox", { name: "agents may propose versions" })).toBeDisabled();
    await disabled(flowPage.getByRole("region", { name: "lifecycle" }).getByRole("button", { name: "Stop…", exact: true }), reads);
    await disabled(flowPage.getByRole("button", { name: `Remove ${slug}` }), reads);

    // The host's pages: the policy is a viewer's to read, not to read again; resources and
    // people aren't.
    await expect(navLink(asVic, "Policy")).toBeVisible();
    await expect(navLink(asVic, "Resources")).toHaveCount(0);
    await expect(navLink(asVic, "People and access")).toHaveCount(0);
    await asVic.goto("/#policy");
    await expect(asVic.getByText("granted without asking:")).toBeVisible();
    await disabled(asVic.getByRole("button", { name: "read the policy again" }), reads);
    await expect(asVic.getByRole("alert")).toHaveCount(0);

    // The admin's page has them all.
    await page.goto(`/#${slug}`);
    await expect(page.locator(`[aria-label="flow ${slug}"]`).getByRole("button", { name: "Pause", exact: true })).toBeEnabled();

    // Bound editor on the flow, the page follows once it reads the access version again (as it
    // does now and then, and on coming back to the window).
    await asVic.goto(`/#${slug}`);
    const pause = flowPage.getByRole("button", { name: "Pause", exact: true });
    await expect(pause).toBeDisabled();
    await sure(control(adminToken, "/bindings", { who: { principal: vic }, role: "editor", scope: { flows: { slug } }, why: "e2e: runs it" }));
    await asVic.evaluate(() => window.dispatchEvent(new Event("focus")));
    await expect(pause).toBeEnabled();
    await expect(pause).toHaveAttribute("title", "stop answering, keep the port and the session");
    await asVic.context().close();
  } finally {
    await control(adminToken, `/deployments/${slug}`, undefined, "DELETE");
  }
});

test("someone who may deploy a flow but not approve what it asks proposes it from the editor, told why; it waits in Approvals", async ({ page, browser, adminToken }) => {
  const n = Date.now().toString(36);
  const slug = `proposes-${n}`;
  const text = (more: string) => `import { action, flow } from "holon"${more ? '\nimport example from "host:example.com"' : ""}\n\nflow("${slug}")\n\nexport const shout = action((input) => input.toUpperCase())\n${more}`;
  deploy(adminToken, slug, text(""));
  try {
    // pia, a viewer, bound on the flow a role that edits and deploys it, and approves nothing.
    const pia = `e2e-pia-${n}`;
    const role = `proposer-${n}`;
    await sure(control(adminToken, `/roles/${role}`, { allows: ["flow.view", "flow.edit", "flow.propose", "flow.deploy"], kinds: ["person"] }, "PUT"));
    const asPia = await editorAs(browser, await joined(adminToken, pia), pia);
    await sure(control(adminToken, "/bindings", { who: { principal: pia }, role, scope: { flows: { slug } }, why: "e2e: proposes its versions" }));

    // A version that calls a host it isn't granted: the dialog says pia proposes it, and why.
    await asPia.goto("/");
    await gotoPage(asPia, "new");
    const canvas = asPia.locator(".hl-canvas");
    await canvas.getByRole("button", { name: "text off" }).click();
    await asPia.getByRole("region", { name: "Text" }).locator(".cm-content").click();
    await asPia.keyboard.press("Control+A");
    await asPia.keyboard.insertText(text("\nexport const ping = action(async () => await example.get(\"/\"))\n"));
    // Compiled: the tools panel lists its new action.
    await expect(asPia.locator('[aria-label="action ping"]')).toBeVisible({ timeout: 15_000 });
    const deployButton = canvas.getByRole("button", { name: /^deploy/ });
    await expect(deployButton).toBeEnabled();
    await deployButton.dispatchEvent("click");
    await asPia.locator('[aria-modal="true"]').locator("input").fill(slug);
    await asPia.keyboard.press("Enter");
    await asPia.getByRole("dialog").getByRole("button", { name: "Update", exact: true }).click();
    const asked = asPia.getByRole("dialog");
    await expect(asked).toContainText(`Propose "${slug}"?`);
    await expect(asked).toContainText("It needs a person's approval to:");
    await expect(asked).toContainText(`You may not approve it (${pia} may not do that: `);
    await expect(asked).toContainText("it waits in Approvals for someone who may.");
    const sent = asPia.waitForResponse((r) => r.request().method() === "POST" && new URL(r.url()).pathname === "/deploy");
    await asked.getByRole("button", { name: "Propose", exact: true }).click();
    expect((await sent).status()).toBe(202);
    await expect(asPia.getByRole("status").filter({ hasText: `of "${slug}": it waits in Approvals for someone who may approve it.` })).toBeVisible();
    await asPia.context().close();

    // It waits, pia's; the version deployed is still the first.
    const { proposals } = await json<{ proposals: { slug: string; state: string; by?: string }[] }>(control(adminToken, "/proposals"));
    expect(proposals.filter((p) => p.slug === slug).map((p) => [p.state, p.by])).toEqual([["waiting", pia]]);
    await page.goto(`/#approvals`);
    await expect(page.getByText(slug).first()).toBeVisible();
  } finally {
    await control(adminToken, `/deployments/${slug}`, undefined, "DELETE");
  }
});

test("a flow whose approval no one here can decide says so, and deploying it asks again, naming it: from the editor and from the review", async ({ page, adminToken }) => {
  const n = Date.now().toString(36);
  const slug = `undecided-${n}`;
  const [first, second] = [`nobody-${n}`, `nobody-else-${n}`];
  const text = (steps: string) =>
    `import { action, approval, flow } from "holon"\n\nflow("${slug}")\n\nexport const ask = action(async () => {\n  const decision = await approval("ok", { steps: [${steps}] })\n  return decision.approved\n})\n`;
  const sent: number[] = [];
  page.on("response", (r) => {
    if (r.request().method() === "POST" && new URL(r.url()).pathname === "/deploy") sent.push(r.status());
  });
  try {
    await page.goto("/");
    await gotoPage(page, "new");
    const canvas = page.locator(".hl-canvas");
    await canvas.getByRole("button", { name: "text off" }).click();
    const typed = page.getByRole("region", { name: "Text" }).locator(".cm-content");
    await typed.click();
    await page.keyboard.press("Control+A");
    await page.keyboard.insertText(text(`{ role: "${first}" }`));
    await expect(page.locator('[aria-label="action ask"]')).toBeVisible({ timeout: 15_000 });

    // The consent shows its approval's row, not ready, saying why; approving it asks again,
    // naming it, and cancelled there nothing is deployed. (Named the first time.)
    const deploy = async (named = true) => {
      await canvas.getByRole("button", { name: /^deploy/ }).dispatchEvent("click");
      if (!named) {
        await page.locator('[aria-modal="true"]').locator("input").fill(slug);
        await page.keyboard.press("Enter");
      }
      const consent = page.getByRole("dialog");
      await expect(consent).toContainText(`approval ok: approved by a person holding ${first}`);
      await expect(consent).toContainText(`${first}: no person holds it here`);
      await consent.getByRole("button", { name: "Approve and deploy", exact: true }).click();
      const again = page.getByRole("dialog");
      await expect(again).toContainText(`Runs of "${slug}" would wait at an approval no one here can decide yet:`);
      await expect(again).toContainText(`• ok: ${first}: no person holds it here`);
      return again;
    };
    await (await deploy(false)).getByRole("button", { name: "Cancel", exact: true }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    expect(sent, "cancelled: nothing deployed").toEqual([]);
    await (await deploy()).getByRole("button", { name: "Approve and deploy anyway", exact: true }).click();
    await expect.poll(() => sent).toEqual([200]);
    await expect(page.locator(".hl-tab--active")).toContainText("deployed", { timeout: 30_000 });

    // A version saved with a second step no one holds either: its review puts the approval first,
    // under what can't be decided here, and approving it asks again.
    await typed.click();
    await page.keyboard.press("Control+A");
    await page.keyboard.insertText(text(`{ role: "${first}" }, { role: "${second}" }`));
    // Compiled: what it may do names the second step (in the bottom pane, closed).
    await expect(page.getByText(`wait for approval "ok" by a person holding ${first}, then a person holding ${second}, and continue later`, { exact: true }).first()).toBeAttached({ timeout: 15_000 });
    // Saved a second after the deploy at least: both are kept in seconds, and a version saved in
    // the second it was deployed reads as the one deployed.
    await page.waitForTimeout(1_100);
    const saved = page.waitForResponse((r) => r.request().method() !== "GET" && new URL(r.url()).pathname.startsWith("/flows"));
    await canvas.getByRole("button", { name: "save", exact: true }).click();
    expect((await saved).ok()).toBe(true);
    await page.goto(`/#approvals.${slug}.v2`);
    const undecided = page.getByRole("group", { name: "Can't be decided here" });
    await expect(undecided).toContainText(`approval ok: +approved by a person holding ${first}, then a person holding ${second}, −approved by a person holding ${first}`);
    await expect(undecided).toContainText(`${first}: no person holds it here`);
    const decide = page.getByRole("complementary", { name: "decision" });
    await decide.getByRole("checkbox").check();
    await decide.getByRole("button", { name: "Approve and deploy", exact: true }).click();
    const again = page.getByRole("dialog");
    await expect(again).toContainText(`Runs of "${slug}" would wait at an approval no one here can decide yet:`);
    await again.getByRole("button", { name: "Approve and deploy anyway", exact: true }).click();
    await expect.poll(() => sent).toEqual([200, 200]);
    await expect(page.getByRole("status").filter({ hasText: `Deployed ${slug} v2.` })).toBeVisible();
  } finally {
    await control(adminToken, `/deployments/${slug}`, undefined, "DELETE");
    await control(adminToken, `/flows/${slug}`, undefined, "DELETE");
  }
});

test("a step whoever started the run may take says so in words, and with its one holder the admin, the flow deploys without asking again: letting the starter in is a changed row", async ({ page, adminToken }) => {
  const n = Date.now().toString(36);
  const slug = `own-review-${n}`;
  const role = `proof-${n}`;
  const text = (step: string) =>
    `import { action, approval, flow } from "holon"\n\nflow("${slug}")\n\nexport const ask = action(async () => {\n  const decision = await approval("ok", { steps: [${step}] })\n  return decision.approved\n})\n`;
  const sent: number[] = [];
  page.on("response", (r) => {
    if (r.request().method() === "POST" && new URL(r.url()).pathname === "/deploy") sent.push(r.status());
  });
  // The admin, who may start its runs, is the one holder of the step's role.
  await sure(control(adminToken, `/roles/${role}`, { allows: ["run.decide"], kinds: ["person"], why: "e2e: a review" }, "PUT"));
  await sure(control(adminToken, "/bindings", { who: { principal: ADMIN.name }, role, scope: { flows: { slug } }, why: "e2e: reviews it" }));
  try {
    await page.goto("/");
    await gotoPage(page, "new");
    const canvas = page.locator(".hl-canvas");
    await canvas.getByRole("button", { name: "text off" }).click();
    const typed = page.getByRole("region", { name: "Text" }).locator(".cm-content");
    await typed.click();
    await page.keyboard.press("Control+A");
    await page.keyboard.insertText(text(`{ role: "${role}" }`));
    await expect(page.locator('[aria-label="action ask"]')).toBeVisible({ timeout: 15_000 });

    // Without `starter`, a run the admin starts can't be decided, and the consent says so; the runs
    // others start can be, so approving it deploys without asking again.
    await canvas.getByRole("button", { name: /^deploy/ }).dispatchEvent("click");
    await page.locator('[aria-modal="true"]').locator("input").fill(slug);
    await page.keyboard.press("Enter");
    const consent = page.getByRole("dialog");
    await expect(consent).toContainText(`approval ok: approved by a person holding ${role}`);
    await expect(consent).toContainText(`a run ${ADMIN.name} starts can't be decided, as they take no step of it`);
    await consent.getByRole("button", { name: "Approve and deploy", exact: true }).click();
    await expect.poll(() => sent).toEqual([200]);
    await expect(page.locator(".hl-tab--active")).toContainText("deployed", { timeout: 30_000 });

    // A version whose step says `starter: true`: a different permission, so its review shows the
    // row changed, in words; ready now, as the admin may take the step of a run they start, it
    // deploys without asking again.
    await typed.click();
    await page.keyboard.press("Control+A");
    await page.keyboard.insertText(text(`{ role: "${role}", starter: true }`));
    // Compiled: what it may do says so (in the bottom pane, closed).
    await expect(page.getByText(`wait for approval "ok" by a person holding ${role}, who may be whoever started the run, or acting for them, and continue later`, { exact: true }).first()).toBeAttached({ timeout: 15_000 });
    // Saved a second after the deploy at least (both are kept in seconds).
    await page.waitForTimeout(1_100);
    const saved = page.waitForResponse((r) => r.request().method() !== "GET" && new URL(r.url()).pathname.startsWith("/flows"));
    await canvas.getByRole("button", { name: "save", exact: true }).click();
    expect((await saved).ok()).toBe(true);
    await page.goto(`/#approvals.${slug}.v2`);
    const may = page.getByRole("region", { name: "what it may do" });
    await expect(may).toContainText(`approval ok: +approved by a person holding ${role}, who may be whoever started the run, or acting for them, −approved by a person holding ${role}`);
    await expect(may).not.toContainText("can't be decided");
    await expect(page.getByRole("group", { name: "Can't be decided here" })).toHaveCount(0);
    const decide = page.getByRole("complementary", { name: "decision" });
    await decide.getByRole("checkbox").check();
    await decide.getByRole("button", { name: "Approve and deploy", exact: true }).click();
    await expect.poll(() => sent).toEqual([200, 200]);
    await expect(page.getByRole("status").filter({ hasText: `Deployed ${slug} v2.` })).toBeVisible();
  } finally {
    await control(adminToken, `/deployments/${slug}`, undefined, "DELETE");
    await control(adminToken, `/flows/${slug}`, undefined, "DELETE");
  }
});

test("while access changes wait for people, an admin's binding of a role that decides waits; another admin approves it in Approvals, and it then holds; one declined from the command line never does", async ({
  page,
  browser,
  adminToken,
}) => {
  const n = Date.now().toString(36);
  const slug = `governed-${n}`;
  deploy(adminToken, slug, `import { action, flow } from "holon"\n\nflow("${slug}")\n\nexport const shout = action((input) => input.toUpperCase())\n`);
  // bob, another admin; dee, a viewer; a role that decides the flow's approvals.
  const [bob, dee, approver] = [`e2e-bob-${n}`, `e2e-dee-${n}`, `e2e-approver-${n}`];
  const bobToken = await joined(adminToken, bob, "admin");
  await joined(adminToken, dee);
  await sure(control(adminToken, `/roles/${approver}`, { allows: ["run.view", "run.decide"], kinds: ["person"], about: "decides" }, "PUT"));
  /** `holon access args…` as `env` signs in: its output. */
  const access = (env: NodeJS.ProcessEnv, args: string[]) => execFileSync(HOLON, ["access", ...args, "--server", String(PORTS.server)], { encoding: "utf8", env, stdio: "pipe" });
  const as = (token: string) => ({ ...process.env, HOLON_TOKEN: token, HOLON_SERVER: "" });
  // This machine's run file: where the setting is turned off at once.
  const local = { ...process.env, HOLON_RUN_DIR: RUN_DIR, HOLON_TOKEN: "", HOLON_SERVER: "" };
  const holding = async () => (await json<{ bindings: { role: string; scope: unknown }[] }>(control(adminToken, `/bindings?who=${dee}`))).bindings.filter((b) => b.role === approver);

  // One person other than its author approves an access change that gives more.
  expect(access(as(adminToken), ["approvals", "--changes", "1", "--why", "e2e: four eyes on access"])).toContain(
    "changes           1    an access change that gives more waits until 1 person other than its author approves it",
  );
  try {
    // The admin binds the role to dee on the flow, through the form on dee's page: it waits.
    await page.goto(`/#people.who.${dee}`);
    const form = page.getByRole("region", { name: `${dee}'s page` }).getByRole("form", { name: `bind a role to ${dee}` });
    await form.getByLabel("bind: role").selectOption(approver);
    await form.getByLabel("bind: where").selectOption("flow");
    await form.getByLabel("bind: flow").fill(slug);
    await form.getByLabel("bind: why").fill("e2e: decides it");
    await form.getByRole("button", { name: "bind", exact: true }).click();
    const what = `bound ${approver} to ${dee} on "${slug}"`;
    await expect(form.getByRole("status")).toContainText(`${what}: waits for 1 person other than you to approve it (access change `);
    await expect(form.getByRole("status").getByRole("link", { name: "Approvals › Access changes" })).toHaveAttribute("href", "#approvals.access");
    expect(await holding()).toEqual([]);

    // History lists it above the versions; holon access proposals, with what it gives and who
    // could act as dee: the admin, who made dee's link.
    await page.goto("/#people.history");
    await expect(page.getByRole("table", { name: "access changes waiting for people" }).getByRole("row").filter({ hasText: what })).toContainText("waits, 0 of 1 approved");
    const listed = access(as(adminToken), ["proposals"]);
    expect(listed).toContain(what);
    // What waits for people of what the role holds: run.decide, not run.view.
    expect(listed).toContain(` gives run.decide to ${dee} on "${slug}", as ${approver}\n`);
    expect(listed).toContain(`${ADMIN.name} made ${dee}'s invite link`);

    // In Approvals, its author withdraws it, and doesn't approve it.
    await page.goto("/#approvals.access");
    const mine = page.getByRole("table", { name: "access changes waiting" }).getByRole("row").filter({ hasText: what });
    await expect(mine.getByRole("button", { name: /^withdraw access change \d+$/ })).toBeEnabled();
    await expect(mine.getByRole("button", { name: /^approve/ })).toHaveCount(0);

    // bob approves it there, reading who could act as dee: it is made, and the binding holds.
    const asBob = await editorAs(browser, bobToken, bob);
    await asBob.goto("/#approvals.access");
    const his = asBob.getByRole("table", { name: "access changes waiting" }).getByRole("row").filter({ hasText: what });
    await expect(his).toContainText(`who could act as whom it gives more, and so doesn't approve it: ${ADMIN.name} made ${dee}'s invite link`);
    await his.getByRole("button", { name: /^approve access change \d+$/ }).click();
    await asBob.locator('[aria-modal="true"]').getByRole("button", { name: "Approve" }).click();
    await expect(asBob.getByText(/the last approval it waited for: made, as access version \d+/)).toBeVisible();
    await expect(asBob.getByRole("table", { name: "access changes decided" }).getByRole("row").filter({ hasText: what })).toContainText(`made as version`);
    expect(await holding()).toEqual([expect.objectContaining({ scope: { flows: { slug } } })]);
    await asBob.context().close();

    // Another, declined by bob from the command line, saying why: it never holds.
    const second = await json<{ proposal: { id: number } }>(control(adminToken, "/bindings", { who: { principal: dee }, role: approver, scope: "host", why: "e2e" }));
    expect(access(as(bobToken), ["decline", String(second.proposal.id), "--reason", "e2e: the flow is enough"])).toContain(
      `declined access change ${second.proposal.id}: bound ${approver} to ${dee} on this host`,
    );
    expect(await holding()).toHaveLength(1);
  } finally {
    // Turned off through this machine's run file, where it applies at once.
    access(local, ["approvals", "--changes", "off", "--why", "e2e: done"]);
  }
  expect((await json<{ approvals: { changes: number | null } }>(control(adminToken, "/access"))).approvals.changes).toBeNull();
});
