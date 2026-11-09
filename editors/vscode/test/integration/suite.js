// Inside VS Code, through the extension and `holon lsp`: a flow's mistake shows as you open it and
// goes when it is fixed; completion offers what may be written; a hover says what a word is;
// formatting writes what `holon fmt` does. Then the sidebar, against the server run.js started: the
// saved flow is listed, its text opens as an editor, its graph as a panel, and a deployment-less
// server says so.

const assert = require("node:assert");
const path = require("node:path");
const vscode = require("vscode");

/** `get()`'s first truthy answer, asked every 100 ms for up to 20 s. */
async function eventually(get, what) {
  for (let i = 0; i < 200; i++) {
    const v = await get();
    if (v) return v;
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`timed out waiting for ${what}`);
}

async function checks() {
  const uri = vscode.Uri.joinPath(vscode.workspace.workspaceFolders[0].uri, "hello.holon.ts");
  const doc = await vscode.workspace.openTextDocument(uri);
  await vscode.window.showTextDocument(doc);
  // A flow is TypeScript to VS Code; holon lsp adds what Holon says of it.
  assert.equal(doc.languageId, "typescript");
  const holonSays = () => vscode.languages.getDiagnostics(uri).filter((d) => d.source === "holon");

  // The mistake, where holon check says it is: `nope`, line 5 (4 from 0), from column 39.
  const [mistake] = await eventually(() => holonSays().length > 0 && holonSays(), "the mistake");
  assert.equal(mistake.message, 'unknown name "nope"');
  assert.deepEqual([mistake.range.start.line, mistake.range.start.character, mistake.range.end.character], [4, 38, 42]);

  // Completion: where a name starts, Holon's vocabulary among what may be written, each said.
  const list = await vscode.commands.executeCommand("vscode.executeCompletionItemProvider", uri, new vscode.Position(4, 38));
  const status = list.items.find((i) => i.label === "status" && /vocabulary/.test(i.detail ?? ""));
  assert.ok(status, `offered: ${list.items.map((i) => i.label).join(", ")}`);

  // Fixed: the mistake goes.
  const fix = new vscode.WorkspaceEdit();
  fix.replace(uri, mistake.range, "`Hello, ${req.params.name}`");
  assert.ok(await vscode.workspace.applyEdit(fix));
  await eventually(() => holonSays().length === 0, "the mistake to go");

  // A hover on `req` says what a request holds; one on `flow`, what the flow may do.
  const hoverAt = async (line, character) => {
    const hovers = await vscode.commands.executeCommand("vscode.executeHoverProvider", uri, new vscode.Position(line, character));
    return hovers.flatMap((h) => h.contents.map((c) => (typeof c === "string" ? c : c.value))).join("\n");
  };
  assert.match(await hoverAt(4, 28), /\.params/);
  const flow = await hoverAt(2, 1);
  assert.match(flow, /GET \/hello\/:name/);
  assert.match(flow, /receive requests/);

  // Formatting: as holon fmt writes it.
  const edits = await vscode.commands.executeCommand("vscode.executeFormatDocumentProvider", uri, { tabSize: 2, insertSpaces: true });
  // (VS Code may split the server's one edit of the whole text into smaller ones.)
  assert.ok(edits && edits.length > 0, `edits: ${JSON.stringify(edits)}`);
  const formatting = new vscode.WorkspaceEdit();
  formatting.set(uri, edits);
  assert.ok(await vscode.workspace.applyEdit(formatting));
  assert.equal(doc.getText(), 'import { flow, route } from "holon"\n\nflow("hello", { port: 8200 })\n\nroute.get("/hello/:name", (req) => `Hello, ${req.params.name}`)\n');
}

/** The sidebar: the saved flow is listed; opening it shows its text as TypeScript; its graph opens as
 *  a panel; with no deployments the view says so rather than showing nothing. */
