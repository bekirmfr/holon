// A deployment's log lines (docs/L4s/logs-L4.md §5), terminal-style beside its runs: what its flow
// wrote with console.log and the rest, kept by the host apart from the runs. On the flow's Logs tab
// every line, following as they come; on a run's page, the lines up to the end of that run, and a
// toggle that narrows them to the run's own.

import { useEffect, useRef, useState } from "react";
import { appended, clock, isDropped, levelName, logQuery, LOG_LEVELS, type LogLevel } from "./logLines";
import { follow, format, type Route } from "./routes";
import { getLogs, type LogLine } from "./server";

/** The newest lines read at first, and the most a following pane keeps. */
const FIRST = 500;
const KEEP = 2000;

export function LogsPane({ slug, run, navigate }: { slug: string; run?: number; navigate: (r: Route) => void }) {
  const [level, setLevel] = useState<LogLevel>("all");
  const [own, setOwn] = useState(false);
  const [following, setFollowing] = useState(true);
  const [lines, setLines] = useState<LogLine[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const term = useRef<HTMLDivElement>(null);
  // With no run the pane is live: it reads on every second; for a run it is read once.
  const live = run === undefined;

  useEffect(() => {
    let alive = true;
    let after = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    setLines(null);
    const read = (first: boolean) => {
      getLogs(slug, logQuery(level, run, own, first ? FIRST : KEEP, first ? undefined : after)).then(
        (got) => {
          if (!alive) return;
          setError(null);
          if (first) setLines(got);
          else if (got.length > 0) setLines((old) => appended(old ?? [], got, KEEP));
          if (got.length > 0) after = got[got.length - 1].at;
          else if (first) after = 0;
          if (live && following) timer = setTimeout(() => read(false), 1000);
        },
        (e: unknown) => {
          if (!alive) return;
          setError(e instanceof Error ? e.message : String(e));
          if (live && following) timer = setTimeout(() => read(false), 3000);
        },
      );
    };
    read(true);
    return () => {
      alive = false;
      if (timer) clearTimeout(timer);
    };
  }, [slug, level, run, own, live, following]);

  // Following, the newest line stays in view.
  useEffect(() => {
    if (live && following && term.current) term.current.scrollTop = term.current.scrollHeight;
  }, [lines, live, following]);

  return (
    <section className="hl-logs" aria-label="log lines">
      <div className="hl-flow__bar">
        <div className="hl-seg" role="group" aria-label="level">
          {LOG_LEVELS.map(([l, label]) => (
            <button key={l} className="hl-seg__opt" aria-pressed={level === l} onClick={() => setLevel(l)}>
              {label}
            </button>
          ))}
        </div>
        {run !== undefined && (
          <label className="hl-logs__own">
            <input type="checkbox" checked={own} onChange={(e) => setOwn(e.target.checked)} /> only run #{run}'s
          </label>
        )}
        <span className="hl-flow__spacer" />
        {live && (
          <button className="hl-flow__button" aria-pressed={following} onClick={() => setFollowing(!following)} title="read on as new lines come">
            {following ? "Following" : "Paused"}
          </button>
        )}
      </div>
      {error && <div className="hl-flow__bad">{error}</div>}
      <div ref={term} className="hl-logs__term" role="log" aria-label="lines" tabIndex={0}>
        {lines === null ? (
          <p className="hl-flow__note">reading…</p>
        ) : lines.length === 0 ? (
          <p className="hl-flow__note">
            {run === undefined ? "no lines yet: a flow writes them with console.log" : own ? `run #${run} wrote no lines` : `no lines up to run #${run}`}
          </p>
        ) : (
          lines.map((l, i) => {
            const to: Route = { kind: "run", slug, seq: l.run };
            return (
              <div key={`${l.at}-${i}`} className={`hl-logs__line hl-logs__line--${isDropped(l) ? "dropped" : l.level}`}>
                <span className="hl-logs__at">{clock(l.at)}</span>
                <span className="hl-logs__level">{levelName(l)}</span>
                <a className="hl-logs__run" href={format(to)} onClick={follow(to, navigate)} title={`run #${l.run}`}>
                  #{l.run}
                </a>
                <span className="hl-logs__text">{isDropped(l) ? `⋯ ${l.text}` : l.text}</span>
              </div>
            );
          })
        )}
      </div>
    </section>
  );
}
