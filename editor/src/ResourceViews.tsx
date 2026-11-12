// Resources' views (web-console-L4.md §9.1), beside HostPages.tsx's tabs and their bind forms:
// what uses a resource, each deployment and what it does with it, and in amber what a version
// waiting for a person would add, read from what it asks to be approved; a database's matrix of
// tables and flows; each flow's collections and channels; and the server's secrets as a table.
// A secret's value is never shown: its mark (set · v3), where it comes from, and its history.

import { Fragment, useEffect, useState } from "react";
import { channelCounts, getCollectionItems, getDeploymentState, getSecretHistory, previewSecretRollback, rollbackSecret, type ChannelCount, type DeploymentEntry, type HeldGrant, type Proposal, type SecretHistory as History, type SecretInfo } from "./server";
import { gateOf, type Answer } from "./useCan";
import { itemParts, secretReaders, usesText, type Holder, type Matrix, type Waiting } from "./holdings";
import { useDryRun } from "./DryRun";
import { secretRollbackReport, sourceLabel } from "./DeploySettings";
import { format } from "./routes";
import { versionName } from "./policyMeans";
import { quiet, small } from "./hostStyles";

const messageOf = (e: unknown) => (e instanceof Error ? e.message : String(e));
const when = (epoch: number) => new Date(epoch * 1000).toLocaleString([], { dateStyle: "short", timeStyle: "short" });
const flowLink = (slug: string, tab: "settings" | "state" | "status" = "settings") => format({ kind: "flow", slug, tab });

/** Who uses a resource, said under its row: each deployment and what it does with it (a link to
 *  its Settings, where its grant changes), then each waiting version and what it would add, in
 *  amber. `known`: whether the grants could be read at all (they are the host's to view). */
export function UsedBy({ holders, waiting, known = true }: { holders: Holder[]; waiting: Waiting[]; known?: boolean }) {
  if (!known) return <div className="hl-used-by" style={quiet}>who uses it is shown to those who may view the host</div>;
  if (holders.length === 0 && waiting.length === 0) return <div className="hl-used-by" style={quiet}>no deployment uses it</div>;
  return (
    <div className="hl-used-by" aria-label="used by">
      {holders.map((h) => (
        <span key={h.slug} className="hl-used-by__one">
          <a href={flowLink(h.slug)} title={`open ${h.slug}'s settings, where its grant changes`}>
            {h.slug}
          </a>{" "}
          {usesText(h.uses).map((u, i) => (
            <Fragment key={i}>
              {i > 0 && ", "}
              <span className={u.revoked ? "hl-used-by__revoked" : undefined} title={u.revoked ? "revoked" : undefined}>
                {u.text}
              </span>
            </Fragment>
          ))}
        </span>
      ))}
      {waiting.map((w) => (
        <span key={`waiting:${w.slug}`} className="hl-used-by__one hl-used-by__waiting" title="a version waiting for a person: what it would add, once approved">
          <a href={format({ kind: "approvals", tab: "deploys" })}>{versionName(w)}</a> would {usesText(w.uses).map((u) => u.text.replace(/^(\w+?)s\b/, "$1")).join(", ")}
        </span>
      ))}
    </div>
  );
}

/** A database's matrix: its tables down, the flows that use it across, read, write or both in each
 *  cell; amber what a waiting version would add. It says who touches which table, in place of who
 *  may touch what for databases. */
