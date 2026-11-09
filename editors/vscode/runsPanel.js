// The Runs panel: a page in VS Code's bottom panel, with a deployment's runs on the left and the record of the
// selected run on the right. The page is ours (its script and its style); it asks this extension for the runs,
// and the extension reads them with the holon command, as every view does. A run's record is its plain data:
// the log's values without their type tags, and without the log's own bookkeeping (its hashes, its nonce).

const vscode = require("vscode");
const { randomBytes } = require("node:crypto");

/** Whether a run failed: it has an error, or answered 400 or more. */
function failed(e) {
  return Boolean(e.error) || (e.status ?? 0) >= 400;
}

/** A run's record for reading: its values as plain values (a record as an object, a list as an array), and
 *  only what an audit reads. */
function runRecord(e) {
  const WVALUES = ["Text", "Number", "Boolean", "Record", "List", "Bytes", "Any", "Unit", "Effect"];
  const plain = (v) => {
    if (v === null || typeof v !== "object") return v;
    if (Array.isArray(v)) return v.map(plain);
    if (WVALUES.includes(v.type) && "value" in v) {
      if (v.type === "Record" && Array.isArray(v.value)) return Object.fromEntries(v.value.map(([k, x]) => [k, plain(x)]));
      if (v.type === "List" && Array.isArray(v.value)) return v.value.map(plain);
      return plain(v.value);
    }
    return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, plain(x)]));
  };
  return {
    when: new Date(e.at_ms).toISOString(),
    session: e.session,
    startedBy: e.route ? `${e.kind} ${e.route}` : e.kind,
    request: e.method || e.url ? `${e.method ?? ""} ${e.url ?? ""}`.trim() : undefined,
    status: e.status ?? undefined,
    durationMs: e.duration_ms,
    error: e.error ?? undefined,
    requestRecord: plain(e.request),
    sourced: Array.isArray(e.sourced) ? Object.fromEntries(e.sourced.map(([k, x]) => [k, plain(x)])) : undefined,
    stateBefore: plain(e.state_before),
    outputs: plain(e.outputs),
    performed: plain(e.performed),
    response: e.response_body ?? undefined,
    timeline: plain(e.timeline),
  };
}

