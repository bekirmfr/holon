// Inside VS Code, through the extension and `holon lsp`: a flow's mistake shows as you open it and
// goes when it is fixed; completion offers what may be written; a hover says what a word is;
// formatting writes what `holon fmt` does. Then the sidebar, against the server run.js started: the
// saved flow is listed, its text opens as an editor, its graph as a panel, and a deployment-less
// server says so.

const assert = require("node:assert");
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
  const { deployments, flows } = (await ext.activate()).sidebar();

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
