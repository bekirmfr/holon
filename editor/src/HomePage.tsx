// The home tab: panes of the person's choosing, in two columns, kept with their account
// (home.ts). "customize" lets them add panes (start, recent flows, deployments, activity,
// marketplace, samples, notes, getting started), move them, make one span both columns, and
// remove them; a notes pane is edited in place.

import { useEffect, useState, type CSSProperties, type ReactNode } from "react";
import { addPane, changePane, movePane, PANE_INFO, PANE_KINDS, removePane, type Pane, type PaneKind } from "./home";
import { getWarnings, listFlows, type DeploymentEntry, type FlowListEntry, type HostWarning } from "./server";
import { ActivityList } from "./Pages";
import type { Sample } from "./samples";
import type { Listing } from "./marketplace";

const small: CSSProperties = { font: "inherit", fontSize: 11, padding: "2px 8px", color: "var(--muted)", background: "var(--surface-2)", border: "1px solid var(--border)", borderRadius: 3, cursor: "pointer" };
const primary: CSSProperties = { ...small, color: "var(--accent-ink)", background: "var(--accent)", border: "none" };
const link: CSSProperties = { font: "inherit", fontSize: 12, color: "var(--accent)", background: "transparent", border: "none", padding: 0, cursor: "pointer", textAlign: "left" };
const dim: CSSProperties = { color: "var(--faint)", fontSize: 11 };

export type HomeActions = {
  newFlow: () => void;
  openFlow: (name: string) => void;
  openSample: (sample: Sample) => void;
  viewDeployment: (slug: string) => void;
  goTo: (page: "flows" | "library") => void;
  openWelcome: () => void;
};

