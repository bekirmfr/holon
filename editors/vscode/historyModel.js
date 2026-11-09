// The History panel's model: a deployment's timeline (its runs, what was done to it, and the versions it was
// deployed with), the points it can go back to, and what a restore's dry run says. This is the web console's
// timeline.ts, worded the same, over the parts `holon history --json` prints. Pure functions: no VS Code, no I/O.

/** Runs, events and versions, newest first. An event comes before the run it names (its `run_seq`); a version
 *  deployed before events were kept goes where its first run would. */
function buildTimeline(runs, events, versions) {
  const byNumber = new Map(versions.map((v) => [v.number, v]));
  const named = new Set(events.filter((e) => e.kind === "deploy" && e.version !== undefined).map((e) => e.version));
  const sorted = [...runs].sort((a, b) => a.seq - b.seq);
  const next = sorted.length > 0 ? sorted[sorted.length - 1].seq + 1 : 0;
  // Where a version without its log seq starts: the first run logged after it was deployed.
  const seqAt = (ms) => (sorted.find((r) => r.at_ms >= ms) ?? { seq: next }).seq;
  // Sorted by (seq, rank, order): what was done before a run comes before it.
  const keyed = [
    ...runs.map((run) => ({ seq: run.seq, rank: 1, order: 0, item: { kind: "run", key: `r${run.seq}`, at_ms: run.at_ms, run } })),
    ...events.map((event) => ({
      seq: event.run_seq,
      rank: 0,
      order: event.seq,
      item: {
        kind: "event",
        key: `e${event.seq}`,
        at_ms: event.at_ms,
        event,
        version: event.kind === "deploy" && event.version !== undefined ? byNumber.get(event.version) : undefined,
      },
    })),
    ...versions
      .filter((v) => !named.has(v.number))
      .map((version) => ({
        seq: version.first_seq ?? seqAt(version.deployed_at * 1000),
        rank: 0,
        order: -1_000_000 + version.number,
        item: { kind: "version", key: `v${version.number}`, at_ms: version.deployed_at * 1000, version },
      })),
  ];
  keyed.sort((a, b) => a.seq - b.seq || a.rank - b.rank || a.order - b.order);
  return keyed.reverse().map((k) => k.item);
}

/** The kinds the timeline filters by. The web's history shows all but runs until asked. */
const FILTERS = [
  ["runs", "runs", "each request, trigger and tick it ran"],
  ["versions", "deploys", "deploys and restores"],
  ["access", "access", "grants, secrets, folders and databases"],
  ["lifecycle", "lifecycle", "pauses, stops and runs again"],
];

function filterOf(item) {
  if (item.kind === "run") return "runs";
  if (item.kind === "version") return "versions";
  const k = item.event.kind;
  return k === "deploy" || k === "restore" || k === "ask" ? "versions" : k === "lifecycle" ? "lifecycle" : "access";
}

/** What a version row says: deployed, or restored as it. */
function versionWhat(v) {
  return v.restored_from ? `restored version ${v.restored_from}'s code, as version ${v.number}` : `deployed version ${v.number}`;
}

function byWho(who) {
  return who ? `by ${who}` : "by the host";
}

/** An item's words, as its row shows them and the filter matches them. */
function itemText(item) {
  if (item.kind === "run") return `#${item.run.seq} ${item.run.route ?? item.run.kind} s${item.run.session}`;
  if (item.kind === "version") return `deploy ${versionWhat(item.version)} ${byWho(item.version.by)}`;
  return `${item.event.kind} ${item.event.what} ${byWho(item.event.by)}`;
}

/** A point's name, as a restore says it: `version 2`, `run 41`. */
function pointLabel(p) {
  return "version" in p ? `version ${p.version}` : `run ${p.run}`;
}

/** A point named as a restore names it (`version 3`, `run 41`), or null. */
function parsePoint(label) {
  const m = /^(version|run) (\d+)$/.exec(label ?? "");
  if (!m) return null;
  return m[1] === "version" ? { version: Number(m[2]) } : { run: Number(m[2]) };
}

