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
const crypto = require("node:crypto");
const { tidy } = require("../../editor/src/graphLayout.ts");

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
/** @type {Listing | undefined} */
let activity;
/** @type {HolonFiles | undefined} */
let flowFiles;
/** @type {Map<string, { panel: vscode.WebviewPanel, document: object }>} */
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

  async getChildren(element) {
    if (element) return element.loadChildren ? element.loadChildren() : element.childItems ?? [];
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

/** A deployment's groups: the grants its version asks for that nothing has approved yet, the runs
 *  waiting for an event, and its latest runs. */
async function deploymentChildren(slug) {
  try {
    const [log, waiting, grant, kept] = await Promise.all([
      holon(["runs", slug, "--last", "10"], { json: true }),
      holon(["runs", slug, "--waiting"], { json: true }),
      holon(["grant", slug], { json: true }),
      stateItems(slug),
    ]);
    const asks = (grant.manifest?.not_granted ?? []).map((item) => grantItem(slug, item));
    const waits = (waiting.runs ?? []).map(waitingItem);
    const runs = (log.entries ?? []).slice(-10).reverse().map((e) => runItem(slug, e));
    return [group("Needs approval", asks, true), group("Waiting for an event", waits, true), group("Recent runs", runs, false), group("State", kept, false)];
  } catch (e) {
    return [problem(e)];
  }
}

function group(label, children, expanded) {
  const state = children.length === 0 ? vscode.TreeItemCollapsibleState.None : expanded ? vscode.TreeItemCollapsibleState.Expanded : vscode.TreeItemCollapsibleState.Collapsed;
  const item = new vscode.TreeItem(label, state);
  item.description = children.length === 0 ? "none" : `${children.length}`;
  item.iconPath = new vscode.ThemeIcon("folder");
  item.childItems = children;
  return item;
}

/** A deployment's state: its values (a click shows one's history) and its collections (a click shows
 *  their items). A flow that keeps none has none to show. */
async function stateItems(slug) {
  let state;
  try {
    state = (await holon(["state", slug], { json: true })).state ?? {};
  } catch {
    return [];
  }
  const items = Object.entries(state).map(([key, value]) => {
    if (key.startsWith("#collection:")) {
      const name = key.slice("#collection:".length);
      const item = new vscode.TreeItem(name, vscode.TreeItemCollapsibleState.None);
      item.description = "a collection";
      item.iconPath = new vscode.ThemeIcon("database");
      item.command = { command: "holon.showCollection", title: "Show collection", arguments: [slug, name] };
      return item;
    }
    const text = JSON.stringify(value);
    const item = new vscode.TreeItem(key, vscode.TreeItemCollapsibleState.None);
    item.description = text.length > 40 ? `${text.slice(0, 39)}…` : text;
    item.tooltip = `${key} = ${text}`;
    item.iconPath = new vscode.ThemeIcon("symbol-variable");
    item.command = { command: "holon.showCell", title: "Show history", arguments: [slug, key] };
    return item;
  });
  return items.length > 0 ? items : [note("Keeps no state")];
}

/** One thing that happened to the server: what, to what, by whom, and when. */
function activityItem(e) {
  const item = new vscode.TreeItem(`${e.action} ${e.target ?? ""}`.trim(), vscode.TreeItemCollapsibleState.None);
  item.description = `${e.by ?? "-"}, ${new Date(e.at * 1000).toLocaleString()}`;
  item.iconPath = new vscode.ThemeIcon("history");
  return item;
}

/** One run of the log: its route (or kind), its status and how long it took, a click opens its record. */
function runItem(slug, e) {
  const failed = Boolean(e.error) || (e.status ?? 0) >= 400;
  const item = new vscode.TreeItem(`#${e.seq} ${e.route ?? e.kind}`, vscode.TreeItemCollapsibleState.None);
  item.description = [e.status, `${e.duration_ms} ms`].filter((s) => s !== null && s !== undefined).join(", ");
  item.iconPath = new vscode.ThemeIcon(failed ? "error" : "pass");
  item.contextValue = "run";
  item.seq = e.seq;
  item.command = { command: "holon.showRun", title: "Show run", arguments: [slug, e.seq] };
  return item;
}

/** A run parked for an event. Only the flow's own link can answer it, so this only says what it waits for. */
function waitingItem(run) {
  const waits = (run.waiting ?? []).map((w) => w.name).join(", ");
  const item = new vscode.TreeItem(`${run.route ?? run.kind} (${run.run_id})`, vscode.TreeItemCollapsibleState.None);
  item.description = `waits for ${waits}`;
  item.tooltip = "The flow answers this through the link it gave the run. Holon can't answer it from here.";
  item.iconPath = new vscode.ThemeIcon("watch");
  return item;
}

/** A grant a version asks for and nothing has approved yet: approving it is the inline check button. */
function grantItem(slug, grant) {
  const item = new vscode.TreeItem(grant, vscode.TreeItemCollapsibleState.None);
  item.description = "its version asks for it";
  item.tooltip = "Approve it: the deployment may do what its version asks for, as you.";
  item.iconPath = new vscode.ThemeIcon("shield");
  item.contextValue = "grant";
  item.slug = slug;
  item.grant = grant;
  return item;
}

function deploymentItem(d) {
  const item = new vscode.TreeItem(d.slug, vscode.TreeItemCollapsibleState.Collapsed);
  const errors = d.error_count ? `, ${d.error_count} error${d.error_count === 1 ? "" : "s"}` : "";
  item.description = `${d.status}${errors}`;
  item.tooltip = d.port ? `${d.slug}, on port ${d.port}` : d.slug;
  item.iconPath = new vscode.ThemeIcon(STATUS_ICONS[d.status] ?? "circle-outline");
  item.contextValue = "deployment";
  item.slug = d.slug;
  item.loadChildren = () => deploymentChildren(d.slug);
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
    const [, kind, ...rest] = uri.path.split("/");
    const file = rest.join("/");
    try {
      if (kind === "history") return await holon(["history", file]);
      if (kind === "run") return await runText(file);
      if (kind === "cell") {
        const [slug, name] = file.split("/");
        return await holon(["history", slug, "--cell", name.slice(0, -4)]);
      }
      if (kind === "collection") {
        const [slug, name] = file.split("/");
        return `${JSON.stringify(await holon(["state", slug, "--collection", name.slice(0, -5)], { json: true }), null, 2)}\n`;
      }
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

/** A run's record, as the server logged it: `holon:/run/<slug>/<seq>.json`. */
async function runText(file) {
  const [slug, name] = file.split("/");
  const seq = Number(name.replace(/\.json$/, ""));
  const log = await holon(["runs", slug, "--last", "200"], { json: true });
  const entry = (log.entries ?? []).find((e) => e.seq === seq);
  if (!entry) throw new Error(`run ${seq} isn't among the last 200 runs of ${slug}`);
  return `${JSON.stringify(entry, null, 2)}\n`;
}

/** Opens one of the server's documents (`holon:/…`) in an editor, read fresh from the server. */
async function openDocument(path) {
  const uri = vscode.Uri.from({ scheme: SCHEME, path });
  flowFiles.changed.fire([{ type: vscode.FileChangeType.Changed, uri }]);
  const doc = await vscode.workspace.openTextDocument(uri);
  await vscode.window.showTextDocument(doc, { preview: true });
}

function openRun(slug, seq) {
  return openDocument(`/run/${slug}/${seq}.json`);
}

/** Approves one grant a deployment's version asks for, as `holon grant --grant` does. */
async function approveGrant(arg) {
  const answer = await vscode.window.showWarningMessage(`Approve "${arg.grant}" for ${arg.slug}?`, { modal: true, detail: "The deployment may do what its version asks for, as you." }, "Approve");
  if (answer !== "Approve") return;
  try {
    await holon(["grant", arg.slug, "--grant", arg.grant]);
    vscode.window.setStatusBarMessage(`Holon: ${arg.grant} approved for ${arg.slug}`, 4000);
  } catch (e) {
    vscode.window.showErrorMessage(`Holon: ${e.message}`);
  }
  refreshAll();
}

const escapeHtml = (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

// The graph's drawing: each node with its name and ports, each wire, and each handler's domain framed
// with its label. The positions come from the shared tidy layout (editor/src/graphLayout.ts), so a flow is
// drawn the same here as in the editor. The page only draws and moves: a click selects, a drag on a node
// moves the selection, a drag on the background selects a box, the middle button pans, the wheel zooms,
// and Tidy asks the extension for a fresh layout.
const NODE_W = 232;
const HEAD = 30;
const ROW = 22;
const FRAME_LABEL = 34;
const FRAME_PAD = 24;

const clip = (s, n) => (String(s).length > n ? `${String(s).slice(0, n - 1)}…` : String(s));

function typeText(ty) {
  if (typeof ty === "string") return ty;
  if (ty.List) return `List<${typeText(ty.List)}>`;
  if (ty.Product) return `(${ty.Product.map(typeText).join(", ")})`;
  if (ty.Sum) return ty.Sum.map(typeText).join(" | ");
  return "?";
}

/** The flow's canvas as the drawing reads it: each node laid out as the canvas has it (its ports, its
 *  params), each wire by the indices of its ends, each domain with its label. */
function graphModel(document) {
  const laid = (document.nodes ?? []).map((n) => {
    const d = n.data ?? {};
    const inputs = d.inputs ?? [];
    const outputs = d.outputs ?? [];
    const params = d.params ?? [];
    const rows = Math.max(inputs.length, outputs.length, 1);
    const h = HEAD + rows * ROW + (params.length ? 8 + params.length * ROW : 0) + 8;
    return { n, d, inputs, outputs, params, rows, h, title: String(d.name ?? n.id) };
  });
  const index = new Map(laid.map((l, i) => [l.n.id, i]));
  const wires = (document.edges ?? []).flatMap((e, i) => {
    const from = index.get(e.source);
    const to = index.get(e.target);
    if (from === undefined || to === undefined) return [];
    const out = laid[from].outputs.findIndex((p) => p.name === e.sourceHandle);
    const inp = laid[to].inputs.findIndex((p) => p.name === e.targetHandle);
    const label = `${e.source}.${e.sourceHandle ?? ""} to ${e.target}.${e.targetHandle ?? ""}`;
    return [{ key: `e:${i}`, label, from, out: Math.max(0, out), to, inp, cls: e.targetHandle === "trigger" ? "trigger" : "wire" }];
  });
  const lay = tidy(
    laid.map((l) => ({ height: l.h })),
    wires.map((w) => ({ from: w.from, to: w.to })),
  );
  const frozen = (l) => Object.fromEntries(l.params.filter((p) => p.kind === "Frozen").map((p) => [p.name, p.value]));
  const domainLabel = (ids) => {
    const request = ids.map((id) => laid[id]).find((l) => l.title === "request" && frozen(l).method);
    if (request) return `${frozen(request).method} ${frozen(request).path ?? ""}`;
    return laid[ids[0]].title;
  };
  const model = {
    nodeW: NODE_W,
    head: HEAD,
    row: ROW,
    label: FRAME_LABEL,
    pad: FRAME_PAD,
    nodes: laid.map((l, i) => ({ key: `n:${l.n.id}`, title: l.title, x: lay.positions[i].x, y: lay.positions[i].y, h: l.h })),
    wires,
    domains: lay.domains.map((ids) => ({ ids, label: domainLabel(ids) })),
  };
  return { laid, model };
}

function graphHtml(name, document) {
  const { laid, model } = graphModel(document);
  if (laid.length === 0) return graphPage(`<p>${escapeHtml(name)} has no nodes.</p>`);
  const nodes = laid.map((l, i) => {
    const id = l.n.id;
    const title = l.title;
    const at = model.nodes[i];
    const parts = [
      `<rect class="node" x="0" y="0" width="${NODE_W}" height="${l.h}" rx="6"/>`,
      `<path class="head" d="M 0 ${HEAD} V 6 Q 0 0 6 0 H ${NODE_W - 6} Q ${NODE_W} 0 ${NODE_W} 6 V ${HEAD} Z"/>`,
      `<text class="title" x="10" y="${HEAD / 2}">${escapeHtml(clip(title, 26))}</text>`,
    ];
    for (let r = 0; r < l.rows; r++) {
      const y = HEAD + r * ROW + ROW / 2;
      const input = l.inputs[r];
      const output = l.outputs[r];
      if (input) {
        const key = escapeHtml(`p:${id}:in:${input.name}`);
        const label = escapeHtml(`input ${input.name} of ${title}`);
        parts.push(`<g class="port-group" data-key="${key}" data-label="${label}"><circle class="port" cx="0" cy="${y}" r="4"/><text class="port-name" x="10" y="${y}">${escapeHtml(input.name)} <tspan class="type">${escapeHtml(typeText(input.ty))}</tspan></text></g>`);
      }
      if (output) {
        const key = escapeHtml(`p:${id}:out:${output.name}`);
        const label = escapeHtml(`output ${output.name} of ${title}`);
        parts.push(`<g class="port-group" data-key="${key}" data-label="${label}"><circle class="port" cx="${NODE_W}" cy="${y}" r="4"/><text class="port-name" text-anchor="end" x="${NODE_W - 10}" y="${y}"><tspan class="type">${escapeHtml(typeText(output.ty))}</tspan> ${escapeHtml(output.name)}</text></g>`);
      }
    }
    l.params.forEach((p, r) => {
      const y = HEAD + l.rows * ROW + 8 + r * ROW + ROW / 2;
      const value = p.kind === "Frozen" ? p.value : `← ${p.bind}`;
      parts.push(`<text class="param" x="10" y="${y}">${escapeHtml(p.name)} <tspan class="type">${escapeHtml(clip(value, 26))}</tspan></text>`);
    });
    return `<g class="node-group" data-key="${escapeHtml(`n:${id}`)}" data-label="${escapeHtml(`node ${title}`)}" transform="translate(${at.x} ${at.y})">${parts.join("")}</g>`;
  });
  // JSON is safe inside the script as long as no "<" can end the script element early.
  const json = JSON.stringify(model).split("<").join(String.fromCharCode(92) + "u003c");
  return graphPage(`<svg id="canvas"><g id="view"><g id="domains"></g><g id="wires"></g><g id="nodes">${nodes.join("")}</g></g><rect id="marquee" class="marquee" display="none"/></svg>`, json);
}

function graphPage(body, model = '{"nodeW":0,"head":0,"row":0,"label":0,"pad":0,"nodes":[],"wires":[],"domains":[]}') {
  const nonce = crypto.randomBytes(16).toString("base64");
  return `<!doctype html>
<html>
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'nonce-${nonce}';">
<style>
  html, body { margin: 0; height: 100%; overflow: hidden; }
  body { background: var(--vscode-editor-background); color: var(--vscode-editor-foreground); font-family: var(--vscode-font-family); font-size: var(--vscode-font-size); }
  svg { display: block; width: 100%; height: 100%; cursor: default; user-select: none; }
  .node { fill: var(--vscode-editorWidget-background); stroke: var(--vscode-editorWidget-border, var(--vscode-focusBorder)); }
  .head { fill: var(--vscode-editorGroupHeader-tabsBackground, var(--vscode-editorWidget-background)); stroke: var(--vscode-editorWidget-border, var(--vscode-focusBorder)); }
  .title { fill: var(--vscode-editor-foreground); font-size: 13px; font-weight: 600; dominant-baseline: middle; }
  .port { fill: var(--vscode-focusBorder); }
  .port-name, .param { fill: var(--vscode-editor-foreground); font-size: 12px; dominant-baseline: middle; }
  .type { fill: var(--vscode-descriptionForeground); font-size: 11px; }
  .wire { fill: none; stroke: var(--vscode-descriptionForeground); stroke-width: 1.5; }
  .trigger { fill: none; stroke: var(--vscode-descriptionForeground); stroke-width: 1.2; stroke-dasharray: 4 3; }
  .hit { fill: none; stroke: transparent; stroke-width: 12; }
  .domain { fill: var(--vscode-editorWidget-background); fill-opacity: 0.4; stroke: var(--vscode-editorWidget-border, var(--vscode-focusBorder)); stroke-dasharray: 6 4; }
  .domain-label { fill: var(--vscode-descriptionForeground); font-size: 12px; font-weight: 600; dominant-baseline: middle; }
  .marquee { fill: var(--vscode-editor-selectionBackground); fill-opacity: 0.35; stroke: var(--vscode-focusBorder); stroke-width: 1; }
  .selected > .node { stroke: var(--vscode-focusBorder); stroke-width: 2; }
  .selected > .wire, .selected > .trigger { stroke: var(--vscode-focusBorder); stroke-width: 2.5; }
  .selected > .port { stroke: var(--vscode-editor-foreground); stroke-width: 2; }
  .bar { position: fixed; top: 8px; right: 8px; display: flex; gap: 4px; }
  .bar button { width: 26px; height: 26px; border: none; cursor: pointer; font-size: 14px; background: var(--vscode-button-secondaryBackground); color: var(--vscode-button-secondaryForeground); }
  .bar button:hover { background: var(--vscode-button-secondaryHoverBackground); }
  .status { position: fixed; left: 8px; bottom: 6px; font-size: 12px; color: var(--vscode-descriptionForeground); }
</style>
</head>
<body>
<div class="bar"><button id="zoom-in" title="Zoom in">+</button><button id="zoom-out" title="Zoom out">&minus;</button><button id="tidy" title="Tidy the layout: columns by dependency, a domain per handler">&#x25A4;</button><button id="fit" title="Fit the flow to the window">&#x2922;</button></div>
${body}
<div class="status" id="status"></div>
<script nonce="${nonce}">
(function () {
  var svg = document.getElementById("canvas");
  var view = document.getElementById("view");
  var wiresLayer = document.getElementById("wires");
  var domainLayer = document.getElementById("domains");
  var status = document.getElementById("status");
  var marquee = document.getElementById("marquee");
  var vscode = typeof acquireVsCodeApi === "function" ? acquireVsCodeApi() : null;
  if (!svg || !view) return;
  var M = ${model};
  var NW = M.nodeW, HD = M.head, RW = M.row, FL = M.label, FP = M.pad;
  var nodeEls = Array.prototype.slice.call(document.querySelectorAll(".node-group"));
  var wirePaths = M.wires.map(function (w) {
    var g = document.createElementNS(svg.namespaceURI, "g");
    g.setAttribute("class", "wire-group");
    g.setAttribute("data-key", w.key);
    g.setAttribute("data-label", "wire " + w.label);
    var hit = document.createElementNS(svg.namespaceURI, "path");
    hit.setAttribute("class", "hit");
    var line = document.createElementNS(svg.namespaceURI, "path");
    line.setAttribute("class", w.cls);
    g.appendChild(hit);
    g.appendChild(line);
    wiresLayer.appendChild(g);
    return { hit: hit, line: line, w: w };
  });
  var domainList = M.domains.map(function (d) {
    var rect = document.createElementNS(svg.namespaceURI, "rect");
    rect.setAttribute("class", "domain");
    var text = document.createElementNS(svg.namespaceURI, "text");
    text.setAttribute("class", "domain-label");
    text.textContent = d.label;
    domainLayer.appendChild(rect);
    domainLayer.appendChild(text);
    return { ids: d.ids, rect: rect, text: text };
  });
  var scale = 1, x = 0, y = 0;
  var selected = new Set();
  var mode = null, start = null, startNodes = [], moved = false, clickKey = null, wasSelected = false;
  function apply() {
    view.setAttribute("transform", "translate(" + x + " " + y + ") scale(" + scale + ")");
  }
  function zoomAt(px, py, factor) {
    var next = Math.min(4, Math.max(0.1, scale * factor));
    factor = next / scale;
    x = px - (px - x) * factor;
    y = py - (py - y) * factor;
    scale = next;
    apply();
  }
  function local(e) {
    var r = svg.getBoundingClientRect();
    return [e.clientX - r.left, e.clientY - r.top];
  }
  function toView(p) {
    return [(p[0] - x) / scale, (p[1] - y) / scale];
  }
  function fit() {
    var box = view.getBBox();
    var W = svg.clientWidth, H = svg.clientHeight;
    scale = Math.min(W / box.width, H / box.height, 1.5) * 0.95;
    x = (W - box.width * scale) / 2 - box.x * scale;
    y = (H - box.height * scale) / 2 - box.y * scale;
    apply();
  }
  function place(i) {
    nodeEls[i].setAttribute("transform", "translate(" + M.nodes[i].x + " " + M.nodes[i].y + ")");
  }
  function wireD(w) {
    var a = M.nodes[w.from], b = M.nodes[w.to];
    var x1 = a.x + NW, y1 = a.y + HD + w.out * RW + RW / 2;
    var x2 = b.x, y2 = w.inp < 0 ? b.y + HD / 2 : b.y + HD + w.inp * RW + RW / 2;
    var bend = Math.max(60, Math.abs(x2 - x1) / 2);
    return "M " + x1 + " " + y1 + " C " + (x1 + bend) + " " + y1 + ", " + (x2 - bend) + " " + y2 + ", " + x2 + " " + y2;
  }
  // A domain's frame follows its nodes, so it stays round them while they are dragged.
  function updateDomains() {
    domainList.forEach(function (d) {
      var x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
      d.ids.forEach(function (id) {
        var nd = M.nodes[id];
        x0 = Math.min(x0, nd.x);
        y0 = Math.min(y0, nd.y);
        x1 = Math.max(x1, nd.x + NW);
        y1 = Math.max(y1, nd.y + nd.h);
      });
      var fx = x0 - FP, fy = y0 - FL;
      d.rect.setAttribute("x", fx);
      d.rect.setAttribute("y", fy);
      d.rect.setAttribute("width", x1 - x0 + 2 * FP);
      d.rect.setAttribute("height", y1 - y0 + FL + FP);
      d.text.setAttribute("x", fx + 10);
      d.text.setAttribute("y", fy + FL / 2);
    });
  }
  function drawWires() {
    wirePaths.forEach(function (p) {
      var d = wireD(p.w);
      p.hit.setAttribute("d", d);
      p.line.setAttribute("d", d);
    });
    updateDomains();
  }
  function render() {
    var labels = [];
    document.querySelectorAll("[data-key]").forEach(function (el) {
      var on = selected.has(el.getAttribute("data-key"));
      el.classList.toggle("selected", on);
      if (on) labels.push(el.getAttribute("data-label"));
    });
    status.textContent = labels.length ? "Selected: " + labels.join(", ") : "Click or drag a box to select; drag a node to move it. Middle button pans, the wheel zooms.";
  }
  function select(key, additive) {
    if (!additive) selected.clear();
    if (key === null) {
      render();
      return;
    }
    if (additive && selected.has(key)) selected.delete(key);
    else selected.add(key);
    render();
  }
  function selectedNodes() {
    var out = [];
    M.nodes.forEach(function (n, i) {
      if (selected.has(n.key)) out.push({ i: i, x: n.x, y: n.y });
    });
    return out;
  }
  svg.addEventListener("mousedown", function (e) {
    if (e.button === 1) e.preventDefault();
  });
  svg.addEventListener("wheel", function (e) {
    e.preventDefault();
    var p = local(e);
    zoomAt(p[0], p[1], e.deltaY < 0 ? 1.1 : 1 / 1.1);
  }, { passive: false });
  svg.addEventListener("pointerdown", function (e) {
    var additive = e.ctrlKey || e.shiftKey;
    if (e.button === 1) {
      mode = "pan";
      start = { x: e.clientX, y: e.clientY };
      svg.setPointerCapture(e.pointerId);
      svg.style.cursor = "grabbing";
      return;
    }
    if (e.button !== 0) return;
    var hit = e.target.closest ? e.target.closest("[data-key]") : null;
    if (hit && hit.classList.contains("node-group")) {
      clickKey = hit.getAttribute("data-key");
      wasSelected = selected.has(clickKey);
      if (!wasSelected) {
        if (!additive) selected.clear();
        selected.add(clickKey);
        render();
      }
      mode = "node";
      moved = false;
      start = { x: e.clientX, y: e.clientY };
      startNodes = selectedNodes();
      svg.setPointerCapture(e.pointerId);
      return;
    }
    if (hit) {
      select(hit.getAttribute("data-key"), additive);
      return;
    }
    mode = "marquee";
    moved = false;
    start = local(e);
    marquee.setAttribute("display", "inline");
    svg.setPointerCapture(e.pointerId);
  });
  svg.addEventListener("pointermove", function (e) {
    if (mode === "pan") {
      x += e.clientX - start.x;
      y += e.clientY - start.y;
      start = { x: e.clientX, y: e.clientY };
      apply();
    } else if (mode === "node") {
      var dx = (e.clientX - start.x) / scale;
      var dy = (e.clientY - start.y) / scale;
      if (dx !== 0 || dy !== 0) moved = true;
      startNodes.forEach(function (s) {
        M.nodes[s.i].x = s.x + dx;
        M.nodes[s.i].y = s.y + dy;
        place(s.i);
      });
      drawWires();
    } else if (mode === "marquee") {
      var p = local(e);
      if (Math.abs(p[0] - start[0]) + Math.abs(p[1] - start[1]) > 3) moved = true;
      marquee.setAttribute("x", Math.min(start[0], p[0]));
      marquee.setAttribute("y", Math.min(start[1], p[1]));
      marquee.setAttribute("width", Math.abs(p[0] - start[0]));
      marquee.setAttribute("height", Math.abs(p[1] - start[1]));
    }
  });
  svg.addEventListener("pointerup", function (e) {
    var prev = mode;
    var additive = e.ctrlKey || e.shiftKey;
    mode = null;
    svg.style.cursor = "default";
    if (prev === "node" && !moved) {
      if (additive && wasSelected) {
        selected.delete(clickKey);
        render();
      } else {
        select(clickKey, false);
      }
    } else if (prev === "marquee") {
      marquee.setAttribute("display", "none");
      if (moved) {
        var a = toView(start), b = toView(local(e));
        var x1 = Math.min(a[0], b[0]), x2 = Math.max(a[0], b[0]);
        var y1 = Math.min(a[1], b[1]), y2 = Math.max(a[1], b[1]);
        if (!additive) selected.clear();
        M.nodes.forEach(function (n) {
          if (n.x < x2 && n.x + NW > x1 && n.y < y2 && n.y + n.h > y1) selected.add(n.key);
        });
        render();
      } else if (!additive) {
        select(null, false);
      }
    }
  });
  svg.addEventListener("dblclick", fit);
  document.addEventListener("keydown", function (e) {
    if (e.key === "Escape") {
      selected.clear();
      render();
    }
  });
  window.addEventListener("message", function (e) {
    var msg = e.data;
    if (msg && msg.type === "layout") {
      msg.positions.forEach(function (p, i) {
        M.nodes[i].x = p.x;
        M.nodes[i].y = p.y;
        place(i);
      });
      drawWires();
      fit();
    }
  });
  document.getElementById("zoom-in").addEventListener("click", function () { zoomAt(svg.clientWidth / 2, svg.clientHeight / 2, 1.25); });
  document.getElementById("zoom-out").addEventListener("click", function () { zoomAt(svg.clientWidth / 2, svg.clientHeight / 2, 0.8); });
  document.getElementById("fit").addEventListener("click", fit);
  document.getElementById("tidy").addEventListener("click", function () {
    if (vscode) vscode.postMessage({ type: "tidy" });
  });
  drawWires();
  render();
  fit();
})();
</script>
</body>
</html>`;
}

/** Opens a flow's graph: one panel per flow. A Tidy from the panel asks for a fresh layout of the flow. */
async function showGraph(arg) {
  const name = named(arg);
  const record = await holon(["flows", "show", name], { json: true });
  let open = graphs.get(name);
  if (!open) {
    const panel = vscode.window.createWebviewPanel("holon.graph", `Graph: ${name}`, vscode.ViewColumn.Active, { enableScripts: true });
    open = { panel, document: {} };
    graphs.set(name, open);
    panel.onDidDispose(() => graphs.delete(name));
    panel.webview.onDidReceiveMessage((msg) => {
      if (msg && msg.type === "tidy") {
        const { model } = graphModel(open.document);
        panel.webview.postMessage({ type: "layout", positions: model.nodes.map((n) => ({ x: n.x, y: n.y })) });
      }
    });
  }
  open.document = record.document ?? {};
  open.panel.webview.html = graphHtml(name, open.document);
  open.panel.reveal();
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
  activity?.refresh();
}

/** Registers the views, editors and commands. Returns the two listings, for the tests. */
function register(context) {
  secrets = context.secrets;
  deployments = new Listing(async () => (await holon(["deployments"], { json: true })).deployments.map(deploymentItem), "No deployments yet. `holon deploy <file>` puts a flow here.");
  flows = new Listing(async () => (await holon(["flows"], { json: true })).map(flowItem), "No saved flows yet. `holon flows save <file>` saves one.");
  activity = new Listing(async () => (await holon(["activity", "--last", "100"], { json: true })).entries.map(activityItem), "Nothing has happened yet.");
  flowFiles = new HolonFiles();

  const timer = setInterval(refreshAll, REFRESH_MS);
  context.subscriptions.push(
    vscode.window.createTreeView("holon.deployments", { treeDataProvider: deployments }),
    vscode.window.createTreeView("holon.flows", { treeDataProvider: flows }),
    vscode.window.createTreeView("holon.activity", { treeDataProvider: activity }),
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
    vscode.commands.registerCommand("holon.showRun", openRun),
    vscode.commands.registerCommand("holon.approveGrant", approveGrant),
    vscode.commands.registerCommand("holon.showCell", (slug, name) => openDocument(`/cell/${slug}/${name}.txt`)),
    vscode.commands.registerCommand("holon.showCollection", (slug, name) => openDocument(`/collection/${slug}/${name}.json`)),
    vscode.commands.registerCommand("holon.run", (arg) => lifecycle("run", arg)),
    vscode.commands.registerCommand("holon.pause", (arg) => lifecycle("pause", arg)),
    vscode.commands.registerCommand("holon.stop", (arg) => lifecycle("stop", arg)),
    vscode.commands.registerCommand("holon.signIn", signIn),
    vscode.commands.registerCommand("holon.startServer", startServer),
  );
  return { deployments, flows, activity };
}

/** Stops the server this extension started, if it did. */
function stop() {
  server?.kill();
}

module.exports = { register, stop };
