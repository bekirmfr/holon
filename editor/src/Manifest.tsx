// A flow's manifest, as the flows and deployed pages show it: what it needs, each item in words
// (permissions.rs's names, described by diagnostics.ts), behind a ▸ that loads it when opened.
// On a deployment, items its grant doesn't cover are marked; permissions (what was granted, and
// the host's controls over it) are DeploymentCard's own section, shown only there.

import { useEffect, useState } from "react";
import { describeItem } from "./diagnostics";

/** What a manifest view shows: its items, and on a deployment the ones not granted. */
export type ManifestView = { items: string[]; notGranted?: string[]; hash?: string };

export function ManifestSection({ load, version = "" }: { load: () => Promise<ManifestView>; version?: string }) {
  const [open, setOpen] = useState(false);
  const [view, setView] = useState<ManifestView | null>(null);
  const [msg, setMsg] = useState("");
  useEffect(() => {
    if (!open) return;
    setMsg("");
    load()
      .then(setView)
      .catch((e: unknown) => setMsg(e instanceof Error ? e.message : String(e)));
    // `load` is a fresh closure each render; `version` says when what it loads may have changed.
  }, [open, version]);
  const row = { display: "flex", gap: 8, paddingLeft: 14 } as const;
  const missing = new Set(view?.notGranted ?? []);
  return (
    <div style={{ fontSize: 10 }}>
      <span
        style={{ cursor: "pointer", color: "#9fb4d4" }}
        onClick={() => setOpen(!open)}
        title="what this flow needs: derived from its graph, plus what its deploy settings ask beyond the host's defaults"
      >
        manifest {open ? "▾" : "▸"}
      </span>
      {open && msg && <div style={{ ...row, color: "#e0916f" }}>{msg}</div>}
      {open && view && view.items.length === 0 && <div style={{ ...row, color: "#7e8ea8" }}>needs nothing beyond computing its answer</div>}
      {open &&
        view?.items.map((item) => (
          <div key={item} style={{ ...row, color: missing.has(item) ? "#e0916f" : "#9fb4d4" }} title={item}>
            <span style={{ flex: 1 }}>• {describeItem(item)}</span>
            {missing.has(item) && <span>not granted</span>}
          </div>
        ))}
      {open && view?.hash && (
        <div style={{ ...row, color: "#7e8ea8" }} title={view.hash}>
          manifest {view.hash.slice(0, 12)}…
        </div>
      )}
    </div>
  );
}
