// The Holon sidebar: the deployments and the saved flows of the server `holon.consoleUrl` names,
// as VS Code's own views, and what they open as VS Code's own editors and panels: a deployment's
// history as a read-only editor, a flow's text as an editor that saves back to the server, a flow's
// graph as a panel, and run, pause and stop through the command. Every read and write is the `holon`
// command with --server, so the server's answers are the CLI's own; a sign-in is kept here, in
// SecretStorage, and passed to the command as HOLON_TOKEN, the way a script signs in.

const vscode = require("vscode");
const childProcess = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

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
/** @type {HolonFiles | undefined} */
let flowFiles;
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

/** The server's history and flows as files. `holon:/flow/<name>.holon.ts` is a saved flow's text,
 *  highlighted as TypeScript: saving it writes the flow back to the server (`holon flows save`), and a
 *  flow someone else saved asks first. `holon:/history/<slug>` is a deployment's history, read-only. */
class HolonFiles {
  constructor() {
    this.changed = new vscode.EventEmitter();
    this.onDidChangeFile = this.changed.event;
  }

  watch() {
    return new vscode.Disposable(() => {});
  }

  async stat(uri) {
    const text = await this.read(uri);
    return { type: vscode.FileType.File, ctime: 0, mtime: Date.now(), size: Buffer.byteLength(text) };
  }

  readDirectory() {
    return [];
  }

  createDirectory() {
    throw vscode.FileSystemError.NoPermissions();
  }

  async readFile(uri) {
    return Buffer.from(await this.read(uri));
  }

  async read(uri) {
    const [, kind, file] = uri.path.split("/");
    try {
      if (kind === "history") return await holon(["history", file]);
      return await holon(["flows", "show", file.replace(/\.holon\.ts$/, "")]);
    } catch (e) {
      throw vscode.FileSystemError.Unavailable(e.message);
    }
  }

  async writeFile(uri, content) {
    const [, kind, file] = uri.path.split("/");
    if (kind !== "flow") throw vscode.FileSystemError.NoPermissions(uri);
    const name = file.replace(/\.holon\.ts$/, "");
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "holon-save-"));
    const saving = path.join(dir, `${name}.holon.ts`);
    fs.writeFileSync(saving, content);
    try {
      await saveFlow(saving, name, false).catch(async (e) => {
        const by = e.message.match(/is saved by ([^:]+):/);
        if (!by) throw vscode.FileSystemError.Unavailable(e.message);
        const answer = await vscode.window.showWarningMessage(`Replace the flow "${name}", saved by ${by[1]}?`, { modal: true }, "Replace");
        if (answer !== "Replace") throw vscode.FileSystemError.Unavailable(`"${name}" stays as ${by[1]} saved it`);
        await saveFlow(saving, name, true).catch((again) => {
          throw vscode.FileSystemError.Unavailable(again.message);
        });
      });
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
    refreshAll();
  }

  delete() {
    throw vscode.FileSystemError.NoPermissions();
  }

  rename() {
    throw vscode.FileSystemError.NoPermissions();
  }
}

/** What `holon deploy` says a deploy needs approval for, as the approval dialog reads it: its headings
 *  kept, the change marks in words ("New", "No longer", …), its command's own advice left out. */
function approvalDetail(text) {
  const marks = { "+": "New", "-": "No longer", "~": "Changed", "!": "Not ready on this server" };
  const headings = [
    [/^What starts it/, "What starts it", "start"],
    [/^It asks to/, "It asks for", "asks"],
  ];
  const lines = [];
  let section = "";
  for (const raw of text.split("\n")) {
    const line = raw.trimEnd();
    if (!line.trim() || /needs approval before|^What it touches|^Approve it here/.test(line)) continue;
    const heading = headings.find(([pattern]) => pattern.test(line));
    if (heading) {
      section = heading[2];
      lines.push(heading[1]);
      continue;
    }
    const change = line.match(/^\s*([+\-~!])\s+(.*)$/);
    if (change && !(change[1] === "-" && section === "asks")) lines.push(`  • ${marks[change[1]]}: ${change[2]}`);
    else if (change) lines.push(`  • ${change[2]}`);
    else lines.push(`  ${line.trim()}`);
  }
  return lines.join("\n");
}

/** Deploys a saved flow as a deployment of the same name: its text is compiled and sent, as the
 *  editor's Deploy does. What it asks for that a person hasn't granted yet is shown, and approved on
 *  a yes (`holon deploy --approve`). */
