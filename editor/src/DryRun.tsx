// A dry run's result, before anything changes: what an action on a deployment's history (a
// restore, a secret rolled back) would do, as the server's dry run says it, and the approval it
// asks for. Nothing happens until it is approved; cancelling leaves everything as it was. Its diff
// shows each change beside what is there now: the flow's text, the grant, the state's values, a
// secret's version and fingerprint.

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { collapse, type DiffLine } from "./diff";

/** One part of a dry run's diff: `lines` from what is there now ("-") to what it would be ("+"),
 *  `legend` saying which is which; `whole` shows every line (a grant's few items), else unchanged
 *  runs far from a change are left out. `note`: why there is no diff, or what it leaves out. */
export type Diff = { title: string; legend: string; lines: DiffLine[]; whole?: boolean; note?: string };

/** What a dry run found: a question, what it changes (a line each), what each part would do, and
 *  the button that does it. `diffs` works out the diff when it is asked for. */
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
  const ok = useRef<HTMLButtonElement>(null);
  const [diff, setDiff] = useState<{ open: boolean; diffs?: Diff[]; error?: string }>({ open: false });
  useEffect(() => {
    ok.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  const toggleDiff = () => {
    const open = !diff.open;
    setDiff((d) => ({ ...d, open }));
    if (open && !diff.diffs && report.diffs) {
      report.diffs().then(
        (diffs) => setDiff((d) => ({ ...d, diffs, error: undefined })),
        (e) => setDiff((d) => ({ ...d, error: e instanceof Error ? e.message : String(e) })),
      );
    }
  };
  const button = { font: "inherit", fontSize: 12, padding: "4px 12px", borderRadius: 3, cursor: "pointer" } as const;
  const approvals = report.approvals ?? [];
  const changes = report.changes ?? [];
  return (
    <div
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
          width: diff.open ? 920 : 560,
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
          {diff.open && (
            <div className="hl-dryrun__diff" style={{ display: "flex", flexDirection: "column", gap: 10 }}>
              {diff.error ? (
                <span style={{ color: "var(--bad)" }}>The diff can't be worked out: {diff.error}</span>
              ) : !diff.diffs ? (
                <span style={{ color: "var(--faint)" }}>Working out the diff…</span>
              ) : diff.diffs.length === 0 ? (
                <span style={{ color: "var(--faint)" }}>Nothing to compare.</span>
              ) : (
                diff.diffs.map((d) => <DiffView key={d.title} diff={d} />)
              )}
            </div>
          )}
        </div>
        <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
          {report.diffs && (
            <button
              onClick={toggleDiff}
              aria-pressed={diff.open}
              title={diff.open ? "hide the diff" : "compare what it would change with what is there now"}
              style={{ ...button, color: diff.open ? "var(--accent-ink)" : "var(--accent)", background: diff.open ? "var(--accent)" : "transparent", border: "1px solid var(--accent)" }}
            >
              diff
            </button>
          )}
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

const lineStyle: Record<string, { color: string; background?: string }> = {
  "+": { color: "var(--ok)", background: "var(--add-bg)" },
  "-": { color: "var(--bad)", background: "var(--del-bg)" },
  " ": { color: "var(--body)" },
  "…": { color: "var(--faintest)" },
};

function DiffView({ diff }: { diff: Diff }) {
  const lines = diff.whole ? diff.lines : collapse(diff.lines);
  return (
    <div className="hl-dryrun__diffpart" data-title={diff.title} style={{ display: "flex", flexDirection: "column", gap: 4 }}>
      <div style={{ display: "flex", gap: 10, alignItems: "baseline" }}>
        <span style={heading}>{diff.title}</span>
        <span style={{ color: "var(--faint)", fontSize: 11 }}>{diff.legend}</span>
      </div>
      {diff.note && <span style={{ color: "var(--faint)", fontSize: 12 }}>{diff.note}</span>}
      {lines.length > 0 && (
        <pre style={{ margin: 0, padding: "4px 0", background: "var(--bg)", border: "1px solid var(--border)", borderRadius: 3, fontSize: 12, lineHeight: 1.45, overflowX: "auto" }}>
          {lines.map((l, i) => (
            <div key={i} className="hl-dryrun__line" data-sign={l.sign} style={{ ...lineStyle[l.sign], padding: "0 8px", whiteSpace: "pre", fontStyle: l.sign === "…" ? "italic" : undefined }}>
              {l.sign === "…" ? `  ⋯ ${l.text}` : `${l.sign} ${l.text}`}
            </div>
          ))}
        </pre>
      )}
    </div>
  );
}
