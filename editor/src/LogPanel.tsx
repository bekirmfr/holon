// The flow log of a deployed flow, live, for view mode: every run (request, trigger, tick) as it
// happens. Selecting one replays it on the canvas (replay.ts) so every node shows what it
// computed; the step controls walk through that run node by node. "live" follows the newest run.

import { useEffect, useRef, useState, type CSSProperties } from "react";
import { getLog, type LogEntry } from "./server";
import { formatValue, summarizeState } from "./diagnostics";

const KEEP = 500;
const POLL_MS = 1500;

const small: CSSProperties = {
  font: "inherit",
  fontSize: 11,
  color: "#9fb4d4",
  background: "transparent",
  border: "1px solid #1b2a45",
  borderRadius: 3,
  padding: "1px 8px",
  cursor: "pointer",
};

const time = (ms: number) => new Date(ms).toLocaleTimeString();
const label = (e: LogEntry) =>
  e.kind === "request" ? (e.route ?? `${e.method ?? ""} ${e.url ?? ""} (no route)`) : e.kind === "tick" ? "clock tick" : "trigger";

export function LogPanel({
  slug,
  selected,
  onSelect,
  step,
  steps,
  onStep,
  verdict,
}: {
  slug: string;
  selected: LogEntry | null;
  onSelect: (e: LogEntry | null) => void;
  /** The step shown (index into the replay order), or null for the whole run. */
  step: number | null;
  /** How many nodes the selected run went through. */
  steps: number;
  onStep: (step: number | null) => void;
  /** Whether replaying the selected run reproduced what was recorded; null when not checkable. */
  verdict: boolean | null;
}) {
  const [entries, setEntries] = useState<LogEntry[]>([]);
  const [live, setLive] = useState(true);
  const [error, setError] = useState("");
  const last = useRef<number | undefined>(undefined);

  // Poll for new runs: everything at first, then only what came after the newest seen.
  useEffect(() => {
    last.current = undefined;
    setEntries([]);
    let stopped = false;
    const poll = () =>
      getLog(slug, last.current)
        .then(({ entries: fresh }) => {
          if (stopped || fresh.length === 0) return;
          last.current = fresh[fresh.length - 1].seq;
          setEntries((old) => [...old, ...fresh].slice(-KEEP));
          setError("");
        })
        .catch((e: unknown) => !stopped && setError(e instanceof Error ? e.message : String(e)));
    poll();
    const timer = setInterval(poll, POLL_MS);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
  }, [slug]);

  // Live: always show the newest run.
  const newest = entries[entries.length - 1];
  useEffect(() => {
    if (live && newest && newest.seq !== selected?.seq) onSelect(newest);
  }, [live, newest, selected, onSelect]);

  const pick = (e: LogEntry) => {
    setLive(false);
    onSelect(e);
  };

  return (
    <div
      className="nodrag nowheel"
      style={{
        fontSize: 11,
        color: "#9fb4d4",
        background: "#0b1527",
        border: "1px solid #1b2a45",
        borderRadius: 4,
        padding: 8,
        width: 360,
        display: "flex",
        flexDirection: "column",
        gap: 6,
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
        <span style={{ color: "#e6eefb", letterSpacing: "0.04em", flex: 1 }}>flow log · {slug}</span>
        <button
          style={{ ...small, color: live ? "#0e1a2f" : "#9fb4d4", background: live ? "#9ad29f" : "transparent" }}
          onClick={() => setLive(!live)}
          title="follow the newest run as it happens"
        >
          ● live
        </button>
      </div>
      {error && <div style={{ color: "#e0916f" }}>{error}</div>}
      <div style={{ maxHeight: "28vh", overflowY: "auto", display: "flex", flexDirection: "column" }}>
        {entries.length === 0 && <div>no runs yet: send it a request</div>}
        {[...entries].reverse().map((e) => (
          <div
            key={e.seq}
            onClick={() => pick(e)}
            style={{
              display: "flex",
              gap: 8,
              padding: "2px 4px",
              cursor: "pointer",
              borderRadius: 3,
              background: e.seq === selected?.seq ? "#13243f" : "transparent",
            }}
          >
            <span style={{ color: "#7e8ea8", width: 64 }}>{time(e.at_ms)}</span>
            <span style={{ flex: 1, color: "#e6eefb", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
              {label(e)}
            </span>
            {e.status !== null && (
              <span style={{ color: e.status < 400 ? "#9ad29f" : "#e0916f", width: 26 }}>{e.status}</span>
            )}
            <span style={{ color: "#7e8ea8", width: 46, textAlign: "right" }}>{e.duration_ms.toFixed(1)}ms</span>
          </div>
        ))}
      </div>

      {selected && (
        <div style={{ borderTop: "1px solid #1b2a45", paddingTop: 6, display: "flex", flexDirection: "column", gap: 3 }}>
          <div style={{ color: "#e6eefb" }}>
            #{selected.seq} · {label(selected)} · session {selected.session}
            {selected.url ? ` · ${selected.method} ${selected.url}` : ""}
          </div>
          <div>read: {summarizeState(selected.state_before)}</div>
          {selected.performed.length > 0 && (
            <div>
              did:{" "}
              {selected.performed.map((p) => `${p.capability} ← ${formatValue(p.payload).slice(0, 60)}`).join("; ")}
            </div>
          )}
          {selected.response_body !== null && (
            <div style={{ wordBreak: "break-word" }}>
              answered {selected.status}: {selected.response_body.slice(0, 160) || "(empty)"}
            </div>
          )}
          {verdict !== null && (
            <div style={{ color: verdict ? "#9ad29f" : "#e0916f" }}>
              {verdict ? "✓ replay reproduces the recorded run" : "✗ replay differs from the recorded run"}
            </div>
          )}
          {steps > 0 && (
            <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
              <button
                style={small}
                onClick={() => onStep(step === null ? 0 : Math.max(0, step - 1))}
                title="step back one node"
              >
                ◀
              </button>
              <span>{step === null ? `all ${steps} nodes` : `node ${step + 1} of ${steps}`}</span>
              <button
                style={small}
                onClick={() => onStep(step === null ? 0 : step + 1 >= steps ? null : step + 1)}
                title="step forward one node"
              >
                ▶
              </button>
              {step !== null && (
                <button style={small} onClick={() => onStep(null)}>
                  all
                </button>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
