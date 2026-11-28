// Accounts, from installing a server onwards (holon-server's accounts.rs, access.rs, auth.rs; the
// editor's AuthGate.tsx, HostPages.tsx), on a server left as installed (7521 on slot 0, ports.mjs)
// and the editor pointed at it (5274): set up with the code it prints, people invited and joining by link,
// each role doing what it may, a service's and an agent's tokens (an owner who isn't an admin
// making them on a page of what they own), and signing in through an identity provider set
// in People and access (the stand-in on 7530). In order: each step builds on the one before.

import { expect, test, type Browser, type Page } from "@playwright/test";
import { editorLoaded, FRESH as SERVER, FRESH_EDITOR as EDITOR, ISSUER } from "./fixtures";
import { dropFromPalette, gotoPage, menuEntry, navLink, newFlow, peopleTab } from "./helpers";

const PASSWORD = "a long enough password";

test.describe.configure({ mode: "serial" });

/** A browser of its own, as another person on another machine would have. */
async function someoneElse(browser: Browser): Promise<Page> {
  return (await browser.newContext()).newPage();
}

async function fill(page: Page, label: string, value: string) {
  await page.getByLabel(label, { exact: true }).fill(value);
}

/** `name` signs in with the password, from the sign-in form `page` shows: once the editor has
 *  loaded, its core too (fixtures.ts's `editorLoaded`). */
async function signIn(page: Page, name: string) {
  await fill(page, "name", name);
  await fill(page, "password", PASSWORD);
  await page.getByRole("form", { name: "sign in" }).getByRole("button", { name: "sign in", exact: true }).click();
  await editorLoaded(page);
}

let invite = "";

test("installing: the server answers nothing until its admin is set up with the printed code", async ({ page, request }) => {
  expect((await request.get(`${SERVER}/flows`)).status()).toBe(403);
  await page.goto(EDITOR);
  const setup = page.getByRole("form", { name: "set up" });
  await expect(setup).toBeVisible();
  await fill(page, "setup code", "WRONG-CODE");
  await fill(page, "your name", "ada");
  await fill(page, "password (at least 10 characters)", PASSWORD);
  await fill(page, "the password again", PASSWORD);
  await setup.getByRole("button", { name: "set up" }).click();
  await expect(setup.getByRole("alert")).toContainText("isn't the setup code");
  await fill(page, "setup code", "E2E0-SETUP");
  await setup.getByRole("button", { name: "set up" }).click();
  // Set up, the first admin is asked where the runs a log lets go of go, while no one has chosen:
  // a folder, none, or later, which leaves the host's warning (what each choice writes is held by
  // AuthGate.backup.test.tsx and the server's tests).
  const backup = page.getByRole("form", { name: "where runs go" });
  await expect(backup).toBeVisible();
  await expect(backup.getByRole("button", { name: "none: delete them" })).toBeVisible();
  await backup.getByRole("button", { name: "later" }).click();
  await editorLoaded(page);
  await expect(page.getByRole("button", { name: "account: ada" })).toBeVisible();
  // Now everyone signs in.
  expect((await request.get(`${SERVER}/flows`)).status()).toBe(401);

  // People and access: add cy as a viewer, with the email she signs in with at the provider.
  await gotoPage(page, "people");
  await page.getByLabel("new person's name").fill("cy");
  await page.getByLabel("new person's role").selectOption("viewer");
  await page.getByLabel("new person's email").fill("cy@example.com");
  await page.getByRole("button", { name: "add person" }).click();
  invite = await page.getByLabel("invite link").inputValue();
  expect(invite).toContain("?invite=hi_");
});

test("joining by the link, a viewer reads and can't change anything", async ({ browser }) => {
  const cy = await someoneElse(browser);
  await cy.goto(invite);
  const join = cy.getByRole("form", { name: "join" });
  await expect(join).toContainText("You were invited as cy");
  await fill(cy, "password (at least 10 characters)", PASSWORD);
  await fill(cy, "the password again", PASSWORD);
  await join.getByRole("button", { name: "join" }).click();
  await editorLoaded(cy);
  await expect(cy.getByRole("button", { name: "account: cy" })).toBeVisible();
  await expect(menuEntry(cy, "resources")).toHaveCount(0);
  await expect(menuEntry(cy, "people")).toHaveCount(0);
  // Save is disabled for her, saying why as the server does (can.tsx); the server's own refusal
  // is held by its tests (crates/holon/src/tests.rs).
  await gotoPage(cy, "new");
  const save = cy.locator(".hl-canvas").getByRole("button", { name: "save", exact: true });
  await expect(save).toBeDisabled();
  await expect(save).toHaveAttribute("title", /a viewer only reads/);
  // Policy she reads (a viewer holds policy.view), and can't read the file again.
  await gotoPage(cy, "policy");
  await expect(cy.getByRole("heading", { level: 1, name: "Policy" })).toBeVisible();
  await expect(cy.getByLabel("limits")).toContainText("The calls out one run may make.");
  await expect(cy.getByRole("button", { name: "read the policy again" })).toBeDisabled();
  // The file itself is for whoever views the host: she is told why it isn't shown, in the
  // server's words, and no line of it (R74).
  await expect(cy.getByText("the policy file itself is read only by those who may view the host")).toBeVisible();
  await expect(cy.getByLabel("the policy file")).toHaveCount(0);
  // The link is used up.
  const again = await someoneElse(browser);
  await again.goto(invite);
  await expect(again.getByRole("form", { name: "sign in" }).getByRole("alert")).toContainText("used, expired or wrong");
});