class RunsPanel {
  /** @param deps { holon(args, opts), openJson(value) } */
  constructor(deps) {
    this.deps = deps;
    this.slug = null;
    this.entries = [];
    this.error = null;
    this.breaks = [];
    this.failedOnly = false;
    this.selected = null;
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

  /** Shows a deployment's runs in the panel, and shows the panel. */
  async choose(slug) {
    this.slug = slug;
    this.selected = null;
    await vscode.commands.executeCommand("holon.runsPanel.focus");
    if (this.view) this.view.title = `Runs: ${slug}`;
    await this.load();
  }

  async load() {
    if (!this.slug) return;
    try {
      const log = await this.deps.holon(["runs", this.slug, "--last", "200"], { json: true });
      this.entries = log.entries ?? [];
      this.breaks = log.breaks ?? [];
      this.error = null;
    } catch (e) {
      this.entries = [];
      this.error = e.message;
    }
    this.push();
  }

  /** The runs as the list shows them: newest first, and only the failed ones if asked. */
  list() {
    return this.entries
      .filter((e) => !this.failedOnly || failed(e))
      .slice()
      .reverse()
      .map((e) => ({ seq: e.seq, label: `#${e.seq} ${e.route ?? e.kind}`, status: e.status ?? null, durationMs: e.duration_ms, failed: failed(e) }));
  }

  /** The record of a run, as the panel shows it; undefined when the log has no such run. */
  recordOf(seq) {
    const e = this.entries.find((x) => x.seq === seq);
    return e ? runRecord(e) : undefined;
  }

  snapshot() {
    return { slug: this.slug, runs: this.list(), failedOnly: this.failedOnly, selected: this.selected };
  }

  push() {
    if (!this.view) return;
    this.view.webview.postMessage({ type: "state", ...this.snapshot(), error: this.error, breaks: this.breaks });
  }

  /** The page's messages: what it asks for. */
  receive(m) {
    switch (m.type) {
      case "ready":
        this.push();
        break;
      case "refresh":
        this.load();
        break;
      case "failedOnly":
        this.failedOnly = Boolean(m.value);
        this.push();
        break;
      case "select": {
        this.selected = m.seq;
        this.push();
        if (this.view) this.view.webview.postMessage({ type: "record", seq: m.seq, record: this.recordOf(m.seq) });
        break;
      }
      case "openJson": {
        const record = this.recordOf(m.seq);
        if (record) this.deps.openJson(record);
        break;
      }
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
  header { display: flex; align-items: center; gap: 12px; padding: 6px 10px; border-bottom: 1px solid var(--vscode-panel-border); }
  header .title { flex: 1; font-weight: 600; }
  header label { color: var(--vscode-descriptionForeground); }
  button { background: var(--vscode-button-secondaryBackground); color: var(--vscode-button-secondaryForeground); border: none; padding: 3px 9px; cursor: pointer; }
  button:hover { background: var(--vscode-button-secondaryHoverBackground); }
  main { display: grid; grid-template-columns: minmax(240px, 34%) 1fr; flex: 1; min-height: 0; }
  #list { overflow: auto; border-right: 1px solid var(--vscode-panel-border); padding: 4px 0; }
  #record { overflow: auto; padding: 8px 12px; }
  .run { display: flex; justify-content: space-between; gap: 10px; padding: 3px 10px; cursor: pointer; }
  .run:hover { background: var(--vscode-list-hoverBackground); }
  .run.selected { background: var(--vscode-list-activeSelectionBackground); color: var(--vscode-list-activeSelectionForeground); }
  .run.failed .label { color: var(--vscode-errorForeground); }
  .meta { color: var(--vscode-descriptionForeground); white-space: nowrap; }
  .leaf { display: flex; gap: 14px; padding: 1px 0 1px 14px; }
  .key { min-width: 150px; }
  .value { color: var(--vscode-descriptionForeground); overflow-wrap: anywhere; }
  details { padding-left: 12px; }
  summary { cursor: pointer; padding: 2px 0; }
  .count { color: var(--vscode-descriptionForeground); }
  .head { font-weight: 600; margin-bottom: 6px; }
  .empty { color: var(--vscode-descriptionForeground); padding: 8px 10px; }
  .error { color: var(--vscode-errorForeground); padding: 6px 10px; }
  .warn { color: var(--vscode-editorWarning-foreground); padding: 6px 10px; }
</style>
</head>
<body>
<header>
  <span id="title" class="title">Runs</span>
  <label><input type="checkbox" id="failed"> Failed only</label>
  <button id="refresh">Refresh</button>
  <button id="open-json">Open as JSON</button>
</header>
<main>
  <div id="list"></div>
  <div id="record"></div>
</main>
<script nonce="${nonce}">
(function () {
  var vscode = acquireVsCodeApi();
  var list = document.getElementById("list");
  var record = document.getElementById("record");
  var title = document.getElementById("title");
  var failed = document.getElementById("failed");
  var refresh = document.getElementById("refresh");
  var openJson = document.getElementById("open-json");
  var selectedSeq = null;

  function el(tag, cls, text) {
    var n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text !== undefined) n.textContent = text;
    return n;
  }

  function renderList(state) {
    title.textContent = state.slug ? "Runs of " + state.slug : "Runs";
    failed.checked = !!state.failedOnly;
    list.textContent = "";
    if (!state.slug) {
      list.appendChild(el("div", "empty", "Click the runs button on a deployment to audit its runs."));
      return;
    }
    if (state.error) list.appendChild(el("div", "error", state.error));
    if (state.breaks && state.breaks.length > 0) {
      list.appendChild(el("div", "warn", "The log was changed outside holon before runs " + state.breaks.join(", ") + "; their record cannot be trusted."));
    }
    if (state.runs.length === 0) list.appendChild(el("div", "empty", state.failedOnly ? "No failed runs." : "No runs yet."));
    state.runs.forEach(function (r) {
      var row = el("div", "run" + (r.failed ? " failed" : "") + (r.seq === state.selected ? " selected" : ""));
      row.appendChild(el("span", "label", r.label));
      row.appendChild(el("span", "meta", (r.status === null ? "" : String(r.status)) + "  " + r.durationMs + " ms"));
      row.addEventListener("click", function () {
        vscode.postMessage({ type: "select", seq: r.seq });
      });
      list.appendChild(row);
    });
  }

  function renderValue(parent, key, v) {
    if (v !== null && typeof v === "object") {
      var det = el("details");
      var count = Array.isArray(v) ? v.length : Object.keys(v).length;
      var sum = el("summary", "", key);
      sum.appendChild(el("span", "count", "  " + count + " " + (Array.isArray(v) ? "item" : "field") + (count === 1 ? "" : "s")));
      det.appendChild(sum);
      Object.keys(v).forEach(function (k) {
        renderValue(det, k, v[k]);
      });
      parent.appendChild(det);
      return;
    }
    var row = el("div", "leaf");
    row.appendChild(el("span", "key", key));
    row.appendChild(el("span", "value", v === null || v === undefined ? "null" : String(v)));
    parent.appendChild(row);
  }

  function renderRecord(seq, rec) {
    record.textContent = "";
    if (rec === undefined) {
      record.appendChild(el("div", "empty", "That run is no longer in the log."));
      return;
    }
    record.appendChild(el("div", "head", "Run #" + seq));
    Object.keys(rec).forEach(function (k) {
      if (rec[k] !== undefined) renderValue(record, k, rec[k]);
    });
  }

  window.addEventListener("message", function (e) {
    var m = e.data;
    if (m.type === "state") {
      selectedSeq = m.selected;
      renderList(m);
      if (m.selected === null) record.textContent = "";
    } else if (m.type === "record") {
      renderRecord(m.seq, m.record);
    }
  });
  failed.addEventListener("change", function () {
    vscode.postMessage({ type: "failedOnly", value: failed.checked });
  });
  refresh.addEventListener("click", function () {
    vscode.postMessage({ type: "refresh" });
  });
  openJson.addEventListener("click", function () {
    if (selectedSeq !== null) vscode.postMessage({ type: "openJson", seq: selectedSeq });
  });
  vscode.postMessage({ type: "ready" });
})();
</script>
</body>
</html>`;
  }
}

/** Registers the Runs panel in the bottom panel. */
function register(context, deps) {
  const panel = new RunsPanel(deps);
  context.subscriptions.push(vscode.window.registerWebviewViewProvider("holon.runsPanel", { resolveWebviewView: (view) => panel.resolveWebviewView(view) }));
  return panel;
}

module.exports = { register, runRecord, RunsPanel };
