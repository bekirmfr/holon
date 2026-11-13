// The editor's bottom pane (web-console-L4.md §5.4), `b`, under the canvas, the text and the
// Inspector. In Edit it has four tabs: Problems (the text's mistakes as you type), Try a request,
// Replay the draft (the text as it is now, run on the deployed flow's newest runs) and Settings
// (the deploy settings of the version in the tab). In View runs it is the run pane (ViewPanes.tsx's
// BottomPane): the runs, and the run picked. Its height is dragged by its top edge, the same in
// both modes.

import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent, type ReactNode } from "react";
import { textMistakes } from "./holonText";
import { follow, format, type Route } from "./routes";
import { replaySummary, whyOf } from "./ReplayRuns";
import type { Replayed, RunReplayed } from "./server";

/** The bottom pane's tabs in Edit, in order, and what each is called. */
export const EDIT_TABS = [
  ["problems", "Problems"],
  ["try", "Try a request"],
  ["replay", "Replay the draft"],
  ["settings", "Settings"],
] as const;
export type EditTab = (typeof EDIT_TABS)[number][0];

const HEIGHT_KEY = "holon.viewHeight";

/** The pane's height, remembered per browser (styles.css `--view-height`; a third of the canvas
 *  when never dragged). */
function rememberedHeight(): number | null {
  try {
    const h = Number(localStorage.getItem(HEIGHT_KEY));
    return Number.isFinite(h) && h > 0 ? h : null;
  } catch {
    return null;
  }
}

/** Where the pane's height may go: never so small the tabs vanish, never most of the window. */
const clampHeight = (h: number) => Math.max(120, Math.min(window.innerHeight * 0.75, h));

/** The pane's height, dragged by its top edge: set on the canvas grid it is a row of
 *  (`--view-height`, styles.css), so the canvas above takes the rest. `pane` goes on the pane,
 *  `onEdge` on its edge. */
