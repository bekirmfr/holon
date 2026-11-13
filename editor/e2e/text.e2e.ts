// The flow's text beside its canvas (TextPane.tsx, useTextPane.ts; docs/L4s/text-pane-L4.md),
// in the real editor: a mistake is marked while the canvas keeps the last version that compiled,
// a fix drives the canvas and leaves the nodes it didn't touch where they were, a node selected on
// the canvas marks its line, completion offers a request's parts, Format writes what `holon fmt`
// does, and a save keeps the text as typed, comments and all.

import type { Locator, Page } from "@playwright/test";
import { expect, SERVER, test } from "./fixtures";
import { deploy, nodesLevel, openSample } from "./helpers";

/** Replace the whole text, as typing it would. */
async function typeText(editor: Locator, page: Page, text: string) {
  await editor.click();
  await page.keyboard.press("Control+A");
  await page.keyboard.insertText(text);
}

const nodeNamed = (page: Page, name: string) => page.locator(`.react-flow__node[data-id^="${name}-"]`);

/** Pick a node on the canvas. Not a pointer click: where the layout puts it, another node or the
 *  text pane's edge can lie over it. */
async function pick(node: Locator) {
  await node.dispatchEvent("click");
  await node.focus();
}

test("a wide window opens a flow with its text beside the canvas", async ({ page }) => {
  await page.setViewportSize({ width: 1700, height: 900 });
  await openSample(page, "Hello API");
  await expect(page.locator(".hl-canvas").getByRole("button", { name: "text on" })).toBeVisible();
  await expect(page.getByRole("region", { name: "Text" }).locator(".cm-content")).toContainText('flow("hello-api", { port: 8101 })');
});

test("the flow's text drives its canvas, and the canvas its text", async ({ page, request, adminToken }) => {
  await openSample(page, "Hello API");
  await nodesLevel(page);
  // Below 1600px wide (the tests' 1280) the text opens on asking (docs/L4s/text-graph-link-L4.md).
  await page.locator(".hl-canvas").getByRole("button", { name: "text off" }).click();
  const pane = page.getByRole("region", { name: "Text" });
  const editor = pane.locator(".cm-content");
  await expect(editor).toContainText('flow("hello-api", { port: 8101 })');
  await expect(editor).toContainText("(req) => `Hello, ${req.params.name}`");
  const nodes = page.locator(".react-flow__node");
  const before = await nodes.count();
  const requestAt = await nodeNamed(page, "request").getAttribute("style");

  // A mistake: marked where `holon check` says, and the canvas keeps the last version that compiled.
  await typeText(editor, page, 'import { flow, route } from "holon"\n\nflow("hello-api", { port: 8101 })\n\nroute.get("/hello/:name", (req) => nope)\n');
  await expect(pane.getByRole("status")).toContainText('unknown name "nope"');
  await expect(pane.getByRole("status")).toContainText("the last version that compiled");
  await expect(pane.locator(".cm-lintRange-error")).toHaveText("nope");
  await expect(nodes).toHaveCount(before);

  // Fixed, with one call more and a comment: the canvas follows, the request where it was.
  await typeText(
    editor,
    page,
    '// says hello, loudly\nimport { flow, route, uppercase } from "holon"\n\nflow("hello-api", { port: 8101 })\n\nroute.get("/hello/:name", (req) => {\n  return uppercase(`Hello, ${req.params.name}`)\n})\n',
  );
  await expect(pane.getByRole("status")).toHaveCount(0);
  await expect(nodeNamed(page, "uppercase")).toHaveCount(1);
  await expect(nodes).toHaveCount(before + 1);
  expect(await nodeNamed(page, "request").getAttribute("style")).toBe(requestAt);

  // A node selected on the canvas marks its line, and the range it is written over.
  await pick(nodeNamed(page, "uppercase"));
  await expect(pane.locator(".hl-text__picked")).toContainText("uppercase(");
  await expect(pane.locator(".hl-text__mark")).toHaveText("uppercase(`Hello, ${req.params.name}`)");

  // Completion: after `req.`, the request's parts.
  await editor.click();
  await page.keyboard.press("Control+End");
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("End");
  await page.keyboard.insertText("\n  const x = req.");
  await page.keyboard.type("p");
  await expect(page.locator(".cm-tooltip-autocomplete")).toContainText("params");
  await page.keyboard.press("Escape");

  // Format: as `holon fmt` writes it (the half-typed line is a mistake, so first take it out).
  await typeText(
    editor,
    page,
    "// says hello, loudly\nimport { flow, route, uppercase } from 'holon'\n\nflow(  'hello-api', { port: 8101 });\n\nroute.get(\"/hello/:name\", (req) => {\n      return uppercase(`Hello, ${req.params.name}`);\n})\n",
  );
  await page.locator(".hl-canvas").getByRole("button", { name: "format", exact: true }).dispatchEvent("click");
  await expect(editor).toContainText('route.get("/hello/:name", (req) => {  return uppercase(`Hello, ${req.params.name}`)})');
  await expect(editor).toContainText('flow("hello-api", { port: 8101 })');
  await expect(editor).not.toContainText(";");
  await page.screenshot({ path: "test-results/text-pane.png" });

  // Saved with the text as typed. At 1280px with the text, the inspector and the bottom pane open, the
  // canvas is a sliver and its toolbar runs under the text pane: its buttons are pressed as such,
  // not at a point another element covers (a click there once waited out the test).
  await page.locator(".hl-canvas").getByRole("button", { name: "save", exact: true }).dispatchEvent("click");
  await expect(page.getByRole("region", { name: "notices" })).toContainText("saved");
  const saved = await request.get(`${SERVER}/flows/hello-api`, { headers: { Authorization: `Bearer ${adminToken}` } });
  const doc = ((await saved.json()) as { document: { source?: string } }).document;
  expect(doc.source).toContain("// says hello, loudly");
  expect(doc.source).toContain("uppercase(");

  // A canvas change writes the text again: the comment goes, and says so; undo brings it back.
  // The canvas commits an edit 400 ms after it settles (history.ts), and the steps above can each
  // come quicker than that (picking a line picks its node, a change too), folding the text's
  // version into the delete, whose undo then goes back to the sample: so the delete waits until
  // the text's version is a step of its own, which enables undo. Undo is pressed as such: a
  // dispatched click doesn't wait for it to be enabled as a pointer click does.
  // `uppercase` alone: the cursor on its line picked the `return` with it, and a route that
  // answers nothing has no TypeScript form.
  const undo = page.locator(".hl-canvas").getByRole("button", { name: "⟲" });
  await expect(undo).toBeEnabled();
  // A click on the empty canvas lets go of what is selected: a real one, a press and a release,
  // as React Flow takes it once a drag there draws a selection box (canvasGestures.ts).
  await page.locator(".react-flow__pane").click({ position: { x: 8, y: 8 } });
  await pick(nodeNamed(page, "uppercase"));
  await expect(page.locator(".react-flow__node.selected")).toHaveCount(1);
  await page.keyboard.press("Delete");
  await expect(pane.getByRole("status")).toContainText("a comment was dropped");
  await expect(editor).not.toContainText("// says hello");
  await undo.dispatchEvent("click");
  await expect(editor).toContainText("// says hello, loudly");
});

