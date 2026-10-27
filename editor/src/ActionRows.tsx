// A deployment's actions as rows: each with a box for what to send it (remembered per action), run,
// the answer under it with its run's number and view run, and the command a script would use. The
// server panel and the deployed page both show them (docs/L4s/triggers-L4.md §7).

import { useState, type CSSProperties } from "react";
import { actionUrl, runAction, type Role } from "./server";

const field: CSSProperties = {
  font: "inherit",
  fontSize: 11,
  color: "#e6eefb",
  background: "#0e1a2f",
  border: "1px solid #1b2a45",
  borderRadius: 3,
  padding: "2px 4px",
};
const button: CSSProperties = { ...field, color: "#9fb4d4", cursor: "pointer", padding: "2px 8px" };
const link: CSSProperties = { ...button, border: "none", background: "transparent", padding: 0, textDecoration: "underline" };

/** What an action's last run answered. */
type Ran = { text: string; ok: boolean; seq?: number };

export function ActionRows({
  names,
  slug,
  role,
  outOfDate = false,
  onViewRun,
}: {
  /** The actions' names (`reset`, for `action reset (input)`). */
  names: string[];
  /** The running deployment that runs them; empty when there is none (the rows are greyed). */
  slug: string;
  /** Who is signed in: a viewer sees the rows, never runs one. */
  role?: Role;
  /** The canvas has changes not deployed: a run runs the deployed version, which says so. */
  outOfDate?: boolean;
  /** Open view mode on the run an action made. */
  onViewRun?: (seq: number) => void;
}) {
  // What to send each action (JSON; empty is {}), and what each last answered.
  const [sent, setSent] = useState<Record<string, string>>({});
  const [ran, setRan] = useState<Record<string, Ran>>({});
  const viewer = role === "viewer";
  const run = (name: string) => {
    const answered = (r: Ran) => setRan((m) => ({ ...m, [name]: r }));
    answered({ text: "running…", ok: true });
    runAction(slug, name, sent[name] ?? "")
      .then((r) => answered({ text: r.waiting ? `waiting (run ${r.seq})` : `${JSON.stringify(r.answer)} (run ${r.seq})`, ok: true, seq: r.seq }))
      .catch((err: unknown) => answered({ text: err instanceof Error ? err.message : String(err), ok: false }));
  };
  if (names.length === 0) return null;
  const why = viewer ? "an editor or an admin runs this" : !slug ? "deploy the flow to run it" : "run this action once, now, on the deployment, with what to send";
  return (
    <>
      {outOfDate && slug && (
        <div style={{ color: "#e0c36f" }} title="deploy to run what the canvas has now">
          runs the deployed version: the canvas has changes not deployed
        </div>
      )}
      {names.map((name) => {
        const r = ran[name];
        return (
          <div key={name} aria-label={`action ${name}`} style={{ display: "flex", flexDirection: "column", gap: 3 }}>
            <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
              <span style={{ color: "#6fd3e0", width: 52, flexShrink: 0 }}>action</span>
              <span style={{ flex: 1, color: "#e6eefb" }}>{name}</span>
              <button
                style={{ ...button, opacity: slug && !viewer ? 1 : 0.4 }}
                disabled={!slug || viewer}
                title={why}
                onClick={() => run(name)}
              >
                run
              </button>
            </div>
            {viewer ? (
              <div style={{ color: "#7e8ea8" }}>an editor or an admin runs this</div>
            ) : (
              <textarea
                value={sent[name] ?? ""}
                onChange={(e) => setSent((m) => ({ ...m, [name]: e.target.value }))}
                placeholder="what to send, as JSON (empty is {})"
                aria-label={`what to send ${name}`}
                rows={1}
                style={{ ...field, resize: "vertical" }}
              />
            )}
            {r && (
              <div style={{ color: r.ok ? "#9ad29f" : "#e0916f", whiteSpace: "pre-wrap", wordBreak: "break-word" }}>
                {name}: {r.text}
                {r.seq !== undefined && onViewRun && (
                  <>
                    {" "}
                    <button style={link} onClick={() => onViewRun(r.seq!)} title="open view mode on this run">
                      view run
                    </button>
                  </>
                )}
              </div>
            )}
            {slug && !viewer && (
              <code style={{ color: "#6f86a8", fontSize: 10, wordBreak: "break-all" }} title="the same run from a terminal">
                curl -X POST {actionUrl(slug, name)} -H "Authorization: Bearer $HOLON_TOKEN" -d '{(sent[name] ?? "").trim() || "{}"}'
              </code>
            )}
          </div>
        );
      })}
    </>
  );
}
