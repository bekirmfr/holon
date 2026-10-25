// Accounts, from installing a server onwards (holon-server's accounts.rs, access.rs, auth.rs; the
// editor's AuthGate.tsx, AdminPage.tsx), on a server left as installed (7521) and the editor
// pointed at it (5274): set up with the code it prints, people invited and joining by link,
// each role doing what it may, an API token, and signing in through an identity provider set
// in the admin panel (the stand-in on 7530). In order: each step builds on the one before.

import { expect, test, type Browser, type Page } from "@playwright/test";

const EDITOR = "http://localhost:5274";
const SERVER = "http://127.0.0.1:7521";
const PASSWORD = "a long enough password";

test.describe.configure({ mode: "serial" });

/** A browser of its own, as another person on another machine would have. */
async function someoneElse(browser: Browser): Promise<Page> {
  return (await browser.newContext()).newPage();
}

async function fill(page: Page, label: string, value: string) {
  await page.getByLabel(label, { exact: true }).fill(value);
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
  await expect(page.getByRole("button", { name: "account: ada" })).toBeVisible();
  // Now everyone signs in.
  expect((await request.get(`${SERVER}/flows`)).status()).toBe(401);

  // The admin panel: add cy as a viewer, with the email she signs in with at the provider.
  await page.getByRole("button", { name: "admin", exact: true }).click();
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
  await expect(cy.getByRole("button", { name: "account: cy" })).toBeVisible();
  await expect(cy.getByRole("button", { name: "admin", exact: true })).toHaveCount(0);
  await cy.getByRole("button", { name: "new", exact: true }).click();
  await cy.locator('[aria-modal="true"] input').fill("cys-flow");
  await cy.keyboard.press("Enter");
  await cy.locator(".hl-canvas").getByRole("button", { name: "save", exact: true }).click();
  await expect(cy.locator(".hl-canvas")).toContainText("cy may not do that: a viewer only reads");
  // The link is used up.
  const again = await someoneElse(browser);
  await again.goto(invite);
  await expect(again.getByRole("form", { name: "sign in" }).getByRole("alert")).toContainText("used, expired or wrong");
});

test("an admin's API token acts with its role, until revoked; a disabled account is signed out", async ({ browser, request }) => {
  const ada = await someoneElse(browser);
  await ada.goto(EDITOR);
  const signIn = ada.getByRole("form", { name: "sign in" });
  await fill(ada, "name", "ada");
  await fill(ada, "password", PASSWORD);
  await signIn.getByRole("button", { name: "sign in" }).click();
  await ada.getByRole("button", { name: "admin", exact: true }).click();
  await ada.getByLabel("new token's label").fill("ci");
  await ada.getByLabel("new token's role").selectOption("editor");
  await ada.getByRole("button", { name: "make token" }).click();
  const token = await ada.getByLabel("new API token").inputValue();
  const asCi = { Authorization: `Bearer ${token}` };
  expect((await request.post(`${SERVER}/flows`, { data: { name: "from-ci", document: { nodes: [], edges: [] } }, headers: asCi })).status()).toBe(200);
  expect((await request.get(`${SERVER}/admin/users`, { headers: asCi })).status()).toBe(403);
  await ada.getByRole("button", { name: "done" }).click();
  await ada.locator("div", { hasText: /^ci/ }).getByRole("button", { name: "revoke" }).first().click();
  await ada.locator('[aria-modal="true"]').getByRole("button", { name: "Revoke" }).click();
  await expect(ada.getByText("no API tokens", { exact: true })).toBeVisible();
  expect((await request.get(`${SERVER}/flows`, { headers: asCi })).status()).toBe(401);
  // The activity log names who did what.
  await expect(ada.getByText(/made an API token ci · by ada/)).toBeVisible();
  // An agent's token proposes, and never approves, stops or deletes.
  await ada.getByLabel("new token's label").fill("claude");
  await ada.getByLabel("new token's role").selectOption("agent");
  await ada.getByRole("button", { name: "make token" }).click();
  const asAgent = { Authorization: `Bearer ${await ada.getByLabel("new API token").inputValue()}` };
  await ada.getByRole("button", { name: "done" }).click();
  expect((await request.post(`${SERVER}/flows`, { data: { name: "from-claude", document: { nodes: [], edges: [] } }, headers: asAgent })).status()).toBe(200);
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
  await fill(cy, "name", "cy");
  await fill(cy, "password", PASSWORD);
  await cy.getByRole("form", { name: "sign in" }).getByRole("button", { name: "sign in" }).click();
  await expect(cy.getByRole("button", { name: "account: cy" })).toBeVisible();
  const cyRow = ada.locator("div", { has: ada.getByLabel("cy's role") }).last();
  await cyRow.getByRole("button", { name: "disable" }).click();
  await expect(cyRow).toContainText("· disabled");
  await cy.getByRole("button", { name: "deployed", exact: true }).click({ timeout: 5_000 }).catch(() => {});
  await expect(cy.getByRole("form", { name: "sign in" })).toBeVisible();
  await fill(cy, "name", "cy");
  await fill(cy, "password", PASSWORD);
  await cy.getByRole("form", { name: "sign in" }).getByRole("button", { name: "sign in" }).click();
  await expect(cy.getByRole("form", { name: "sign in" }).getByRole("alert")).toContainText("don't match");
  await cyRow.getByRole("button", { name: "enable" }).click();
  await expect(cyRow).not.toContainText("· disabled");
});

test("signing in through the identity provider an admin set, matched by email", async ({ browser }) => {
  const ada = await someoneElse(browser);
  await ada.goto(EDITOR);
  await fill(ada, "name", "ada");
  await fill(ada, "password", PASSWORD);
  await ada.getByRole("form", { name: "sign in" }).getByRole("button", { name: "sign in" }).click();
  await ada.getByRole("button", { name: "admin", exact: true }).click();
  await ada.getByLabel("issuer").fill("http://127.0.0.1:7530");
  await ada.getByLabel("client id").fill("holon-e2e");
  await ada.getByRole("button", { name: "turn on" }).click();
  await expect(ada.getByText("on: http://127.0.0.1:7530 as holon-e2e")).toBeVisible();

  const cy = await someoneElse(browser);
  await cy.goto(EDITOR);
  await cy.getByRole("button", { name: "sign in through 127.0.0.1:7530" }).click();
  await cy.getByRole("link", { name: "sign in as cy@example.com" }).click();
  await expect(cy.getByRole("button", { name: "account: cy" })).toBeVisible();

  // Someone the provider vouches for, without an account here, is refused.
  const eve = await someoneElse(browser);
  await eve.goto(EDITOR);
  await eve.getByRole("button", { name: "sign in through 127.0.0.1:7530" }).click();
  await eve.getByRole("link", { name: "sign in as eve@example.com" }).click();
  await expect(eve.getByRole("form", { name: "sign in" }).getByRole("alert")).toContainText("has no account here");
});
