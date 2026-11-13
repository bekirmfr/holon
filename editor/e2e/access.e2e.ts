// People and access (PeoplePages.tsx): an admin defines a role and binds roles through the forms;
// a person's page shows what is bound to them and what it lets them do, and they then see the
// flow; unbinding, which says why, takes it away; History says each change, and a version what it
// changed. Someone who holds less than a binding gives is refused it in the server's words. And
// what a principal's page says it may do (access.ts givesOf) is what the server's `can` answers
// them, for each way of being bound. And the buttons follow `can` (can.ts): a viewer's flow page
// has each that changes something disabled, saying why in the server's words, until a binding
// lets them, which the page then follows; someone who may deploy a flow but not approve what it
// asks proposes it, told why, and it waits in Approvals.

import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Browser, Locator } from "@playwright/test";
import { givesOf, type Tagged } from "../src/access";
import type { Principal, RoleEntry } from "../src/server";
import { ADMIN, expect, HOLON, OWN_PORTS, PORTS, SAMPLE_SHIFT, SERVER, test } from "./fixtures";
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
  await expect(mayDo).toContainText("flow view · run view, export · state view · secret list");

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

    // The header: pausing and stopping are an editor's.
    await asVic.goto(`/#${slug}`);
    const flowPage = asVic.locator(`[aria-label="flow ${slug}"]`);
    await disabled(flowPage.getByRole("button", { name: "Pause", exact: true }), reads);
    await disabled(flowPage.getByRole("button", { name: "Stop", exact: true }), reads);
    // Status: its action is seen, not run.
    const shout = flowPage.locator('[aria-label="action shout"]');
    await disabled(shout.getByRole("button", { name: "run", exact: true }), reads);
    await expect(shout).toContainText(reads);
    // Settings: revoking, whether agents may propose, removing it.
    await asVic.goto(`/#${slug}.settings`);
    const revokes = flowPage.getByRole("button", { name: "Revoke", exact: true });
    await expect(revokes).not.toHaveCount(0);
    for (const revoke of await revokes.all()) await disabled(revoke, reads);
    await expect(flowPage.getByRole("checkbox", { name: "agents may propose versions" })).toBeDisabled();
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
