// Holon for VS Code (docs/L4s/language-server-L4.md): a flow is a TypeScript file, `NAME.holon.ts`,
// which VS Code highlights as TypeScript; this starts `holon lsp` for those files, which says each
// mistake as you type, what a word is on hover, and formats as `holon fmt` writes. It is started
// again when `holon.path` changes. The Holon sidebar (sidebar.js) shows the server's deployments and
// saved flows as VS Code's own views, with their history, text and graph.

const vscode = require("vscode");
const { LanguageClient } = require("vscode-languageclient/node");
const sidebar = require("./sidebar");

/** @type {LanguageClient | undefined} */
let client;
/** @type {ReturnType<typeof sidebar.register> | undefined} */
let views;

async function start() {
  const command = vscode.workspace.getConfiguration("holon").get("path") || "holon";
  client = new LanguageClient("holon", "Holon", { command, args: ["lsp"] }, { documentSelector: [{ scheme: "file", pattern: "**/*.holon.ts" }, { scheme: "holon", pattern: "**/*.holon.ts" }] });
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
  views = sidebar.register(context);
  await start();
  // The extension's API: what the tests reach the sidebar's listings through.
  return { sidebar: () => views };
}

function deactivate() {
  sidebar.stop();
  return stop();
}

module.exports = { activate, deactivate };
