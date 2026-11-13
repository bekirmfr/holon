// The Inspector at the Statements level (web-console-L4.md §5.3): it follows the cursor in the
// text, or the row picked on the canvas (which puts the cursor on it). On a name the language
// knows, its forms, the manual and a sample's lines; on a name the flow imports, what it is, where
// it is bound on this host and what the flow may do through it, each with its line; on a statement,
// its kind, its lines and what it may do there. Always, "This flow": its port and what starts it.
// The name's section is WordSection, which the Nodes level shows above a node's too.

import { useEffect, useMemo, useState, type ReactNode } from "react";
import { inspectAt, type FlowGraph, type GraphRow, type Inspected } from "./holonText";
import { IMPORT_WORD, importsOf, textCase, through, type Imported, type PickedRow } from "./inspectText";
import type { RowMay } from "./canvasLevel";
import { KIND_WORD, rowKey } from "./StatementGraph";
import { samples } from "./samples";
import { listDatabases, listFeeds, listFolders, listMails, listQueues, listSecrets, listServerSecrets } from "./server";
import { useCan } from "./useCan";

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="hl-itext__section">
      <div className="hl-itext__title">{title}</div>
      {children}
    </div>
  );
}

/** The lines a sample uses `word` on, when the language gives no example of its own: the first
 *  sample whose code says it, its first three such lines. */
function sampleUsing(word: string): { sample: string; lines: string[] } | null {
  const said = new RegExp(`(^|[^\\w$.])${word.replace(/[$]/g, "\\$")}\\b`);
  for (const s of samples()) {
    const lines = s.text.split("\n").filter((l) => !l.trimStart().startsWith("//") && !l.startsWith("import") && said.test(l));
    if (lines.length > 0) return { sample: s.name, lines: lines.slice(0, 3).map((l) => l.trim()) };
  }
  return null;
}

/** The name at the text's cursor, as `holon editor`'s inspector says it: its hover, the forms it
 *  is written in (from the language reference), the manual, and a sample's lines that use it. */
export function WordSection({ word, onManual, onSample }: { word: Inspected; onManual?: () => void; onSample?: (name: string) => void }) {
  const { forms = [], example: own = null } = word.usage ?? {};
  const example = useMemo(() => own ?? (forms.length > 0 ? sampleUsing(word.word) : null), [own, forms.length, word.word]);
  return (
    <section className="hl-word" aria-label={`the name ${word.word}`}>
      <div className="hl-word__head">
        Inspector: <code>{word.word}</code>
      </div>
      {/* The hover writes code between backticks. */}
      <div className="hl-word__hover">{word.hover.split("`").map((part, i) => (i % 2 === 1 ? <code key={i}>{part}</code> : part))}</div>
      {forms.length > 0 && (
        <Section title="forms">
          <ul className="hl-word__forms">
            {forms.map(([form, meaning]) => (
              <li key={form}>
                <code>{form}</code>
                {meaning && <span className="hl-word__meaning"> {meaning}</span>}
              </li>
            ))}
          </ul>
          {onManual && (
            <button className="hl-word__link" onClick={onManual}>
              In the manual →
            </button>
          )}
        </Section>
      )}
      {example && (
        <Section title={`example · ${example.sample}`}>
          <pre className="hl-word__example">{example.lines.join("\n")}</pre>
          {onSample && (
            <button className="hl-word__link" onClick={() => onSample(example.sample)}>
              Open the sample
            </button>
          )}
        </Section>
      )}
    </section>
  );
}

/** What a row lets the flow do: each item in words, marked when the deployed version may not. */
function MayRows({ may, fresh, none }: { may: RowMay[]; fresh: Set<string>; none: string }) {
  if (may.length === 0) return <div className="hl-itext__dim">{none}</div>;
  return (
    <ul className="hl-itext__may">
      {may.map((m) => (
        <li key={m.item} title={m.item}>
          {m.said}
          {fresh.has(m.item) && <span className="hl-itext__new"> · new: needs approval</span>}
        </li>
      ))}
    </ul>
  );
}

const lines = (r: GraphRow) => (r.last > r.line ? `lines ${r.line} to ${r.last}` : `line ${r.line}`);

function StatementSection({ row, may, fresh }: { row: GraphRow; may: RowMay[] | null; fresh: Set<string> }) {
  return (
    <section className="hl-itext" aria-label={`the statement on line ${row.line}`}>
      <div className="hl-itext__head">
        <span className={`hl-itext__kind hl-sg--${row.kind}`}>{row.head ? "starts runs" : KIND_WORD[row.kind]}</span> · {lines(row)}
      </div>
      <code className="hl-itext__code">{row.text}</code>
      {row.does && <div className="hl-itext__dim">{row.head ? `on ${row.does}` : `is ${row.does}`}</div>}
      {row.fallback !== null && <div className="hl-itext__dim">missing → {row.fallback}</div>}
      {row.outside.length > 0 && <div className="hl-itext__dim">takes {row.outside.join(", ")} from outside</div>}
      {may && (
        <Section title="It may, here">
          <MayRows may={may} fresh={fresh} none="nothing outside the flow: it only computes" />
        </Section>
      )}
    </section>
  );
}

