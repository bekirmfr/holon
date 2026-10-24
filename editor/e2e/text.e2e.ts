// The flow's text beside its canvas (TextPane.tsx, useTextPane.ts; docs/L4s/text-pane-L4.md),
// in the real editor: a mistake is marked while the canvas keeps the last version that compiled,
// a fix drives the canvas and leaves the nodes it didn't touch where they were, a node selected on
// the canvas marks its line, completion offers a request's parts, Format writes what `holon fmt`
// does, and a save keeps the text as typed, comments and all.

import type { Locator, Page } from "@playwright/test";
import { expect, SERVER, test } from "./fixtures";

async function openSample(page: Page, title: string) {
  await page.goto("/");
  await page.getByRole("button", { name: "flows", exact: true }).click();
  const card = page.getByText(title, { exact: true }).locator("xpath=../..");
  await card.getByRole("button", { name: "open" }).click();
  await expect(page.locator(".hl-tab--active")).toBeVisible();
}

/** Replace the whole text, as typing it would. */
async function typeText(editor: Locator, page: Page, text: string) {
  await editor.click();
  await page.keyboard.press("Control+A");
  await page.keyboard.insertText(text);
}

const nodeNamed = (page: Page, name: string) => page.locator(`.react-flow__node[data-id^="${name}-"]`);

test("the flow's text drives its canvas, and the canvas its text", async ({ page, request, adminToken }) => {
  await openSample(page, "Hello API");
  await page.locator(".hl-canvas").getByRole("button", { name: "text off" }).click();
  const pane = page.getByRole("region", { name: "Text" });
  const editor = pane.locator(".cm-content");
  await expect(editor).toContainText("flow hello-api");
  await expect(editor).toContainText('respond "Hello, {req.params.name}"');
  const nodes = page.locator(".react-flow__node");
  const before = await nodes.count();
  const requestAt = await nodeNamed(page, "request").getAttribute("style");

  // A mistake: marked where `holon check` says, and the canvas keeps the last version that compiled.
  await typeText(editor, page, 'flow hello-api\nport 8101\n\nroute GET /hello/:name (req) {\n  respond nope\n}\n');
  await expect(pane.getByRole("status")).toContainText('unknown name "nope"');
  await expect(pane.getByRole("status")).toContainText("the last version that compiled");
  await expect(pane.locator(".cm-lintRange-error")).toHaveText("nope");
  await expect(nodes).toHaveCount(before);

  // Fixed, with one call more and a comment: the canvas follows, the request where it was.
  await typeText(editor, page, '# says hello, loudly\nflow hello-api\nport 8101\n\nroute GET /hello/:name (req) {\n  respond uppercase("Hello, {req.params.name}")\n}\n');
  await expect(pane.getByRole("status")).toHaveCount(0);
  await expect(nodeNamed(page, "uppercase")).toHaveCount(1);
  await expect(nodes).toHaveCount(before + 1);
  expect(await nodeNamed(page, "request").getAttribute("style")).toBe(requestAt);

  // A node selected on the canvas marks its line.
  await nodeNamed(page, "uppercase").click();
  await expect(pane.locator(".hl-text__picked")).toContainText("uppercase(");

  // Completion: after `req.`, the request's parts.
  await editor.click();
  await page.keyboard.press("Control+End");
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("End");
  await page.keyboard.insertText("\n  x = req.");
  await page.keyboard.type("p");
  await expect(page.locator(".cm-tooltip-autocomplete")).toContainText("params");
  await page.keyboard.press("Escape");

  // Format: as `holon fmt` writes it (the half-typed line is a mistake, so first take it out).
  await typeText(editor, page, '# says hello, loudly\nflow   hello-api\nport 8101\nroute GET /hello/:name (req) {\n      respond uppercase("Hello, {req.params.name}")\n}\n');
  await pane.getByRole("button", { name: "Format" }).click();
  await expect(editor).toContainText('route GET /hello/:name (req) {  respond uppercase("Hello, {req.params.name}")}');
  await expect(editor).toContainText("flow hello-apiport 8101");
  await page.screenshot({ path: "test-results/text-pane.png" });

  // Saved with the text as typed.
  await page.locator(".hl-canvas").getByRole("button", { name: "save", exact: true }).click();
  await expect(page.locator(".hl-canvas")).toContainText("saved");
  const saved = await request.get(`${SERVER}/flows/hello-api`, { headers: { Authorization: `Bearer ${adminToken}` } });
  const doc = ((await saved.json()) as { document: { source?: string } }).document;
  expect(doc.source).toContain("# says hello, loudly");
  expect(doc.source).toContain("uppercase(");

  // A canvas change writes the text again: the comment goes, and says so; undo brings it back.
  await nodeNamed(page, "uppercase").click();
  await page.keyboard.press("Delete");
  await expect(pane.getByRole("status")).toContainText("a comment was dropped");
  await expect(editor).not.toContainText("# says hello");
  await page.locator(".hl-canvas").getByRole("button", { name: "⟲" }).click();
  await expect(editor).toContainText("# says hello, loudly");
});