test("the It may strip says what an edit adds to what the flow may do; Problems, Check and find", async ({ page }) => {
  await openSample(page, "Hello API");
  const may = page.getByRole("region", { name: "it may" });
  // Not deployed yet it says what it may once deployed; an earlier test may have deployed it.
  await expect(may).toContainText(/^(Deployed, it may|It may)/);
  await deploy(page);
  await expect(may).toContainText(/^It may/);
  await expect(may.locator(".hl-may__chip--new")).toHaveCount(0);

  // A collection written by the route: a new thing it may do, marked new until it is deployed.
  await page.locator(".hl-canvas").getByRole("button", { name: "text off" }).click();
  const pane = page.getByRole("region", { name: "Text" });
  const editor = pane.locator(".cm-content");
  await typeText(
    editor,
    page,
    'import { flow, route } from "holon"\nimport seen from "collection:seen"\n\nflow("hello-api", { port: 8101 })\n\nroute.get("/hello/:name", (req) => {\n  seen[req.params.name] = true\n  return `Hello, ${req.params.name}`\n})\n',
  );
  await expect(may.locator(".hl-may__chip--new")).not.toHaveCount(0);
  await expect(may.locator(".hl-may__chip--new").first()).toContainText("new");
  await expect(may.locator(".hl-may__chip--new").first()).toHaveAttribute("title", /seen/);

  // A mistake is listed in the bottom pane's Problems as it is typed; check counts it, and a click
  // on it goes there.
  await typeText(editor, page, 'import { flow, route } from "holon"\n\nflow("hello-api", { port: 8101 })\n\nroute.get("/hello/:name", (req) => nope)\n');
  const bottom = page.getByRole("region", { name: "bottom" });
  await expect(bottom.getByRole("tab", { name: /^Problems/ })).toContainText("1");
  await page.locator(".hl-canvas").getByRole("button", { name: "check", exact: true }).dispatchEvent("click");
  const problems = bottom.getByRole("tabpanel", { name: "Problems" });
  await expect(problems.getByRole("status")).toHaveText("Checked: 1 mistake.");
  const listed = problems.getByRole("list", { name: "problems" });
  await expect(listed).toContainText("line 5");
  await expect(listed).toContainText('unknown name "nope"');
  await editor.click();
  await page.keyboard.press("Control+Home");
  await listed.getByRole("button").first().click();
  await expect(pane.locator(".cm-activeLine")).toContainText("nope");

  // Find: Ctrl+F opens the search, looking for what is selected.
  await editor.click();
  await page.keyboard.press("Control+f");
  await expect(pane.locator(".cm-search")).toBeVisible();
});

test("the Inspector says the name at the text's cursor: its forms and a sample's lines", async ({ page }) => {
  await openSample(page, "Hello API");
  await page.locator(".hl-canvas").getByRole("button", { name: "text off" }).click();
  const pane = page.getByRole("region", { name: "Text" });
  await pane.locator(".cm-line", { hasText: "route.get" }).click({ position: { x: 4, y: 6 } });
  const word = page.getByRole("complementary", { name: "Inspector" }).getByRole("region", { name: "the name route" });
  await expect(word).toContainText("Inspector: route");
  await expect(word.locator(".hl-word__forms")).toContainText("route.");
  await expect(word.locator(".hl-word__example")).toContainText("route.");
  await word.getByRole("button", { name: "In the manual →" }).click();
  await expect(page).toHaveURL(/#docs\.language\/reference\.md$/);
});
