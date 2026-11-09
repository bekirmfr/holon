// The History panel: a page in VS Code's bottom panel, beside the Runs panel, with a deployment's timeline on the
// left (its runs, what was done to it, and its versions, filtered as the web console filters them) and the picked
// row on the right: its details, what can be restored to it, and the restore's dry run, with its diffs, before
// anything changes. Nothing is restored until the dry run is approved. The page asks this extension for everything;
// the extension reads the server with the holon command, as every view does, and the timeline's rules are in
// historyModel.js, which is the web console's timeline.ts.

const vscode = require("vscode");
const { randomBytes } = require("node:crypto");
const model = require("./historyModel");

/** When an event or a run happened, as the web's history shows it (UTC here, the same instant). */
function when(ms) {
  return new Date(ms).toISOString().slice(0, 19).replace("T", " ");
}

/** A row of the timeline: its key, its filter, its time and its words. */
function rowOf(item) {
  const text = model.itemText(item);
  const failed = item.kind === "run" && (Boolean(item.run.error) || (item.run.status ?? 0) >= 400);
  return { key: item.key, filter: model.filterOf(item), when: when(item.at_ms), text, failed };
}

/** What a row's details say, as the web's itemDetails does: what happened, and where going back to it goes. */
function detailsOf(item, target) {
  const fields = [];
  if (item.kind === "event") {
    const e = item.event;
    fields.push(["what", e.what], ["kind", e.kind], ["when", when(e.at_ms)], ["by", e.by ?? "the host"], ["before run", "#" + e.run_seq]);
    if (e.session != null) fields.push(["session", String(e.session)]);
    if (e.name) fields.push(["about", e.name]);
    if (e.kind === "ask" && e.from) fields.push(["asks", e.from + "'s " + (e.restores ?? []).join(" and ") + ", for a person to restore"]);
    else if (e.from) fields.push(["put back", "the state " + e.from + " left"]);
    fields.push(["event", "#" + e.seq]);
  } else if (item.kind === "run") {
    const r = item.run;
    fields.push(["what", r.route ?? r.kind], ["when", when(r.at_ms)], ["session", String(r.session)]);
    if (r.method || r.url) fields.push(["request", [r.method, r.url].filter(Boolean).join(" ")]);
    if (r.status != null) fields.push(["answered", String(r.status)]);
    if (r.error) fields.push(["error", r.error]);
    if (r.duration_ms != null) fields.push(["took", r.duration_ms + " ms"]);
  }
  const v = item.kind === "run" ? undefined : item.version;
  if (v) {
    if (item.kind === "version") fields.push(["what", model.versionWhat(v)], ["when", when(v.deployed_at * 1000)], ["by", v.by ?? "the host"]);
    fields.push(["version", v.number + (v.current ? " (deployed now)" : "")], ["unit", v.unit]);
    if (v.restored_from) fields.push(["restores", "version " + v.restored_from + "'s code"]);
    if (v.first_seq != null) fields.push(["first run", "#" + v.first_seq]);
  }
  if (target) {
    fields.push(
      ["back to its code", target.code && target.why.code === null ? model.pointLabel(target.code) + "'s" : "no: " + target.why.code],
      ["back to its state", target.state && target.why.state === null ? "the state " + model.pointLabel(target.state) + " left" : "no: " + target.why.state],
    );
  }
  return fields;
}

class HistoryPanel {
  /** @param deps { holon(args, opts), openText(text, language) } */
  constructor(deps) {
    this.deps = deps;
    this.slug = null;
    this.timeline = [];
    this.versions = [];
    this.unrecorded = [];
    this.currentUnit = undefined;
    this.shown = new Set(model.FILTERS.map(([name]) => name).filter((name) => name !== "runs"));
    this.selected = null;
    this.report = null;
    this.pending = null;
    this.note = null;
    this.view = undefined;
  }

  resolveWebviewView(view) {
    this.view = view;
    view.webview.options = { enableScripts: true };
    view.webview.html = this.page();
    view.webview.onDidReceiveMessage((m) => this.receive(m));
    view.onDidDispose(() => {
      this.view = undefined;
    });
    this.push();
  }