/** A version: its code unless it is the one deployed now or from before restores (rc.29); the state it left,
 *  when kept. A null means it can. */
function versionOptions(v) {
  return {
    code: v.current
      ? "it is the version deployed now"
      : v.restorable
        ? null
        : "it was deployed before Holon kept what a deploy asks (rc.29 or earlier): open its canvas and deploy it again",
    state: v.current ? "the state it left is the live one" : v.state_kept ? null : "the state it left wasn't kept (deployed with rc.29 or earlier)",
  };
}

/** A run: the code it ran on, unless that is deployed now; the state it left, when it committed one that was kept. */
function runOptions(run, currentUnit) {
  return {
    code: !run.unit ? "the code it ran on isn't recorded" : run.unit === currentUnit ? "it ran on the code deployed now" : null,
    state: run.state_after ? null : "it left no state to restore: it didn't commit, or it ran before Holon kept the state runs leave",
  };
}

/** Where going back to a row goes: the point its code comes from and the point its state comes from, and why not,
 *  for each, when it can't. A run's and a version's are themselves; anything else done to it, how it was right
 *  after: the version deployed then, and the state then. */
function targetOf(timeline, i, currentUnit) {
  const item = timeline[i];
  if (item.kind === "run") {
    const p = { run: item.run.seq };
    return { label: `run ${item.run.seq}`, code: p, state: p, why: runOptions(item.run, currentUnit) };
  }
  if (item.version) {
    const p = { version: item.version.number };
    return { label: `version ${item.version.number}`, code: p, state: p, why: versionOptions(item.version) };
  }
  const event = item.event;
  // An agent's restore, saved for a person: what it asks, of the point it names.
  if (event.kind === "ask") {
    const point = parsePoint(event.from);
    const asks = (part) => (!point ? "it names no point" : (event.restores ?? []).includes(part) ? null : `it asks for its ${part === "code" ? "state" : "code"} only`);
    const why = { code: asks("code"), state: asks("state") };
    return { label: event.from ?? `event #${event.seq}`, code: why.code === null ? point : null, state: why.state === null ? point : null, why, event };
  }
  const before = timeline.slice(i + 1);
  // The code: the version deployed then.
  const deployed = before.map((x) => (x.kind === "run" ? undefined : x.version)).find((v) => v !== undefined);
  const code = deployed
    ? { point: { version: deployed.number }, why: deployed.current ? "the code it had then is the code deployed now" : versionOptions(deployed).code }
    : { point: null, why: "no version was deployed before it" };
  // The state: a restore's is what it put back; anything else's, what was there before it.
  const state = event.kind === "restore" ? restoredState(event) : stateBefore(before);
  return { label: `event #${event.seq}`, code: code.point, state: state.point, why: { code: code.why, state: state.why }, event };
}

/** The point a restore put back: its `from`, or, recorded before it had one, its words. */
function restoredState(event) {
  const said = /^put back the state (.+) left, in session \d+$/.exec(event.what);
  const point = parsePoint(event.from) ?? parsePoint(said ? said[1] : undefined);
  return point ? { point, why: null } : { point: null, why: "it was recorded before a restore named the point it put back" };
}

/** The state as it was after `before` (newest first): the last run's that committed one, or what a restore after it put back. */
function stateBefore(before) {
  for (const x of before) {
    if (x.kind === "run") {
      if (x.run.state_after) return { point: { run: x.run.seq }, why: null };
      continue;
    }
    if (x.kind !== "event") continue;
    if (x.event.kind === "restore") return restoredState(x.event);
    // A run after a stop starts from a state of its own, unless it goes on from the latest.
    if (x.event.kind === "lifecycle" && x.event.session != null && !x.event.what.startsWith("ran from latest")) {
      return { point: null, why: `its state then came from a stop ("${x.event.what}"), which isn't a point to go back to` };
    }
  }
  return { point: null, why: "no run had left a state before it" };
}

