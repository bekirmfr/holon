// The Runs panel: a page in VS Code's bottom panel, with a deployment's runs on the left and the selected run on
// the right, in the tabs the web and terminal consoles show: Detail (its record), Steps (the flow's text with each
// statement's value), Timeline (when each step ran) and Error (where a failed run stopped). The page is ours (its
// script and its style); it asks this extension for the runs, and the extension reads them with the holon command,
// as every view does. A run's record is its plain data: the log's values without their type tags, and without the
// log's own bookkeeping (its hashes, its nonce).

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
    /** Each run's steps, as `holon runs --run` answers them: a run never changes, so each is read once. */
    this.explained = new Map();
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
    this.explained = new Map();
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

  /** A run's steps: `{ value }` as `holon runs --run N --json` answers them, or `{ error }`. */
  async explainOf(seq) {
    const key = `${this.slug}#${seq}`;
    if (!this.explained.has(key)) {
      try {
        this.explained.set(key, { value: await this.deps.holon(["runs", this.slug, "--run", String(seq)], { json: true }) });
      } catch (e) {
        this.explained.set(key, { error: e.message });
      }
    }
    return this.explained.get(key);
  }

  snapshot() {
    return { slug: this.slug, runs: this.list(), failedOnly: this.failedOnly, selected: this.selected };
  }

  push() {
    if (!this.view) return;
    this.view.webview.postMessage({ type: "state", ...this.snapshot(), error: this.error, breaks: this.breaks });
  }

  /** Sends the selected run's record and steps, once its steps are read; a newer selection goes first. */
  async sendRun(seq) {
    const explained = await this.explainOf(seq);
    if (this.view && this.selected === seq) {
      this.view.webview.postMessage({ type: "run", seq, record: this.recordOf(seq), explain: explained.value ?? null, explainError: explained.error ?? null });
    }
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
      case "select":
        this.selected = m.seq;
        this.push();
        this.sendRun(m.seq);
        break;
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
  #pane { display: flex; flex-direction: column; min-width: 0; min-height: 0; }
  #tabs { display: flex; gap: 2px; padding: 0 8px; border-bottom: 1px solid var(--vscode-panel-border); }
  #body { overflow: auto; flex: 1; padding: 8px 12px; }
  .tab { background: transparent; color: var(--vscode-descriptionForeground); border-bottom: 2px solid transparent; border-radius: 0; padding: 5px 9px; }
  .tab:hover { background: var(--vscode-list-hoverBackground); color: var(--vscode-foreground); }
  .tab.active { color: var(--vscode-panelTitle-activeForeground); border-bottom-color: var(--vscode-panelTitle-activeBorder); }
  .tab.bad { color: var(--vscode-errorForeground); }
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
  .line { display: grid; grid-template-columns: 34px minmax(0, 1fr); gap: 0 8px; padding: 1px 0; }
  .num { color: var(--vscode-descriptionForeground); text-align: right; }
  .code { font-family: var(--vscode-editor-font-family); white-space: pre; overflow-x: auto; }
  .note { color: var(--vscode-descriptionForeground); font-family: var(--vscode-editor-font-family); white-space: pre-wrap; overflow-wrap: anywhere; }
  .note.bad { color: var(--vscode-errorForeground); }
  .timing { display: grid; grid-template-columns: 34px minmax(120px, 1fr) minmax(120px, 2fr) 70px; gap: 8px; align-items: center; padding: 2px 0; }
  .label-cell { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: var(--vscode-editor-font-family); }
  .track { position: relative; height: 10px; background: var(--vscode-editorWidget-background); border-radius: 2px; }
  .bar { position: absolute; top: 0; bottom: 0; min-width: 2px; background: var(--vscode-charts-blue); border-radius: 2px; }
  .took { color: var(--vscode-descriptionForeground); text-align: right; white-space: nowrap; }
  .scale { display: flex; justify-content: space-between; color: var(--vscode-descriptionForeground); padding: 0 0 6px 0; }
</style>
</head>
<body>
<header>
  <span id="title" class="title">Runs</span>
  <label><input type="checkbox" id="failed-only"> Failed only</label>
  <button id="refresh">Refresh</button>
  <button id="open-json">Open as JSON</button>
</header>
<main>
  <div id="list"></div>
  <section id="pane">
    <nav id="tabs"></nav>
    <div id="body"></div>
  </section>
</main>
<script nonce="${nonce}">
(function () {
  var vscode = acquireVsCodeApi();
  var list = document.getElementById("list");
  var tabs = document.getElementById("tabs");
  var body = document.getElementById("body");
  var title = document.getElementById("title");
  var failedOnly = document.getElementById("failed-only");
  var refresh = document.getElementById("refresh");
  var openJson = document.getElementById("open-json");
  var TABS = [["detail", "Detail"], ["steps", "Steps"], ["timeline", "Timeline"], ["error", "Error"]];
  var selectedSeq = null;
  var current = null;
  var tab = "detail";

  function el(tag, cls, text) {
    var n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text !== undefined) n.textContent = text;
    return n;
  }

  function us(v) {
    return v < 1000 ? Math.round(v) + " µs" : (v / 1000).toFixed(1) + " ms";
  }

  function renderList(state) {
    title.textContent = state.slug ? "Runs of " + state.slug : "Runs";
    failedOnly.checked = !!state.failedOnly;
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

  function failedRun(rec) {
    return !!rec && (!!rec.error || (rec.status || 0) >= 400);
  }

  function renderTabs() {
    tabs.textContent = "";
    TABS.forEach(function (t) {
      var b = el("button", "tab" + (t[0] === tab ? " active" : "") + (t[0] === "error" && current && failedRun(current.record) ? " bad" : ""), t[1]);
      b.addEventListener("click", function () {
        tab = t[0];
        renderTabs();
        renderBody();
      });
      tabs.appendChild(b);
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

  // Detail: the record, its values as an expandable list.
  function detailNodes(seq, rec) {
    if (rec === undefined) return [el("div", "empty", "That run is no longer in the log.")];
    var out = [el("div", "head", "Run #" + seq)];
    var holder = el("div");
    Object.keys(rec).forEach(function (k) {
      if (rec[k] !== undefined) renderValue(holder, k, rec[k]);
    });
    out.push(holder);
    return out;
  }

  // Steps: the flow's text, each statement with its value under it, as holon runs --run writes it.
  function stepsNodes(explain, explainError) {
    if (explainError) return [el("div", "error", "The steps could not be read: " + explainError)];
    if (!explain) return [el("div", "empty", "Reading the steps…")];
    var out = [];
    if (explain.no_steps) out.push(el("div", "empty", "This run recorded no steps: its values are left out."));
    explain.text.forEach(function (text, i) {
      var line = el("div", "line");
      line.appendChild(el("span", "num", String(i + 1)));
      line.appendChild(el("span", "code", text));
      out.push(line);
      (explain.notes[String(i + 1)] || []).forEach(function (n) {
        var note = el("div", "line");
        note.appendChild(el("span", "num", ""));
        note.appendChild(el("span", "note" + (n.indexOf("gave nothing") >= 0 ? " bad" : ""), n));
        out.push(note);
      });
    });
    explain.after.forEach(function (n) {
      out.push(el("div", "note" + (n.indexOf("failed:") === 0 ? " bad" : ""), n));
    });
    return out;
  }

  // Timeline: when each statement ran, from the run's start, to scale.
  function timelineNodes(explain, explainError) {
    if (explainError) return [el("div", "error", "The timeline could not be read: " + explainError)];
    if (!explain) return [el("div", "empty", "Reading the timeline…")];
    if (explain.steps.length === 0) return [el("div", "empty", "This run recorded no timings.")];
    var total = Math.max.apply(null, explain.steps.map(function (s) { return s.end_us; }).concat([1]));
    var out = [el("div", "scale")];
    out[0].appendChild(el("span", "", "0"));
    out[0].appendChild(el("span", "", us(total / 2)));
    out[0].appendChild(el("span", "", us(total)));
    explain.steps.forEach(function (s) {
      var row = el("div", "timing");
      row.appendChild(el("span", "num", String(s.line)));
      row.appendChild(el("span", "label-cell", (explain.text[s.line - 1] || "").trim()));
      var track = el("div", "track");
      var bar = el("div", "bar");
      bar.style.left = (s.start_us / total) * 100 + "%";
      bar.style.width = Math.max(((s.end_us - s.start_us) / total) * 100, 0.4) + "%";
      track.appendChild(bar);
      row.appendChild(track);
      row.appendChild(el("span", "took", us(s.end_us - s.start_us)));
      out.push(row);
    });
    return out;
  }

  // Error: why a failed run stopped: its answer, its error, and each statement that gave nothing.
  function errorNodes(rec, explain, explainError) {
    if (!rec) return [el("div", "empty", "Select a run to see its error.")];
    var out = [];
    if ((rec.status || 0) >= 400) out.push(el("div", "error", "answered " + rec.status));
    if (rec.error) out.push(el("div", "error", "failed: " + rec.error));
    if (explainError) out.push(el("div", "error", "The steps could not be read: " + explainError));
    if (explain) {
      Object.keys(explain.notes).forEach(function (line) {
        explain.notes[line].forEach(function (n) {
          if (n.indexOf("gave nothing") >= 0) out.push(el("div", "error", "line " + line + ": " + n));
        });
      });
      explain.after.forEach(function (n) {
        if (n.indexOf("failed:") === 0 && n !== "failed: " + rec.error) out.push(el("div", "error", n));
      });
    }
    if (out.length === 0) out.push(el("div", "empty", "No error: the run finished."));
    return out;
  }

  function renderBody() {
    body.textContent = "";
    if (!current) {
      body.appendChild(el("div", "empty", "Select a run to see its record."));
      return;
    }
    var nodes;
    if (tab === "detail") nodes = detailNodes(current.seq, current.record);
    else if (tab === "steps") nodes = stepsNodes(current.explain, current.explainError);
    else if (tab === "timeline") nodes = timelineNodes(current.explain, current.explainError);
    else nodes = errorNodes(current.record, current.explain, current.explainError);
    nodes.forEach(function (n) {
      body.appendChild(n);
    });
  }

  window.addEventListener("message", function (e) {
    var m = e.data;
    if (m.type === "state") {
      selectedSeq = m.selected;
      renderList(m);
      if (m.selected === null) {
        current = null;
        renderTabs();
        renderBody();
      }
    } else if (m.type === "run" && m.seq === selectedSeq) {
      current = m;
      renderTabs();
      renderBody();
    }
  });
  failedOnly.addEventListener("change", function () {
    vscode.postMessage({ type: "failedOnly", value: failedOnly.checked });
  });
  refresh.addEventListener("click", function () {
    vscode.postMessage({ type: "refresh" });
  });
  openJson.addEventListener("click", function () {
    if (selectedSeq !== null) vscode.postMessage({ type: "openJson", seq: selectedSeq });
  });
  renderTabs();
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
