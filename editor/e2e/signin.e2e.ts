// Signing in through an identity provider (holon-server's oidc.rs, the editor's signin.ts),
// against the stand-in provider and the server that uses it (playwright.config.ts): the editor
// on 5274, its server on 7521.

import { expect, test, type Page } from "@playwright/test";

const EDITOR = "http://localhost:5274";

/** Sign in as `email` at the stand-in provider, from the editor's header. */
async function signInAs(page: Page, email: string) {
  await page.getByRole("button", { name: /^sign in/ }).click();
  await page.getByRole("dialog", { name: "server access" }).getByRole("button", { name: "sign in" }).click();
  await page.getByRole("link", { name: `sign in as ${email}` }).click();
  await expect(page.getByRole("button", { name: `${email} ●` })).toBeVisible();
}

test("signing in: the provider says who, the users file what they may do", async ({ page, request }) => {
  // Before signing in, the server answers nothing but where to sign in.
  expect((await request.get("http://127.0.0.1:7521/flows")).status()).toBe(401);
  expect((await request.get("http://127.0.0.1:7521/auth/config")).status()).toBe(200);
  await page.goto(EDITOR);
  await signInAs(page, "ada@example.com");

  // As ada (an admin): a saved flow names her.
  await page.getByRole("button", { name: "new", exact: true }).click();
  await page.locator('[aria-modal="true"] input').fill("signed");
  await page.keyboard.press("Enter");
  await expect(page.locator(".hl-tab--active")).toContainText("signed");
  await page.locator(".hl-canvas").getByRole("button", { name: "save", exact: true }).click();
  await expect(page.locator(".hl-canvas")).toContainText("saved");
  await page.getByRole("button", { name: "flows", exact: true }).click();
  await expect(page.getByText(/ by ada$/)).toBeVisible();

  // Signed out, then in as cy (a viewer): reading works, saving is refused.
  await page.getByRole("button", { name: "ada@example.com ●" }).click();
  await page.getByRole("button", { name: "sign out" }).click();
  await signInAs(page, "cy@example.com");
  await page.getByRole("button", { name: "flows", exact: true }).click();
  await expect(page.getByText("signed", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "new", exact: true }).click();
  await page.locator('[aria-modal="true"] input').fill("not-allowed");
  await page.keyboard.press("Enter");
  await expect(page.locator(".hl-tab--active")).toContainText("not-allowed");
  await page.locator(".hl-canvas").getByRole("button", { name: "save", exact: true }).click();
  await expect(page.locator(".hl-canvas")).toContainText("cy is a viewer: reads only");
});

test("someone the provider vouches for but the server doesn't name is refused", async ({ page }) => {
  await page.goto(EDITOR);
  await signInAs(page, "eve@example.com");
  await page.getByRole("button", { name: "flows", exact: true }).click();
  await expect(page.getByText("signed in as eve@example.com, who isn't a user of this server")).toBeVisible();
});