  /** Shows a deployment's history in the panel, and shows the panel. */
  async choose(slug) {
    this.slug = slug;
    this.selected = null;
    this.report = null;
    this.pending = null;
    this.note = null;
    await vscode.commands.executeCommand("holon.historyPanel.focus");
    if (this.view) this.view.title = `History: ${slug}`;
    await this.load();
  }

  async load() {
    if (!this.slug) return;
    try {
      const page = await this.deps.holon(["history", this.slug, "--last", "200"], { json: true });
      const runs = page.runs ?? [];
      const events = page.events ?? [];
      this.versions = page.versions ?? [];
      this.unrecorded = page.unrecorded ?? [];
      this.currentUnit = this.versions.find((v) => v.current)?.unit;
      this.timeline = model.buildTimeline(runs, events, this.versions);
      this.error = null;
    } catch (e) {
      this.timeline = [];
      this.error = e.message;
    }
    this.push();
  }

  /** The rows the filters show, newest first. */
  rows() {
    return this.timeline.filter((item) => this.shown.has(model.filterOf(item))).map(rowOf);
  }

  /** The picked row's details, and what going back to it goes: null when no row is picked. */
  selection() {
    if (!this.selected) return null;
    const i = this.timeline.findIndex((item) => item.key === this.selected);
    if (i < 0) return null;
    const item = this.timeline[i];
    const target = model.targetOf(this.timeline, i, this.currentUnit);
    const versionNumber = item.version ? item.version.number : null;
    return {
      key: item.key,
      label: target.label,
      fields: detailsOf(item, target),
      versionNumber,
      code: { ok: target.why.code === null, why: target.why.code, label: target.code ? model.pointLabel(target.code) + "'s code" : "its code" },
      state: { ok: target.why.state === null, why: target.why.state, label: target.state ? "the state " + model.pointLabel(target.state) + " left" : "its state" },
    };
  }

  snapshot() {
    return {
      slug: this.slug,
      error: this.error ?? null,
      unrecorded: this.unrecorded,
      filters: model.FILTERS.map(([name, label, title]) => ({ name, label, title, shown: this.shown.has(name) })),
      rows: this.rows(),
      selected: this.selection(),
      report: this.report,
      note: this.note,
    };
  }

  push() {
    if (!this.view) return;
    this.view.webview.postMessage({ type: "state", ...this.snapshot() });
  }

  /** The page's messages: what it asks for. */
  async receive(m) {
    switch (m.type) {
      case "ready":
        this.push();
        break;
      case "refresh":
        await this.load();
        break;
      case "filter":
        if (m.value) this.shown.add(m.name);
        else this.shown.delete(m.name);
        this.push();
        break;
      case "select":
        this.selected = m.key;
        this.report = null;
        this.pending = null;
        this.note = null;
        this.push();
        break;
      case "restore":
        await this.dryRun(m.key, { code: Boolean(m.code), state: Boolean(m.state) });
        break;
      case "approve":
        await this.restore();
        break;
      case "cancel":
        this.report = null;
        this.pending = null;
        this.push();
        break;
      case "showCode":
        await this.showCode(m.key);
        break;
    }
  }

  /** The restore's parts, each dry-run through `holon restore --dry-run`, then the report for them all. Nothing changes. */
  async dryRun(key, what) {
    const i = this.timeline.findIndex((item) => item.key === key);
    if (i < 0) return;
    const target = model.targetOf(this.timeline, i, this.currentUnit);
    const parts = model.restoreParts(target, what);
    if (parts.length === 0) {
      const why = [target.why.code && "its code: " + target.why.code, target.why.state && "its state: " + target.why.state].filter(Boolean);
      this.note = "Nothing to restore to " + target.label + ": " + why.join("; ");
      this.push();
      return;
    }
    try {
      for (const part of parts) {
        const flags = [...(part.what.code ? ["--code"] : []), ...(part.what.state ? ["--state"] : [])];
        const root = await this.deps.holon(["restore", this.slug, ...model.pointArgs(part.point), ...flags, "--dry-run"], { json: true });
        if (root.preview.refused) throw new Error("this host can't deploy that code now: " + root.preview.refused);
        part.preview = root.preview;
        part.touches = root.touches ?? [];
        part.codeDiff = root.code_diff;
      }
      this.pending = { key, target, parts };
      this.report = model.restoreReport(this.slug, target, parts, this.versions);
      this.note = null;
    } catch (e) {
      this.pending = null;
      this.report = null;
      this.note = e.message;
    }
    this.push();
  }

