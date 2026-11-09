// Holon for VS Code (docs/L4s/language-server-L4.md): a flow is a TypeScript file, `NAME.holon.ts`,
// which VS Code highlights as TypeScript; this starts `holon lsp` for those files, which says each
// mistake as you type, what a word is on hover, and formats as `holon fmt` writes. It is started
// again when `holon.path` changes. `Holon: Open console` shows the console (the editor that
// `holon serve` hosts) in a panel beside the code, starting `holon serve` at a loopback address
// when nothing answers there.

const vscode = require("vscode");
const childProcess = require("node:child_process");
const { LanguageClient } = require("vscode-languageclient/node");

const DEFAULT_CONSOLE = "http://127.0.0.1:7420";

/** @type {LanguageClient | undefined} */
let client;
/** @type {vscode.WebviewPanel | undefined} */
let panel;
/** The `holon serve` this extension started, if it did; it is stopped with the extension. */
/** @type {childProcess.ChildProcess | undefined} */
let server;
/** @type {vscode.OutputChannel | undefined} */
let output;

function holonCommand() {
  return vscode.workspace.getConfiguration("holon").get("path") || "holon";
}

async function start() {
  const command = holonCommand();
  client = new LanguageClient("holon", "Holon", { command, args: ["lsp"] }, { documentSelector: [{ scheme: "file", pattern: "**/*.holon.ts" }] });
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

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** Whether a server answers its health check at `url`. */
async function answers(url) {
  try {
    const res = await fetch(new URL("/health", url), { signal: AbortSignal.timeout(1000) });
    return res.ok;
  } catch {
    return false;
  }
}

/** Starts `holon serve` at `url`'s port on the loopback address, its output in the Holon channel
 *  (a new server prints the setup code for its first admin there). Resolves to why it did not
 *  start, or to undefined once it answers. */
function startServer(url) {
  const port = new URL(url).port || "80";
  output ??= vscode.window.createOutputChannel("Holon");
  output.show(true);
  const child = childProcess.spawn(holonCommand(), ["serve", "--at", `127.0.0.1:${port}`], { stdio: ["ignore", "pipe", "pipe"] });
  server = child;
  child.stdout.on("data", (chunk) => output.append(chunk.toString()));
  child.stderr.on("data", (chunk) => output.append(chunk.toString()));
  return new Promise((resolve) => {
    let done = false;
    const finish = (why) => {
      if (done) return;
      done = true;
      if (server === child) server = undefined;
      resolve(why);
    };
    child.once("error", (e) => finish(e.message));
    child.once("exit", (code) => finish(`holon serve exited (${code})`));
    const deadline = Date.now() + 15000;
    (async () => {
      while (!done) {
        if (await answers(url)) return finish(undefined);
        if (Date.now() > deadline) return finish("it did not answer within 15 s");
        await sleep(250);
      }
    })();
  });
}

/** The console's page: the server's own editor in a frame, which the page's CSP lets through and
 *  nothing else. The frame is the server's origin (asExternalUri maps it for a remote workspace). */
async function consoleHtml(url) {
  const src = (await vscode.env.asExternalUri(vscode.Uri.parse(url))).toString();
  const origin = new URL(src).origin;
  const attribute = src.replace(/&/g, "&amp;").replace(/"/g, "&quot;");
  return `<!doctype html>
<html>
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; frame-src ${origin}; style-src 'unsafe-inline';">
<style>html, body, iframe { margin: 0; width: 100%; height: 100%; border: 0; overflow: hidden; }</style>
</head>
<body>
<iframe src="${attribute}" allow="clipboard-read; clipboard-write"></iframe>
</body>
</html>`;
}

async function openConsole() {
  const url = vscode.workspace.getConfiguration("holon").get("consoleUrl") || DEFAULT_CONSOLE;
  if (!(await answers(url))) {
    const loopback = ["127.0.0.1", "localhost", "[::1]"].includes(new URL(url).hostname);
    if (!loopback) {
      vscode.window.showErrorMessage(`Holon: nothing answers at ${url}. Start its server, or set holon.consoleUrl to where it is.`);
      return;
    }
    const why = await startServer(url);
    if (why) {
      output.show(true);
      vscode.window.showErrorMessage(`Holon: "holon serve" didn't start (${why}). Install holon, or set holon.path to where it is.`);
      return;
    }
  }
  if (!panel) {
    panel = vscode.window.createWebviewPanel("holon.console", "Holon console", vscode.ViewColumn.Active, { enableScripts: false, retainContextWhenHidden: true });
    panel.onDidDispose(() => {
      panel = undefined;
    });
    panel.webview.html = await consoleHtml(url);
  }
  panel.reveal();
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
    vscode.commands.registerCommand("holon.openConsole", openConsole),
  );
  await start();
}

function deactivate() {
  server?.kill();
  return stop();
}

module.exports = { activate, deactivate };
