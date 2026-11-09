// A cell's history, from view mode's flow state (ViewPanes.tsx): the ⧗ beside a cell or an item
// opens a window listing the runs that changed it, newest first, each with its value before and
// after (cellChanges.ts), and a restore or a deploy between runs that moved it. A run's row opens
// that run in view mode.

import { useEffect, useState, type CSSProperties } from "react";
import { createPortal } from "react-dom";
import { cellName, cellRows, shown, type CellRow } from "./cellChanges";
import { getChanges, getVersions, type CellHistory, type VersionEntry } from "./server";

const dim = "var(--faint)";
const PAGE = 50;

/** The ⧗ button for `name` (a cell, or a collection with `item` one of its items). */
export function CellHistoryButton({ slug, name, item, collection, onOpenRun }: { slug: string; name: string; item?: string; collection?: boolean; onOpenRun?: (seq: number) => void }) {
  const [open, setOpen] = useState(false);
  const label = `history of ${cellName({ name, item: item ?? null, collection: collection ?? item !== undefined })}`;
  return (
    <>
      <button
        className="hl-cellhistory__open"
        onClick={(e) => {
          e.stopPropagation();
          setOpen(true);
        }}
        title={`${label}: the runs that changed it`}
        aria-label={label}
        style={{ background: "transparent", border: "1px solid var(--border)", borderRadius: 3, color: "var(--muted)", cursor: "pointer", font: "inherit", fontSize: 10, lineHeight: 1.4, padding: "0 4px", flexShrink: 0 }}
      >
        ⧗
      </button>
      {open && createPortal(<CellHistoryModal slug={slug} name={name} item={item} onOpenRun={onOpenRun} onClose={() => setOpen(false)} />, document.body)}
    </>
  );
}

function CellHistoryModal({ slug, name, item, onOpenRun, onClose }: { slug: string; name: string; item?: string; onOpenRun?: (seq: number) => void; onClose: () => void }) {
  const [history, setHistory] = useState<CellHistory | null>(null);
  const [versions, setVersions] = useState<VersionEntry[]>([]);
  const [error, setError] = useState("");
  const [loadingOlder, setLoadingOlder] = useState(false);
  useEffect(() => {
    getChanges(slug, name, { item, limit: PAGE }).then(setHistory, (e: unknown) => setError(e instanceof Error ? e.message : String(e)));
    getVersions(slug).then(setVersions, () => setVersions([]));
  }, [slug, name, item]);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  // The changes past the oldest shown: before its run (or the run after it, for one between).
  const older = () => {
    const last = history?.changes[history.changes.length - 1];
    const before = last ? (last.seq ?? last.between?.[1]) : undefined;
    if (!history || before === undefined) return;
    setLoadingOlder(true);
    getChanges(slug, name, { item, before, limit: PAGE })
      .then((h) => setHistory({ ...h, changes: [...history.changes, ...h.changes] }))
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoadingOlder(false));
  };
  const rows = history ? cellRows(history, versions) : [];
  const title = history ? cellName(history) : cellName({ name, item: item ?? null, collection: item !== undefined });
  const cell: CSSProperties = { padding: "5px 8px", fontSize: 12, borderBottom: "1px solid var(--surface)", verticalAlign: "top" };
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="cell history"
      className="hl-cellhistory"
      onMouseDown={(e) => e.target === e.currentTarget && onClose()}
      style={{ position: "fixed", inset: 0, background: "var(--overlay)", display: "flex", alignItems: "center", justifyContent: "center", zIndex: 1100 }}
    >
      <div style={{ background: "var(--surface-2)", border: "1px solid var(--border)", borderRadius: 6, padding: 16, width: 760, maxWidth: "calc(100vw - 32px)", color: "var(--ink)", fontSize: 13, display: "flex", flexDirection: "column", gap: 10 }}>
        <div style={{ display: "flex", gap: 8, alignItems: "baseline" }}>
          <span style={{ fontWeight: 600, fontFamily: "var(--mono)" }}>{title}</span>
          <span style={{ color: dim, flex: 1 }}>
            {history && !(history.collection && history.item === null) ? `now ${shown(history.now)}` : ""}
          </span>
          <button onClick={onClose} style={{ background: "transparent", border: "none", color: dim, cursor: "pointer", fontSize: 16, padding: "0 4px", lineHeight: 1 }} title="close" aria-label="close">
            ×
          </button>
        </div>
        <div style={{ color: dim }}>The runs that changed it, newest first, each with its value before and after it; a restore or a deploy between runs that moved it has a row of its own.</div>
        <div style={{ maxHeight: "60vh", overflowY: "auto" }}>
          {error ? (
            <span style={{ color: "var(--bad)" }}>{error}</span>
          ) : !history ? (
            <span style={{ color: dim }}>reading the log…</span>
          ) : rows.length === 0 ? (
            <span className="hl-cellhistory__empty" style={{ color: dim }}>
              No run in the log changed it.
            </span>
          ) : (
            <table style={{ width: "100%", borderCollapse: "collapse" }}>
              <tbody>
                {rows.map((r) => (
                  <Row key={r.key} row={r} cell={cell} onOpen={r.seq !== undefined && onOpenRun ? () => (onOpenRun(r.seq!), onClose()) : undefined} />
                ))}
              </tbody>
            </table>
          )}
          {history?.more && (
            <button onClick={older} disabled={loadingOlder} style={{ marginTop: 8, background: "transparent", border: "1px solid var(--border)", borderRadius: 3, color: "var(--muted)", cursor: "pointer", font: "inherit", fontSize: 12, padding: "2px 10px" }}>
              {loadingOlder ? "reading…" : "older changes"}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

function Row({ row, cell, onOpen }: { row: CellRow; cell: CSSProperties; onOpen?: () => void }) {
  const between = row.seq === undefined;
  return (
    <tr className="hl-cellhistory__row" data-seq={row.seq ?? ""} style={{ background: between ? "var(--surface)" : undefined }}>
      <td style={{ ...cell, whiteSpace: "nowrap", color: between ? "var(--warn)" : "var(--ink)" }}>
        {onOpen ? (
          <button onClick={onOpen} title={`open ${row.label} in view mode`} style={{ background: "transparent", border: "none", color: "var(--muted)", cursor: "pointer", font: "inherit", padding: 0, textDecoration: "underline" }}>
            {row.label}
          </button>
        ) : (
          row.label
        )}
        {row.failed ? <span style={{ color: "var(--bad)" }}> · failed</span> : null}
        <div style={{ color: dim, fontSize: 11 }}>{row.at_ms ? new Date(row.at_ms).toLocaleString() : ""}</div>
      </td>
      <td style={{ ...cell, color: dim }}>{row.what}</td>
      <td className="hl-cellhistory__values" style={{ ...cell, fontFamily: "var(--mono)", whiteSpace: "pre-line" }}>
        {row.values.join("\n")}
      </td>
    </tr>
  );
}
