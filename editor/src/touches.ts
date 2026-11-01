// What a flow touches, as a dry run shows it (a deploy's consent, a restore's dry run): the rows
// the server works out (crates/holon/src/touches.rs), said as `holon editor` and the CLI say them:
// grouped, a row each, marked with what the change does to it, a note on what the server has for
// it, why it isn't ready, and what a restore does to it.

/** One use of a thing a flow touches: its capability item and what it does with it. */
export type TouchUse = { item: string; what: string; words: string; change: Change; approval: boolean; optional: boolean };
type Change = "new" | "gone" | "same" | "changed";

/** One thing a flow touches (a folder, a database, a secret, a host, a key…), as the server
 *  answers it in a preview's `touches`. */
export type Touch = {
  group: string;
  kind: string;
  name: string;
  change: Change;
  uses: TouchUse[];
  /** Why this server isn't ready for it. */
  ready: string | null;
  /** What this server has for it (a secret's version and source). */
  has: string | null;
  /** A restore: how many times the runs since did it, which a restore doesn't undo. */
  since?: number;
  /** A restore, for a key or a collection: what putting the state back does to it. */
  restores?: "changes" | "brings back" | "drops" | "keeps";
};

/** A deploy setting that changes: how often it runs, the sites whose pages it answers. */
export type SettingChange = { name: string; now: string; then: string };

const GROUPS: [string, string][] = [
  ["triggers", "What starts it"],
  ["resources", "Folders, databases and secrets"],
  ["outside", "What it reaches outside"],
  ["state", "What it keeps"],
  ["limits", "Its limits"],
  ["other", "Also"],
];

export type Mark = "head" | "new" | "approve" | "gone" | "same" | "changed" | "unready";
/** A line of a dry run: its mark, what it says, and a note on it. */
export type TouchLine = { mark: Mark; text: string; note: string };

const SIGN: Record<Mark, string> = { head: "", new: "+ ", approve: "+ ", gone: "- ", same: "· ", changed: "~ ", unready: "! " };

/** One of a kind, said without a name: the requests it receives, the clock it reads. */
const ALONE: Record<string, string> = { requests: "requests", clock: "the clock", process: "processes", randomness: "randomness" };

/** `touches` and `settings` as a dry run's lines, group by group. */
export function touchLines(touches: Touch[] = [], settings: SettingChange[] = []): TouchLine[] {
  const out: TouchLine[] = [];
  for (const [group, heading] of GROUPS) {
    const rows = touches.filter((t) => t.group === group);
    if (rows.length === 0) continue;
    out.push({ mark: "head", text: heading, note: "" });
    out.push(...rows.map(rowLine));
  }
  if (settings.length > 0) {
    out.push({ mark: "head", text: "Its settings", note: "" });
    out.push(...settings.map((s): TouchLine => ({ mark: "changed", text: `${s.name}: ${s.now} → ${s.then}`, note: "" })));
  }
  return out;
}

/** Dry-run lines as plain text: a heading as it is, a row after its sign, its note after a dash. */
export function touchText(lines: TouchLine[]): string[] {
  return lines.map((l) => (l.mark === "head" ? l.text : `  ${SIGN[l.mark]}${l.text}${l.note ? ` — ${l.note}` : ""}`));
}

function rowLine(row: Touch): TouchLine {
  const approval = row.uses.some((u) => u.approval);
  let mark: Mark = row.change === "new" ? (approval ? "approve" : "new") : row.change === "gone" ? "gone" : row.change === "changed" ? "changed" : "same";
  // A use is marked when it changes and the row as a whole doesn't say so.
  const sign = (u: TouchUse) => (mark !== "changed" ? "" : u.change === "new" ? "+" : u.change === "gone" ? "−" : "");
  const said = row.uses.filter((u) => u.what !== "").map((u) => `${sign(u)}${u.what}${u.optional ? " (optional)" : ""}`);
  const { kind, name } = row;
  const thing =
    ALONE[kind] ?? (kind === "network" && name === "" ? "the network" : kind === "other" || kind === "network" || kind === "limit" ? name : name === "" ? `${kind}s` : `${kind} ${name}`);
  const text = kind === "limit" ? `${thing} = ${said.join(", ")}` : said.length === 0 ? thing : `${thing}: ${said.join(", ")}`;
  const notes: string[] = [];
  if (mark === "approve") notes.push("approved by confirming");
  else if (mark === "new") notes.push("new");
  else if (mark === "changed" && approval) notes.push("the new use approved by confirming");
  if (row.ready) {
    notes.push(row.ready);
    if (mark !== "gone") mark = "unready";
  } else if (row.has) notes.push(row.has);
  if (mark === "gone") notes.push("no longer");
  if (row.restores) notes.push(row.restores === "keeps" ? "its value stays as it is" : `putting the state back ${row.restores} it`);
  if (row.since) notes.push(`done ${row.since} time${row.since === 1 ? "" : "s"} by the runs since, not undone`);
  return { mark, text, note: notes.join("; ") };
}
