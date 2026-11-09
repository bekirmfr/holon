// The Holon sidebar: the deployments and the saved flows of the server `holon.consoleUrl` names,
// as VS Code's own views, and what they open as VS Code's own editors and panels: a deployment's
// history and a flow's text as editors, a flow's graph as a panel, and run, pause and stop through
// the command. Every read is the `holon` command with --server and --json, so the server's answers
// are the CLI's own; a sign-in is kept here, in SecretStorage, and passed to the command as
// HOLON_TOKEN, the way a script signs in.

const vscode = require("vscode");
const childProcess = require("node:child_process");

const DEFAULT_SERVER = "http://127.0.0.1:7420";
const SCHEME = "holon";
const REFRESH_MS = 10000;
const STATUS_ICONS = { running: "play", paused: "debug-pause", stopped: "debug-stop" };
const DONE = { run: "started", pause: "paused", stop: "stopped" };

/** @type {vscode.SecretStorage | undefined} */
let secrets;
/** @type {vscode.OutputChannel | undefined} */
let output;
/** The `holon serve` this extension started, if it did; stopped with the extension. */
/** @type {childProcess.ChildProcess | undefined} */
let server;
/** @type {Listing | undefined} */
let deployments;
/** @type {Listing | undefined} */
let flows;
/** @type {HolonText | undefined} */
let textProvider;
/** @type {Map<string, vscode.WebviewPanel>} */
const graphs = new Map();

function settings() {
  const config = vscode.workspace.getConfiguration("holon");
  return { command: config.get("path") || "holon", url: config.get("consoleUrl") || DEFAULT_SERVER };
}

const tokenKey = (url) => `holon.session ${url}`;

/** `holon ARGS --server URL` (with --json when asked): what it printed, parsed when JSON, or why it
 *  failed. A sign-in kept for this server is passed as HOLON_TOKEN. */
async function holon(args, { json = false } = {}) {
  const { command, url } = settings();
  const env = { ...process.env };
  const saved = secrets && (await secrets.get(tokenKey(url)));
  if (saved) env.HOLON_TOKEN = JSON.parse(saved).token;
  const full = [...args, "--server", url, ...(json ? ["--json"] : [])];
  return new Promise((resolve, reject) => {
    childProcess.execFile(command, full, { env, maxBuffer: 64 * 1024 * 1024 }, (error, stdout, stderr) => {
      if (error) return reject(new Error((stderr || error.message).trim()));
      try {
        resolve(json ? JSON.parse(stdout) : stdout);
      } catch (e) {
        reject(e);
      }
    });
  });
}

/** A view's items, loaded when VS Code asks; `refresh` asks again. A failure is one item that says
 *  what to do about it: sign in, or start the local server. */
class Listing {
  constructor(load, empty) {
    this.load = load;
    this.empty = empty;
    this.changed = new vscode.EventEmitter();
    this.onDidChangeTreeData = this.changed.event;
  }

  refresh() {
    this.changed.fire(undefined);
  }

  getTreeItem(item) {
    return item;
  }

  async getChildren() {
    try {
      const items = await this.load();
      return items.length > 0 ? items : [note(this.empty)];
    } catch (e) {
      return [problem(e)];
    }
  }
}

function note(text) {
  const item = new vscode.TreeItem(text, vscode.TreeItemCollapsibleState.None);
  item.iconPath = new vscode.ThemeIcon("info");
  return item;
}

function problem(error) {
  const why = error instanceof Error ? error.message : String(error);
  const signIn = /sign/i.test(why);
  const item = new vscode.TreeItem(signIn ? "Sign in to the server" : "Can't reach the server", vscode.TreeItemCollapsibleState.None);
  item.description = why.split("\n")[0];
  item.tooltip = why;
  item.iconPath = new vscode.ThemeIcon("warning");
  item.command = signIn ? { command: "holon.signIn", title: "Sign in" } : { command: "holon.startServer", title: "Start the local server" };
  return item;
}

function deploymentItem(d) {
  const item = new vscode.TreeItem(d.slug, vscode.TreeItemCollapsibleState.None);
  const errors = d.error_count ? `, ${d.error_count} error${d.error_count === 1 ? "" : "s"}` : "";
  item.description = `${d.status}${errors}`;
  item.tooltip = d.port ? `${d.slug}, on port ${d.port}` : d.slug;
  item.iconPath = new vscode.ThemeIcon(STATUS_ICONS[d.status] ?? "circle-outline");
  item.contextValue = "deployment";
  item.slug = d.slug;
  item.command = { command: "holon.showHistory", title: "Show history", arguments: [d.slug] };
  return item;
}

function flowItem(f) {
  const item = new vscode.TreeItem(f.name, vscode.TreeItemCollapsibleState.None);
  item.description = f.saved_by ? `saved by ${f.saved_by}` : undefined;
  item.iconPath = new vscode.ThemeIcon("file-code");
  item.contextValue = "flow";
  item.slug = f.name;
  item.command = { command: "holon.showFlow", title: "Show flow", arguments: [f.name] };
  return item;
}

