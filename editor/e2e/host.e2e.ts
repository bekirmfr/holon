// The host's pages (HostPages.tsx), each at an address of its own: Resources' tabs, each at its
// own, and a server secret's versions under Secrets, rolled back after a dry run; Collections and
// channels; the agents' line under People and access; Policy with what it holds off, what each
// limit means, its file, and the check of flows against it; and Resources as a role holding
// resource.view alone sees it.

import type { APIResponse } from "@playwright/test";
import { expect, FOLDERS, SERVER, test } from "./fixtures";
import { gotoPage, navLink, resourcesTab } from "./helpers";

test("a server secret's versions, and rolling one back after its dry run", async ({ page }) => {
  const name = `probe_${Date.now().toString(36)}`;
  await page.goto("/");
  await gotoPage(page, "resources");
  await expect(page).toHaveURL(/#resources$/);
  await expect(navLink(page, "Resources")).toHaveAttribute("aria-current", "page");
  await resourcesTab(page, "Secrets");
  await expect(page).toHaveURL(/#resources\.secrets$/);
  const bind = async (value: string) => {
    await page.getByLabel("secret name").fill(name);
    await page.getByLabel("secret value").fill(value);
    await page.getByRole("button", { name: "bind secret" }).click();
  };
  await bind("first-test-value");
  const row = page.getByLabel(`server secret ${name}`);
  await expect(row).toContainText("v1");
  await bind("second-test-value");
  await expect(row).toContainText("v2");

  // Its versions: v2 current, v1 rolled back to once the dry run is approved.
  await page.getByRole("button", { name: `versions of ${name}` }).click();
  await page.getByRole("button", { name: "roll back" }).click();
  const dry = page.locator('[aria-modal="true"]');
  await expect(dry).toContainText(`Roll back the secret ${name} of the server to version 1?`);
  await dry.getByRole("button", { name: "Roll back" }).click();
  await expect(page.getByText("version 1 is current again, as version 3")).toBeVisible();
  await expect(row).toContainText("v3");

  // Kept where an env variable names it: the field asks for the name, not a value.
  await page.getByLabel("where its value is kept").selectOption("env");
  await expect(page.getByLabel("secret value")).toHaveAttribute("type", "text");
  await expect(page.getByLabel("secret value")).toHaveAttribute("placeholder", "HOLON_SECRET_STRIPE_KEY");

  // Each tab has its own address; one the server doesn't serve yet says so.
  await page.goto("/#resources.who-may");
  await expect(page.getByLabel("show what")).toBeVisible();
  await page.goto("/#resources.mail");
  // Its explanation, which shows whether or not a mail is bound (samples bind some on this server).
  await expect(page.getByText("The mail flows send:")).toBeVisible();
  await resourcesTab(page, "Feeds");
  await expect(page.getByText("The feeds flows listen to:")).toBeVisible();
  // Databases come first.
  await resourcesTab(page, "Databases");
  await expect(page.getByLabel("database name")).toBeVisible();
});

test("people and access says how an agent connects; policy says what it holds off", async ({ page }) => {
  await page.goto("/");
  await gotoPage(page, "people");
  await expect(page.getByRole("heading", { level: 1, name: "People and access" })).toBeVisible();
  await expect(page.getByLabel("the holon mcp line")).toHaveValue(/^holon mcp setup claude-code --server http/);
  await gotoPage(page, "policy");
  await expect(page.getByRole("heading", { level: 1, name: "Policy" })).toBeVisible();
  await expect(page.getByLabel("limits")).toBeVisible();
  await expect(page.getByText("agents may propose: a version of theirs that needs a person waits in Approvals")).toBeVisible();
  await expect(page.locator('[data-section="held-off"]')).toBeVisible();
  // What each limit means, beside it; and the file as the server read it (server.mjs writes one).
  await expect(page.getByLabel("limits")).toContainText("The calls out one run may make.");
  await expect(page.getByLabel("the policy file")).toContainText("allow_plain_http = true");
  // Checking flows against it says which deployments run within it.
  await page.getByRole("button", { name: "check flows against it" }).click();
  await expect(page.getByRole("region", { name: "flows against the policy" })).toContainText(/\d+ of \d+ deployments? run within it/);
});

test("Collections and channels says what each flow keeps by name", async ({ page }) => {
  await page.goto("/");
  await gotoPage(page, "resources");
  await resourcesTab(page, "Collections and channels");
  await expect(page).toHaveURL(/#resources\.collections$/);
  await expect(page.getByText("What each flow keeps on this host by name")).toBeVisible();
  // Read from each flow's state: a table, or that none keeps one (whichever samples this server runs).
  await expect(page.getByRole("table", { name: "collections and channels" }).or(page.getByText("no flow keeps a collection or sends on a channel"))).toBeVisible();
});

test("a role holding resource.view alone sees Resources, binds nothing, and isn't shown where a folder or a database is", async ({ browser, request, adminToken }) => {
  const { mkdirSync } = await import("node:fs");
  const { join } = await import("node:path");
  const stamp = Date.now().toString(36);
  const [role, person, folder, database] = [`resource-reader-${stamp}`, `rr-${stamp}`, `rr_folder_${stamp}`, `rr_db_${stamp}`];
  const admin = { Authorization: `Bearer ${adminToken}` };
  // In the server's folders, which server.mjs empties when it starts: the server may hold the
  // database's file open after it is unbound.
  const dir = join(FOLDERS, `rr-${stamp}`);
  mkdirSync(dir, { recursive: true });
  const ok = async (sent: Promise<APIResponse>) => {
    const answer = await sent;
    expect(answer.ok(), await answer.text()).toBe(true);
    return answer;
  };
  // A role an admin defines, holding resource.view and nothing else; a person holding it; a
  // folder and a database bound to look at.
  await ok(request.put(`${SERVER}/roles/${role}`, { headers: admin, data: { about: "reads the host's resources", allows: ["resource.view"], kinds: ["person"], why: "e2e" } }));
  await ok(request.post(`${SERVER}/admin/folders`, { headers: admin, data: { name: folder, path: dir, access: "read", keep: "bytes", cap: null, deployment: null } }));
  await ok(request.post(`${SERVER}/admin/databases`, { headers: admin, data: { name: database, path: join(dir, "rr.db"), access: "read", schema: "create table notes (id integer primary key)", deployment: null } }));
  const invite = ((await (await ok(request.post(`${SERVER}/people`, { headers: admin, data: { name: person, kind: "person", role } }))).json()) as { invite: string }).invite;
  const token = ((await (await ok(request.post(`${SERVER}/auth/invite`, { data: { token: invite, password: "a long enough password" } }))).json()) as { token: string }).token;
  const context = await browser.newContext({ baseURL: test.info().project.use.baseURL });
  try {
    // The server leaves out where they are for whoever may not bind (L4 Q4).
    const theirs = { Authorization: `Bearer ${token}` };
    const folders = (await (await ok(request.get(`${SERVER}/admin/folders`, { headers: theirs }))).json()) as Record<string, unknown>[];
    expect(folders.find((f) => f.name === folder)).not.toHaveProperty("path");
    const databases = (await (await ok(request.get(`${SERVER}/admin/databases`, { headers: theirs }))).json()) as Record<string, unknown>[];
    const db = databases.find((d) => d.name === database)!;
    for (const where of ["path", "connection", "address"]) expect(db).not.toHaveProperty(where);

    await context.addInitScript((session) => localStorage.setItem("holon.session", session), JSON.stringify({ token, name: person, kind: "person", role: null }));
    const page = await context.newPage();
    await page.goto("/");
    await expect(page.getByRole("button", { name: `account: ${person}` })).toBeVisible();
    // Resources is theirs to read; People and access and Policy aren't.
    await expect(navLink(page, "Resources")).toBeVisible();
    await expect(navLink(page, "People and access")).toHaveCount(0);
    await gotoPage(page, "resources");
    await resourcesTab(page, "Folders");
    const row = page.getByRole("table", { name: "folder bound" }).getByLabel(`folder ${folder}`, { exact: true });
    await expect(row).toContainText("read only");
    await expect(row.locator(".hl-resources__details")).toContainText("—");
    await expect(page.getByText(dir)).toHaveCount(0);
    // Its buttons and the bind form are disabled, saying why as the server does.
    const unbind = page.getByRole("button", { name: `unbind ${folder}` });
    await expect(unbind).toBeDisabled();
    await expect(unbind).toHaveAttribute("title", new RegExp(`${person} may not do that: .*resource\.unbind`));
    await expect(page.getByRole("button", { name: "bind folder" })).toBeDisabled();
    await expect(page.locator("fieldset.hl-gated", { has: page.getByRole("button", { name: "bind folder" }) })).toHaveAttribute("title", /may not do that: .*resource\.bind/);

    await resourcesTab(page, "Databases");
    const dbRow = page.getByRole("table", { name: "database bound" }).getByLabel(`database ${database}`, { exact: true });
    await expect(dbRow).toContainText("sqlite · read only");
    await expect(dbRow.locator(".hl-resources__details")).toContainText("—");
    await expect(page.getByText(join(dir, "rr.db"))).toHaveCount(0);
    await expect(page.getByRole("button", { name: `unbind ${database}` })).toBeDisabled();
    await expect(page.getByRole("button", { name: "bind database" })).toBeDisabled();
  } finally {
    await context.close();
    // What it made goes: the person (and what is bound to them), then the role, and the bindings.
    for (const at of [`people/${person}`, `roles/${role}?why=e2e`, `admin/folders/${folder}`, `admin/databases/${database}`]) {
      expect.soft((await request.delete(`${SERVER}/${at}`, { headers: admin })).status(), at).toBe(204);
    }
  }
});