test("a service's token acts with its role, until revoked; a disabled person is signed out", async ({ browser, request }) => {
  const ada = await someoneElse(browser);
  await ada.goto(EDITOR);
  await signIn(ada, "ada");
  await gotoPage(ada, "people");
  await peopleTab(ada, "Agents and tokens");
  // ci, a service ada owns, signs in with a token of its own.
  const add = async (name: string, kind: string, role: string) => {
    await ada.getByLabel("new agent's or service's name").fill(name);
    await ada.getByLabel("new agent's or service's kind").selectOption(kind);
    await ada.getByLabel("new agent's or service's role").selectOption(role);
    await ada.getByRole("button", { name: `add ${kind}` }).click();
    await expect(ada.getByLabel(`${name}'s owner`)).toHaveValue("ada");
  };
  const tokenFor = async (name: string, label: string) => {
    await ada.getByRole("button", { name: `make a token for ${name}` }).click();
    await ada.locator('[aria-modal="true"] input').fill(label);
    await ada.locator('[aria-modal="true"]').getByRole("button", { name: "make token" }).click();
    const token = await ada.getByLabel("new API token").inputValue();
    await ada.getByRole("button", { name: "done" }).click();
    return { Authorization: `Bearer ${token}` };
  };
  await add("ci", "service", "editor");
  await expect(ada.getByText("ci has no token: it can't sign in yet")).toBeVisible();
  const asCi = await tokenFor("ci", "a script");
  expect((await request.post(`${SERVER}/flows`, { data: { name: "from-ci", text: 'flow("from-ci")' }, headers: asCi })).status()).toBe(200);
  expect((await request.get(`${SERVER}/people`, { headers: asCi })).status()).toBe(403);
  await ada.getByRole("button", { name: "revoke ci's token a script" }).click();
  await ada.locator('[aria-modal="true"]').getByRole("button", { name: "Revoke" }).click();
  await expect(ada.getByText("ci has no token: it can't sign in yet")).toBeVisible();
  expect((await request.get(`${SERVER}/flows`, { headers: asCi })).status()).toBe(401);
  // The activity log names who did what.
  await expect(ada.getByText(/made a token for ci · by ada/)).toBeVisible();
  // An agent's token proposes, and never approves, stops or deletes.
  await add("claude", "agent", "agent");
  const asAgent = await tokenFor("claude", "claude code");
  expect((await request.post(`${SERVER}/flows`, { data: { name: "from-claude", text: 'flow("from-claude")' }, headers: asAgent })).status()).toBe(200);
  const approving = await request.post(`${SERVER}/deploy`, { data: { slug: "from-claude", unit: "0".repeat(64), acknowledge: ["NetworkIngress"] }, headers: asAgent });
  expect(approving.status()).toBe(403);
  expect((await approving.json()).error).toContain("an agent doesn't approve");
  expect((await request.post(`${SERVER}/deployments/from-claude/stop`, { data: {}, headers: asAgent })).status()).toBe(403);
  expect((await request.delete(`${SERVER}/flows/from-claude`, { headers: asAgent })).status()).toBe(403);

  // cy, signed in elsewhere, is disabled: her next step finds her signed out (or the editor's own
  // poll of the deployments, every few seconds, does first: a 401 there signs her out too, so the
  // button she would click may already be gone), and she can't sign in again.
  const cy = await someoneElse(browser);
  await cy.goto(EDITOR);
  await signIn(cy, "cy");
  await expect(cy.getByRole("button", { name: "account: cy" })).toBeVisible();
  await peopleTab(ada, "People");
  const cyRow = ada.locator("div", { has: ada.getByLabel("cy's role") }).last();
  // Asked first, saying what it does: one click no longer ends someone's sign-ins (R43).
  await cyRow.getByRole("button", { name: "disable" }).click();
  const asked = ada.locator('[aria-modal="true"]');
  await expect(asked).toContainText("Disable cy? Their sign-ins end, and they can't sign in until enabled again.");
  await asked.getByRole("button", { name: "Disable", exact: true }).click();
  await expect(cyRow).toContainText("· disabled");
  await menuEntry(cy, "library").click({ timeout: 5_000 }).catch(() => {});
  await expect(cy.getByRole("form", { name: "sign in" })).toBeVisible();
  await fill(cy, "name", "cy");
  await fill(cy, "password", PASSWORD);
  await cy.getByRole("form", { name: "sign in" }).getByRole("button", { name: "sign in" }).click();
  await expect(cy.getByRole("form", { name: "sign in" }).getByRole("alert")).toContainText("don't match");
  await cyRow.getByRole("button", { name: "enable" }).click();
  await expect(asked).toContainText("Enable cy? They can sign in again.");
  await asked.getByRole("button", { name: "Enable", exact: true }).click();
  await expect(cyRow).not.toContainText("· disabled");
});

