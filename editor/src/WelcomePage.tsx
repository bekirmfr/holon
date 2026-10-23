// The welcome tab: how to start, step by step, and the docs, one topic each (guide.ts). It opens
// on start until "show this on start" is unticked (kept with the account, home.ts); the account
// menu opens it again.

import { useState, type CSSProperties } from "react";
import { START_STEPS, TOPICS, type Step } from "./guide";

const small: CSSProperties = { font: "inherit", fontSize: 11, padding: "2px 10px", color: "#0e1a2f", background: "#6fd3e0", border: "none", borderRadius: 3, cursor: "pointer" };
const head: CSSProperties = { color: "#cfe3ff", fontSize: 14, letterSpacing: "0.04em", margin: "22px 0 8px" };

export function WelcomePage({
  name,
  showOnStart,
  onShowOnStart,
  onAction,
}: {
  /** Who is signed in. */
  name: string;
  showOnStart: boolean;
  onShowOnStart: (show: boolean) => void;
  /** A step's button: open the samples, start a new flow, the deployed page, the flows page. */
  onAction: (action: NonNullable<Step["action"]>) => void;
}) {
  const [open, setOpen] = useState<string | null>(TOPICS[0].title);
  const label = { samples: "open the samples", new: "new flow", deployed: "the deployed page", flows: "the flows page" };
  return (
    <div style={{ maxWidth: 760, color: "#9fb4d4", fontSize: 12, lineHeight: 1.55 }}>
      <div style={{ color: "#e6eefb", fontSize: 18, marginBottom: 4 }}>Welcome to Holon, {name}</div>
      <div>
        Build services by wiring small, pure parts together, deploy them with exactly the permissions they need, and see every run as it
        happened.
      </div>

      <div style={head}>how to start</div>
      <ol style={{ margin: 0, paddingLeft: 20, display: "grid", gap: 10 }}>
        {START_STEPS.map((s) => (
          <li key={s.title}>
            <span style={{ color: "#e6eefb" }}>{s.title}.</span> {s.body}{" "}
            {s.action && (
              <button style={small} onClick={() => onAction(s.action!)}>
                {label[s.action]}
              </button>
            )}
          </li>
        ))}
      </ol>

      <div style={head}>docs</div>
      {TOPICS.map((t) => (
        <section key={t.title} style={{ borderTop: "1px solid #1b2a45" }}>
          <button
            aria-expanded={open === t.title}
            onClick={() => setOpen(open === t.title ? null : t.title)}
            style={{ font: "inherit", fontSize: 13, color: "#cfe3ff", background: "transparent", border: "none", padding: "8px 0", cursor: "pointer", width: "100%", textAlign: "left" }}
          >
            {open === t.title ? "▾" : "▸"} {t.title}
          </button>
          {open === t.title && t.paragraphs.map((p, i) => <p key={i} style={{ margin: "0 0 8px" }}>{p}</p>)}
        </section>
      ))}

      <label style={{ display: "flex", gap: 8, alignItems: "center", marginTop: 22, color: "#cfe3ff" }}>
        <input type="checkbox" checked={showOnStart} onChange={(e) => onShowOnStart(e.target.checked)} />
        show this on start
      </label>
      <div style={{ color: "#7e8ea8", fontSize: 11, marginTop: 4 }}>Kept with your account. It is always under your name in the header.</div>
    </div>
  );
}
