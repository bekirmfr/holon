// One run's record, at `#<slug>.run-<n>` (gaps-web.md G15c, web-console-L4.md §8.2): how it ended,
// its steps timed by the server, what came in and went out, what it read, the secrets it read and
// where they went, what it did outside the flow and what the log holds of it, witnessed or not;
// checked offline with `holon verify`, or stepped through in the editor.

import { LogsPane } from "./LogsPane";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { useDialog } from "./Dialog";
import { formatValue } from "./diagnostics";
import type { WValue } from "./holon";
import { secretUses, witnessed, type Witness } from "./flowFacts";
import { executionOrder, nodeTimings } from "./replay";
import type { Route } from "./routes";
import { field, outside, runLabel, runOutcome, startedBy, text } from "./runs";
import { downloadRuns, getDeployedDocument, getLog, getSummary, type FlowDocument, type LogEntry } from "./server";
import { useKeys } from "./ui/Keys";

const show = (v: WValue | null | undefined) => (v === null || v === undefined ? "—" : formatValue(v));
const us = (t: number) => (t >= 1000 ? `${(t / 1000).toFixed(2)} ms` : `${t.toFixed(0)} µs`);

/** The log's entry for run `seq`, the canvas of the version it ran on (for its steps' names), and
 *  the runs kept just before and after it (`,` and `.` go there). */
function useRun(slug: string, seq: number) {
  const [run, setRun] = useState<LogEntry | null | undefined>(undefined);
  const [head, setHead] = useState("");
  const [doc, setDoc] = useState<FlowDocument | null>(null);
  const [beside, setBeside] = useState<{ before: number | null; after: number | null }>({ before: null, after: null });
  useEffect(() => {
    let stale = false;
    setRun(undefined);
    setBeside({ before: null, after: null });
    // From the run before it on: the log answers the runs after the seq it is given.
    getLog(slug, seq > 1 ? seq - 2 : undefined)
      .then(({ entries, head: h }) => {
        if (stale) return;
        const found = entries.find((e) => e.seq === seq) ?? null;
        setRun(found);
        setHead(h);
        setBeside({ before: entries.filter((e) => e.seq < seq).pop()?.seq ?? null, after: entries.find((e) => e.seq > seq)?.seq ?? null });
        if (found?.unit) getDeployedDocument(slug, found.unit).then((d) => !stale && setDoc(d), () => undefined);
      })
      .catch(() => !stale && setRun(null));
    return () => {
      stale = true;
    };
  }, [slug, seq]);
  return { run, head, doc, beside };
}

/** The witness the host's heads go to, read once (`/summary`'s `witness`); undefined until read. */
function useWitness(): Witness | undefined {
  const [witness, setWitness] = useState<Witness | undefined>(undefined);
  useEffect(() => {
    let live = true;
    getSummary(1).then(
      (s) => live && setWitness(s.chain.witness ?? null),
      () => live && setWitness(null),
    );
    return () => {
      live = false;
    };
  }, []);
  return witness;
}

