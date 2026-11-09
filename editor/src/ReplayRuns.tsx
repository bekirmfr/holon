// A version replayed on a deployment's recent runs (gaps-web.md G11), offline, by the server
// (GET /deployments/<slug>/replay, verify/replay.rs): "20 replayed · 17 same · 3 differ", then each
// run that differs or can't be told, with why. A run's outbound requests and reads are answered by
// what that run was answered, so nothing is sent; one the version asks that the run never did
// can't be told. Shown where a version is decided: its review (ReviewPage.tsx) and the editor's
// deploy (useDeployFlow.ts).

import { useEffect, useState } from "react";
import { follow, format, type Route } from "./routes";
import { replayVersion, type Replayed, type RunReplayed } from "./server";

/** "20 replayed · 17 same · 3 differ · 1 can't tell", leaving out what is none. */
export function replaySummary(r: Replayed): string {
  if (r.replayed === 0) return "no runs to replay yet";
  const parts = [`${r.replayed} replayed`, `${r.same} same`];
  if (r.differ > 0) parts.push(`${r.differ} differ${r.differ === 1 ? "s" : ""}`);
  if (r.cant_tell > 0) parts.push(`${r.cant_tell} can't tell`);
  return parts.join(" · ");
}

/** What a run was, as its row names it: `POST /orders`, `action price`, a clock's label. */
function what(run: RunReplayed): string {
  return run.route ?? run.kind;
}

/** Why it differs or can't be told, as one line. */
export function whyOf(run: RunReplayed): string {
  return (run.why ?? []).join("; ");
}

export function ReplayRuns({ slug, unit, navigate }: { slug: string; unit: string; navigate?: (r: Route) => void }) {
  const [replayed, setReplayed] = useState<Replayed | null>(null);
  const [error, setError] = useState("");
  useEffect(() => {
    let fresh = true;
    setReplayed(null);
    setError("");
    replayVersion(slug, unit).then(
      (r) => fresh && setReplayed(r),
      (e: unknown) => fresh && setError(e instanceof Error ? e.message : String(e)),
    );
    return () => {
      fresh = false;
    };
  }, [slug, unit]);

  const told = replayed?.runs.filter((r) => r.outcome !== "same") ?? [];
  return (
    <div className="hl-review-page__card hl-replay" role="group" aria-label="on recent runs">
      <h2>On recent runs</h2>
      {error ? (
        <p className="hl-flow__note">It can't be replayed: {error}</p>
      ) : !replayed ? (
        <p className="hl-flow__note">replaying…</p>
      ) : (
        <>
          <p className="hl-replay__summary" aria-label="replay summary">
            {replaySummary(replayed)}
          </p>
          {told.length > 0 && (
            <ul className="hl-replay__runs">
              {told.map((run) => (
                <li key={run.seq} className={`hl-replay__run hl-replay__run--${run.outcome}`}>
                  {navigate ? (
                    <a href={format({ kind: "run", slug, seq: run.seq })} onClick={follow({ kind: "run", slug, seq: run.seq }, navigate)}>
                      run {run.seq}
                    </a>
                  ) : (
                    <span>run {run.seq}</span>
                  )}{" "}
                  <span className="hl-replay__what">{what(run)}</span>{" "}
                  <span className="hl-replay__outcome">{run.outcome === "differs" ? "differs" : "can't tell"}</span>
                  <div className="hl-flow__note">{whyOf(run)}</div>
                </li>
              ))}
            </ul>
          )}
          {replayed.replayed > 0 && (
            <p className="hl-flow__note">
              Its last {replayed.replayed === 1 ? "run" : `${replayed.replayed} runs`}, run again offline with this version: each request it sends is answered as that run was, and nothing is sent or changed.
            </p>
          )}
        </>
      )}
    </div>
  );
}