test("an owner who isn't an admin makes and revokes their agent's tokens, on a page of what they own", async ({ browser, request }) => {
  // ada adds scout, an agent cy owns.
  const ada = await someoneElse(browser);
  await ada.goto(EDITOR);
  await signIn(ada, "ada");
  await gotoPage(ada, "people");
  await peopleTab(ada, "Agents and tokens");
  await ada.getByLabel("new agent's or service's name").fill("scout");
  await ada.getByLabel("new agent's or service's kind").selectOption("agent");
  await ada.getByLabel("new agent's or service's owner").selectOption("cy");
  await ada.getByRole("button", { name: "add agent" }).click();
  await expect(ada.getByLabel("scout's owner")).toHaveValue("cy");

  // cy, a viewer, has no host's pages but what she owns: scout, and none of ada's.
  const cy = await someoneElse(browser);
  await cy.goto(EDITOR);
  await signIn(cy, "cy");
  await expect(navLink(cy, "People and access")).toHaveCount(0);
  await navLink(cy, "Your agents and services").click();
  const page = cy.locator('[aria-label="Your agents and services"]');
  await expect(page).toContainText("scout has no token: it can't sign in yet");
  for (const theirs of ["claude", "ci"]) await expect(page.getByText(theirs, { exact: true })).toHaveCount(0);

  // She makes it a token, shown once, which signs in as scout; revoked, it doesn't.
  await cy.getByRole("button", { name: "make a token for scout" }).click();
  await cy.locator('[aria-modal="true"] input').fill("her laptop");
  await cy.locator('[aria-modal="true"]').getByRole("button", { name: "make token" }).click();
  const asScout = { Authorization: `Bearer ${await cy.getByLabel("new API token").inputValue()}` };
  await cy.getByRole("button", { name: "done" }).click();
  await expect(cy.getByLabel("new API token")).toHaveCount(0);
  expect(await (await request.get(`${SERVER}/auth/me`, { headers: asScout })).json()).toMatchObject({ name: "scout", kind: "agent", owner: "cy" });
  await expect(page).toContainText("a token of scout, made by cy");
  await cy.getByRole("button", { name: "revoke scout's token her laptop" }).click();
  await cy.locator('[aria-modal="true"]').getByRole("button", { name: "Revoke" }).click();
  await expect(page).toContainText("scout has no token: it can't sign in yet");
  expect((await request.get(`${SERVER}/auth/me`, { headers: asScout })).status()).toBe(401);
  // ada's People and access says who made and revoked it.
  await ada.reload();
  await editorLoaded(ada);
  await expect(ada.getByText(/revoked a token of scout · by cy/)).toBeVisible();
});

test("signing in through the identity provider an admin set, matched by email", async ({ browser }) => {
  const ada = await someoneElse(browser);
  await ada.goto(EDITOR);
  await signIn(ada, "ada");
  await gotoPage(ada, "people");
  await peopleTab(ada, "Signing in");
  // The tab reads the settings it has when it opens and puts them in these fields (PeoplePages.tsx's
  // SignIn): a read answering after the fill empties them, and "turn on" stays disabled. So fill
  // and click again until the click lands, which it does only with both fields filled.
  await expect(async () => {
    await ada.getByLabel("issuer").fill(ISSUER);
    await ada.getByLabel("client id").fill("holon-e2e");
    await ada.getByRole("button", { name: "turn on" }).click({ timeout: 1_000 });
  }).toPass();
  await expect(ada.getByText(`on: ${ISSUER} as holon-e2e`)).toBeVisible();

  const cy = await someoneElse(browser);
  await cy.goto(EDITOR);
  await cy.getByRole("button", { name: `sign in through ${new URL(ISSUER).host}` }).click();
  await cy.getByRole("link", { name: "sign in as cy@example.com", exact: true }).click();
  await editorLoaded(cy);
  await expect(cy.getByRole("button", { name: "account: cy" })).toBeVisible();

  // Someone the provider vouches for, without an account here, is refused.
  const eve = await someoneElse(browser);
  await eve.goto(EDITOR);
  await eve.getByRole("button", { name: `sign in through ${new URL(ISSUER).host}` }).click();
  await eve.getByRole("link", { name: "sign in as eve@example.com" }).click();
  await expect(eve.getByRole("form", { name: "sign in" }).getByRole("alert")).toContainText("has no account here");
});