async function deployFlow(arg) {
  const name = named(arg);
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "holon-deploy-"));
  const file = path.join(dir, `${name}.holon.ts`);
  try {
    await holon(["flows", "show", name, "--out", file]);
    await deploy(file, name, false).catch(async (e) => {
      if (!/needs approval/.test(e.message)) throw e;
      const answer = await vscode.window.showWarningMessage(`Deploy "${name}"?`, { modal: true, detail: approvalDetail(e.message) }, "Approve and deploy");
      if (answer !== "Approve and deploy") return;
      await deploy(file, name, true);
    });
    vscode.window.setStatusBarMessage(`Holon: ${name} deployed`, 4000);
  } catch (e) {
    vscode.window.showErrorMessage(`Holon: ${e.message}`);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
  refreshAll();
}

function deploy(file, name, approve) {
  return holon(["deploy", file, "--slug", name, ...(approve ? ["--approve"] : [])]);
}

/** Saves a flow file under `name` on the server; with `replace`, over a flow someone else saved. */
function saveFlow(file, name, replace) {
  return holon(["flows", "save", file, "--name", name, ...(replace ? ["--replace"] : [])]);
}

async function openText(kind, name) {
  const uri = vscode.Uri.from({ scheme: SCHEME, path: `/${kind}/${name}${kind === "flow" ? ".holon.ts" : ""}` });
  flowFiles.changed.fire([{ type: vscode.FileChangeType.Changed, uri }]);
  const doc = await vscode.workspace.openTextDocument(uri);
  await vscode.window.showTextDocument(doc, { preview: true });
}