export function DatabaseMatrix({ name, matrix, known = true }: { name: string; matrix: Matrix; known?: boolean }) {
  if (!known) return <div className="hl-used-by" style={quiet}>who uses it is shown to those who may view the host</div>;
  if (matrix.flows.length === 0) return <div className="hl-used-by" style={quiet}>no deployment uses it</div>;
  return (
    <div className="hl-matrix-wrap">
      <table className="hl-table hl-matrix" aria-label={`${name}'s tables and the flows that use them`}>
        <thead>
          <tr>
            <th>table</th>
            {matrix.flows.map((f) => (
              <th key={f.slug} className={f.waiting ? "hl-matrix__waiting" : undefined}>
                <a href={flowLink(f.slug)} title={`open ${f.slug}'s settings`}>
                  {f.slug}
                </a>
                {f.version !== null && <span className="hl-matrix__version"> v{f.version} waits</span>}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {matrix.rows.map((r) => (
            <tr key={r.table ?? ""}>
              <td>{r.table ?? <span className="hl-table__dim">every table</span>}</td>
              {r.cells.map((c, i) => {
                const now = c.now.join(" and ");
                const would = c.waiting.join(" and ");
                const said = c.revoked ? "revoked" : [now, would && `${now ? "+ " : ""}${would} in v${matrix.flows[i].version ?? "next"}`].filter(Boolean).join(" ");
                return (
                  <td key={matrix.flows[i].slug} className={c.waiting.length > 0 ? "hl-matrix__waiting" : c.revoked ? "hl-used-by__revoked" : undefined}>
                    {said || <span className="hl-table__dim">—</span>}
                  </td>
                );
              })}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** One collection or channel a flow keeps, and how many items it holds (a collection's, counted to
 *  `COUNTED` at most; a channel's, the packs it keeps for subscribers, with how many subscribe). */
type Kept = { slug: string; name: string; kind: "collection" | "channel"; items: number | null; more: boolean; subscribers?: number };
const COUNTED = 1000;
const named = (items: string[], kinds: string[]) => items.flatMap((i) => {
  const { kind, arg } = itemParts(i);
  return kinds.includes(kind) && arg !== null ? [arg] : [];
});

/** What an items cell says on hover: a channel's are the packs it keeps for whoever subscribes. */
function keptSaid(k: Kept): string | undefined {
  if (k.kind === "collection") return k.more ? `counted to ${COUNTED}` : undefined;
  if (k.items === null) return "its state isn't yours to view";
  return `the newest packs it keeps for whoever subscribes; ${k.subscribers ?? 0} subscriber${k.subscribers === 1 ? "" : "s"} now`;
}

/** Each flow's collections and channels: from its state (a collection that holds anything), its
 *  channels' counts and its grant (one it names, empty so far), each with how many items it holds
 *  and a link to the flow's State. What the caller may not view of a flow's state isn't counted. */
export function CollectionsTab({ deployments, grants }: { deployments: DeploymentEntry[]; grants: HeldGrant[] }) {
  const [kept, setKept] = useState<Kept[] | null>(null);
  const [said, setSaid] = useState("");
  const key = deployments.map((d) => `${d.slug}@${d.unit}`).join(",") + `|${grants.length}`;
  useEffect(() => {
    let live = true;
    (async () => {
      const all: Kept[] = [];
      for (const d of deployments) {
        const items = grants.find((g) => g.slug === d.slug)?.items ?? [];
        let state: Record<string, unknown> = {};
        if (d.has_state) {
          try {
            state = await getDeploymentState(d.slug);
          } catch {
            // Not the caller's to view, or it keeps none.
          }
        }
        const held = Object.keys(state).filter((k) => k.startsWith("#collection:")).map((k) => k.slice("#collection:".length));
        for (const name of [...new Set([...held, ...named(items, ["CollectionRead", "CollectionWrite"])])].sort()) {
          let n: number | null = 0;
          let more = false;
          if (held.includes(name)) {
            try {
              const page = await getCollectionItems(d.slug, name, { limit: COUNTED });
              n = page.items.length;
              more = !!page.next;
            } catch {
              n = null;
            }
          }
          all.push({ slug: d.slug, name, kind: "collection", items: n, more });
        }
        let counts: ChannelCount[] | null = null;
        try {
          counts = await channelCounts(d.slug);
        } catch {
          // Not the caller's to view.
        }
        for (const name of [...new Set([...(counts ?? []).map((c) => c.name), ...named(items, ["ChannelSend", "ChannelRead"])])].sort()) {
          const c = counts?.find((c) => c.name === name);
          all.push({ slug: d.slug, name, kind: "channel", items: counts === null ? null : (c?.kept ?? 0), more: false, subscribers: c?.subscribers ?? 0 });
        }
      }
      if (live) setKept(all);
    })().catch((e: unknown) => live && setSaid(messageOf(e)));
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  if (said) return <div style={{ ...quiet, color: "var(--bad)" }}>{said}</div>;
  if (kept === null) return <div style={quiet}>reading each flow&apos;s state…</div>;
  if (kept.length === 0) return <div style={quiet}>no flow keeps a collection or sends on a channel</div>;
  return (
    <div className="hl-matrix-wrap">
      <table className="hl-table" aria-label="collections and channels">
        <thead>
          <tr>
            <th>name</th>
            <th>kind</th>
            <th className="hl-table__num">items</th>
            <th>kept by</th>
          </tr>
        </thead>
        <tbody>
          {kept.map((k) => (
            <tr key={`${k.slug}/${k.kind}/${k.name}`} aria-label={`${k.kind} ${k.name} of ${k.slug}`}>
              <td>{k.name}</td>
              <td>{k.kind}</td>
              <td className="hl-table__num" title={keptSaid(k)}>
                {k.items === null ? <span className="hl-table__dim">—</span> : `${k.items}${k.more ? "+" : ""}`}
              </td>
              <td>
                <a href={flowLink(k.slug, "state")} title={`open ${k.slug}'s state`}>
                  {k.slug}
                </a>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** Who set a secret's current version, from its history: by name, when the server knows. */
function useSetBy(secrets: SecretInfo[]): Map<string, string> {
  const [by, setBy] = useState<Map<string, string>>(new Map());
  const key = secrets.map((s) => `${s.name}@${s.version}`).join(",");
  useEffect(() => {
    let live = true;
    Promise.all(
      secrets.map((s) =>
        getSecretHistory(null, s.name).then(
          (h) => [s.name, [...h.events].reverse().find((e) => e.version === s.version && e.by)?.by] as const,
          () => [s.name, undefined] as const,
        ),
      ),
    ).then((all) => live && setBy(new Map(all.filter((e): e is readonly [string, string] => !!e[1]))));
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  return by;
}

/** Where a server secret may go, said as the table says it. */
const goesTo = (to: string[] | undefined) =>
  to ? (to.length ? to.map((d) => (d.startsWith("db:") ? `the database "${d.slice(3)}"` : `https://${d}`)).join(", ") : "nowhere") : "wherever its flows' approvals show";

/** The server's secrets as a table: name, its mark (never its value), where its value comes from,
 *  when it was set and by whom, where it may go, which flows read it (and which waiting versions
 *  would), and Set a new value, where…, History and ×. History opens under its row. */
export function SecretsTable({
  secrets,
  grants,
  known,
  proposals,
  bind,
  unbind,
  onSetNew,
  onWhere,
  onRemove,
  onChanged,
}: {
  secrets: SecretInfo[];
  grants: HeldGrant[];
  known: boolean;
  proposals: Proposal[];
  bind: Answer;
  unbind: Answer;
  onSetNew: (s: SecretInfo) => void;
  onWhere: (s: SecretInfo) => void;
  onRemove: (s: SecretInfo) => void;
  onChanged: () => void;
}) {
  const [open, setOpen] = useState<string | null>(null);
  const by = useSetBy(secrets);
  if (secrets.length === 0) return <div style={{ color: "var(--faint)", fontSize: 11, marginBottom: 6 }}>no secrets bound</div>;
  return (
    <div className="hl-matrix-wrap">
      <table className="hl-table hl-secrets" aria-label="the server's secrets">
        <thead>
          <tr>
            <th>secret</th>
            <th>value</th>
            <th>from</th>
            <th>set</th>
            <th>may go to</th>
            <th>read by</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {secrets.map((s) => {
            const readers = secretReaders(s.name, grants, proposals);
            return (
              <Fragment key={s.name}>
                <tr aria-label={`server secret ${s.name}`}>
                  <td>{s.name}</td>
                  <td>
                    <span className="hl-secret-mark" title="a secret's value is never shown: which version is set">
                      set · v{s.version}
                    </span>
                    {s.problem && <div style={{ ...quiet, color: "var(--bad)" }}>can&apos;t be read: {s.problem}</div>}
                  </td>
                  <td className="hl-secrets__from">{sourceLabel(s.source)}</td>
                  <td>
                    {when(s.set_at)}
                    {by.get(s.name) && <span className="hl-table__dim"> · {by.get(s.name)}</span>}
                  </td>
                  <td className="hl-secrets__wrap">{goesTo(s.to)}</td>
                  <td className="hl-secrets__wrap">
                    {!known ? (
                      <span className="hl-table__dim" title="who reads it is shown to those who may view the host">—</span>
                    ) : (
                      <>
                        {readers.now.map((slug, i) => (
                          <Fragment key={slug}>
                            {i > 0 && ", "}
                            <a href={flowLink(slug)}>{slug}</a>
                          </Fragment>
                        ))}
                        {readers.waiting.map((w) => (
                          <div key={w.slug} className="hl-used-by__waiting">
                            {versionName(w)} would read it
                          </div>
                        ))}
                        {readers.now.length === 0 && readers.waiting.length === 0 && <span className="hl-table__dim">no flow</span>}
                      </>
                    )}
                  </td>
                  <td className="hl-secrets__do">
                    <button style={small} {...gateOf(bind, { title: "set a new version: the old ones are kept, to roll back to" })} onClick={() => onSetNew(s)}>
                      Set a new value
                    </button>
                    <button style={small} {...gateOf(bind, { title: "change where it may go" })} onClick={() => onWhere(s)}>
                      where…
                    </button>
                    <button style={small} aria-expanded={open === s.name} aria-label={`versions of ${s.name}`} title="its versions, and what was done to it, by whom" onClick={() => setOpen(open === s.name ? null : s.name)}>
                      History
                    </button>
                    <button style={small} aria-label={`remove ${s.name}`} {...gateOf(unbind, { title: "remove the server's secret, every version" })} onClick={() => onRemove(s)}>
                      ×
                    </button>
                  </td>
                </tr>
                {open === s.name && (
                  <tr className="hl-flow__history-row">
                    <td colSpan={7}>
                      <SecretHistory name={s.name} onChanged={onChanged} may={bind} />
                    </td>
                  </tr>
                )}
              </Fragment>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

/** A server secret's versions and changes; an older version is made current again after a dry
 *  run, as a deployment's is (DeploySettings.tsx's SecretRow). Never a value: versions are told
 *  apart by their fingerprints. */
function SecretHistory({ name, onChanged, may }: { name: string; onChanged: () => void; may: Answer }) {
  const [history, setHistory] = useState<History | null>(null);
  const [said, setSaid] = useState("");
  const dryRun = useDryRun();
  const load = () => getSecretHistory(null, name).then(setHistory).catch((e: unknown) => setSaid(messageOf(e)));
  useEffect(() => {
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [name]);
  const rollback = async (to: number) => {
    try {
      if (!(await dryRun.ask(secretRollbackReport(null, name, await previewSecretRollback(null, name, to))))) return;
      const v = await rollbackSecret(null, name, to);
      setSaid(`version ${to} is current again, as version ${v}`);
      onChanged();
      void load();
    } catch (e) {
      setSaid(messageOf(e));
    }
  };
  return (
    <div style={{ fontSize: 11 }}>
      {said && <div style={quiet}>{said}</div>}
      {history && (
        <div style={{ display: "grid", gap: 3, padding: "4px 8px", borderLeft: "2px solid var(--border)", marginTop: 4 }}>
          {history.versions.map((v) => (
            <div key={v.version} style={{ display: "flex", gap: 8, alignItems: "center" }}>
              <span style={{ color: v.current ? "var(--ok)" : "var(--muted)", minWidth: 24 }}>v{v.version}</span>
              <span style={{ flex: 1 }}>{sourceLabel(v.source)}</span>
              <span style={quiet}>{when(v.set_at)}</span>
              {v.current ? (
                <span style={{ color: "var(--ok)", width: 64, textAlign: "right" }}>current</span>
              ) : (
                <button style={{ ...small, width: 64 }} onClick={() => void rollback(v.version)} {...gateOf(may, { title: `make version ${v.version} current again` })}>
                  roll back
                </button>
              )}
            </div>
          ))}
          {history.events
            .slice()
            .reverse()
            .map((e, i) => (
              <div key={i} style={quiet}>
                {when(e.at)} · {e.action}
                {e.version ? ` v${e.version}` : ""}
                {e.source ? ` · ${sourceLabel(e.source)}` : ""}
                {e.detail ? ` · ${e.detail}` : ""}
                {e.by ? ` · by ${e.by}` : ""}
              </div>
            ))}
        </div>
      )}
      {dryRun.modal}
    </div>
  );
}
