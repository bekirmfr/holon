// A deployed flow's own page (FlowPage.tsx): its tabs, each at an address of its own.

import { expect, test } from "./fixtures";
import { deploy, flowRow, gotoPage, openSample } from "./helpers";

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

  // A run opens its record: how it ended, its steps timed, what came in and went out; checked
  // offline with holon verify, or stepped through in the editor.
  await rows.nth(1).getByRole("link").click();
  await expect(page).toHaveURL(new RegExp(`#hello-api\\.run-${newest}$`));
  const record = page.getByRole("article", { name: `run ${newest}` });
  await expect(record).toContainText("GET /hello/:name");
  await expect(record.getByRole("list", { name: "steps" }).getByRole("listitem").first()).toBeVisible();
  await expect(record.getByRole("region", { name: "in" })).toContainText("/hello/Grace");
  await record.getByRole("button", { name: "Check offline…" }).click();
  const offline = page.getByRole("dialog");
  await expect(offline).toContainText("holon verify hello-api-runs.json --head ");
  await offline.getByRole("button", { name: "Cancel" }).click();
  await record.getByRole("button", { name: "Step through in the editor" }).click();
  await expect(page).toHaveURL(new RegExp(`#editor\\.hello-api\\.view\\.run-${newest}$`));
  await expect(page.locator(".hl-view__bottom")).toContainText(`#${newest}`);
});

test("a flow's status: its last day, what it may since when and by whom, its runs and versions; paused and resumed", async ({ page, request }) => {
  await openSample(page, "Hello API");
  const port = await deploy(page);
  expect((await request.get(`http://127.0.0.1:${port}/hello/Ada`)).status()).toBe(200);

  await page.goto("/#hello-api");
  await expect(page.getByRole("navigation", { name: "flow tabs" }).getByRole("link", { name: "Status" })).toHaveAttribute("aria-current", "page");
  await expect(page.getByRole("region", { name: "last 24 hours" })).toContainText("runs");
  await expect(page.getByRole("region", { name: "where it runs" })).toContainText(`:${port}`);
  const may = page.getByRole("region", { name: "it may" });
  await expect(may.getByRole("listitem").filter({ hasText: "receive requests" })).toContainText(/since v\d+.* · approved by e2e/);
  await expect(page.getByRole("region", { name: "latest runs" }).getByRole("row").first()).toContainText("GET /hello/:name");
  await expect(page.getByRole("region", { name: "versions" })).toContainText("deployed now");

  const head = page.getByLabel("flow hello-api");
  await head.getByRole("button", { name: "Pause", exact: true }).click();
  await expect(head.locator(".hl-flow__status")).toContainText("paused");
  await head.getByRole("button", { name: "Resume", exact: true }).click();
  await expect(head.locator(".hl-flow__status")).toContainText("running");
});

test("a flow's settings: a permission revoked pauses it and granted back runs it, and it is removed by typing its name", async ({ page, request }) => {
  await openSample(page, "Hello API");
  const port = await deploy(page);
  await page.goto("/#hello-api.settings");
  const head = page.getByLabel("flow hello-api");
  const permissions = page.getByRole("region", { name: "permissions" });
  const ingress = permissions.getByRole("listitem").filter({ hasText: "receive requests" });
  await ingress.getByRole("button", { name: "Revoke" }).click();
  await expect(permissions.locator(".hl-flow__revoked")).toContainText("receive requests");
  await expect(head.locator(".hl-flow__status")).toContainText("paused");
  await permissions.getByRole("button", { name: "Grant back" }).click();
  await expect(permissions.getByRole("listitem").filter({ hasText: "receive requests" }).getByRole("button", { name: "Revoke" })).toBeVisible();
  // Granted back, it runs again by itself.
  await expect(head.locator(".hl-flow__status")).toContainText("running");
  expect((await request.get(`http://127.0.0.1:${port}/hello/Ada`)).status()).toBe(200);
  await expect(page.getByRole("region", { name: "sessions" }).getByRole("row").first()).toBeVisible();

  // Removed: once stopped, by typing its name.
  const remove = page.getByRole("region", { name: "remove" });
  await expect(remove.getByRole("button", { name: "Remove hello-api" })).toBeDisabled();
  await head.getByRole("button", { name: "Stop", exact: true }).click();
  await expect(head.locator(".hl-flow__status")).toContainText("stopped");
  await remove.getByRole("textbox").fill("hello-api");
  await remove.getByRole("button", { name: "Remove hello-api" }).click();
  await expect(page).toHaveURL(/#flows$/);
});

test("Flows: saved and deployed in one table, each with its state, filtered and found; a row opens its page", async ({ page }) => {
  await openSample(page, "Hello API");
  await deploy(page);
  await gotoPage(page, "flows");
  const row = flowRow(page, "hello-api");
  await expect(row).toContainText("live");
  await expect(row.getByRole("cell").nth(2)).toHaveText(/^\d+$/);
  const show = page.getByRole("group", { name: "show" });
  await show.getByRole("button", { name: /^Paused/ }).click();
  await expect(row).toHaveCount(0);
  await show.getByRole("button", { name: /^Live/ }).click();
  await page.getByLabel("search flows").fill("hello-a");
  await expect(row).toBeVisible();
  await page.getByLabel("search flows").fill("no-such-flow");
  await expect(page.getByRole("region", { name: "flows" })).toContainText('no flow matches "no-such-flow"');
  await page.getByLabel("search flows").fill("");
  await row.getByRole("link", { name: "hello-api", exact: true }).click();
  await expect(page).toHaveURL(/#hello-api$/);
});

test("the Library: samples found by what they do, one's details with its command line, opened as an unsaved copy", async ({ page }) => {
  await page.goto("/#library");
  const samples = page.getByRole("region", { name: "samples" });
  await page.getByRole("group", { name: "show flows that" }).getByRole("button", { name: "use a database" }).click();
  await expect(samples.getByRole("article", { name: "Orders API (a database on the host)" })).toBeVisible();
  await expect(samples.getByRole("article", { name: "Hello API" })).toHaveCount(0);
  // Keywords, in any order: "claims" is one of what it shows, "payment" in its description.
  await page.getByLabel("search samples").fill("claims payment");
  await expect(samples.getByRole("article")).toHaveCount(1);
  await samples.getByRole("article").getByRole("heading").getByRole("button").click();
  const details = page.getByRole("dialog");
  await expect(details).toContainText("holon new --sample refund-once");
  await details.getByRole("button", { name: "Open an unsaved copy" }).click();
  await expect(page.locator(".hl-tab--active")).toContainText("refund-once");

  await page.getByRole("navigation", { name: "main" }).getByRole("link", { name: "Library", exact: true }).click();
  await page.getByRole("navigation", { name: "library tabs" }).getByRole("link", { name: "Tutorials" }).click();
  await expect(page).toHaveURL(/#library\.tutorials$/);
  await page.getByRole("region", { name: "tutorials" }).getByRole("link", { name: "Getting started" }).click();
  await expect(page).toHaveURL(/#docs\.getting-started\.md$/);
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