/** The name a command was given: a name, or the tree item it was run on. */
const named = (arg) => (typeof arg === "string" ? arg : arg.slug);

/** A deployment's history, or a saved flow's text, as an editor: `holon:/history/<slug>` and
 *  `holon:/flow/<name>.holon.ts` (the last one highlighted as TypeScript, as a file is). */
class HolonText {
  constructor() {
    this.changed = new vscode.EventEmitter();
    this.onDidChange = this.changed.event;
  }

  async provideTextDocumentContent(uri) {
    const [, kind, ...rest] = uri.path.split("/");
    const name = rest.join("/");
    try {
      if (kind === "history") return await holon(["history", name]);
      return await holon(["flows", "show", name.replace(/\.holon\.ts$/, "")]);
    } catch (e) {
      return `${e.message}\n`;
    }
  }
}

async function openText(kind, name) {
  const uri = vscode.Uri.from({ scheme: SCHEME, path: `/${kind}/${name}${kind === "flow" ? ".holon.ts" : ""}` });
  textProvider.changed.fire(uri);
  const doc = await vscode.workspace.openTextDocument(uri);
  await vscode.window.showTextDocument(doc, { preview: true });
}

const escapeHtml = (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

/** A flow's canvas as a static picture: its nodes where the canvas has them, its edges between,
 *  coloured by the editor's theme, so it reads as the rest of VS Code does. */
function graphHtml(name, document) {
  const nodes = document.nodes ?? [];
  const edges = document.edges ?? [];
  const W = 176;
  const H = 46;
  const PAD = 24;
  const at = new Map(nodes.map((n) => [n.id, n.position ?? { x: 0, y: 0 }]));
  const xs = [...at.values()].map((p) => p.x);
  const ys = [...at.values()].map((p) => p.y);
  const minX = Math.min(0, ...xs) - PAD;
  const minY = Math.min(0, ...ys) - PAD;
  const width = Math.max(0, ...xs) + W + PAD - minX;
  const height = Math.max(0, ...ys) + H + PAD - minY;
  const lines = edges
    .filter((e) => at.has(e.source) && at.has(e.target))
    .map((e) => {
      const a = at.get(e.source);
      const b = at.get(e.target);
      const x1 = a.x + W;
      const y1 = a.y + H / 2;
      const x2 = b.x;
      const y2 = b.y + H / 2;
      const bend = Math.max(40, Math.abs(x2 - x1) / 2);
      return `<path d="M ${x1} ${y1} C ${x1 + bend} ${y1}, ${x2 - bend} ${y2}, ${x2} ${y2}"/>`;
    });
  const boxes = nodes.map((n) => {
    const p = at.get(n.id);
    const label = String(n.data?.name ?? n.type ?? n.id);
    const shown = label.length > 22 ? `${label.slice(0, 21)}…` : label;
    return `<g><rect x="${p.x}" y="${p.y}" width="${W}" height="${H}" rx="6"/><text x="${p.x + W / 2}" y="${p.y + H / 2}" text-anchor="middle">${escapeHtml(shown)}</text></g>`;
  });
  const body = nodes.length === 0 ? `<p>${escapeHtml(name)} has no nodes.</p>` : `<svg viewBox="${minX} ${minY} ${width} ${height}" preserveAspectRatio="xMidYMid meet">${lines.join("")}${boxes.join("")}</svg>`;
  return `<!doctype html>
<html>
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline';">
<style>
  html, body { margin: 0; height: 100%; }
  body { background: var(--vscode-editor-background); color: var(--vscode-editor-foreground); font-family: var(--vscode-font-family); font-size: var(--vscode-font-size); }
  svg { width: 100%; height: 100%; }
  rect { fill: var(--vscode-editorWidget-background); stroke: var(--vscode-focusBorder); }
  text { fill: var(--vscode-editor-foreground); font-size: 13px; dominant-baseline: middle; }
  path { fill: none; stroke: var(--vscode-descriptionForeground); stroke-width: 1.5; }
</style>
</head>
<body>${body}</body>
</html>`;
}

async function showGraph(arg) {
  const name = named(arg);
  const record = await holon(["flows", "show", name], { json: true });
  let panel = graphs.get(name);
  if (!panel) {
    panel = vscode.window.createWebviewPanel("holon.graph", `Graph: ${name}`, vscode.ViewColumn.Active, { enableScripts: false });
    graphs.set(name, panel);
    panel.onDidDispose(() => graphs.delete(name));
  }
  panel.webview.html = graphHtml(name, record.document ?? {});
  panel.reveal();
}

/** Run, pause or stop a deployment: the command's own; stopping asks first. */
async function lifecycle(verb, arg) {
  const slug = named(arg);
  if (verb === "stop") {
    const answer = await vscode.window.showWarningMessage(`Stop ${slug}? Its runs end; Run starts it again.`, { modal: true }, "Stop");
    if (answer !== "Stop") return;
  }
  try {
    await holon([verb, slug]);
    vscode.window.setStatusBarMessage(`Holon: ${slug} ${DONE[verb]}`, 4000);
  } catch (e) {
    vscode.window.showErrorMessage(`Holon: ${e.message}`);
  }
  refreshAll();
}

/** Sign in to the server: the first admin's account when it has none yet (with the setup code it
 *  printed), else the account's name and password. The answer is kept, for this server only. */
async function signIn() {
  const { url } = settings();
  let state;
  try {
    state = await (await fetch(new URL("/auth/state", url))).json();
  } catch {
    vscode.window.showErrorMessage(`Holon: nothing answers at ${url}. Start the local server first.`);
    return;
  }
  const body = {};
  if (state.setup) {
    const code = await vscode.window.showInputBox({ prompt: "Setup code: the server printed it in the Holon output", ignoreFocusOut: true });
    if (!code) return;
    body.code = code.trim();
  }
  const name = await vscode.window.showInputBox({ prompt: state.setup ? "Name for the first admin" : "Your name", ignoreFocusOut: true });
  if (!name) return;
  const password = await vscode.window.showInputBox({ prompt: "Password", password: true, ignoreFocusOut: true });
  if (!password) return;
  Object.assign(body, { name: name.trim(), password });
  const res = await fetch(new URL(state.setup ? "/auth/setup" : "/auth/sign-in", url), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  const answer = await res.json().catch(() => ({}));
  if (!res.ok) {
    vscode.window.showErrorMessage(`Holon: ${answer.error ?? `the server answered ${res.status}`}`);
    return;
  }
  await secrets.store(tokenKey(url), JSON.stringify({ token: answer.token, name: answer.name }));
  vscode.window.setStatusBarMessage(`Holon: signed in as ${answer.name}`, 4000);
  refreshAll();
}

/** Whether a server answers its health check at `url`. */
async function answers(url) {
  try {
    const res = await fetch(new URL("/health", url), { signal: AbortSignal.timeout(1000) });
    return res.ok;
  } catch {
    return false;
  }
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** Starts `holon serve` at the loopback address `url` names, its output in the Holon channel (a new
 *  server prints the setup code for its first admin there). Resolves to why it did not start, or to
 *  undefined once it answers. */
function startLocal(url, command) {
  const port = new URL(url).port || "80";
  output ??= vscode.window.createOutputChannel("Holon");
  output.show(true);
  const child = childProcess.spawn(command, ["serve", "--at", `127.0.0.1:${port}`], { stdio: ["ignore", "pipe", "pipe"] });
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

async function startServer() {
  const { command, url } = settings();
  if (await answers(url)) {
    refreshAll();
    return;
  }
  const loopback = ["127.0.0.1", "localhost", "[::1]"].includes(new URL(url).hostname);
  if (!loopback) {
    vscode.window.showErrorMessage(`Holon: nothing answers at ${url}. Start its server, or set holon.consoleUrl to where it is.`);
    return;
  }
  const why = await startLocal(url, command);
  if (why) {
    output.show(true);
    vscode.window.showErrorMessage(`Holon: "holon serve" didn't start (${why}). Install holon, or set holon.path to where it is.`);
    return;
  }
  refreshAll();
}

function refreshAll() {
  deployments?.refresh();
  flows?.refresh();
}

/** Registers the views, editors and commands. Returns the two listings, for the tests. */
function register(context) {
  secrets = context.secrets;
  deployments = new Listing(async () => (await holon(["deployments"], { json: true })).deployments.map(deploymentItem), "No deployments yet. `holon deploy <file>` puts a flow here.");
  flows = new Listing(async () => (await holon(["flows"], { json: true })).map(flowItem), "No saved flows yet. `holon flows save <file>` saves one.");
  textProvider = new HolonText();

  const timer = setInterval(refreshAll, REFRESH_MS);
  context.subscriptions.push(
    vscode.window.createTreeView("holon.deployments", { treeDataProvider: deployments }),
    vscode.window.createTreeView("holon.flows", { treeDataProvider: flows }),
    vscode.workspace.registerTextDocumentContentProvider(SCHEME, textProvider),
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration("holon.consoleUrl")) refreshAll();
    }),
    { dispose: () => clearInterval(timer) },
    vscode.commands.registerCommand("holon.refresh", refreshAll),
    vscode.commands.registerCommand("holon.showHistory", (arg) => openText("history", named(arg))),
    vscode.commands.registerCommand("holon.showFlow", (arg) => openText("flow", named(arg))),
    vscode.commands.registerCommand("holon.openGraph", showGraph),
    vscode.commands.registerCommand("holon.run", (arg) => lifecycle("run", arg)),
    vscode.commands.registerCommand("holon.pause", (arg) => lifecycle("pause", arg)),
    vscode.commands.registerCommand("holon.stop", (arg) => lifecycle("stop", arg)),
    vscode.commands.registerCommand("holon.signIn", signIn),
    vscode.commands.registerCommand("holon.startServer", startServer),
  );
  return { deployments, flows };
}

/** Stops the server this extension started, if it did. */
function stop() {
  server?.kill();
}

module.exports = { register, stop };