/** Where an imported name is bound on this host, in words: undefined while asked, null when it
 *  isn't, or isn't something bound. */
function useBound(imported: Imported, slug: string | null, may: boolean): string | null | undefined {
  const [bound, setBound] = useState<{ key: string; said: string | null } | null>(null);
  const key = `${imported.kind}:${imported.target}@${slug ?? ""}`;
  useEffect(() => {
    if (!may) return;
    // A binding for this flow only comes before one for every flow.
    const pick = <T extends { name: string; deployment: string | null }>(all: T[]) =>
      all.find((e) => e.name === imported.target && e.deployment !== null && e.deployment === slug) ?? all.find((e) => e.name === imported.target && e.deployment === null);
    const whose = (e: { deployment: string | null }) => (e.deployment === null ? "for every flow" : "for this flow");
    const ask: Promise<string | null | undefined> | null =
      imported.kind === "database"
        ? listDatabases().then((all) => {
            const e = pick(all);
            return e && `${e.engine} ${e.path ?? e.address ?? e.connection ?? ""}, ${e.access}, ${whose(e)}`;
          })
        : imported.kind === "folder"
          ? listFolders().then((all) => {
              const e = pick(all);
              return e && `${e.path}, ${e.access}, ${whose(e)}`;
            })
          : imported.kind === "mail"
            ? listMails().then((all) => {
                const e = pick(all);
                return e && `${e.address}, from ${e.from}, ${whose(e)}`;
              })
            : imported.kind === "feed"
              ? listFeeds().then((all) => {
                  const e = pick(all);
                  return e && `${e.address}, ${whose(e)}`;
                })
              : imported.kind === "queue"
                ? listQueues().then((all) => {
                    const e = pick(all);
                    return e && `${e.address}, ${whose(e)}`;
                  })
                : null;
    if (!ask) return;
    let live = true;
    ask.then(
      (said) => live && setBound({ key, said: said ?? null }),
      () => live && setBound({ key, said: null }),
    );
    return () => {
      live = false;
    };
    // `key` says everything the ask depends on.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, may]);
  return bound?.key === key ? bound.said : undefined;
}

/** A secret's mark, never its value: its version where it is set (the flow's, else the server's
 *  for those who may view the host's), else that it isn't yet. Undefined while asked. */
function useSecretMark(name: string, slug: string | null, deployed: boolean, mayServer: boolean): string | undefined {
  const [mark, setMark] = useState<{ key: string; said: string } | null>(null);
  const key = `${name}@${deployed ? (slug ?? "") : ""}:${mayServer}`;
  useEffect(() => {
    let live = true;
    (async () => {
      const own = deployed && slug ? await listSecrets(slug).catch(() => []) : [];
      const s = own.find((x) => x.name === name);
      if (s) return `set · v${s.version}${s.server ? ", the server's" : ""}`;
      const server = mayServer ? await listServerSecrets().catch(() => []) : [];
      const t = server.find((x) => x.name === name);
      return t ? `set · v${t.version}, the server's` : "not set yet";
    })().then((said) => live && setMark({ key, said }));
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  return mark?.key === key ? mark.said : undefined;
}

const UNBOUND: Partial<Record<Imported["kind"], string>> = {
  state: "kept by this flow on this host: nothing to bind",
  collection: "kept by this flow on this host: nothing to bind",
  channel: "named on this host: flows send and receive on it by name",
};

function ImportSection({
  imported,
  graph,
  may,
  fresh,
  slug,
  deployed,
}: {
  imported: Imported;
  graph: FlowGraph | null;
  may: Map<string, RowMay[]> | null;
  fresh: Set<string>;
  slug: string | null;
  deployed: boolean;
}) {
  const can = useCan(["resource.view"]);
  const view = can.holds("resource.view");
  const bound = useBound(imported, slug, view);
  const secret = imported.kind === "secret";
  const mark = useSecretMark(imported.target, slug, deployed, view && secret);
  // What the flow may do through it, each with its row's line.
  const done = useMemo(() => {
    if (!graph || !may) return null;
    const out: { item: string; said: string; line: number }[] = [];
    graph.bands.forEach((band, b) =>
      band.rows.forEach((row, i) => {
        for (const m of may.get(rowKey(b, i)) ?? []) if (through(m.item, imported) && !out.some((o) => o.item === m.item && o.line === row.line)) out.push({ ...m, line: row.line });
      }),
    );
    return out.sort((a, b) => a.line - b.line);
  }, [graph, may, imported]);
  const source = imported.kind === "state" ? "state" : `${imported.kind}:${imported.target}`;
  const where =
    imported.kind === "host"
      ? `called over HTTPS: https://${imported.target}`
      : imported.kind === "flow"
        ? `the action ${imported.name} of the flow ${imported.target}, run on this host`
        : secret
          ? null
          : (UNBOUND[imported.kind] ?? (!can.known ? "…" : !view ? `not shown to you: ${can.may("resource.view")}` : bound === undefined ? "…" : (bound ?? "not bound on this host yet: an admin binds it")));
  return (
    <section className="hl-itext" aria-label={`the import ${imported.name}`}>
      <div className="hl-itext__head">
        <code>{imported.name}</code>: {IMPORT_WORD[imported.kind]}
        {imported.optional ? ", optional" : ""}
      </div>
      <code className="hl-itext__code">
        {source} · line {imported.line}
      </code>
      {where !== null && (
        <Section title="Bound here">
          <div className="hl-itext__body">{where}</div>
        </Section>
      )}
      {secret && (
        <Section title="Its value">
          <div className="hl-itext__body">{mark ?? "…"}</div>
        </Section>
      )}
      {done && (
        <Section title="It may, through it">
          {done.length === 0 ? (
            <div className="hl-itext__dim">nothing yet: no statement uses it</div>
          ) : (
            <ul className="hl-itext__may">
              {done.map((d) => (
                <li key={`${d.item} ${d.line}`} title={d.item}>
                  {d.said} <span className="hl-itext__dim">· line {d.line}</span>
                  {fresh.has(d.item) && <span className="hl-itext__new"> · new: needs approval</span>}
                </li>
              ))}
            </ul>
          )}
        </Section>
      )}
    </section>
  );
}

/** A route's trigger: the band's title starts with its method. */
const ROUTE = /^(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS) /;

export function InspectorText({
  text,
  caret,
  graph,
  picked = null,
  may,
  fresh,
  flow,
  onManual,
  onSample,
}: {
  /** The text as typed (the text pane's), and where its cursor is; null while the pane is shut. */
  text: string | null;
  caret: number | null;
  /** The statement graph drawn, and the row of it picked last. */
  graph: FlowGraph | null;
  picked?: PickedRow | null;
  /** What each row may do, by rowKey (canvasLevel.ts's rowsMay); null while not worked out. */
  may: Map<string, RowMay[]> | null;
  /** The items new against the deployed version. */
  fresh: Set<string>;
  flow: { name: string | null; port: string | number; deployed: boolean };
  onManual?: () => void;
  onSample?: (name: string) => void;
}) {
  const said = useMemo(() => (text !== null && caret !== null ? inspectAt(text, caret) : null), [text, caret]);
  const at = useMemo(() => (text !== null && caret !== null ? textCase(text, caret, graph, said, picked) : null), [text, caret, graph, said, picked]);
  const bands = graph?.bands ?? [];
  const routes = bands.some((b) => ROUTE.test(b.title));
  const imports = useMemo(() => (text === null ? [] : importsOf(text)), [text]);
  return (
    <>
      {at?.case === "word" && <WordSection word={at.word} onManual={onManual} onSample={onSample} />}
      {at?.case === "import" && <ImportSection imported={at.imported} graph={graph} may={may} fresh={fresh} slug={flow.name} deployed={flow.deployed} />}
      {at?.case === "statement" && <StatementSection row={at.row} may={may?.get(rowKey(at.band, at.index)) ?? null} fresh={fresh} />}
      {at === null && <div className="hl-itext__dim">Pick a statement on the canvas, or put the text cursor on a name.</div>}
      <section className="hl-itext" aria-label="this flow">
        <Section title="This flow">
          <div className="hl-itext__body">
            <code>{flow.name ?? "untitled"}</code>
            {routes ? `, on port ${flow.port}` : ""}
          </div>
          {bands.length === 0 ? (
            <div className="hl-itext__dim">nothing starts it yet: write a route, a clock, a handler or an action</div>
          ) : (
            <ul className="hl-itext__starts" aria-label="what starts it">
              {bands.map((b, i) => (
                <li key={i}>
                  <code>{b.title}</code>
                </li>
              ))}
            </ul>
          )}
          {imports.length > 0 && (
            <div className="hl-itext__dim">
              imports {imports.map((i) => `${i.name} (${i.kind})`).join(", ")}
            </div>
          )}
        </Section>
      </section>
    </>
  );
}