export function HomePage({
  panes,
  onPanes,
  deployments,
  samples,
  market,
  actions,
  admin = false,
}: {
  panes: Pane[];
  onPanes: (panes: Pane[]) => void;
  deployments: DeploymentEntry[];
  samples: Sample[];
  market: { url: string; entries: { listing: Listing; adoption: number }[] };
  actions: HomeActions;
  /** An admin sees what the host warns of (storage.rs), first. */
  admin?: boolean;
}) {
  const [customizing, setCustomizing] = useState(false);
  const [warnings, setWarnings] = useState<HostWarning[]>([]);
  useEffect(() => {
    if (!admin) return;
    getWarnings()
      .then(setWarnings)
      .catch(() => setWarnings([]));
  }, [admin, deployments]);
  const [flows, setFlows] = useState<FlowListEntry[]>([]);
  useEffect(() => {
    listFlows()
      .then((f) => setFlows([...f].sort((a, b) => b.saved_at_epoch_seconds - a.saved_at_epoch_seconds)))
      .catch(() => setFlows([]));
  }, []);

  const body = (p: Pane): ReactNode => {
    switch (p.kind) {
      case "start":
        return (
          <div style={{ display: "grid", gap: 6 }}>
            <button style={link} onClick={actions.newFlow}>
              ＋ new flow
            </button>
            <button style={link} onClick={() => actions.goTo("flows")}>
              your flows, saved and deployed
            </button>
            <button style={link} onClick={() => actions.goTo("library")}>
              start from a sample
            </button>
            <button style={link} onClick={actions.openWelcome}>
              the welcome: how to start, and the docs
            </button>
          </div>
        );
      case "recent":
        return flows.length === 0 ? (
          <span style={dim}>nothing saved yet</span>
        ) : (
          <div style={{ display: "grid", gap: 4 }}>
            {flows.slice(0, 8).map((f) => (
              <span key={f.name} style={{ display: "flex", gap: 8 }}>
                <button style={link} onClick={() => actions.openFlow(f.name)}>
                  {f.name}
                </button>
                <span style={dim}>
                  {new Date(f.saved_at_epoch_seconds * 1000).toLocaleString()}
                  {f.saved_by ? ` · ${f.saved_by}` : ""}
                </span>
              </span>
            ))}
          </div>
        );
      case "deployments":
        return deployments.length === 0 ? (
          <span style={dim}>nothing deployed yet</span>
        ) : (
          <div style={{ display: "grid", gap: 4 }}>
            {deployments.map((d) => (
              <span key={d.slug} style={{ display: "flex", gap: 8, alignItems: "baseline" }}>
                <button style={link} onClick={() => actions.viewDeployment(d.slug)} title="view it, with its runs">
                  {d.slug}
                </button>
                <span style={{ color: d.status === "running" ? "var(--ok)" : "var(--warn)", fontSize: 11 }}>{d.status}</span>
                <span style={dim}>
                  {d.port ? `:${d.port} · ` : ""}
                  {d.trigger_count} runs{d.error_count ? `, ${d.error_count} failed` : ""}
                  {d.attention ? ` · ${d.attention}` : ""}
                </span>
              </span>
            ))}
          </div>
        );
      case "activity":
        return <ActivityList bare version={deployments.map((d) => `${d.slug}:${d.status}`).join(",")} />;
      case "marketplace":
        return !market.url ? (
          <span style={dim}>no marketplace set (the flows page)</span>
        ) : market.entries.length === 0 ? (
          <span style={dim}>nothing published yet</span>
        ) : (
          <div style={{ display: "grid", gap: 4 }}>
            {market.entries.slice(0, 8).map(({ listing, adoption }) => (
              <span key={listing.id} style={{ display: "flex", gap: 8 }}>
                <span style={{ color: "var(--body)" }}>{listing.name}</span>
                <span style={dim}>
                  {listing.description}
                  {adoption ? ` · ↑${adoption}` : ""}
                </span>
              </span>
            ))}
            <span style={dim}>place them from the palette's marketplace</span>
          </div>
        );
      case "samples":
        return (
          <div style={{ display: "grid", gap: 4 }}>
            {samples.map((s) => (
              <button key={s.name} style={link} onClick={() => actions.openSample(s)} title={s.description}>
                {s.title}
              </button>
            ))}
          </div>
        );
      case "notes":
        return (
          <textarea
            aria-label="notes"
            defaultValue={p.note ?? ""}
            placeholder="anything you want to keep here"
            onBlur={(e) => e.target.value !== (p.note ?? "") && onPanes(changePane(panes, p.id, { note: e.target.value }))}
            style={{ font: "inherit", fontSize: 12, width: "100%", minHeight: 90, color: "var(--ink)", background: "var(--bg)", border: "1px solid var(--border)", borderRadius: 3, padding: 6, resize: "vertical", boxSizing: "border-box" }}
          />
        );
      case "guide": {
        const steps: [string, boolean][] = [
          ["save a flow", flows.length > 0],
          ["deploy one", deployments.length > 0],
          ["send it a request", deployments.some((d) => d.trigger_count > 0)],
          ["set a marketplace", !!market.url],
        ];
        return (
          <div style={{ display: "grid", gap: 4 }}>
            {steps.map(([what, done]) => (
              <span key={what} style={{ color: done ? "var(--ok)" : "var(--muted)" }}>
                {done ? "✓" : "○"} {what}
              </span>
            ))}
            <button style={link} onClick={actions.openWelcome}>
              how, step by step
            </button>
          </div>
        );
      }
    }
  };

  return (
    <div style={{ maxWidth: 980 }}>
      {warnings.length > 0 && (
        <div role="alert" aria-label="what the host warns of" style={{ marginBottom: 12, padding: "8px 10px", border: "1px solid var(--bad)", borderRadius: 4, background: "var(--bad-bg)", fontSize: 12, display: "grid", gap: 4 }}>
          {warnings.map((w) => (
            <div key={w.key}>
              <span style={{ color: "var(--bad-ink)" }}>⚠ {w.says}</span>
              <div style={{ color: "var(--bad-ink)", fontSize: 11 }}>{w.todo}</div>
            </div>
          ))}
        </div>
      )}
      <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 12 }}>
        <span style={{ color: "var(--body)", fontSize: 14, letterSpacing: "0.04em", flex: 1 }}>home</span>
        {customizing && (
          <select
            aria-label="add a pane"
            value=""
            onChange={(e) => e.target.value && onPanes(addPane(panes, e.target.value as PaneKind))}
            style={{ ...small, padding: "3px 6px" }}
          >
            <option value="">add a pane…</option>
            {PANE_KINDS.map((k) => (
              <option key={k} value={k}>
                {PANE_INFO[k].title}: {PANE_INFO[k].about}
              </option>
            ))}
          </select>
        )}
        <button style={customizing ? primary : small} onClick={() => setCustomizing(!customizing)}>
          {customizing ? "done" : "customize"}
        </button>
      </div>
      {panes.length === 0 && <div style={dim}>no panes: customize to add some</div>}
      <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(360px, 1fr))", gap: 12 }}>
        {panes.map((p, i) => (
          <section
            key={p.id}
            aria-label={PANE_INFO[p.kind].title}
            style={{ gridColumn: p.wide ? "1 / -1" : undefined, background: "var(--surface-2)", border: "1px solid var(--border)", borderRadius: 6, padding: 12, minWidth: 0 }}
          >
            <div style={{ display: "flex", alignItems: "center", gap: 6, marginBottom: 8 }}>
              <span style={{ color: "var(--body)", fontSize: 12, letterSpacing: "0.04em", flex: 1 }}>{PANE_INFO[p.kind].title}</span>
              {customizing && (
                <>
                  <button style={small} disabled={i === 0} onClick={() => onPanes(movePane(panes, p.id, -1))} aria-label={`move ${PANE_INFO[p.kind].title} earlier`}>
                    ↑
                  </button>
                  <button style={small} disabled={i === panes.length - 1} onClick={() => onPanes(movePane(panes, p.id, 1))} aria-label={`move ${PANE_INFO[p.kind].title} later`}>
                    ↓
                  </button>
                  <button style={small} onClick={() => onPanes(changePane(panes, p.id, { wide: !p.wide }))} title={p.wide ? "one column" : "both columns"}>
                    {p.wide ? "narrow" : "wide"}
                  </button>
                  <button style={small} onClick={() => onPanes(removePane(panes, p.id))} aria-label={`remove ${PANE_INFO[p.kind].title}`}>
                    ×
                  </button>
                </>
              )}
            </div>
            <div style={{ fontSize: 12, color: "var(--muted)" }}>{body(p)}</div>
          </section>
        ))}
      </div>
    </div>
  );
}
