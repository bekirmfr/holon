// The console's frame (gaps-web.md §3, G4): the side nav with every page and its key, the host
// card under it, and the top bar over the page: where you are (the crumbs), the command palette,
// the interface, the look and who is signed in; in the Terminal interface, a status bar under the
// page (G33). Narrow, the nav is a rail of icons, and on a phone a drawer the top bar opens (G34).
// The pages themselves are App's; this only goes to them.

import { useEffect, useState, type ReactNode } from "react";
import { AccountMenu, type Me } from "../AuthGate";
import { THEMES, type Theme, type Ui } from "../look";
import { pageOf, type AppPage } from "../Pages";
import { crumbs, follow, format, labelOf, type Route } from "../routes";
import { getSummary, getVersion, hostAddress, type Summary } from "../server";

type Entry = { label: string; to: Route; icon: string; key?: number };

/** The nav's groups, in order: the work, the host's own pages (an admin's), and help. The
 *  numbers are keymap.ts's PAGE_KEYS. */
const WORK: Entry[] = [
  { label: "Overview", to: { kind: "overview" }, icon: "◉", key: 1 },
  { label: "Approvals", to: { kind: "approvals", tab: "deploys" }, icon: "✓", key: 2 },
  { label: "Flows", to: { kind: "flows" }, icon: "≋", key: 3 },
  { label: "Editor", to: { kind: "editor", slug: null, view: false, seq: null }, icon: "✎", key: 4 },
  { label: "Library", to: { kind: "library", tab: "samples" }, icon: "▤", key: 5 },
];
const HOST: Entry[] = [
  { label: "Resources", to: { kind: "resources", tab: "databases" }, icon: "◫", key: 6 },
  { label: "People and access", to: { kind: "people", tab: null }, icon: "◌", key: 7 },
  { label: "Policy", to: { kind: "policy" }, icon: "§", key: 8 },
];
const HELP: Entry[] = [
  { label: "Getting started", to: { kind: "start" }, icon: "▸" },
  { label: "Docs", to: { kind: "docs", page: null, anchor: null }, icon: "?" },
];

/** The keys worth knowing where `r` is, for the status bar (keymap.ts has them all). */
export function hintsAt(r: Route): [string, string][] {
  switch (r.kind) {
    case "editor":
      return r.view
        ? [["j k", "step"], [", .", "runs"], ["e", "edit"]]
        : [["ctrl+s", "save"], ["ctrl+d", "deploy"], ["c x i", "panes"]];
    case "review":
      return [["[ ]", "the queue"], ["esc", "back"]];
    case "flow":
    case "run":
      return [["esc", "up"], ["F6", "panes"]];
    default:
      return [["1–8", "pages"], ["F6", "panes"]];
  }
}


