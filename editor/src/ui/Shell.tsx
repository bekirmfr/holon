// The console's frame (gaps-web.md §3, G4), as the Console design draws it: the side nav with every
// page, its icon and its key, the host card under it (the run log's state), and the top bar over
// the page: where you are (the crumbs), the command palette, the interface, the look, the keys
// and who is signed in (the account dialog); in the Terminal interface, a status bar under the
// page (G33). Narrow, the nav is a rail of icons, and on a phone a drawer the top bar opens (G34).
// The pages themselves are App's; this only goes to them.

import { useEffect, useState, type ReactNode } from "react";
import { AccountButton, type Me } from "../AuthGate";
import { THEMES, type Theme, type Ui } from "../look";
import { pageOf, type AppPage } from "../Pages";
import { crumbs, follow, format, labelOf, type Route } from "../routes";
import { ServerError, getSummary, getVersion, hostAddress, type Summary } from "../server";
import type { HostPages } from "../useCan";

type Entry = { label: string; to: Route; icon: keyof typeof ICON; key?: number | string };

/** The nav's icons: 24-unit strokes, drawn at 16 px. */
const ICON = {
  overview: <><path d="M3 12 12 4l9 8" /><path d="M5 10v10h14V10" /></>,
  approvals: <><path d="M9 12l2 2 4-4" /><path d="M12 3l8 4v5c0 5-3.5 8-8 9-4.5-1-8-4-8-9V7z" /></>,
  flows: <><path d="M4 6h10M4 12h16M4 18h7" /><circle cx="18" cy="6" r="2" /><circle cx="15" cy="18" r="2" /></>,
  editor: <path d="M8 6 3 12l5 6M16 6l5 6-5 6" />,
  library: <><path d="M4 4h5v16H4zM10 4h5v16h-5z" /><path d="m16 5 4 1-3 14-4-1" /></>,
  resources: <><ellipse cx="12" cy="6" rx="7" ry="3" /><path d="M5 6v6c0 1.7 3 3 7 3s7-1.3 7-3V6M5 12v6c0 1.7 3 3 7 3s7-1.3 7-3v-6" /></>,
  people: <><circle cx="9" cy="8" r="3.5" /><path d="M2.5 20c.8-3.6 3.4-5.5 6.5-5.5s5.7 1.9 6.5 5.5" /><circle cx="17" cy="9" r="2.5" /><path d="M17 14.5c2.4 0 4 1.5 4.5 4" /></>,
  policy: <><path d="M6 3h9l4 4v14H6z" /><path d="M14 3v5h5M9 13h7M9 17h5" /></>,
  start: <><circle cx="12" cy="12" r="9" /><path d="m10 8 6 4-6 4z" /></>,
  docs: <><path d="M5 4h10a4 4 0 0 1 4 4v12H9a4 4 0 0 1-4-4z" /><path d="M9 8h6M9 12h6" /></>,
  add: <path d="M12 5v14M5 12h14" />,
};

function Icon({ name }: { name: keyof typeof ICON }) {
  return (
    <svg className="hl-nav__icon" viewBox="0 0 24 24" aria-hidden="true">
      {ICON[name]}
    </svg>
  );
}

/** The nav's groups, in order: the work, the host's own pages (each to whoever may view it; a
 *  person who may not see everyone, what they own), and help. The keys are keymap.ts's anywhere keys. */
