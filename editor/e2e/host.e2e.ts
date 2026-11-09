// The host's pages (HostPages.tsx), each at an address of its own: a server secret's versions
// under Resources, rolled back after a dry run; the agents' line under People and access; and
// Policy with what it holds off.

import { expect, test } from "./fixtures";
import { gotoPage, navLink } from "./helpers";

test("a server secret's versions, and rolling one back after its dry run", async ({ page }) => {
  const name = `probe_${Date.now().toString(36)}`;
  await page.goto("/");
  await gotoPage(page, "resources");
  await expect(page).toHaveURL(/#resources$/);
  await expect(navLink(page, "Resources")).toHaveAttribute("aria-current", "page");
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

  // A section has its own address.
  await page.goto("/#resources.secrets");
  await expect(page.locator('[data-section="secrets"]')).toBeInViewport();
});

test("people and access says how an agent connects; policy says what it holds off", async ({ page }) => {
  await page.goto("/");
  await gotoPage(page, "people");
  await expect(page.getByRole("heading", { level: 1, name: "People and access" })).toBeVisible();
  await expect(page.getByLabel("the holon mcp line")).toHaveValue(/^holon mcp setup claude-code --server http/);
  await gotoPage(page, "policy");
  await expect(page.getByRole("heading", { level: 1, name: "Policy" })).toBeVisible();
  await expect(page.getByLabel("limits")).toBeVisible();
  await expect(page.locator('[data-section="held-off"]')).toBeVisible();
});