  /** Each approved part, the code first, as `holon restore` (with --approve for what the code needs). */
  async restore() {
    if (!this.pending) return;
    const { target, parts } = this.pending;
    const said = [];
    try {
      for (const part of parts) {
        const flags = [...(part.what.code ? ["--code"] : []), ...(part.what.state ? ["--state"] : []), ...((part.preview.needs_approval ?? []).length > 0 ? ["--approve"] : [])];
        const done = await this.deps.holon(["restore", this.slug, ...model.pointArgs(part.point), ...flags], { json: true });
        if (done.code !== undefined) said.push("version " + done.code + "'s code is deployed again, as version " + done.version);
        if (done.state) said.push("the state " + model.pointLabel(part.point) + " left is live again, in session " + done.state.session);
      }
      this.note = said.length > 0 ? "Restored " + target.label + ": " + said.join("; ") : "Nothing to restore";
      this.report = null;
      this.pending = null;
      await this.load();
    } catch (e) {
      this.note = (said.length > 0 ? "Restored " + said.join("; ") + "; then " : "") + e.message;
      this.report = null;
      this.pending = null;
      await this.load();
    }
  }

  /** A version's flow, as `holon history --version` prints it, read-only beside the panel. */
  async showCode(key) {
    const item = this.timeline.find((x) => x.key === key);
    const number = item && item.version ? item.version.number : null;
    if (number === null) return;
    try {
      const answer = await this.deps.holon(["history", this.slug, "--version", String(number)], { json: true });
      await this.deps.openText(answer.text, "typescript");
    } catch (e) {
      this.note = e.message;
      this.push();
    }
  }