const escapeHtml = (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

/** A flow's canvas as a picture: each node as the canvas draws it (its name, its input ports on the
 *  left and output ports on the right, each with its type, its params), and each wire from the
 *  output port it leaves to the input port it reaches. Sized to the canvas, so it scrolls. */
function graphHtml(name, document) {
  const NODE_W = 232;
  const HEAD = 30;
  const ROW = 22;
  const PAD = 40;
  const clip = (s, n) => (String(s).length > n ? `${String(s).slice(0, n - 1)}…` : String(s));
  const typeText = (ty) => {
    if (typeof ty === "string") return ty;
    if (ty.List) return `List<${typeText(ty.List)}>`;
    if (ty.Product) return `(${ty.Product.map(typeText).join(", ")})`;
    if (ty.Sum) return ty.Sum.map(typeText).join(" | ");
    return "?";
  };
  const laid = (document.nodes ?? []).map((n) => {
    const d = n.data ?? {};
    const inputs = d.inputs ?? [];
    const outputs = d.outputs ?? [];
    const params = d.params ?? [];
    const rows = Math.max(inputs.length, outputs.length, 1);
    const h = HEAD + rows * ROW + (params.length ? 8 + params.length * ROW : 0) + 8;
    return { n, d, inputs, outputs, params, rows, x: n.position?.x ?? 0, y: n.position?.y ?? 0, h };
  });
  const byId = new Map(laid.map((l) => [l.n.id, l]));
  const portY = (l, i) => l.y + HEAD + i * ROW + ROW / 2;

  const wires = (document.edges ?? []).flatMap((e) => {
    const from = byId.get(e.source);
    const to = byId.get(e.target);
    if (!from || !to) return [];
    const out = Math.max(0, from.outputs.findIndex((p) => p.name === e.sourceHandle));
    const inp = to.inputs.findIndex((p) => p.name === e.targetHandle);
    const x1 = from.x + NODE_W;
    const y1 = portY(from, out);
    const x2 = to.x;
    const y2 = inp < 0 ? to.y + HEAD / 2 : portY(to, inp);
    const bend = Math.max(60, Math.abs(x2 - x1) / 2);
    const cls = e.targetHandle === "trigger" ? "trigger" : "wire";
    return [`<path class="${cls}" d="M ${x1} ${y1} C ${x1 + bend} ${y1}, ${x2 - bend} ${y2}, ${x2} ${y2}"/>`];
  });

  const nodes = laid.map((l) => {
    const parts = [
      `<rect class="node" x="${l.x}" y="${l.y}" width="${NODE_W}" height="${l.h}" rx="6"/>`,
      `<path class="head" d="M ${l.x} ${l.y + HEAD} V ${l.y + 6} Q ${l.x} ${l.y} ${l.x + 6} ${l.y} H ${l.x + NODE_W - 6} Q ${l.x + NODE_W} ${l.y} ${l.x + NODE_W} ${l.y + 6} V ${l.y + HEAD} Z"/>`,
      `<text class="title" x="${l.x + 10}" y="${l.y + HEAD / 2}">${escapeHtml(clip(l.d.name ?? l.n.id, 26))}</text>`,
    ];
    for (let i = 0; i < l.rows; i++) {
      const y = portY(l, i);
      const input = l.inputs[i];
      const output = l.outputs[i];
      if (input) {
        parts.push(`<circle class="port" cx="${l.x}" cy="${y}" r="4"/>`);
        parts.push(`<text class="port-name" x="${l.x + 10}" y="${y}">${escapeHtml(input.name)} <tspan class="type">${escapeHtml(typeText(input.ty))}</tspan></text>`);
      }
      if (output) {
        parts.push(`<circle class="port" cx="${l.x + NODE_W}" cy="${y}" r="4"/>`);
        parts.push(`<text class="port-name" text-anchor="end" x="${l.x + NODE_W - 10}" y="${y}"><tspan class="type">${escapeHtml(typeText(output.ty))}</tspan> ${escapeHtml(output.name)}</text>`);
      }
    }
    l.params.forEach((p, i) => {
      const y = l.y + HEAD + l.rows * ROW + 8 + i * ROW + ROW / 2;
      const value = p.kind === "Frozen" ? p.value : `← ${p.bind}`;
      parts.push(`<text class="param" x="${l.x + 10}" y="${y}">${escapeHtml(p.name)} <tspan class="type">${escapeHtml(clip(value, 26))}</tspan></text>`);
    });
    return `<g>${parts.join("")}</g>`;
  });

  if (laid.length === 0) return graphPage(`<p>${escapeHtml(name)} has no nodes.</p>`);
  const minX = Math.min(...laid.map((l) => l.x)) - PAD;
  const minY = Math.min(...laid.map((l) => l.y)) - PAD;
  const maxX = Math.max(...laid.map((l) => l.x + NODE_W)) + PAD;
  const maxY = Math.max(...laid.map((l) => l.y + l.h)) + PAD;
  const width = maxX - minX;
  const height = maxY - minY;
  return graphPage(`<svg width="${width}" height="${height}" viewBox="${minX} ${minY} ${width} ${height}">${wires.join("")}${nodes.join("")}</svg>`);
}

function graphPage(body) {
  return `<!doctype html>
<html>
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline';">
<style>
  html, body { margin: 0; }
  body { background: var(--vscode-editor-background); color: var(--vscode-editor-foreground); font-family: var(--vscode-font-family); font-size: var(--vscode-font-size); }
  .node { fill: var(--vscode-editorWidget-background); stroke: var(--vscode-editorWidget-border, var(--vscode-focusBorder)); }
  .head { fill: var(--vscode-editorGroupHeader-tabsBackground, var(--vscode-editorWidget-background)); stroke: var(--vscode-editorWidget-border, var(--vscode-focusBorder)); }
  .title { fill: var(--vscode-editor-foreground); font-size: 13px; font-weight: 600; dominant-baseline: middle; }
  .port { fill: var(--vscode-focusBorder); }
  .port-name, .param { fill: var(--vscode-editor-foreground); font-size: 12px; dominant-baseline: middle; }
  .type { fill: var(--vscode-descriptionForeground); font-size: 11px; }
  .wire { fill: none; stroke: var(--vscode-descriptionForeground); stroke-width: 1.5; }
  .trigger { fill: none; stroke: var(--vscode-descriptionForeground); stroke-width: 1.2; stroke-dasharray: 4 3; }
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
  flowFiles = new HolonFiles();

  const timer = setInterval(refreshAll, REFRESH_MS);
  context.subscriptions.push(
    vscode.window.createTreeView("holon.deployments", { treeDataProvider: deployments }),
    vscode.window.createTreeView("holon.flows", { treeDataProvider: flows }),
    vscode.workspace.registerFileSystemProvider(SCHEME, flowFiles, { isCaseSensitive: true }),
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration("holon.consoleUrl")) refreshAll();
    }),
    { dispose: () => clearInterval(timer) },
    vscode.commands.registerCommand("holon.refresh", refreshAll),
    vscode.commands.registerCommand("holon.showHistory", (arg) => openText("history", named(arg))),
    vscode.commands.registerCommand("holon.showFlow", (arg) => openText("flow", named(arg))),
    vscode.commands.registerCommand("holon.openGraph", showGraph),
    vscode.commands.registerCommand("holon.deployFlow", deployFlow),
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