export function useBottomHeight() {
  const pane = useRef<HTMLDivElement>(null);
  const [height, setHeight] = useState<number | null>(rememberedHeight);
  useEffect(() => {
    const grid = pane.current?.parentElement;
    if (!grid) return;
    if (height === null) grid.style.removeProperty("--view-height");
    else grid.style.setProperty("--view-height", `${Math.round(height)}px`);
    return () => {
      grid.style.removeProperty("--view-height");
    };
  }, [height]);
  const onEdge = (e: ReactPointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    const startY = e.clientY;
    const startH = pane.current?.getBoundingClientRect().height ?? height ?? 240;
    let last = startH;
    const move = (m: PointerEvent) => {
      last = clampHeight(startH + startY - m.clientY);
      setHeight(last);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      try {
        localStorage.setItem(HEIGHT_KEY, String(Math.round(last)));
      } catch {
        // Not remembered, then.
      }
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };
  return { pane, onEdge };
}

/** The bottom pane in Edit: its tabs, each panel kept while another is shown, and the pane kept
 *  while hidden (a request typed stays typed). `panels` are the tabs' contents; `problems` is how
 *  many mistakes the text has. */
export function EditBottomPane({
  shown,
  tab,
  onTab,
  problems,
  panels,
  onClose,
}: {
  shown: boolean;
  tab: EditTab;
  onTab: (t: EditTab) => void;
  problems: number;
  panels: Record<EditTab, ReactNode>;
  onClose: () => void;
}) {
  const { pane, onEdge } = useBottomHeight();
  return (
    <div ref={pane} className="nodrag nowheel hl-bottom" role="region" aria-label="bottom" hidden={!shown}>
      <div className="hl-view__edge" onPointerDown={onEdge} title="drag to resize" />
      <div className="hl-bottom__head">
        <div className="hl-bottom__tabs" role="tablist" aria-label="bottom tabs">
          {EDIT_TABS.map(([t, label]) => (
            <button key={t} role="tab" aria-selected={t === tab} className="hl-bottom__tab" onClick={() => onTab(t)}>
              {label}
              {t === "problems" && <span className={problems > 0 ? "hl-bottom__count hl-bottom__count--bad" : "hl-bottom__count"}>{problems}</span>}
            </button>
          ))}
        </div>
        <kbd className="hl-pane-key" aria-hidden="true">
          b
        </kbd>
        <span style={{ flex: 1 }} />
        <button className="hl-panel-header__btn" onClick={onClose} title="hide the bottom pane (b)" aria-label="hide the bottom pane">
          ×
        </button>
      </div>
      {EDIT_TABS.map(([t, label]) => (
        <div key={t} role="tabpanel" aria-label={t === "settings" ? "Deployment settings" : label} className="hl-bottom__panel" hidden={t !== tab}>
          {panels[t]}
        </div>
      ))}
    </div>
  );
}

/** One mistake in the text: where it is (an offset and its line), and what `holon check` says. */
export type Problem = { from: number; line: number; message: string };

/** The text's mistakes, as `holon check` finds them (`holon_mistakes`), each with its line. */
export function problemsOf(text: string | null): Problem[] {
  if (!text) return [];
  let found: ReturnType<typeof textMistakes>;
  try {
    found = textMistakes(text);
  } catch {
    return [];
  }
  return found.map((m) => {
    const from = Math.min(m.from, text.length);
    return { from, line: text.slice(0, from).split("\n").length, message: m.message };
  });
}

/** Problems: the text's mistakes, a row each; a row's click goes to its line. `checked` is what
 *  the last Check said. */
export function ProblemsTab({ problems, checked, onGo }: { problems: Problem[]; checked: string; onGo: (p: Problem) => void }) {
  return (
    <div className="hl-problems">
      {checked && (
        <div className="hl-problems__checked" role="status">
          {checked}
        </div>
      )}
      {problems.length === 0 ? (
        <p className="hl-bottom__none">No problems: the text reads as `holon check` would have it.</p>
      ) : (
        <ul className="hl-problems__list" aria-label="problems">
          {problems.map((p) => (
            <li key={`${p.from} ${p.message}`}>
              <button onClick={() => onGo(p)} title={`go to line ${p.line}`}>
                <span className="hl-problems__line">line {p.line}</span> {p.message}
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** What the replay of a draft says for one run, by its outcome: a call it makes that the run
 *  never made has no recorded answer, so the replay can't tell (verify/replay.rs). */
export function outcomeSaid(run: RunReplayed): string {
  if (run.outcome === "same") return "same";
  if (run.outcome === "differs") return "differs";
  return /the run didn't/.test(whyOf(run)) ? "no recorded answer" : "can't tell";
}

/** Replay the draft: the text as it is now, run offline on the deployed flow's newest 20 runs
 *  (`replay`: the unit staged, then `GET …/replay?unit=&runs=20`, as the Review does), a row each:
 *  the run, what came in, what it answered then and now, and whether that is the same. Run again
 *  as the draft changes while it is shown. */
export function ReplayDraft({
  slug,
  deployed,
  unit,
  why = "",
  shown,
  replay,
  navigate,
}: {
  slug: string | null;
  /** Whether the flow has a deployment whose runs it can replay on. */
  deployed: boolean;
  /** The draft's unit (its server's hash), or null while it doesn't make one. */
  unit: string | null;
  /** Why it doesn't make one. */
  why?: string;
  /** Whether the tab is shown: it replays only then. */
  shown: boolean;
  replay: (slug: string, unit: string) => Promise<Replayed>;
  navigate?: (r: Route) => void;
}) {
  const [replayed, setReplayed] = useState<Replayed | null>(null);
  const [error, setError] = useState("");
  const [again, setAgain] = useState(0);
  // The draft last replayed, so showing the tab again doesn't replay what it has.
  const done = useRef("");
  const ask = deployed && slug && unit ? `${slug}\u0000${unit}\u0000${again}` : "";
  useEffect(() => {
    if (!shown || !ask || ask === done.current) return;
    let fresh = true;
    // The text is replayed once typing pauses, not on each key.
    const wait = setTimeout(() => {
      setError("");
      setReplayed(null);
      done.current = ask;
      replay(slug!, unit!).then(
        (r) => fresh && setReplayed(r),
        (e: unknown) => {
          if (!fresh) return;
          done.current = "";
          setError(e instanceof Error ? e.message : String(e));
        },
      );
    }, 400);
    return () => {
      fresh = false;
      clearTimeout(wait);
    };
  }, [shown, ask, replay, slug, unit]);

  if (!deployed || !slug) return <p className="hl-bottom__none">No runs to replay yet: deploy it first, or try a request.</p>;
  if (!unit) return <p className="hl-bottom__none">It can't be replayed as it is: {why || "it doesn't make a version yet"}.</p>;
  const runLink = (seq: number) => {
    const to: Route = { kind: "run", slug, seq };
    return navigate ? (
      <a href={format(to)} onClick={follow(to, navigate)}>
        #{seq}
      </a>
    ) : (
      <>#{seq}</>
    );
  };
  return (
    <div className="hl-replay-draft">
      <div className="hl-replay-draft__head">
        <span className="hl-replay__summary" aria-label="replay summary">
          {error ? `It can't be replayed: ${error}` : !replayed ? "replaying…" : replayed.replayed === 0 ? "No runs to replay yet: try a request." : replaySummary(replayed)}
        </span>
        <button className="hl-bottom__button" onClick={() => setAgain((n) => n + 1)} disabled={!replayed && !error} title="run the draft on the newest runs again">
          Replay again
        </button>
      </div>
      {replayed && replayed.runs.length > 0 && (
        <table className="hl-table hl-replay-draft__table" aria-label="replayed runs">
          <thead>
            <tr>
              <th>Run</th>
              <th>What came in</th>
              <th>Was</th>
              <th>Now</th>
              <th>Same or differs</th>
            </tr>
          </thead>
          <tbody>
            {replayed.runs
              .slice()
              .sort((a, b) => b.seq - a.seq)
              .map((run) => {
                const said = outcomeSaid(run);
                return (
                  <tr key={run.seq} className={`hl-replay-draft__row hl-replay-draft__row--${run.outcome}`}>
                    <td>{runLink(run.seq)}</td>
                    <td className="hl-flow__what">{run.route ?? run.kind}</td>
                    <td className="hl-table__dim">{run.status ?? "—"}</td>
                    <td className="hl-table__dim">{run.outcome === "cant_tell" ? "?" : (run.now_status ?? "—")}</td>
                    <td>
                      <span className="hl-replay-draft__outcome">{said}</span>
                      {run.outcome !== "same" && whyOf(run) && <div className="hl-flow__note">{whyOf(run)}</div>}
                    </td>
                  </tr>
                );
              })}
          </tbody>
        </table>
      )}
      {replayed && replayed.replayed > 0 && (
        <p className="hl-flow__note">
          Run offline on the deployed flow's newest {replayed.replayed === 1 ? "run" : `${replayed.replayed} runs`}: each call it makes is answered as that run was answered, and nothing is sent or changed.
        </p>
      )}
    </div>
  );
}