const WORK: Entry[] = [
  { label: "Overview", to: { kind: "overview" }, icon: "overview", key: 1 },
  { label: "Approvals", to: { kind: "approvals", tab: "deploys" }, icon: "approvals", key: 2 },
  { label: "Flows", to: { kind: "flows" }, icon: "flows", key: 3 },
  { label: "Editor", to: { kind: "editor", slug: null, view: false, seq: null }, icon: "editor", key: 4 },
  { label: "Library", to: { kind: "library", tab: "samples" }, icon: "library", key: 5 },
];
const HOST: Entry[] = [
  { label: "Resources", to: { kind: "resources", tab: "databases" }, icon: "resources", key: 6 },
  { label: "People and access", to: { kind: "people", tab: null }, icon: "people", key: 7 },
  { label: "Policy", to: { kind: "policy" }, icon: "policy", key: 8 },
];
/** A person who may not see everyone: the agents and services they own, on People and access's page. */
const OWNED: Entry = { label: "Your agents and services", to: { kind: "people", tab: null }, icon: "people", key: 7 };
/** The host's pages the nav lists, as `can` answers who may view them (useCan.tsx's hostPagesOf). */
export const hostEntries = (pages: HostPages): Entry[] => [
  ...(pages.resources ? [HOST[0]] : []),
  ...(pages.people === "everyone" ? [HOST[1]] : pages.people === "owned" ? [OWNED] : []),
  ...(pages.policy ? [HOST[2]] : []),
];
const HELP: Entry[] = [
  { label: "Getting started", to: { kind: "start" }, icon: "start", key: "s" },
  { label: "Docs", to: { kind: "docs", page: null, anchor: null }, icon: "docs", key: "d" },
];

/** The keys worth knowing where `r` is, for the status bar (keymap.ts has them all): a page's
 *  own first, its `d` among them where it isn't the docs. */
export function hintsAt(r: Route): [string, string][] {
  switch (r.kind) {
    case "editor":
      return r.view
        ? [["j k", "step"], [", .", "runs"], ["e", "edit"], ["[ ]", "tabs"]]
        : [["ctrl+s", "save"], ["ctrl+d", "deploy"], ["c x i", "panes"], ["l", "level"], ["[ ]", "tabs"]];
    case "review":
      return [["[ ]", "the queue"], ["d", "the diff"], ["w", "whole"], ["esc", "back"]];
    case "flows":
      return [["j k", "select"], ["enter", "open"], ["1–8", "pages"]];
    case "approvals":
      return [["j k", "select"], ["a", "approve"], ["d", "decline"], ["[ ]", "tabs"]];
    case "flow":
      return r.tab === "runs" ? [["j k", "select"], ["enter", "open"], ["[ ]", "tabs"], ["esc", "up"]] : [["[ ]", "tabs"], ["esc", "up"], ["F6", "panes"]];
    case "run":
      return [[", .", "runs"], ["[ ]", "tabs"], ["esc", "up"]];
    default:
      return [["1–8", "pages"], ["s d", "start, docs"], ["F6", "panes"]];
  }
}

/** What waits for a person: versions to approve (Approvals' Deploys) and the runs' approvals
 *  theirs to decide. */
export type Waiting = { changes: number; runs: number };

/** What waits, as the status bar says it: "waits: 2 changes, 2 runs". */
export function waitsSaid({ changes, runs }: Waiting): string {
  const parts = [changes > 0 && `${changes} change${changes === 1 ? "" : "s"}`, runs > 0 && `${runs} run${runs === 1 ? "" : "s"}`].filter(Boolean);
  return `waits: ${parts.length > 0 ? parts.join(", ") : "nothing"}`;
}