async function sidebarChecks() {
  const ext = vscode.extensions.getExtension("bekirmfr.holon");
  if (!ext) throw new Error(`no bekirmfr.holon among: ${vscode.extensions.all.map((e) => e.id).join(", ")}`);
  const { deployments, flows, activity, runs: auditRuns, history: historyPanel, review: reviewDeploy, grant: reviewGrant } = (await ext.activate()).sidebar();

  const item = await eventually(async () => (await flows.getChildren()).find((i) => i.label === "hello"), "the saved flow hello in the Flows view");
  assert.equal(item.contextValue, "flow");

  // The deployment is listed; its recent runs are a group under it, and a run's record opens as JSON.
  const deployed = await eventually(async () => (await deployments.getChildren()).find((i) => i.label === "hello"), "the deployment hello");
  const runs = (await deployments.getChildren(deployed)).find((g) => g.label === "Recent runs");
  assert.ok(runs, "a Recent runs group under the deployment");
  const [run] = await eventually(async () => {
    const children = await deployments.getChildren(runs);
    return children.length > 0 ? children : null;
  }, "the run of the call");
  assert.match(run.label, /GET \/hello\/:name/);
  await vscode.commands.executeCommand("holon.showRun", "hello", run.seq);
  const record = await eventually(() => vscode.workspace.textDocuments.find((d) => d.uri.scheme === "holon" && d.uri.path === `/run/hello/${run.seq}.json`), "the run's record");
  assert.match(record.getText(), /"route": "GET \/hello\/:name"/);

  await vscode.commands.executeCommand("holon.showFlow", item);
  const text = await eventually(() => vscode.workspace.textDocuments.find((d) => d.uri.scheme === "holon" && d.uri.path === "/flow/hello.holon.ts"), "the flow's text as an editor");
  assert.equal(text.languageId, "typescript");
  assert.match(text.getText(), /flow\("hello"/);

  await vscode.commands.executeCommand("holon.openGraph", item);
  const graph = await eventually(() => vscode.window.tabGroups.all.flatMap((group) => group.tabs).find((tab) => tab.label === "Graph: hello"), "the graph panel");
  assert.ok(graph.input instanceof vscode.TabInputWebview);

  // The server's activity is listed, the deploy of hello among it.
  const events = await eventually(async () => {
    const children = await activity.getChildren();
    return children.length > 0 ? children : null;
  }, "the server's activity");
  assert.ok(events.some((e) => /deployed hello/.test(e.label)), `activity: ${events.map((e) => e.label).join(", ")}`);

  // Runs: the Runs panel lists the chosen deployment's runs; a run is selected to show its record, and the failed-only filter
  // hides the runs that succeeded.
  const deployedHello = (await deployments.getChildren()).find((i) => i.label === "hello");
  await vscode.commands.executeCommand("holon.showRuns", deployedHello);
  const helloRun = auditRuns.snapshot().runs.find((r) => /GET \/hello/.test(r.label));
  assert.ok(helloRun, "the GET /hello run in the Runs panel");
  auditRuns.receive({ type: "select", seq: helloRun.seq });
  assert.equal(auditRuns.snapshot().selected, helloRun.seq, "the run is selected in the panel");
  assert.match(auditRuns.recordOf(helloRun.seq).startedBy, /GET \/hello/, "the record names the call");
  // Steps: the run's steps are read through `holon runs --run --json`: the flow's text with each statement's part.
  const steps = await auditRuns.explainOf(helloRun.seq);
  assert.ok(steps.value && steps.value.steps.length > 0, `the steps of the GET /hello run: ${steps.error ?? "none"}`);
  assert.ok(steps.value.text.some((line) => /hello/.test(line)), "the flow's text is among the steps");
  auditRuns.receive({ type: "failedOnly", value: true });
  assert.ok(!auditRuns.snapshot().runs.some((r) => /GET \/hello/.test(r.label)), "the successful run hidden");
  auditRuns.receive({ type: "failedOnly", value: false });
  assert.ok(auditRuns.snapshot().runs.some((r) => /GET \/hello/.test(r.label)), "the run back in the list");
  // History: the timeline lists the run and the deploy; going back to what is live is refused, and a restore's dry run
  // is only a report, which a cancel drops.
  await vscode.commands.executeCommand("holon.showHistory", deployedHello);
  // The runs are a filter of their own (the web's history hides them until asked).
  await historyPanel.receive({ type: "filter", name: "runs", value: true });
  const helloRows = historyPanel.snapshot().rows;
  assert.ok(helloRows.some((r) => /GET \/hello/.test(r.text)), `the GET /hello run in the history: ${historyPanel.snapshot().error ?? helloRows.map((r) => r.text).join(" | ")}`);
  const live = helloRows.find((r) => /deploy/.test(r.text));
  assert.ok(live, "the deploy in the history");
  await historyPanel.receive({ type: "select", key: live.key });
  assert.ok(historyPanel.snapshot().selected.fields.length > 0, "the picked row has its details");
  await historyPanel.receive({ type: "restore", key: live.key, code: true, state: true });
  assert.match(historyPanel.snapshot().note, /Nothing to restore/, "what is live has nothing to go back to");
  // A state restore of the counter's /hit run: its dry run says what the state would do, and a cancel drops it.
  const counterDeployed = (await deployments.getChildren()).find((i) => i.label === "counter");
  await vscode.commands.executeCommand("holon.showHistory", counterDeployed);
  const hit = historyPanel.snapshot().rows.find((r) => /\/hit/.test(r.text));
  assert.ok(hit, "the /hit run in the counter's history");
  await historyPanel.receive({ type: "restore", key: hit.key, state: true });
  const report = historyPanel.snapshot().report;
  assert.ok(report && report.sections.some((s) => s.heading === "state"), "a state restore's dry run says what the state does");
  await historyPanel.receive({ type: "cancel" });
  assert.equal(historyPanel.snapshot().report, null, "cancelled: the report is dropped");
  // Approved: the same restore, put back for real.
  await historyPanel.receive({ type: "restore", key: hit.key, state: true });
  await historyPanel.receive({ type: "approve" });
  assert.match(historyPanel.snapshot().note, /^Restored/, "approved: the state is put back");
  // A deploy's review, before an approval: the text it would deploy, beside the text deployed now.
  const review = await reviewDeploy("hello");
  assert.match(review.new_text, /flow\("hello"/, "the text it would deploy");
  assert.equal(typeof review.deployed_text, "string", "the text deployed now");
  assert.ok(Array.isArray(review.touches) && Array.isArray(review.asks), "what it touches and what it asks");
  assert.ok(Array.isArray(review.items) && Array.isArray(review.withheld), "the permissions its version asks for");
  // A grant's review, before an approval: what is granted now, and what it would be granted with the item.
  const grantReview = await reviewGrant("hello", "Clock");
  assert.ok(grantReview.after.includes("Clock"), "approving adds the item");
  assert.ok(grantReview.granted.every((item) => grantReview.after.includes(item)), "what is granted stays granted");
  // Explain: the flow's outline (its routes) opens as a document; manifest: what it touches and may do.
  const hello = (await flows.getChildren()).find((i) => i.label === "hello");
  await vscode.commands.executeCommand("holon.explainFlow", hello);
  const outline = await eventually(() => vscode.window.activeTextEditor && vscode.window.activeTextEditor.document.getText().includes("hello: what it does") ? vscode.window.activeTextEditor.document.getText() : null, "the outline of hello");
  assert.match(outline, /GET \/hello/);
  await vscode.commands.executeCommand("holon.manifestFlow", hello);
  const manifest = await eventually(() => vscode.window.activeTextEditor && vscode.window.activeTextEditor.document.getText().includes("nothing is deployed") ? vscode.window.activeTextEditor.document.getText() : null, "the manifest of hello");
  assert.match(manifest, /What it touches/);
  // A deployment that keeps state: its State group lists the value it kept.
  const counter = await eventually(async () => (await deployments.getChildren()).find((i) => i.label === "counter"), "the counter deployment");
  assert.equal(counter.contextValue, "deployment-running", "a running deployment says so, for its buttons");
  // A click on a deployment shows its runs and its history in the bottom panels; its arrow expands its overview.
  assert.equal(counter.command.command, "holon.showDeployment", "a click shows the deployment's runs and history");
  await vscode.commands.executeCommand(counter.command.command, ...counter.command.arguments);
  assert.equal(auditRuns.snapshot().slug, "counter", "the runs panel shows the deployment");
  assert.equal(historyPanel.snapshot().slug, "counter", "the history panel shows the deployment");
  const overview = await deployments.getChildren(counter);
  assert.equal(overview[0].label, "Status", "the overview starts with Status");
  const status = await deployments.getChildren(overview[0]);
  assert.equal(status[0].label, "running", "Status says how it runs");
  const kept = (await deployments.getChildren(counter)).find((g) => g.label === "State");
  assert.ok(kept, "a State group under the counter deployment");
  const values = await eventually(async () => {
    const children = await deployments.getChildren(kept);
    return children.some((i) => i.label === "hits") ? children : null;
  }, "the hits value under State");
  assert.ok(values.some((i) => i.label === "hits"));

  // New flow: the command writes a starter and opens it, without asking (given a name and a folder).
  const folder = vscode.workspace.workspaceFolders[0].uri.fsPath;
  await vscode.commands.executeCommand("holon.newFlow", "fresh-flow", folder);
  const fresh = await eventually(() => vscode.workspace.textDocuments.find((d) => d.uri.fsPath === path.join(folder, "fresh-flow.holon.ts")), "the new flow opened");
  assert.equal(fresh.languageId, "typescript");
  assert.match(fresh.getText(), /flow\(/);
  // Saved on the server, the new flow is listed in the Flows view.
  const config = vscode.workspace.getConfiguration("holon");
  require("node:child_process").execFileSync(config.get("path"), ["flows", "save", path.join(folder, "fresh-flow.holon.ts"), "--server", config.get("consoleUrl")], { stdio: "inherit" });
  const listed = await eventually(async () => (await flows.getChildren()).find((i) => i.label === "fresh-flow"), "the saved new flow in the Flows view");
  assert.equal(listed.contextValue, "flow");

  // A flow file the server does not keep is listed under Not on the server; saving it adds it to the server.
  const fsModule = require("node:fs");
  const localFile = path.join(folder, "local-only.holon.ts");
  fsModule.writeFileSync(localFile, fsModule.readFileSync(path.join(folder, "fresh-flow.holon.ts"), "utf8"));
  const pending = await eventually(async () => (await flows.getChildren()).find((i) => i.label === "Not on the server"), "the Not on the server group");
  const local = (await flows.getChildren(pending)).find((i) => i.label === "local-only");
  assert.ok(local, "local-only listed under Not on the server");
  await vscode.commands.executeCommand("holon.saveToServer", local);
  await eventually(async () => (await flows.getChildren()).find((i) => i.label === "local-only" && i.contextValue === "flow"), "local-only on the server");

  // Editable: an edit saved from the editor is written to the server, and read back from it.
  const at = text.getText().indexOf("Hello");
  const edit = new vscode.WorkspaceEdit();
  edit.replace(text.uri, new vscode.Range(text.positionAt(at), text.positionAt(at + "Hello".length)), "Howdy");
  assert.ok(await vscode.workspace.applyEdit(edit));
  assert.ok(await text.save(), "the flow saved to the server");
  assert.match(Buffer.from(await vscode.workspace.fs.readFile(text.uri)).toString(), /Howdy/);
}

/** The checks; what failed is written to the workspace's failure.txt for run.js to print, since
 *  VS Code's test host doesn't always pass its console through. */
async function run() {
  try {
    await checks();
    await sidebarChecks();
  } catch (e) {
    const file = require("node:path").join(vscode.workspace.workspaceFolders[0].uri.fsPath, "failure.txt");
    require("node:fs").writeFileSync(file, e instanceof Error ? e.stack ?? e.message : String(e));
    throw e;
  }
}

module.exports = { run };