export function RunDetail({ slug, seq, navigate }: { slug: string; seq: number; navigate: (r: Route) => void }) {
  const { run, head, doc, beside } = useRun(slug, seq);
  const witness = useWitness();
  const dialog = useDialog();
  const go = (to: number | null) => (to === null ? false : navigate({ kind: "run", slug, seq: to }));
  useKeys("run", { ",": () => go(beside.before), ".": () => go(beside.after) });
  const steps = useMemo(() => {
    if (!run) return [];
    const timings = nodeTimings(run, doc?.trace);
    const name = (id: string) => ((doc?.nodes ?? []) as { id: string; data?: { name?: string } }[]).find((n) => n.id === id)?.data?.name ?? id;
    if (timings.size > 0) return executionOrder(timings).map((id) => ({ label: name(id), ...timings.get(id)! }));
    // No canvas to name them by: the server's steps as it timed them.
    return (run.timeline ?? []).map((t) => ({ label: `step ${t.path.join(".")}`, start_us: t.start_us, end_us: t.end_us }));
  }, [run, doc]);
  if (run === undefined) return <p className="hl-flow__none">reading run #{seq}…</p>;
  if (run === null) return <p className="hl-flow__none">The log here holds no run #{seq}.</p>;

  const o = runOutcome(run);
  const { sent, did } = outside(run);
  const span = Math.max(1, ...steps.map((s) => s.end_us));
  // This run alone (web-console-L4.md Q3): its line names the hash of the one before it, so it is
  // checked by itself; tying it to the newest head takes the whole log.
  const checkOffline = async () => {
    const yes = await dialog.confirm(
      `Check run #${seq} offline`,
      "Export this run",
      <div className="hl-run__offline">
        <p>The export holds this run&apos;s line exactly as written, which names the hash of the line before it, and the units it ran. Anywhere, without this server:</p>
        <pre>holon verify {slug}-run-{seq}.json</pre>
        <p>It runs the run again and compares what it produces with what its line says, and checks the line&apos;s hash.</p>
        <p>To check it against the newest head as well, export the whole log (Runs, Export, or <code>holon export {slug}</code>):</p>
        <pre>
          holon verify {slug}-runs.json{head ? ` --head ${head}` : ""}
        </pre>
      </div>,
    );
    if (yes) await downloadRuns(slug, seq).catch((e: unknown) => dialog.notice(e instanceof Error ? e.message : String(e)));
  };
  const exchanges = (run.rounds ?? []).flatMap((r) => r.exchanges);
  const secrets = secretUses(run);
  const seen = witnessed(run.at_ms + run.duration_ms, witness ?? null);
  return (
    <article className="hl-run" aria-label={`run ${seq}`}>
      <header className="hl-run__head">
        <h2>
          Run #{seq} <span className="hl-flow__what">· {runLabel(run)}</span>
        </h2>
        <span className="hl-flow__spacer" />
        <button className="hl-flow__button" onClick={() => void checkOffline()}>
          Check offline…
        </button>
        <button className="hl-flow__button hl-flow__button--go" onClick={() => navigate({ kind: "editor", slug, view: true, seq })}>
          Step through in the editor
        </button>
      </header>
      <div className="hl-tiles">
        <Tile label="answer" value={o === "failed" ? `${run.status ?? ""} failed`.trim() : o === "waiting" ? "waiting" : String(run.status ?? "done")} tone={o} />
        <Tile label="took" value={run.duration_ms < 1 ? "<1 ms" : `${run.duration_ms.toFixed(1)} ms`} />
        <Tile label="when" value={new Date(run.at_ms).toLocaleTimeString()} />
        <Tile label="session" value={String(run.session)} />
        <Tile label="did outside" value={`↗${sent} ✎${did}`} />
        {(run.retries ?? 0) > 0 && <Tile label="ran again" value={String(run.retries)} />}
      </div>
      {run.error && <div className="hl-flow__banner hl-flow__bad">{run.error}</div>}

      <Section title="Steps">
        {steps.length === 0 ? (
          <p className="hl-flow__note">the server didn't time its steps</p>
        ) : (
          <ol className="hl-gantt" aria-label="steps">
            {steps.map((s, i) => (
              <li key={i} className="hl-gantt__row">
                <span className="hl-gantt__label">{s.label}</span>
                <span className="hl-gantt__track">
                  <span className="hl-gantt__bar" style={{ left: `${(s.start_us / span) * 100}%`, width: `${Math.max(0.5, ((s.end_us - s.start_us) / span) * 100)}%` }} />
                </span>
                <span className="hl-gantt__time">{us(s.end_us - s.start_us)}</span>
              </li>
            ))}
          </ol>
        )}
      </Section>

      <div className="hl-flow__columns">
        <Section title="In">
          {run.url && <Line name="request">{`${run.method ?? ""} ${run.url}`}</Line>}
          {run.request && <Value v={run.request} />}
          <Line name="started by">
            <span title={run.by ?? undefined}>{startedBy(run.kind, run.by)}</span>
          </Line>
        </Section>
        <Section title="Out">
          {run.response_body !== null ? <Line name={`answered ${run.status ?? ""}`}>{run.response_body || "(empty)"}</Line> : <p className="hl-flow__note">no answer</p>}
          {run.outputs.length > 0 && run.response_body === null && run.outputs.map((v, i) => <Value key={i} v={v} />)}
        </Section>
      </div>

      <div className="hl-flow__columns">
        <Section title="Read">
          {run.sourced.map(([cap, v]) => (
            <Line key={cap} name={cap.toLowerCase()}>
              {show(v)}
            </Line>
          ))}
          {(run.reads ?? []).map((r, i) => (
            <Line key={`r${i}`} name={r.file ? "file" : "collection"}>
              {r.file ? `${r.file.hash.slice(0, 12)} · ${r.file.size} bytes` : `${show(r.request)} → ${show(r.answer)}`}
            </Line>
          ))}
          {run.sourced.length === 0 && (run.reads ?? []).length === 0 && <p className="hl-flow__note">nothing from outside</p>}
        </Section>
        <Section title="Did outside">
          {exchanges.map((x, i) => (
            <Line key={`x${i}`} name="↗ sent">
              {`${text(field(x.request, "method"))} ${text(field(x.request, "url"))} → ${text(field(x.response, "error")) || text(field(x.response, "status"))}`}
            </Line>
          ))}
          {run.performed.map((p, i) => (
            <Line key={`p${i}`} name={`✎ ${p.capability}`}>
              {show(p.payload)}
            </Line>
          ))}
          {exchanges.length === 0 && run.performed.length === 0 && <p className="hl-flow__note">nothing</p>}
        </Section>
      </div>

      {secrets.length > 0 && (
        <Section title="Secrets it read">
          <table className="hl-table" aria-label="secrets it read">
            <thead>
              <tr>
                <th>Secret</th>
                <th>Version</th>
                <th>Where it went</th>
              </tr>
            </thead>
            <tbody>
              {secrets.map((s) => (
                <tr key={s.name}>
                  <td className="hl-flow__what">{s.name}</td>
                  <td title="its version and fingerprint: which value it had, never the value">
                    {[s.version, s.fingerprint].filter(Boolean).join(" ") || "—"}
                  </td>
                  <td>{s.to.length > 0 ? s.to.join(", ") : <span className="hl-table__dim">nowhere outside: no request it sent held it</span>}</td>
                </tr>
              ))}
            </tbody>
          </table>
          <p className="hl-flow__note">The log keeps which value it had, by its version and fingerprint, never the value.</p>
        </Section>
      )}

      <Section title="Log lines">
        <LogsPane slug={slug} run={seq} navigate={navigate} />
      </Section>

      <Section title="On record">
        <dl className="hl-flow__facts">
          <Fact name="run id">{run.run_id || "—"}</Fact>
          {run.nonce && <Fact name="nonce">{run.nonce}</Fact>}
          {run.grant && <Fact name="grant">{run.grant}</Fact>}
          {run.unit && <Fact name="unit">{run.unit}</Fact>}
          {(run.parked ?? []).length > 0 && <Fact name="waits at">{(run.parked ?? []).join(", ")}</Fact>}
          {head && <Fact name="log head">{head}</Fact>}
          <Fact name="witnessed">{witness === undefined ? "…" : seen.said}</Fact>
        </dl>
      </Section>
    </article>
  );
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="hl-flow__section" aria-label={title.toLowerCase()}>
      <h2>{title}</h2>
      {children}
    </section>
  );
}

function Tile({ label, value, tone = "ok" }: { label: string; value: string; tone?: string }) {
  return (
    <div className="hl-tile">
      <div className={`hl-tile__value${tone === "failed" ? " hl-flow__bad" : tone === "waiting" ? " hl-flow__waiting" : ""}`}>{value}</div>
      <div className="hl-tile__label">{label}</div>
    </div>
  );
}

function Line({ name, children }: { name: string; children: ReactNode }) {
  return (
    <div className="hl-run__line">
      <span className="hl-run__name">{name}</span>
      <span className="hl-run__said">{children}</span>
    </div>
  );
}

function Value({ v }: { v: WValue | null }) {
  return <pre className="hl-run__value">{show(v)}</pre>;
}

function Fact({ name, children }: { name: string; children: ReactNode }) {
  return (
    <div>
      <dt>{name}</dt>
      <dd>
        <code>{children}</code>
      </dd>
    </div>
  );
}
