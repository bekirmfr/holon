// The console's frame (gaps-web.md §3, G4): the side nav with every page and its key, the host
// card under it, and the top bar over the page: where you are (the crumbs), the command palette,
// the look and who is signed in. The pages themselves are App's; this only goes to them.

import { useEffect, useState, type ReactNode } from "react";
import { AccountMenu, type Me } from "../AuthGate";
import { THEMES, type Theme } from "../look";
import { pageOf, type AppPage } from "../Pages";
import { crumbs, follow, format, labelOf, type Route } from "../routes";
import { getVersion, hostAddress } from "../server";

type Entry = { label: string; to: Route; key?: number };

/** The nav's groups, in order: the work, the host's own pages (an admin's), and help. The
 *  numbers are keymap.ts's PAGE_KEYS. */
const WORK: Entry[] = [
  { label: "Overview", to: { kind: "overview" }, key: 1 },
  { label: "Approvals", to: { kind: "approvals", tab: "deploys" }, key: 2 },
  { label: "Flows", to: { kind: "flows" }, key: 3 },
  { label: "Editor", to: { kind: "editor", slug: null, view: false, seq: null }, key: 4 },
  { label: "Library", to: { kind: "library", tab: "samples" }, key: 5 },
];
const HOST: Entry[] = [
  { label: "Resources", to: { kind: "resources", tab: "databases" }, key: 6 },
  { label: "People and access", to: { kind: "people", tab: null }, key: 7 },
  { label: "Policy", to: { kind: "policy" }, key: 8 },
];
const HELP: Entry[] = [
  { label: "Getting started", to: { kind: "start" } },
  { label: "Docs", to: { kind: "docs", page: null, anchor: null } },
];


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
  /** How many things wait for a person (approvals.ts): Approvals' count. */
  waiting?: number;
  children: ReactNode;
}) {
  const groups: [string, Entry[]][] = [["work", WORK], ...(me.role === "admin" ? ([["host", HOST]] as [string, Entry[]][]) : []), ["help", HELP]];
  // A flow's page is one of the flows'.
  const at = page === "flow" ? "flows" : page;
  const current = [...WORK, ...HOST, ...HELP].find((e) => pageOf(e.to) === at);
  const trail = crumbs(route);
  return (
    <div className="hl-shell">
      <nav className="hl-nav" aria-label="main">
        <div className="hl-nav__brand">
          holon <span className="hl-nav__mode">console</span>
        </div>
        <button className="hl-nav__new" onClick={onNew}>
          new flow
        </button>
        {groups.map(([name, entries]) => (
          <ul key={name} className="hl-nav__group" aria-label={name}>
            {entries.map((e) => (
              <li key={e.label}>
                <a href={format(e.to)} className="hl-nav__link" aria-current={e === current ? "page" : undefined} onClick={follow(e.to, navigate)}>
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
      </div>
    </div>
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