export function Shell({
  route,
  page,
  navigate,
  me,
  pages,
  onNew,
  onFind,
  onStart,
  onKeys,
  theme,
  onTheme,
  ui,
  onUi,
  waiting = { changes: 0, runs: 0 },
  children,
}: {
  route: Route;
  /** The page shown: its entry is the current one (the first, where two show the same page). */
  page: AppPage;
  navigate: (r: Route) => void;
  me: Me;
  /** The host's pages `me` is shown (useCan.tsx's useHostPages). */
  pages: HostPages;
  onNew: () => void;
  /** Open the command palette. */
  onFind: () => void;
  /** Open Getting started. */
  onStart: () => void;
  /** Show the keys. */
  onKeys: () => void;
  theme: Theme;
  onTheme: (t: Theme) => void;
  ui: Ui;
  onUi: (u: Ui) => void;
  /** What waits for a person (approvals.ts): Approvals' count, all told. */
  waiting?: Waiting;
  children: ReactNode;
}) {
  const host = hostEntries(pages);
  const waits = waiting.changes + waiting.runs;
  const groups: [string, Entry[]][] = [["work", WORK], ...(host.length > 0 ? ([["host", host]] as [string, Entry[]][]) : []), ["help", HELP]];
  const version = useVersion();
  // A flow's page is one of the flows'.
  const at = page === "flow" ? "flows" : page;
  const current = [...WORK, ...host, ...HELP].find((e) => pageOf(e.to) === at);
  const trail = crumbs(route);
  // People and access is, for someone who may not see everyone, what they own.
  const crumb = (r: Route) => (r.kind === "people" && r.tab === null && pages.people === "owned" ? OWNED.label : labelOf(r));
  // On a phone the nav is a drawer: the top bar's menu opens it, going anywhere closes it.
  const [drawer, setDrawer] = useState(false);
  const routeKey = format(route);
  useEffect(() => setDrawer(false), [routeKey]);
  return (
    <div className="hl-shell">
      {drawer && <div className="hl-nav__scrim" onClick={() => setDrawer(false)} aria-hidden="true" />}
      <nav className={`hl-nav${drawer ? " hl-nav--open" : ""}`} aria-label="main" id="hl-nav">
        <a className="hl-nav__brand" href={format({ kind: "overview" })} onClick={follow({ kind: "overview" }, navigate)} aria-label="holon: the overview">
          <svg className="hl-nav__logo" width="20" height="20" viewBox="0 0 24 24" aria-hidden="true">
            <path d="M12 2 21 7v10l-9 5-9-5V7z" fill="var(--accent)" />
            <path d="M12 7 16.5 9.5v5L12 17l-4.5-2.5v-5z" fill="var(--surface)" />
          </svg>
          <span className="hl-nav__name">holon</span>
          {version && <span className="hl-nav__version">{version}</span>}
        </a>
        <button className="hl-nav__new" onClick={onNew} title="new flow">
          <Icon name="add" />
          <span className="hl-nav__label">new flow</span>
        </button>
        {groups.map(([name, entries]) => (
          <ul key={name} className="hl-nav__group" aria-label={name}>
            {name !== "work" && (
              <li className="hl-nav__heading" aria-hidden="true">
                {name}
              </li>
            )}
            {entries.map((e) => (
              <li key={e.label}>
                <a href={format(e.to)} className="hl-nav__link" aria-current={e === current ? "page" : undefined} title={e.label} onClick={follow(e.to, navigate)}>
                  <Icon name={e.icon} />
                  <span className="hl-nav__label">{e.label}</span>
                  {e.to.kind === "approvals" && waits > 0 && (
                    <span className="hl-nav__badge" title={`${waits} waiting for a person`}>
                      {waits}
                    </span>
                  )}
                  {e.key !== undefined && (
                    <kbd className="hl-nav__key" aria-hidden="true">
                      {e.key}
                    </kbd>
                  )}
                </a>
              </li>
            ))}
          </ul>
        ))}
        <HostCard version={version} />
      </nav>
      <div className="hl-shell__main">
        <header className="hl-top">
          <button className="hl-top__menu" aria-label="menu" aria-controls="hl-nav" aria-expanded={drawer} onClick={() => setDrawer((d) => !d)}>
            ☰
          </button>
          <nav className="hl-crumbs" aria-label="crumbs">
            <ol>
              {trail.map((r, i) => (
                <li key={format(r)}>
                  {i === trail.length - 1 ? (
                    <span aria-current="location">{crumb(r)}</span>
                  ) : (
                    <a href={format(r)} onClick={follow(r, navigate)}>
                      {crumb(r)}
                    </a>
                  )}
                </li>
              ))}
            </ol>
          </nav>
          <button className="hl-top__find" onClick={onFind} title="the command palette: go anywhere, run a command, : for one by name">
            <svg className="hl-top__mag" width="15" height="15" viewBox="0 0 24 24" aria-hidden="true">
              <circle cx="11" cy="11" r="7" />
              <path d="m20 20-3.5-3.5" />
            </svg>
            <span>Search flows, runs, pages, or type : for a command</span>
            <kbd aria-hidden="true">Ctrl K</kbd>
          </button>
          <span className="hl-top__right">
            <span className="hl-seg hl-top__ui" role="group" aria-label="interface" title="the interface: Terminal is mono, square, keys on show, with a status bar (t)">
              {(["clean", "terminal"] as const).map((u) => (
                <button key={u} aria-pressed={ui === u} onClick={() => onUi(u)}>
                  {u === "clean" ? "Clean" : "Terminal"}
                </button>
              ))}
            </span>
            <span className="hl-seg hl-top__theme" role="group" aria-label="theme" title="the look: auto follows the system; kept with your account">
              {THEMES.map((t) => (
                <button key={t} aria-pressed={theme === t} onClick={() => onTheme(t)}>
                  {t[0].toUpperCase() + t.slice(1)}
                </button>
              ))}
            </span>
            <button className="hl-top__keys" onClick={onKeys} aria-label="keys" title="every key (?)">
              ?
            </button>
            <AccountButton me={me} onStart={onStart} />
          </span>
        </header>
        {children}
        {ui === "terminal" && <StatusBar route={route} waiting={waiting} />}
      </div>
    </div>
  );
}

