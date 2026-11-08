// A deployed flow's own page (FlowPage.tsx): its tabs, each at an address of its own.

import { expect, test } from "./fixtures";
import { deploy, openSample } from "./helpers";

test("a flow's runs: newest first, filtered, the chain checked, a run opened", async ({ page, request }) => {
  await openSample(page, "Hello API");
  const port = await deploy(page);
  for (const name of ["Ada", "Grace"]) expect((await request.get(`http://127.0.0.1:${port}/hello/${name}`)).status()).toBe(200);

  await page.goto("/#hello-api.runs");
  await expect(page.getByRole("navigation", { name: "main" }).getByRole("link", { name: "Flows", exact: true })).toHaveAttribute("aria-current", "page");
  const tabs = page.getByRole("navigation", { name: "flow tabs" });
  await expect(tabs.getByRole("link", { name: "Runs" })).toHaveAttribute("aria-current", "page");
  const runs = page.getByRole("region", { name: "runs" });
  const rows = runs.getByRole("row");
  await expect(rows.nth(1)).toContainText("GET /hello/:name");
  await expect(rows.nth(1)).toContainText("200");
  const newest = Number((await rows.nth(1).getByRole("link").textContent())!.slice(1));
  const next = Number((await rows.nth(2).getByRole("link").textContent())!.slice(1));
  expect(newest).toBeGreaterThan(next);

  await runs.getByRole("button", { name: /^Failed/ }).click();
  await expect(runs).toContainText("no failed runs");
  await runs.getByRole("button", { name: /^All/ }).click();

  await runs.getByRole("button", { name: "Check the chain" }).click();
  await expect(runs.getByRole("status")).toContainText("the chain is unbroken");

  // A run opens its record: until it has a page of its own, in the editor, viewed.
  await rows.nth(1).getByRole("link").click();
  await expect(page).toHaveURL(new RegExp(`#editor\\.hello-api\\.view\\.run-${newest}$`));
});

test("a flow's state: its collection's items now, and the runs that changed one", async ({ page, request }) => {
  await openSample(page, "Notes API");
  const port = await deploy(page);
  // A key that sorts first: on the first page, whatever other tests kept.
  const id = `0-${Date.now()}`;
  expect((await request.put(`http://127.0.0.1:${port}/notes/${id}`, { data: { text: "buy milk" } })).status()).toBeLessThan(300);

  await page.goto("/#notes-api.state");
  const notes = page.getByRole("region", { name: "state" }).getByRole("group", { name: "collection notes" });
  const row = notes.getByRole("row").filter({ hasText: id });
  await expect(row).toContainText("buy milk");
  await row.getByRole("button", { name: /^history of / }).click();
  const history = page.getByRole("dialog", { name: "cell history" });
  await expect(history).toContainText("buy milk");
});