export function Shell({
  route,
  page,
  navigate,
  me,
  onNew,
  onFind,
  onWelcome,
  theme,
  onTheme,
  ui,
  onUi,
  waiting = 0,
  children,
}: {
  route: Route;
  /** The page shown: its entry is the current one (the first, where two show the same page). */
  page: AppPage;
  navigate: (r: Route) => void;
  me: Me;
  onNew: () => void;
  /** Open the command palette. */
  onFind: () => void;
  onWelcome: () => void;
  theme: Theme;
  onTheme: (t: Theme) => void;
  ui: Ui;
  onUi: (u: Ui) => void;
  /** How many things wait for a person (approvals.ts): Approvals' count. */
  waiting?: number;
  children: ReactNode;
}) {
  const groups: [string, Entry[]][] = [["work", WORK], ...(me.role === "admin" ? ([["host", HOST]] as [string, Entry[]][]) : []), ["help", HELP]];
  // A flow's page is one of the flows'.
  const at = page === "flow" ? "flows" : page;
  const current = [...WORK, ...HOST, ...HELP].find((e) => pageOf(e.to) === at);
  const trail = crumbs(route);
  // On a phone the nav is a drawer: the top bar's menu opens it, going anywhere closes it.
  const [drawer, setDrawer] = useState(false);
  const routeKey = format(route);
  useEffect(() => setDrawer(false), [routeKey]);
  return (
    <div className="hl-shell">
      {drawer && <div className="hl-nav__scrim" onClick={() => setDrawer(false)} aria-hidden="true" />}
      <nav className={`hl-nav${drawer ? " hl-nav--open" : ""}`} aria-label="main" id="hl-nav">
        <div className="hl-nav__brand">
          holon <span className="hl-nav__mode">console</span>
        </div>
        <button className="hl-nav__new" onClick={onNew} title="new flow">
          <span className="hl-nav__icon" aria-hidden="true">+</span>
          <span className="hl-nav__label">new flow</span>
        </button>
        {groups.map(([name, entries]) => (
          <ul key={name} className="hl-nav__group" aria-label={name}>
            {entries.map((e) => (
              <li key={e.label}>
                <a href={format(e.to)} className="hl-nav__link" aria-current={e === current ? "page" : undefined} title={e.label} onClick={follow(e.to, navigate)}>
                  <span className="hl-nav__icon" aria-hidden="true">
                    {e.icon}
                  </span>
                  <span className="hl-nav__label">{e.label}</span>
                  {e.to.kind === "approvals" && waiting > 0 && (
                    <span className="hl-nav__badge" title={`${waiting} waiting for a person`}>
                      {waiting}
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
        <HostCard />
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
                    <span aria-current="location">{labelOf(r)}</span>
                  ) : (
                    <a href={format(r)} onClick={follow(r, navigate)}>
                      {labelOf(r)}
                    </a>
                  )}
                </li>
              ))}
            </ol>
          </nav>
          <button className="hl-top__find" onClick={onFind} title="the command palette: go anywhere, run a command, : for one by name">
            <span>Find or run…</span>
            <kbd aria-hidden="true">Ctrl K</kbd>
          </button>
          <span className="hl-top__ui" role="group" aria-label="interface" title="the interface: Terminal is mono, square, keys on show, with a status bar (t)">
            {(["clean", "terminal"] as const).map((u) => (
              <button key={u} aria-pressed={ui === u} onClick={() => onUi(u)}>
                {u === "clean" ? "Clean" : "Terminal"}
              </button>
            ))}
          </span>
          <button
            className="hl-top__theme"
            aria-label={`theme: ${theme}`}
            title="the look: auto follows the system; kept with your account"
            onClick={() => onTheme(THEMES[(THEMES.indexOf(theme) + 1) % THEMES.length])}
          >
            {theme === "dark" ? "☾" : theme === "light" ? "☀" : "◐"} {theme}
          </button>
          <AccountMenu me={me} style={{ font: "inherit", fontSize: 12, padding: "3px 10px", color: "var(--muted)", background: "transparent", border: "1px solid var(--border)", borderRadius: 3, cursor: "pointer" }} onWelcome={onWelcome} theme={theme} onTheme={onTheme} />
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
function StatusBar({ route, waiting }: { route: Route; waiting: number }) {
  const [summary, setSummary] = useState<Summary | null>(null);
  useEffect(() => {
    let on = true;
    const read = () =>
      getSummary(1).then(
        (s) => on && setSummary(s),
        () => undefined,
      );
    read();
    const every = setInterval(read, 15_000);
    return () => {
      on = false;
      clearInterval(every);
    };
  }, []);
  const newest = summary?.latest[0] && summary.deployments.find((d) => d.slug === summary.latest[0].slug);
  const head = newest?.head ?? summary?.deployments[0]?.head;
  const breaks = summary ? (summary.chain.checked ? summary.chain.checked.breaks.length : summary.chain.breaks) : 0;
  return (
    <footer className="hl-status" aria-label="status">
      <span>{summary ? `${summary.deployments.length} flow${summary.deployments.length === 1 ? "" : "s"} · ${summary.totals.running} live` : "…"}</span>
      <span className={waiting > 0 ? "hl-status__warn" : undefined}>waits: {waiting}</span>
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

/** Which host the console is on: its address and the version it runs, or that it can't be
 *  reached. Read once; the pages say what is wrong when a request fails. */
function HostCard() {
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
  return (
    <section className="hl-host" aria-label="host">
      <span className={`hl-host__dot hl-host__dot--${version === null ? "bad" : version ? "ok" : "wait"}`} aria-hidden="true" />
      <span className="hl-host__address">{hostAddress()}</span>
      <span className="hl-host__version">{version === undefined ? "…" : version === null ? "can't be reached" : `holon ${version}`}</span>
    </section>
  );
}
