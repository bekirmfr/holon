// Inside VS Code, through the extension and `holon lsp`: a flow's mistake shows as you open it and
// goes when it is fixed; a hover says what a word is; formatting writes what `holon fmt` does.

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

async function run() {
  const uri = vscode.Uri.joinPath(vscode.workspace.workspaceFolders[0].uri, "hello.holon");
  const doc = await vscode.workspace.openTextDocument(uri);
  await vscode.window.showTextDocument(doc);
  assert.equal(doc.languageId, "holon");

  // The mistake, where holon check says it is: `nope`, line 4 (3 from 0), from column 15.
  const [mistake] = await eventually(() => vscode.languages.getDiagnostics(uri).length > 0 && vscode.languages.getDiagnostics(uri), "the mistake");
  assert.equal(mistake.message, 'unknown name "nope"');
  assert.equal(mistake.source, "holon");
  assert.deepEqual([mistake.range.start.line, mistake.range.start.character, mistake.range.end.character], [3, 14, 18]);

  // Fixed: the mistake goes.
  const fix = new vscode.WorkspaceEdit();
  fix.replace(uri, mistake.range, '"Hello, {req.params.name}"');
  assert.ok(await vscode.workspace.applyEdit(fix));
  await eventually(() => vscode.languages.getDiagnostics(uri).length === 0, "the mistake to go");

  // A hover on `req` says what a request holds; one on `flow`, what the flow may do.
  const hoverAt = async (line, character) => {
    const hovers = await vscode.commands.executeCommand("vscode.executeHoverProvider", uri, new vscode.Position(line, character));
    return hovers.flatMap((h) => h.contents.map((c) => (typeof c === "string" ? c : c.value))).join("\n");
  };
  assert.match(await hoverAt(2, 25), /\.params/);
  const flow = await hoverAt(0, 1);
  assert.match(flow, /GET \/hello\/:name/);
  assert.match(flow, /receive requests/);

  // Formatting: as holon fmt writes it.
  const edits = await vscode.commands.executeCommand("vscode.executeFormatDocumentProvider", uri, { tabSize: 2, insertSpaces: true });
  // (VS Code may split the server's one edit of the whole text into smaller ones.)
  assert.ok(edits && edits.length > 0, `edits: ${JSON.stringify(edits)}`);
  const formatting = new vscode.WorkspaceEdit();
  formatting.set(uri, edits);
  assert.ok(await vscode.workspace.applyEdit(formatting));
  assert.equal(doc.getText(), 'flow hello\nport 8200\n\nroute GET /hello/:name (req) {\n  respond "Hello, {req.params.name}"\n}\n');
}

module.exports = { run };
