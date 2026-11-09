// The Library (gaps-web.md G25): the manual's tutorials, the samples (one per feature, each
// opened as an unsaved copy, or made from the command line with `holon new --sample`), and the
// marketplace this browser publishes to and places from. Each tab is a place of its own.

import { useMemo, useState } from "react";
import { describeItem } from "./holon";
import { CAPABILITIES, capabilitiesOf, matches } from "./library";
import { contents } from "./manual";
import { setMarketplaceUrl, type Listing } from "./marketplace";
import { useDialog } from "./Dialog";
import { follow, format, type LibraryTab, type Route } from "./routes";
import type { Sample } from "./samples";

const TABS: [LibraryTab, string][] = [
  ["tutorials", "Tutorials"],
  ["samples", "Samples"],
  ["market", "Marketplace"],
];

export function LibraryPage({
  tab,
  navigate,
  samples,
  itemsOf,
  onOpenSample,
  textOfSample,
  marketplace,
}: {
  tab: LibraryTab;
  navigate: (r: Route) => void;
  /** Sample flows (holon-author's samples.rs). */
  samples: Sample[];
  /** What deploying a sample asks for (manifest items); throws why it can't be derived. */
  itemsOf: (sample: Sample) => string[];
  onOpenSample: (sample: Sample) => void;
  textOfSample: (sample: Sample) => string;
  /** The marketplace this browser uses ("" for none), told when it changes; its listings, every
   *  version, and yanking one (answers what happened). */
  marketplace: {
    url: string;
    onChange: () => void;
    listings: Listing[];
    adoption: (root: string, name: string) => number;
    onYank: (id: string) => Promise<string>;
  };
}) {
  return (
    <div className="hl-flow hl-library">
      <header className="hl-flow__head">
        <h1 className="hl-flow__name">Library</h1>
        <span className="hl-flow__status">tutorials, samples to start from, and flows others published</span>
      </header>
      <nav className="hl-flow__tabs" aria-label="library tabs">
        {TABS.map(([t, label]) => {
          const to: Route = { kind: "library", tab: t };
          return (
            <a key={t} href={format(to)} aria-current={t === tab ? "page" : undefined} onClick={follow(to, navigate)}>
              {label}
            </a>
          );
        })}
      </nav>
      {tab === "tutorials" ? (
        <Tutorials navigate={navigate} />
      ) : tab === "samples" ? (
        <Samples samples={samples} itemsOf={itemsOf} onOpenSample={onOpenSample} textOfSample={textOfSample} />
      ) : (
        <Marketplace {...marketplace} />
      )}
    </div>
  );
}

/** The manual's pages that teach (all but its reference), by section; each opens in Docs. */
function Tutorials({ navigate }: { navigate: (r: Route) => void }) {
  const sections = useMemo(() => contents().filter((s) => !/^(reference|conventions)$/i.test(s.title)), []);
  return (
    <section className="hl-library__tutorials" aria-label="tutorials">
      {sections.map((s) => (
        <div key={s.title} className="hl-flow__section">
          <h2>{s.title}</h2>
          <ul className="hl-library__pages">
            {s.pages.map((p) => {
              const to: Route = { kind: "docs", page: p.path, anchor: null };
              return (
                <li key={p.path}>
                  <a href={format(to)} onClick={follow(to, navigate)}>
                    {p.title}
                  </a>
                </li>
              );
            })}
          </ul>
        </div>
      ))}
    </section>
  );
}