/** The Terminal interface's status bar: the flows and how many are live, what waits for a person,
 *  the run log's newest head and whether it is intact, and the keys worth knowing here. The host's
 *  summary is read again every 15 s while it shows. */
function StatusBar({ route, waiting }: { route: Route; waiting: Waiting }) {
  const { summary } = useSummary(15_000, 15_000);
  const newest = summary?.latest[0] && summary.deployments.find((d) => d.slug === summary.latest[0].slug);
  const head = newest?.head ?? summary?.deployments[0]?.head;
  const breaks = summary ? (summary.chain.checked ? summary.chain.checked.breaks.length : summary.chain.breaks) : 0;
  return (
    <footer className="hl-status" aria-label="status">
      <span>{summary ? `${summary.deployments.length} flow${summary.deployments.length === 1 ? "" : "s"} · ${summary.totals.running} live` : "…"}</span>
      <span className={waiting.changes + waiting.runs > 0 ? "hl-status__warn" : undefined}>{waitsSaid(waiting)}</span>
      {summary && (
        <span>
          chain{" "}
          {head ? (
            <>
              <code title={head}>{head.slice(0, 8)}</code> <span className={breaks ? "hl-status__bad" : undefined}>{breaks ? `${breaks} break${breaks === 1 ? "" : "s"}` : "intact"}</span>
            </>
          ) : (
            "empty"
          )}
        </span>
      )}
      <span className="hl-status__keys">
        {[...hintsAt(route), ["?", "keys"], ["/", "find"], ["t", "clean"]].map(([k, said]) => (
          <span key={k}>
            <kbd>{k}</kbd> {said}
          </span>
        ))}
      </span>
    </footer>
  );
}

/** The version the host runs: undefined while it is asked, null when it can't be reached. Read
 *  once; the pages say what is wrong when a request fails. */
function useVersion(): string | null | undefined {
  const [version, setVersion] = useState<string | null | undefined>(undefined);
  useEffect(() => {
    let on = true;
    getVersion()
      .then((v) => on && setVersion(v))
      .catch(() => on && setVersion(null));
    return () => {
      on = false;
    };
  }, []);
  return version;
}

/** The host's summary, read now and again every `every` ms while it answers and every `retry` ms
 *  while it doesn't (null until one comes; a failed read keeps the last), whether the last read got
 *  an answer, a refusal included (undefined before the first), whether it is being read, and a way
 *  to read it again at once. */
