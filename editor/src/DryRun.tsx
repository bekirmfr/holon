// A dry run's result, before anything changes: what an action on a deployment's history (a
// restore, a secret rolled back) would do, as the server's dry run says it, and the approval it
// asks for. Nothing happens until it is approved; cancelling leaves everything as it was. Its diff
// shows each change beside what is there now, as a review in hunks (Review.tsx): the flow's text,
// the grant, the state's values, a file, a secret's version and fingerprint.

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import type { DiffLine } from "./diff";
import { diffHunk } from "./hunks";
import { Review } from "./Review";
import { useFocusTrap } from "./ui/focusTrap";

/** One part of a dry run's diff: `lines` from what is there now ("-") to what it would be ("+"),
 *  `legend` saying which is which; `whole` shows every line (a grant's few items), else unchanged
 *  runs far from a change are left out. `note`: why there is no diff, or what it leaves out. */
export type Diff = { title: string; legend: string; lines: DiffLine[]; whole?: boolean; note?: string };

/** What a dry run found: a question, what it changes (a line each), what each part would do, and
 *  the button that does it. `diffs` works out the diff, shown as soon as it is there. */
export type DryRunReport = {
  title: string;
  changes?: string[];
  sections: { heading: string; lines: string[] }[];
  /** What it needs approved (a restored version's permissions), in words. */
  approvals?: string[];
  action: string;
  diffs?: () => Promise<Diff[]>;
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

const heading = { color: "var(--accent)", fontSize: 11, textTransform: "uppercase", letterSpacing: "0.04em" } as const;

function DryRunModal({ report, onClose }: { report: DryRunReport; onClose: (ok: boolean) => void }) {
  const box = useRef<HTMLDivElement>(null);
  const ok = useRef<HTMLButtonElement>(null);
  useFocusTrap(box, true, () => ok.current);
  const [diff, setDiff] = useState<{ diffs?: Diff[]; error?: string }>({});
  useEffect(() => {
    let live = true;
    report.diffs?.().then(
      (diffs) => live && setDiff({ diffs }),
      (e) => live && setDiff({ error: e instanceof Error ? e.message : String(e) }),
    );
    return () => {
      live = false;
    };
  }, [report]);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  const button = { font: "inherit", fontSize: 12, padding: "4px 12px", borderRadius: 3, cursor: "pointer" } as const;
  const approvals = report.approvals ?? [];
  const changes = report.changes ?? [];
  return (
    <div
      ref={box}
      role="dialog"
      aria-modal="true"
      aria-label="dry run"
      className="hl-dryrun"
      onMouseDown={(e) => e.target === e.currentTarget && onClose(false)}
      style={{ position: "fixed", inset: 0, background: "var(--overlay)", display: "flex", alignItems: "center", justifyContent: "center", zIndex: 1100 }}
    >
      <div
        style={{
          background: "var(--surface-2)",
          border: "1px solid var(--border)",
          borderRadius: 6,
          padding: 16,
          width: report.diffs ? 920 : 560,
          maxHeight: "calc(100vh - 32px)",
          maxWidth: "calc(100vw - 32px)",
          color: "var(--ink)",
          fontSize: 13,
          display: "flex",
          flexDirection: "column",
          gap: 10,
        }}
      >
        <div style={{ display: "flex", gap: 8, alignItems: "baseline" }}>
          <span style={{ color: "var(--accent-ink)", background: "var(--warn)", borderRadius: 3, padding: "0 6px", fontSize: 11, flexShrink: 0 }}>dry run</span>
          <span style={{ fontWeight: 600 }}>{report.title}</span>
        </div>
        <div style={{ color: "var(--faint)" }}>Nothing has changed yet: this is what it would do.</div>
        <div style={{ display: "flex", flexDirection: "column", gap: 10, maxHeight: "65vh", overflowY: "auto" }}>
          {changes.length > 0 && (
            <div className="hl-dryrun__changes" style={{ display: "flex", flexDirection: "column", gap: 3, background: "var(--surface)", borderRadius: 4, padding: "8px 10px" }}>
              <span style={heading}>what it changes</span>
              {changes.map((c, i) => (
                <span key={i} style={{ fontWeight: 500 }}>
                  • {c}
                </span>
              ))}
            </div>
          )}
          {report.sections.map((s) => (
            <div key={s.heading} className="hl-dryrun__section" style={{ display: "flex", flexDirection: "column", gap: 3 }}>
              <span style={heading}>{s.heading}</span>
              {s.lines.map((line, i) => (
                <span key={i} style={{ whiteSpace: "pre-line" }}>
                  {line}
                </span>
              ))}
            </div>
          ))}
          {approvals.length > 0 && (
            <div className="hl-dryrun__approvals" style={{ display: "flex", flexDirection: "column", gap: 3, borderLeft: "2px solid var(--warn)", paddingLeft: 8 }}>
              <span style={{ color: "var(--warn)" }}>It needs your approval to:</span>
              {approvals.map((a) => (
                <span key={a}>• {a}</span>
              ))}
            </div>
          )}
          {report.diffs && (
            <div className="hl-dryrun__diff">
              {diff.error ? (
                <span style={{ color: "var(--bad)" }}>The diff can't be worked out: {diff.error}</span>
              ) : !diff.diffs ? (
                <span style={{ color: "var(--faint)" }}>Working out the diff…</span>
              ) : diff.diffs.length === 0 ? (
                <span style={{ color: "var(--faint)" }}>Nothing to compare.</span>
              ) : (
                <Review hunks={diff.diffs.map(diffHunk)} label="what it changes, beside what is there now" />
              )}
            </div>
          )}
        </div>
        <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
          <span style={{ flex: 1 }} />
          <button onClick={() => onClose(false)} style={{ ...button, color: "var(--muted)", background: "transparent", border: "1px solid var(--border)" }}>
            Cancel
          </button>
          <button ref={ok} onClick={() => onClose(true)} style={{ ...button, color: "var(--accent-ink)", background: approvals.length > 0 ? "var(--warn)" : "var(--accent)", border: "none" }}>
            {report.action}
          </button>
        </div>
      </div>
    </div>
  );
}
