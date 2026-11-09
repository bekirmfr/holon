// Getting started (`#start`): how to start, step by step, each ticked off once done (guide.ts),
// the person's own notes, and the docs, one topic each. It opens on start until "show this on
// start" is unticked (kept with the account, home.ts, as the notes are); the account menu opens it
// again.

import { useState, type CSSProperties } from "react";
import { START_STEPS, TOPICS, type Done, type Step } from "./guide";

const small: CSSProperties = { font: "inherit", fontSize: 11, padding: "2px 10px", color: "var(--accent-ink)", background: "var(--accent)", border: "none", borderRadius: 3, cursor: "pointer" };
const head: CSSProperties = { color: "var(--body)", fontSize: 14, letterSpacing: "0.04em", margin: "22px 0 8px" };

export function WelcomePage({
  name,
  showOnStart,
  onShowOnStart,
  onAction,
  onManual,
  done,
  notes,
  onNotes,
}: {
  /** Who is signed in. */
  name: string;
  showOnStart: boolean;
  onShowOnStart: (show: boolean) => void;
  /** A step's button: open the samples, start a new flow, the flows page. */
  onAction: (action: NonNullable<Step["action"]>) => void;
  /** Opens the manual, in the docs tab. */
  onManual: () => void;
  /** What is done already, for the steps' ticks. */
  done: Done;
  notes: string;
  onNotes: (notes: string) => void;
}) {
  const [open, setOpen] = useState<string | null>(TOPICS[0].title);
  const label = { samples: "open the samples", new: "new flow", flows: "the flows page" };
  return (
    <div style={{ maxWidth: 760, color: "var(--muted)", fontSize: 12, lineHeight: 1.55 }}>
      <div style={{ color: "var(--ink)", fontSize: 18, marginBottom: 4 }}>Welcome to Holon, {name}</div>
      <div>
        Build services by wiring small, pure parts together, deploy them with exactly the permissions they need, and see every run as it
        happened.
      </div>

      <div style={head}>how to start</div>
      <ol style={{ margin: 0, paddingLeft: 20, display: "grid", gap: 10 }}>
        {START_STEPS.map((s) => (
          <li key={s.title} aria-label={`${s.title}${s.doneWhen && done[s.doneWhen] ? ", done" : ""}`}>
            {s.doneWhen && done[s.doneWhen] && <span style={{ color: "var(--ok)" }}>✓ </span>}
            <span style={{ color: "var(--ink)" }}>{s.title}.</span> {s.body}{" "}
            {s.action && (
              <button style={small} onClick={() => onAction(s.action!)}>
                {label[s.action]}
              </button>
            )}
          </li>
        ))}
      </ol>

      <div style={head}>your notes</div>
      <textarea
        aria-label="notes"
        defaultValue={notes}
        placeholder="anything you want to keep here: kept with your account"
        onBlur={(e) => e.target.value !== notes && onNotes(e.target.value)}
        style={{ font: "inherit", fontSize: 12, width: "100%", minHeight: 90, color: "var(--ink)", background: "var(--bg)", border: "1px solid var(--border)", borderRadius: 3, padding: 6, resize: "vertical", boxSizing: "border-box" }}
      />

      <div style={head}>docs</div>
      <div style={{ marginBottom: 8 }}>
        The short version, one topic each; the whole of it is in the manual.{" "}
        <button style={small} onClick={onManual}>
          the manual
        </button>
      </div>
      {TOPICS.map((t) => (
        <section key={t.title} style={{ borderTop: "1px solid var(--border)" }}>
          <button
            aria-expanded={open === t.title}
            onClick={() => setOpen(open === t.title ? null : t.title)}
            style={{ font: "inherit", fontSize: 13, color: "var(--body)", background: "transparent", border: "none", padding: "8px 0", cursor: "pointer", width: "100%", textAlign: "left" }}
          >
            {open === t.title ? "▾" : "▸"} {t.title}
          </button>
          {open === t.title && t.paragraphs.map((p, i) => <p key={i} style={{ margin: "0 0 8px" }}>{p}</p>)}
        </section>
      ))}

      <label style={{ display: "flex", gap: 8, alignItems: "center", marginTop: 22, color: "var(--body)" }}>
        <input type="checkbox" checked={showOnStart} onChange={(e) => onShowOnStart(e.target.checked)} />
        show this on start
      </label>
      <div style={{ color: "var(--faint)", fontSize: 11, marginTop: 4 }}>Kept with your account. It is always under your name in the header.</div>
    </div>
  );
}