function useSummary(
  every: number,
  retry: number,
): { summary: Summary | null; answers: boolean | undefined; reading: boolean; refresh: () => void } {
  const [summary, setSummary] = useState<Summary | null>(null);
  const [answers, setAnswers] = useState<boolean | undefined>(undefined);
  const [reading, setReading] = useState(false);
  const [asked, setAsked] = useState(0);
  useEffect(() => {
    let on = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const read = () => {
      setReading(true);
      getSummary(1)
        .then(
          (s) => {
            if (!on) return;
            setSummary(s);
            setAnswers(true);
            timer = setTimeout(read, every);
          },
          (e: unknown) => {
            if (!on) return;
            // A refusal (say, signed out) is an answer; only a request that got none is not.
            const answered = e instanceof ServerError;
            setAnswers(answered);
            timer = setTimeout(read, answered ? every : retry);
          },
        )
        .finally(() => on && setReading(false));
    };
    read();
    return () => {
      on = false;
      clearTimeout(timer);
    };
  }, [every, retry, asked]);
  return { summary, answers, reading, refresh: () => setAsked((n) => n + 1) };
}

/** Which host the console is on: its address (and whether it answers, as of its last read, or
 *  of the version's before one), whether its run log is intact, how many runs it holds and the
 *  newest head, as of when it was last read (dimmed while it doesn't answer); the refresh reads it
 *  again now. The version is the brand's. */
function HostCard({ version }: { version: string | null | undefined }) {
  const { summary, answers, reading, refresh } = useSummary(60_000, 10_000);
  const up = answers ?? (version === null ? false : version ? true : undefined);
  // A page's request that got no answer: read the host now, not at the next turn of the clock.
  useEffect(() => {
    const unanswered = () => !reading && answers !== false && refresh();
    window.addEventListener("holon:unanswered", unanswered);
    return () => window.removeEventListener("holon:unanswered", unanswered);
  }, [reading, answers, refresh]);
  const breaks = summary ? (summary.chain.checked ? summary.chain.checked.breaks.length : summary.chain.breaks) : 0;
  const newest = summary?.latest[0] && summary.deployments.find((d) => d.slug === summary.latest[0].slug);
  const head = newest?.head ?? summary?.deployments[0]?.head;
  const asOf = summary && new Date(summary.at_ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
  return (
    <section className={`hl-host${up === false ? " hl-host--down" : ""}`} aria-label="host">
      <span className="hl-host__row">
        <b className="hl-host__address">{hostAddress()}</b>
        <span
          className={`hl-host__dot hl-host__dot--${up === false ? "bad" : up ? "ok" : "wait"}`}
          title={up === false ? "not answering" : up ? "answering" : "asking…"}
          aria-hidden="true"
        />
      </span>
      <span className="hl-host__row hl-host__asof">
        <span title={up === false && asOf ? `what shows below was read at ${asOf}` : undefined}>
          {up === false ? "can't be reached" : asOf ? `as of ${asOf}` : "…"}
        </span>
        <button className="hl-host__refresh" onClick={refresh} disabled={reading} aria-label="refresh the host card" title="read the host's summary again now">
          <svg viewBox="0 0 24 24" width="13" height="13" aria-hidden="true" className={reading ? "hl-host__spin" : undefined}>
            <path d="M20 11a8 8 0 1 0-2.3 5.7" />
            <path d="M20 4v7h-7" />
          </svg>
        </button>
      </span>
      {summary && (
        <>
          <span className="hl-host__row hl-host__log">
            <span>Run log</span>
            <span className={breaks ? "hl-host__bad" : "hl-host__ok"}>{breaks ? `${breaks} break${breaks === 1 ? "" : "s"}` : "intact"}</span>
          </span>
          <span className="hl-host__row hl-host__runs">
            <span>
              {summary.chain.on_record.toLocaleString("en")} run{summary.chain.on_record === 1 ? "" : "s"}
            </span>
            {head && <code title={head}>{head.slice(0, 8)}</code>}
          </span>
        </>
      )}
    </section>
  );
}