/** One call a restore makes: a point, and what of it. Code and state from one point go together; from two, one
 *  after the other, the code first. */
function restoreParts(target, what) {
  const code = what.code && target.why.code === null ? target.code : null;
  const state = what.state && target.why.state === null ? target.state : null;
  if (code && state && pointLabel(code) === pointLabel(state)) return [{ point: code, what: { code: true, state: true } }];
  return [...(code ? [{ point: code, what: { code: true, state: false } }] : []), ...(state ? [{ point: state, what: { code: false, state: true } }] : [])];
}

/** The `holon restore` arguments that go back to a point. */
function pointArgs(point) {
  return "version" in point ? ["--to", String(point.version)] : ["--to-run", String(point.run)];
}

/** A capability a run performed, as the dry run counts it. */
function effectWord(name) {
  return (
    { HttpRequest: "call(s) out", FileWrite: "file write(s)", DbWrite: "database write(s)", ChannelSend: "message(s) sent", RunsAction: "action(s) run", Spawn: "run(s) started" }[name] ?? name
  );
}

/** A file the runs since a point wrote, as a dry run says it: how many writes, by which runs, and whether it is on disk as the last one left it. */
function fileLine(f) {
  const n = f.writes;
  const more = f.runs.length < n ? "…, " : "";
  const how = { write: "written", copy: "copied over", delete: "deleted" }[f.last.kind] ?? "written";
  let line = `${f.folder}/${f.name}: ${n} write${n === 1 ? "" : "s"} since, by run${n === 1 ? "" : "s"} ${more}${f.runs.join(", ")} (the last ${how})`;
  const last = f.last.run;
  if (f.unreadable) line += `; can't be checked on disk now: ${f.unreadable}`;
  else if (f.outside) line += f.disk ? `; changed outside a run since run ${last}: on disk now #${f.disk.hash.slice(0, 8)}, not what run ${last} left` : `; gone from disk since run ${last}, which left it`;
  return line;
}

/** What putting a state back would change: the keys, and the runs since with what they did outside. */
function stateLines(change) {
  const parts = [];
  for (const [word, list] of [
    ["changes", change.changed],
    ["brings back", change.added],
    ["drops", change.removed],
  ]) {
    // A collection's key in the state (`#collection:prices`) said as a person reads it.
    if (list.length > 0) parts.push(`${word} ${list.map((k) => (k.startsWith("#collection:") ? `the collection ${k.slice("#collection:".length)}` : k)).join(", ")}`);
  }
  const lines = [parts.length === 0 ? `The state ${change.from} left is the state now: putting it back changes no key.` : `The state ${change.from} left is put back: it ${parts.join("; ")}.`];
  const effects = Object.entries(change.effects_since ?? {}).map(([name, n]) => `${n} ${effectWord(name)}`);
  const runs = change.runs_since;
  lines.push(
    runs === 0
      ? `No run was logged since ${change.from}.`
      : effects.length === 0
        ? `${runs} run(s) logged since ${change.from}, which did nothing outside its state.`
        : `${runs} run(s) logged since ${change.from} also did, outside its state, what a restore doesn't undo: ${effects.join(", ")}.`,
  );
  const unrecorded = change.unrecorded ?? [];
  if (unrecorded.length > 0) lines.push(`Runs of ${unrecorded.join(", ")} (unrecorded) aren't logged: what they did outside its state isn't counted here.`);
  lines.push("The state it replaces is kept, as the ending session's.");
  return lines;
}

/** The grant now beside the one the restored code would have (its items, less what is withheld): null when there is
 *  no grant now or no code is deployed. */
function grantDiff(preview) {
  if (!preview.granted_now || !preview.items) return null;
  const withheld = new Set(preview.withheld ?? []);
  return setDiff(preview.granted_now, preview.items.filter((i) => !withheld.has(i)));
}