function Samples({
  samples,
  itemsOf,
  onOpenSample,
  textOfSample,
}: {
  samples: Sample[];
  itemsOf: (sample: Sample) => string[];
  onOpenSample: (sample: Sample) => void;
  textOfSample: (sample: Sample) => string;
}) {
  const dialog = useDialog();
  const [search, setSearch] = useState("");
  const [picked, setPicked] = useState<string[]>([]);
  const [msg, setMsg] = useState("");
  // What each sample may do, derived once: the chips are read from it.
  const items = useMemo(() => {
    const out = new Map<string, string[] | null>();
    for (const s of samples) {
      try {
        out.set(s.name, itemsOf(s));
      } catch {
        out.set(s.name, null);
      }
    }
    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [samples]);
  const has = (s: Sample) => capabilitiesOf(items.get(s.name) ?? []);
  const shown = samples.filter((s) => matches(s, has(s), picked, search, (items.get(s.name) ?? []).map(describeItem)));
  const toggle = (key: string) => setPicked((p) => (p.includes(key) ? p.filter((k) => k !== key) : [...p, key]));
  const download = (s: Sample) => {
    try {
      const url = URL.createObjectURL(new Blob([textOfSample(s)], { type: "text/plain" }));
      const a = document.createElement("a");
      a.href = url;
      a.download = `${s.name}.holon.ts`;
      a.click();
      URL.revokeObjectURL(url);
    } catch (e) {
      setMsg(e instanceof Error ? e.message : String(e));
    }
  };
  const details = async (s: Sample) => {
    const may = items.get(s.name);
    const body = (
      <div className="hl-library__detail">
        <p>{s.description}</p>
        <dl className="hl-flow__facts">
          <div>
            <dt>shows</dt>
            <dd>{s.shows.join(" · ")}</dd>
          </div>
          <div>
            <dt>it may</dt>
            <dd>{may ? may.map(describeItem).join(" · ") : "?"}</dd>
          </div>
          {s.secrets && s.secrets.length > 0 && (
            <div>
              <dt>needs secrets</dt>
              <dd>{s.secrets.map((x) => (x.from ? `${x.name} (${x.from})` : x.name)).join(", ")}, set in its deploy settings</dd>
            </div>
          )}
          <div>
            <dt>try it</dt>
            <dd>{s.tryIt}</dd>
          </div>
        </dl>
        <p className="hl-flow__note">From the command line, the same flow as a file of your own:</p>
        <pre className="hl-library__cli">holon new --sample {s.name}</pre>
        <button className="hl-flow__button" onClick={() => download(s)}>
          Download {s.name}.holon.ts
        </button>
      </div>
    );
    if (await dialog.confirm(s.title, "Open an unsaved copy", body)) onOpenSample(s);
  };
  return (
    <section className="hl-library__samples" aria-label="samples">
      <div className="hl-flow__bar">
        <input className="hl-flow__filter" type="search" placeholder="Search samples by keyword" title="every word, in any order: in a sample's title, description, what it shows and may do" aria-label="search samples" value={search} onChange={(e) => setSearch(e.target.value)} />
        <span className="hl-flow__note">{shown.length === samples.length ? `${samples.length} samples` : `${shown.length} of ${samples.length}`}</span>
      </div>
      <div className="hl-chips" role="group" aria-label="show flows that">
        <span className="hl-flow__note">Show flows that</span>
        {CAPABILITIES.map((c) => (
          <button key={c.key} className="hl-chip hl-chip--pick" aria-pressed={picked.includes(c.key)} onClick={() => toggle(c.key)}>
            {c.label}
          </button>
        ))}
      </div>
      {msg && <div className="hl-flow__bad">{msg}</div>}
      <div className="hl-library__grid">
        {shown.map((s) => (
          <article key={s.name} className="hl-library__card" aria-label={s.title}>
            <h3>
              <button className="hl-library__title" onClick={() => void details(s)} title="what it is, what it may do, and how to make it from the command line">
                {s.title}
              </button>
            </h3>
            <p title={s.description}>{s.description}</p>
            <div className="hl-chips">
              {has(s).map((k) => (
                <span key={k} className="hl-chip">
                  {CAPABILITIES.find((c) => c.key === k)!.label}
                </span>
              ))}
            </div>
            <div className="hl-library__do">
              <button className="hl-flow__button hl-flow__button--go" aria-label={`open ${s.title}`} title={`open "${s.name}" as an unsaved copy`} onClick={() => onOpenSample(s)}>
                Open
              </button>
              <button className="hl-flow__button" title="download it as text (.holon.ts), as it is written: read it, change it anywhere, check it with `holon check`" onClick={() => download(s)}>
                .holon.ts
              </button>
            </div>
          </article>
        ))}
        {shown.length === 0 && <p className="hl-flow__none">No sample does all of that{search.trim() ? ` and matches "${search.trim()}"` : ""}.</p>}
      </div>
    </section>
  );
}

/** The marketplace this browser uses (a deployment of the Marketplace sample, marketplace.ts):
 *  set here, published to from Flows, placed from the palette; its listings, each yanked here. */
function Marketplace({
  url,
  onChange,
  listings,
  adoption,
  onYank,
}: {
  url: string;
  onChange: () => void;
  listings: Listing[];
  adoption: (root: string, name: string) => number;
  onYank: (id: string) => Promise<string>;
}) {
  const dialog = useDialog();
  const [msg, setMsg] = useState("");
  return (
    <section className="hl-library__market" aria-label="marketplace">
      <div className="hl-flow__bar">
        <span className="hl-flow__said">
          {url ? (
            <>
              publishing to and placing from <code>{url}</code>
            </>
          ) : (
            "none set: deploy the Marketplace sample (or use someone's), then set its address"
          )}
        </span>
        <button
          className="hl-flow__button"
          onClick={async () => {
            const next = await dialog.prompt("The marketplace's address (a Marketplace deployment, e.g. http://127.0.0.1:8120); empty for none:", url);
            if (next === null) return;
            setMarketplaceUrl(next);
            onChange();
          }}
        >
          {url ? "change" : "set"}
        </button>
      </div>
      {msg && <div className="hl-flow__said">{msg}</div>}
      <ul className="hl-library__listings">
        {listings.map((l) => (
          <li key={l.id}>
            <span className="hl-flow__what">{l.name}</span>
            <span className="hl-flow__note" title={l.root}>
              {l.root.slice(0, 8)} · by {l.publisher} · {new Date(l.published_at * 1000).toLocaleString()}
              {adoption(l.root, l.name) > 0 && ` · placed in ${adoption(l.root, l.name)} other(s)`}
            </span>
            <span className="hl-flow__spacer" />
            {l.yanked ? (
              <span className="hl-flow__bad">yanked</span>
            ) : (
              <button className="hl-flow__button" onClick={() => onYank(l.id).then((m) => m && setMsg(m))} title="flag this version as yanked (it stays listed, never edited)">
                yank
              </button>
            )}
          </li>
        ))}
      </ul>
      {url && listings.length === 0 && <p className="hl-flow__none">Nothing published there yet: publish a saved flow from Flows.</p>}
    </section>
  );
}
