// A dry run's result, before anything changes: what an action on a deployment's history (a
// restore, a secret rolled back) would do, as the server's dry run says it, and the approval it
// asks for. Nothing happens until it is approved; cancelling leaves everything as it was.

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";

/** What a dry run found: a question, what each part would do, and the button that does it. */
export type DryRunReport = {
  title: string;
  sections: { heading: string; lines: string[] }[];
  /** What it needs approved (a restored version's permissions), in words. */
  approvals?: string[];
  action: string;
};

/** Show a dry run's report and wait for the answer: `ask` resolves true only when approved. */
export function useDryRun(): { ask: (report: DryRunReport) => Promise<boolean>; modal: ReactNode } {
  const [shown, setShown] = useState<{ report: DryRunReport; resolve: (ok: boolean) => void } | null>(null);
  const ask = useCallback((report: DryRunReport) => new Promise<boolean>((resolve) => setShown({ report, resolve })), []);
  const close = useCallback(
    (ok: boolean) => {
      shown?.resolve(ok);
      setShown(null);
    },
    [shown],
  );
  // On the page itself, above any window it was asked from (the deploy settings).
  return { ask, modal: shown ? createPortal(<DryRunModal report={shown.report} onClose={close} />, document.body) : null };
}

function DryRunModal({ report, onClose }: { report: DryRunReport; onClose: (ok: boolean) => void }) {
  const ok = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    ok.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  const button = { font: "inherit", fontSize: 12, padding: "4px 12px", borderRadius: 3, cursor: "pointer" } as const;
  const approvals = report.approvals ?? [];
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="dry run"
      className="hl-dryrun"
      onMouseDown={(e) => e.target === e.currentTarget && onClose(false)}
      style={{ position: "fixed", inset: 0, background: "rgba(5, 10, 20, 0.6)", display: "flex", alignItems: "center", justifyContent: "center", zIndex: 1100 }}
    >
      <div
        style={{
          background: "#0e1a2f",
          border: "1px solid #1b2a45",
          borderRadius: 6,
          padding: 16,
          width: 520,
          maxWidth: "calc(100vw - 32px)",
          color: "#e6eefb",
          fontSize: 13,
          display: "flex",
          flexDirection: "column",
          gap: 10,
        }}
      >
        <div style={{ display: "flex", gap: 8, alignItems: "baseline" }}>
          <span style={{ color: "#0e1a2f", background: "#e0c36f", borderRadius: 3, padding: "0 6px", fontSize: 11, flexShrink: 0 }}>dry run</span>
          <span style={{ fontWeight: 600 }}>{report.title}</span>
        </div>
        <div style={{ color: "#7e8ea8" }}>Nothing has changed yet: this is what it would do.</div>
        <div style={{ display: "flex", flexDirection: "column", gap: 10, maxHeight: "55vh", overflowY: "auto" }}>
          {report.sections.map((s) => (
            <div key={s.heading} className="hl-dryrun__section" style={{ display: "flex", flexDirection: "column", gap: 3 }}>
              <span style={{ color: "#6fd3e0", fontSize: 11, textTransform: "uppercase", letterSpacing: "0.04em" }}>{s.heading}</span>
              {s.lines.map((line, i) => (
                <span key={i} style={{ whiteSpace: "pre-line" }}>
                  {line}
                </span>
              ))}
            </div>
          ))}
          {approvals.length > 0 && (
            <div className="hl-dryrun__approvals" style={{ display: "flex", flexDirection: "column", gap: 3, borderLeft: "2px solid #e0c36f", paddingLeft: 8 }}>
              <span style={{ color: "#e0c36f" }}>It needs your approval to:</span>
              {approvals.map((a) => (
                <span key={a}>• {a}</span>
              ))}
            </div>
          )}
        </div>
        <div style={{ display: "flex", justifyContent: "flex-end", gap: 8 }}>
          <button onClick={() => onClose(false)} style={{ ...button, color: "#9fb4d4", background: "transparent", border: "1px solid #1b2a45" }}>
            Cancel
          </button>
          <button ref={ok} onClick={() => onClose(true)} style={{ ...button, color: "#0e1a2f", background: approvals.length > 0 ? "#e0c36f" : "#6fd3e0", border: "none" }}>
            {report.action}
          </button>
        </div>
      </div>
    </div>
  );
}