/** What a restore changes, a line each: the code deployed, the grant, the state. */
function restoreChanges(parts, versions) {
  const changes = [];
  const now = versions.find((v) => v.current)?.number;
  const next = versions.reduce((n, v) => Math.max(n, v.number), 0) + 1;
  for (const { preview } of parts) {
    if (preview.code !== null && preview.code !== undefined) {
      changes.push(`code: version ${preview.code}'s is deployed again, as version ${next}${now !== undefined ? `, in place of version ${now}` : ""}`);
      const grant = grantDiff(preview);
      if (grant) {
        const { added, removed } = diffCounts(grant);
        const kept = grant.length - added - removed;
        changes.push(added + removed === 0 ? `permissions: unchanged (${kept} kept)` : `permissions: ${added} added, ${removed} dropped, ${kept} kept`);
      }
    }
    if (preview.state) {
      const s = preview.state;
      const said = [
        [s.changed.length, "changed"],
        [s.added.length, "brought back"],
        [s.removed.length, "dropped"],
      ]
        .filter(([n]) => n)
        .map(([n, word]) => `${n} key${n === 1 ? "" : "s"} ${word}`);
      const items = Object.entries(s.collections ?? {}).map(([name, c]) => {
        const n = Object.keys(c.items).length;
        return `${name}: ${n}${c.more ? "+" : ""} item${n === 1 && !c.more ? "" : "s"}`;
      });
      const detail = items.length > 0 ? ` (${items.join(", ")})` : "";
      changes.push(said.length === 0 ? `state: as ${s.from} left it, the same as now` : `state: as ${s.from} left it, ${said.join(", ")}${detail}`);
      const files = s.files ?? [];
      if (files.length > 0) {
        const outside = files.filter((f) => f.outside).length;
        const n = `${files.length}${s.files_more ? "+" : ""}`;
        changes.push(`files: left as they are (${n} written since${outside > 0 ? `, ${outside} of them changed outside a run` : ""})`);
      }
    }
  }
  return changes;
}

/** A restore's dry run, as the dry-run window shows it: what it changes, a line each, what it goes back to, what its code and its state would do, and what it asks to approve.
 *  Each part is `{ point, what, preview, touches, codeDiff }`: its dry run's preview, the touches it has as lines, and its code's diff. */
function restoreReport(slug, target, parts, versions) {
  const code = parts.some((p) => p.what.code);
  const state = parts.some((p) => p.what.state);
  const which = code && state ? "code and state" : code ? "code" : "state";
  const sections = [];
  if (target.event) {
    const had = parts.flatMap((p) => [...(p.what.code ? [`${pointLabel(p.point)}'s code`] : []), ...(p.what.state ? [`the state ${pointLabel(p.point)} left`] : [])]);
    sections.push({ heading: "as it was then", lines: [`As it was after "${target.event.what}": ${had.join(", and ")}.`] });
  }
  const approvals = [];
  for (const { point, what, preview, touches } of parts) {
    if (what.code) {
      if (preview.code === null || preview.code === undefined) {
        sections.push({ heading: "code", lines: [`${pointLabel(point)} ran on the code deployed now: only its state is restored.`] });
      } else {
        sections.push({ heading: "code", lines: [`Version ${preview.code}'s code is deployed again, as a new version, checked against the grant "${slug}" has now.`] });
        approvals.push(...(preview.needs_approval ?? []));
      }
    }
    // Everything it touches, beside what it touches now: what is not ready (a secret not set), what putting the state back does to each key, what the runs since did that isn't undone.
    if (touches.length > 0) sections.push({ heading: "what it touches (+ new, - no longer, ~ changed, ! not ready)", lines: touches });
    if (preview.state) sections.push({ heading: "state", lines: stateLines(preview.state) });
    const files = preview.state?.files ?? [];
    if (files.length > 0) {
      sections.push({
        heading: "files, not put back",
        lines: [...files.map(fileLine), ...(preview.state.files_more ? ["and more"] : []), "A restore never changes a folder's files: these stay as they are now."],
      });
    }
  }
  return {
    title: `Restore "${slug}" to ${target.label}: its ${which}?`,
    changes: restoreChanges(parts, versions),
    sections,
    diffs: restoreDiffs(parts, versions),
    approvals,
    action: approvals.length > 0 ? "Approve and restore" : "Restore",
  };
}

