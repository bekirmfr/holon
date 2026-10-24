// Holon for VS Code (docs/L4s/language-server-L4.md): the language and its grammar are declared in
// package.json; this starts `holon lsp`, which says each mistake as you type, what a word is on
// hover, and formats as `holon fmt` writes. It is started again when `holon.path` changes.

const vscode = require("vscode");
const { LanguageClient } = require("vscode-languageclient/node");

/** @type {LanguageClient | undefined} */
let client;

async function start() {
  const command = vscode.workspace.getConfiguration("holon").get("path") || "holon";
  client = new LanguageClient("holon", "Holon", { command, args: ["lsp"] }, { documentSelector: [{ language: "holon" }] });
  try {
    await client.start();
  } catch (e) {
    client = undefined;
    const why = e instanceof Error ? e.message : String(e);
    vscode.window.showErrorMessage(`Holon: "${command} lsp" didn't start (${why}). Install holon, or set holon.path to where it is.`);
  }
}

async function stop() {
  const running = client;
  client = undefined;
  if (running) await running.stop();
}

/** @param {vscode.ExtensionContext} context */
async function activate(context) {
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration(async (e) => {
      if (e.affectsConfiguration("holon.path")) {
        await stop();
        await start();
      }
    }),
  );
  await start();
}

function deactivate() {
  return stop();
}

module.exports = { activate, deactivate };