  page() {
    const nonce = randomBytes(16).toString("base64");
    return `<!doctype html>
<html>
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'nonce-${nonce}';">
<style>
  html, body { height: 100%; margin: 0; }
  body { display: flex; flex-direction: column; background: var(--vscode-panel-background); color: var(--vscode-foreground); font-family: var(--vscode-font-family); font-size: var(--vscode-font-size); }
  header { display: flex; align-items: center; gap: 12px; padding: 6px 10px; border-bottom: 1px solid var(--vscode-panel-border); flex-wrap: wrap; }
  header .title { flex: 1; font-weight: 600; }
  header label { color: var(--vscode-descriptionForeground); margin-right: 8px; }
  button { background: var(--vscode-button-secondaryBackground); color: var(--vscode-button-secondaryForeground); border: none; padding: 3px 9px; cursor: pointer; }
  button:hover:not(:disabled) { background: var(--vscode-button-secondaryHoverBackground); }
  button:disabled { opacity: 0.45; cursor: default; }
  button.primary { background: var(--vscode-button-background); color: var(--vscode-button-foreground); }
  button.primary:hover:not(:disabled) { background: var(--vscode-button-hoverBackground); }
  main { display: grid; grid-template-columns: minmax(280px, 40%) 1fr; flex: 1; min-height: 0; }
  #list { overflow: auto; border-right: 1px solid var(--vscode-panel-border); padding: 4px 0; }
  #pane { overflow: auto; padding: 8px 12px; min-width: 0; }
  .row { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 8px; padding: 3px 10px; cursor: pointer; }
  .row:hover { background: var(--vscode-list-hoverBackground); }
  .row.selected { background: var(--vscode-list-activeSelectionBackground); color: var(--vscode-list-activeSelectionForeground); }
  .row.failed .text { color: var(--vscode-errorForeground); }
  .row .text { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .row .when { color: var(--vscode-descriptionForeground); white-space: nowrap; }
  .empty { color: var(--vscode-descriptionForeground); padding: 8px 10px; }
  .error { color: var(--vscode-errorForeground); padding: 6px 10px; }
  .warn { color: var(--vscode-editorWarning-foreground); padding: 6px 10px; }
  .field { display: flex; gap: 14px; padding: 1px 0; }
  .field .name { min-width: 130px; color: var(--vscode-descriptionForeground); }
  .field .value { overflow-wrap: anywhere; }
  .field .value.no { color: var(--vscode-descriptionForeground); }
  .restore { display: flex; flex-wrap: wrap; gap: 6px; align-items: center; padding: 6px 0; border-bottom: 1px solid var(--vscode-panel-border); margin-bottom: 8px; }
  .restore .label { flex: 1; font-weight: 600; }
  .section { margin: 10px 0 4px 0; font-weight: 600; }
  .line { font-family: var(--vscode-editor-font-family); white-space: pre-wrap; overflow-wrap: anywhere; padding-left: 8px; }
  .line.plain { color: var(--vscode-descriptionForeground); }
  .add { color: var(--vscode-gitDecoration-addedResourceForeground); }
  .del { color: var(--vscode-gitDecoration-deletedResourceForeground); }
  .diff { font-family: var(--vscode-editor-font-family); white-space: pre; overflow-x: auto; border-left: 2px solid var(--vscode-panel-border); padding-left: 6px; }
  .note { color: var(--vscode-descriptionForeground); padding: 2px 0 6px 0; }
  .actions { display: flex; gap: 8px; padding-top: 10px; border-top: 1px solid var(--vscode-panel-border); margin-top: 10px; }
  .filters { display: flex; flex-wrap: wrap; gap: 4px 12px; padding: 4px 10px; border-bottom: 1px solid var(--vscode-panel-border); }
</style>
</head>
<body>
<header>
  <span id="title" class="title">History</span>
  <button id="refresh">Refresh</button>
</header>
<div id="filters" class="filters"></div>
<main>
  <div id="list"></div>
  <section id="pane"></section>
</main>
<script nonce="${nonce}">
(function () {
  var vscode = acquireVsCodeApi();
  var title = document.getElementById("title");
  var refresh = document.getElementById("refresh");
  var filters = document.getElementById("filters");
  var list = document.getElementById("list");
  var pane = document.getElementById("pane");
  var state = null;

  function el(tag, cls, text) {
    var n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text !== undefined) n.textContent = text;
    return n;
  }

  function renderFilters() {
    filters.textContent = "";
    state.filters.forEach(function (f) {
      var label = el("label");
      var box = el("input");
      box.type = "checkbox";
      box.checked = f.shown;
      box.title = f.title;
      box.addEventListener("change", function () {
        vscode.postMessage({ type: "filter", name: f.name, value: box.checked });
      });
      label.appendChild(box);
      label.appendChild(document.createTextNode(" " + f.label));
      label.title = f.title;
      filters.appendChild(label);
    });
  }

  function renderList() {
    list.textContent = "";
    if (state.error) list.appendChild(el("div", "error", state.error));
    if (state.unrecorded.length > 0) {
      list.appendChild(el("div", "warn", state.unrecorded.length + " unrecorded handler(s): their runs are counted, not logged."));
    }
    if (state.rows.length === 0 && !state.error) list.appendChild(el("div", "empty", "Nothing in its history yet, with these filters."));
    state.rows.forEach(function (r) {
      var row = el("div", "row" + (r.failed ? " failed" : "") + (state.selected && state.selected.key === r.key ? " selected" : ""));
      row.appendChild(el("span", "text", r.text));
      row.appendChild(el("span", "when", r.when));
      row.addEventListener("click", function () {
        vscode.postMessage({ type: "select", key: r.key });
      });
      list.appendChild(row);
    });
  }

  function renderField(parent, name, value) {
    var row = el("div", "field");
    row.appendChild(el("span", "name", name));
    row.appendChild(el("span", "value" + (String(value).indexOf("no: ") === 0 ? " no" : ""), String(value)));
    parent.appendChild(row);
  }

  function renderDetail(parent) {
    var s = state.selected;
    if (!s) {
      parent.appendChild(el("div", "empty", "Pick a row to see what it did and what going back to it changes."));
      return;
    }
    parent.appendChild(el("div", "section", s.label));
    var bar = el("div", "restore");
    bar.appendChild(el("span", "label", "Restore"));
    if (s.versionNumber !== null) {
      var show = el("button", "", "Code");
      show.title = "Show version " + s.versionNumber + "'s flow";
      show.addEventListener("click", function () {
        vscode.postMessage({ type: "showCode", key: s.key });
      });
      bar.appendChild(show);
    }
    [["code", "Code", { code: true, state: false }, s.code], ["state", "State", { code: false, state: true }, s.state], ["both", "Both", { code: true, state: true }, s.code.ok && s.state.ok ? { ok: true } : { ok: false, why: s.code.why || s.state.why, label: "" }]].forEach(function (b) {
      var btn = el("button", "", b[1]);
      var why = b[3].ok ? null : (b[3].why || "");
      btn.disabled = !!why;
      btn.title = why ? why : b[0] === "code" ? "deploy " + s.code.label + " again, as a new version" : b[0] === "state" ? "put back " + s.state.label + ", in a new session" : s.code.label + ", then " + s.state.label;
      btn.addEventListener("click", function () {
        vscode.postMessage({ type: "restore", key: s.key, code: b[2].code, state: b[2].state });
      });
      bar.appendChild(btn);
    });
    parent.appendChild(bar);
    s.fields.forEach(function (f) {
      renderField(parent, f[0], f[1]);
    });
  }

  function renderDiff(parent, d) {
    parent.appendChild(el("div", "section", d.title));
    parent.appendChild(el("div", "note", d.legend));
    if (d.note) parent.appendChild(el("div", "note", d.note));
    if (d.lines.length === 0) parent.appendChild(el("div", "note", "No lines differ."));
    var block = el("div", "diff");
    d.lines.forEach(function (l) {
      var cls = l.sign === "+" ? "add" : l.sign === "-" ? "del" : "";
      block.appendChild(el("div", cls, (l.sign === " " ? "  " : l.sign + " ") + l.text));
    });
    parent.appendChild(block);
  }

  function renderReport(parent) {
    var r = state.report;
    parent.appendChild(el("div", "section", r.title));
    if (r.changes.length > 0) {
      r.changes.forEach(function (c) {
        parent.appendChild(el("div", "line", "• " + c));
      });
    }
    r.sections.forEach(function (s) {
      parent.appendChild(el("div", "section", s.heading));
      s.lines.forEach(function (l) {
        parent.appendChild(el("div", "line plain", l));
      });
    });
    r.diffs.forEach(function (d) {
      renderDiff(parent, d);
    });
    if (r.approvals.length > 0) {
      parent.appendChild(el("div", "section", "Asks to approve"));
      r.approvals.forEach(function (a) {
        parent.appendChild(el("div", "line", "• " + a));
      });
    }
    var actions = el("div", "actions");
    var cancel = el("button", "", "Cancel");
    cancel.addEventListener("click", function () {
      vscode.postMessage({ type: "cancel" });
    });
    var go = el("button", "primary", r.action);
    go.addEventListener("click", function () {
      vscode.postMessage({ type: "approve" });
    });
    actions.appendChild(cancel);
    actions.appendChild(go);
    parent.appendChild(actions);
  }

  function render() {
    title.textContent = state.slug ? "History of " + state.slug : "History";
    renderFilters();
    renderList();
    pane.textContent = "";
    if (state.note) pane.appendChild(el("div", "warn", state.note));
    if (state.report) renderReport(pane);
    else renderDetail(pane);
  }

  window.addEventListener("message", function (e) {
    var m = e.data;
    if (m.type === "state") {
      state = m;
      render();
    }
  });
  refresh.addEventListener("click", function () {
    vscode.postMessage({ type: "refresh" });
  });
  vscode.postMessage({ type: "ready" });
})();
</script>
</body>
</html>`;
  }
}

/** Registers the History panel in the bottom panel, beside the Runs panel. */
function register(context, deps) {
  const panel = new HistoryPanel(deps);
  context.subscriptions.push(vscode.window.registerWebviewViewProvider("holon.historyPanel", { resolveWebviewView: (view) => panel.resolveWebviewView(view) }));
  return panel;
}

module.exports = { register, HistoryPanel };