/** The diffs a restore's dry run shows, part by part: the code (as `holon restore --dry-run` prints it), the grant, the
 *  state's keys, and each file the runs since wrote. */
function restoreDiffs(parts, versions) {
  const diffs = [];
  const now = versions.find((v) => v.current);
  for (const { preview, codeDiff } of parts) {
    if (preview.code !== null && preview.code !== undefined) {
      const legend = `− version ${now?.number ?? "?"}, deployed now   + version ${preview.code}`;
      if (codeDiff && codeDiff.lines) {
        const note = (codeDiff.note ?? "").trim();
        diffs.push({ title: "code", legend, lines: codeDiff.lines.map(parseCliLine), note: note || undefined });
      } else {
        diffs.push({ title: "code", legend, lines: [], note: `The text can't be compared: ${codeDiff?.error ?? "it isn't printed"}` });
      }
      const grant = grantDiff(preview);
      if (grant) diffs.push({ title: "permissions", legend: "− granted now, dropped   + added   kept", lines: grant, whole: true });
    }
    if (preview.state) diffs.push(stateDiff(preview.state));
    for (const f of preview.state?.files ?? []) diffs.push(fileDiff(f, preview.state.from));
  }
  return diffs;
}

/** One line `holon restore --dry-run` printed as a diff: `- ` removed, `+ ` added, two spaces unchanged. */
function parseCliLine(line) {
  const sign = line.startsWith("- ") ? "-" : line.startsWith("+ ") ? "+" : " ";
  return { sign, text: line.slice(2) };
}

/** A value as its line of a state diff. */
function showValue(v) {
  if (v === null || v === undefined) return "(none)";
  switch (v.type) {
    case "Text":
      return JSON.stringify(v.value);
    case "Number":
    case "Boolean":
      return String(v.value);
    case "Unit":
      return "()";
    case "Record":
      return `{ ${(v.value ?? []).map(([k, x]) => `${k}: ${showValue(x)}`).join(", ")} }`;
    case "List":
      return `[${(v.value ?? []).map(showValue).join(", ")}]`;
    default:
      return JSON.stringify(v.value ?? v);
  }
}

/** The keys a state restore touches, as they are now beside as they would be. */
function stateDiff(change) {
  const shown = [
    ...Object.entries(change.values ?? {}),
    ...Object.entries(change.collections ?? {}).flatMap(([name, c]) => Object.entries(c.items).map(([k, v]) => [`${name}[${JSON.stringify(k)}]`, v])),
  ].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  const side = (pick) => shown.flatMap(([k, pair]) => (pick(pair) ? [`${k}: ${showValue(pick(pair))}`] : []));
  const touched = change.changed.length + change.added.length + change.removed.length;
  const keys = Object.keys(change.values ?? {}).length + Object.keys(change.collections ?? {}).length;
  const partial = Object.entries(change.collections ?? {}).filter(([, c]) => c.more).map(([name]) => name);
  const notes = [
    touched === 0 ? "No key changes." : touched > keys ? `${touched - keys} more key(s) change: their values aren't shown.` : "",
    partial.length > 0 ? `Only some of ${partial.join(", ")}'s items are compared or shown.` : "",
  ].filter(Boolean);
  return { title: "state", legend: `− now   + as ${change.from} left it`, lines: lineDiff(side((p) => p.now), side((p) => p.then)), note: notes.join(" ") || undefined };
}