/** A flow changed and not saved, in a tab of its own: a node dropped on a new draft. */
async function unsavedDraft(page: Page) {
  await newFlow(page);
  await dropFromPalette(page, "uppercase");
  await expect(page.locator(".hl-tab--active .hl-tab__dirty")).toBeVisible();
}

test("a sign-in that ends keeps the open flows: signing in again goes on with them; another tab's sign-in leaves whom this one acts as", async ({ browser, request }) => {
  const context = await browser.newContext();
  const ada = await context.newPage();
  await ada.goto(EDITOR);
  await signIn(ada, "ada");
  await expect(ada.getByRole("button", { name: "account: ada" })).toBeVisible();
  // Her session's token, as the browser keeps it now, to sign it out elsewhere below.
  const token = await ada.evaluate(() => (JSON.parse(localStorage.getItem("holon.session") ?? "null") as { token?: string } | null)?.token ?? "");
  await unsavedDraft(ada);

  // Another tab of the same browser signs in as cy, a viewer: the browser keeps cy's session, and
  // ada's tab still acts as ada (it read the browser's on every request, so it acted as cy).
  const other = await context.newPage();
  await other.addInitScript(() => localStorage.removeItem("holon.session"));
  await other.goto(EDITOR);
  await signIn(other, "cy");
  await expect(other.getByRole("button", { name: "account: cy" })).toBeVisible();
  await other.close();
  await gotoPage(ada, "people");
  await peopleTab(ada, "People");
  await expect(ada.getByLabel("cy's role"), "the people list, which ada may read and cy may not").toBeVisible();

  // Ada's sign-in ends elsewhere (her session signed out): the editor's next read is refused, and
  // a sign-in covers it, the editor kept beneath.
  expect((await request.post(`${SERVER}/auth/sign-out`, { data: {}, headers: { Authorization: `Bearer ${token}` } })).status()).toBe(204);
  const again = ada.getByRole("dialog", { name: "sign in again" });
  await expect(again).toContainText("Your sign-in ended", { timeout: 15_000 });
  await expect(again.getByLabel("name", { exact: true })).toHaveValue("ada");
  await again.getByLabel("password", { exact: true }).fill(PASSWORD);
  await again.getByRole("button", { name: "sign in", exact: true }).click();
  await expect(again).toHaveCount(0);
  // Back where she was, the draft still open and still not saved.
  await expect(ada.getByLabel("cy's role")).toBeVisible();
  await navLink(ada, "Editor").click();
  await expect(ada.getByRole("tablist", { name: "open tabs" }).locator(".hl-tab__dirty")).toHaveCount(1);
  await context.close();
});

test("a provider's sign-in that expires is signed in again in a pop-up, the open flows kept", async ({ browser }) => {
  const cy = await someoneElse(browser);
  await cy.goto(EDITOR);
  await cy.getByRole("button", { name: `sign in through ${new URL(ISSUER).host}` }).click();
  // An ID token good for 90 s: the editor holds it expired half a minute early, a minute in, time
  // enough on a slow runner to load the editor and open a draft before it does.
  await cy.getByRole("link", { name: "sign in as cy@example.com for 90 s", exact: true }).click();
  await editorLoaded(cy);
  await expect(cy.getByRole("button", { name: "account: cy" })).toBeVisible();
  await unsavedDraft(cy);
  const again = cy.getByRole("dialog", { name: "sign in again" });
  await expect(again).toContainText("Your sign-in ended", { timeout: 90_000 });
  const popup = cy.waitForEvent("popup");
  await again.getByRole("button", { name: `sign in through ${new URL(ISSUER).host}` }).click();
  const provider = await popup;
  await provider.getByRole("link", { name: "sign in as cy@example.com", exact: true }).click();
  await provider.waitForEvent("close");
  await expect(again).toHaveCount(0);
  await expect(cy.getByRole("button", { name: "account: cy" })).toBeVisible();
  await expect(cy.getByRole("tablist", { name: "open tabs" }).locator(".hl-tab__dirty")).toHaveCount(1);
});