/** How a file the runs since `from` wrote changed since, which a restore leaves as it is: its text then beside now, when the log has both; else what is known, by hash. */
function fileDiff(f, from) {
  const short = (h) => (h ? `#${h.slice(0, 8)}` : "none");
  const notes = ["A restore leaves it as it is now."];
  if (f.outside) notes.push(f.disk ? `Changed outside a run since run ${f.last.run}.` : `Gone from disk since run ${f.last.run}.`);
  if (f.unreadable) notes.push(`It can't be checked on disk now: ${f.unreadable}.`);
  const lines = (text, absent) => (absent ? [] : text === null || text === undefined ? null : text.replace(/\n$/, "").split("\n"));
  const then = f.at_point ? lines(f.then_text, f.at_point.hash === null) : null;
  const now = f.unreadable ? null : lines(f.now_text, f.disk === null);
  if (!f.at_point) notes.push(`No run wrote it before ${from}: its text then isn't known.`);
  else if (then === null) notes.push(`Its text at ${from} isn't in the log (no run read it): ${short(f.at_point.hash)} then, ${short(f.disk?.hash)} now.`);
  else if (now === null && !f.unreadable) notes.push(`It is too big to show: ${short(f.at_point.hash)} then, ${short(f.disk?.hash)} now.`);
  return {
    title: `file ${f.folder}/${f.name}`,
    legend: `− at ${from}   + now on disk`,
    lines: then !== null && now !== null ? lineDiff(then, now) : [],
    note: notes.join(" "),
  };
}

/** Past this many cells the middle of a diff is shown as all removed then all added, not matched line by line. */
const MAX_CELLS = 4_000_000;

/** `before` to `after`, line by line: the longest run of lines they share is kept, the rest removed or added. */
function lineDiff(before, after) {
  let start = 0;
  while (start < before.length && start < after.length && before[start] === after[start]) start++;
  let end = 0;
  while (end < before.length - start && end < after.length - start && before[before.length - 1 - end] === after[after.length - 1 - end]) end++;
  const a = before.slice(start, before.length - end);
  const b = after.slice(start, after.length - end);
  const middle = a.length * b.length > MAX_CELLS ? [...a.map((text) => ({ sign: "-", text })), ...b.map((text) => ({ sign: "+", text }))] : matched(a, b);
  const same = (text) => ({ sign: " ", text });
  return [...before.slice(0, start).map(same), ...middle, ...before.slice(before.length - end).map(same)];
}

function matched(a, b) {
  const w = b.length + 1;
  // longest[i * w + j]: the longest common run of a[i..] and b[j..].
  const longest = new Uint32Array((a.length + 1) * w);
  for (let i = a.length - 1; i >= 0; i--) {
    for (let j = b.length - 1; j >= 0; j--) {
      longest[i * w + j] = a[i] === b[j] ? longest[(i + 1) * w + j + 1] + 1 : Math.max(longest[(i + 1) * w + j], longest[i * w + j + 1]);
    }
  }
  const out = [];
  let i = 0;
  let j = 0;
  while (i < a.length || j < b.length) {
    if (i < a.length && j < b.length && a[i] === b[j]) {
      out.push({ sign: " ", text: a[i] });
      i++;
      j++;
    } else if (j >= b.length || (i < a.length && longest[(i + 1) * w + j] >= longest[i * w + j + 1])) {
      out.push({ sign: "-", text: a[i++] });
    } else {
      out.push({ sign: "+", text: b[j++] });
    }
  }
  return out;
}

/** What is only in `before`, only in `after`, and in both, sorted: a grant's items, say. */
function setDiff(before, after) {
  const was = new Set(before);
  const is = new Set(after);
  return [...new Set([...was, ...is])].sort().map((text) => ({ sign: was.has(text) ? (is.has(text) ? " " : "-") : "+", text }));
}

function diffCounts(lines) {
  return { added: lines.filter((l) => l.sign === "+").length, removed: lines.filter((l) => l.sign === "-").length };
}

module.exports = {
  buildTimeline,
  FILTERS,
  filterOf,
  versionWhat,
  itemText,
  pointLabel,
  parsePoint,
  targetOf,
  restoreParts,
  pointArgs,
  restoreReport,
};
