// A flow's Settings tab (FlowPage.tsx): what it is granted and the host's controls over it
// (revoke, grant back, lower a limit, keep running), its secrets, the settings it was deployed
// with, whether agents may propose its versions, its waiting runs, its sessions and what it holds,
// and removing it.

import { useEffect, useRef, useState } from "react";
import { SecretRow } from "./DeployBar";
import { approvalSaid, isApproval } from "./decisions";
import { summarizeState } from "./diagnostics";
import { describeItem, permissionKind } from "./holon";
import { KindBadge } from "./Manifest";
import { holdsLines } from "./storage";
import {
  changeGrant,
  getEnvSecretNames,
  getGrant,
  getHolds,
  getManifest,
  getPermissions,
  getSessions,
  listSecrets,
  listWaitingRuns,
  setAgentsPropose,
  type DeploymentEntry,
  type GrantView,
  type Holds,
  type ManifestReply,
  type Role,
  type SecretInfo,
  type SessionEntry,
  type WaitingRun,
} from "./server";

const when = (secs: number) => new Date(secs * 1000).toLocaleString();
const say = (e: unknown) => (e instanceof Error ? e.message : String(e));

export function SettingsTab({
  slug,
  entry,
  role,
  focus,
  onChanged,
  onRunFrom,
  onRemove,
}: {
  slug: string;
  entry: DeploymentEntry;
  role: Role;
  /** Asked for from the admin page: each new value brings the permissions into view. */
  focus?: number;
  /** Its grant changed here: the table should be fetched again. */
  onChanged: () => void;
  /** Run a new session from a stopped one's state. */
  onRunFrom: (session: number) => Promise<void>;
  /** Remove it, every session and its state. */
  onRemove: () => Promise<void>;
}) {
  const version = `${entry.unit} ${entry.status} ${entry.attention ?? ""}`;
  const [manifest, setManifest] = useState<ManifestReply | null>(null);
  useEffect(() => {
    getManifest(slug).then(setManifest, () => setManifest(null));
  }, [slug, version]);
  const secretNames = (manifest?.capabilities ?? []).flatMap((c) => {
    const m = /^SecretRead\("(.*)"\)$/.exec(c);
    return m ? [m[1]] : [];
  });
  return (
    <div className="hl-flow__settings">
      <Permissions slug={slug} version={version} manifest={manifest} focus={focus} onChanged={onChanged} />
      {secretNames.length > 0 && <Secrets slug={slug} names={secretNames} admin={role === "admin"} />}
      <DeploySettings manifest={manifest} />
      <Agents slug={slug} entry={entry} viewer={role === "viewer"} onChanged={onChanged} />
      {(entry.waiting ?? 0) > 0 && <Waiting slug={slug} count={entry.waiting ?? 0} />}
      <Sessions entry={entry} onRunFrom={onRunFrom} />
      <Remove slug={slug} stopped={entry.status === "stopped"} onRemove={onRemove} />
    </div>
  );
}

/** Whether agents may propose its versions (proposals.rs): off, an agent's deploy of it is
 *  refused, neither proposed nor saved. */
function Agents({ slug, entry, viewer, onChanged }: { slug: string; entry: DeploymentEntry; viewer: boolean; onChanged: () => void }) {
  const [on, setOn] = useState(entry.agents_propose !== false);
  const [msg, setMsg] = useState("");
  // Whether the host takes agents' proposals at all (its policy's `[agents] propose`).
  const [hostTakes, setHostTakes] = useState(true);
  useEffect(() => {
    let live = true;
    getPermissions().then(
      (p) => live && setHostTakes(p.agents_propose !== false),
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, []);
  useEffect(() => setOn(entry.agents_propose !== false), [entry.agents_propose]);
  const change = (next: boolean) => {
    // Shown at once; back as it was if the server refuses.
    setMsg("");
    setOn(next);
    setAgentsPropose(slug, next).then(onChanged, (e: unknown) => {
      setOn(!next);
      setMsg(say(e));
    });
  };
  return (
    <section className="hl-flow__section" aria-label="agents">
      <h2>Agents</h2>
      {msg && <div className="hl-flow__bad">{msg}</div>}
      <label className="hl-flow__check">
        <input type="checkbox" checked={on} disabled={viewer} onChange={(e) => change(e.target.checked)} />
        agents may propose versions
      </label>
      <p className="hl-flow__note">
        {on ? "An agent's version that asks for nothing new deploys at once; one that does waits in Approvals, for a person." : "An agent's deploy of it is refused."} Which actions agents may run is among the settings it was deployed with.
      </p>
      {on && !hostTakes && (
        <p className="hl-flow__note" role="note">
          This host takes no proposals from agents (its policy's agents.propose = false): an agent's version that asks for something new is refused, whatever this says.
        </p>
      )}
    </section>
  );
}

/** What it is granted, each item with revoke; what was taken back, with grant back; its limits,
 *  each lowered without a redeploy; whether an uncovered grant pauses it; and the last changes. */
function Permissions({ slug, version, manifest, focus, onChanged }: { slug: string; version: string; manifest: ManifestReply | null; focus?: number; onChanged: () => void }) {
  const [view, setView] = useState<GrantView | null>(null);
  const [msg, setMsg] = useState("");
  const [lowerTo, setLowerTo] = useState<Record<string, string>>({});
  const here = useRef<HTMLElement>(null);
  useEffect(() => {
    getGrant(slug).then(setView, (e: unknown) => setMsg(say(e)));
  }, [slug, version]);
  useEffect(() => {
    if (focus !== undefined) here.current?.scrollIntoView({ block: "start" });
  }, [focus]);
  const change = (c: Parameters<typeof changeGrant>[1]) => {
    setMsg("");
    changeGrant(slug, c)
      .then((v) => {
        setView(v);
        onChanged();
      })
      .catch((e: unknown) => setMsg(say(e)));
  };
  const grant = view?.grant;
  const optional = manifest?.optional ?? [];
  const trigger = manifest?.trigger ?? null;
  const revokeTitle = (item: string) =>
    permissionKind(item, optional, trigger).kind === "optional"
      ? "take this back without redeploying: it keeps running, these calls refused"
      : grant?.keep_running
        ? "take this back without redeploying: it keeps running, these calls failing"
        : "take this back without redeploying: it pauses until this is granted back";
  const limits = Object.entries(grant?.requirements.limits ?? {});
  return (
    <section ref={here} className="hl-flow__section" aria-label="permissions">
      <h2>Permissions</h2>
      {msg && <div className="hl-flow__bad">{msg}</div>}
      {view?.attention && <div className="hl-flow__banner hl-flow__bad">⚠ {view.attention}</div>}
      {grant && (
        <>
          <ul className="hl-flow__grant">
            {grant.items.map((item) => (
              <li key={item} title={item}>
                <span className="hl-flow__what">✓ {describeItem(item)}</span>
                {!item.startsWith("Limit(") && <KindBadge item={item} optional={optional} trigger={trigger} />}
                <span className="hl-flow__spacer" />
                <button className="hl-flow__button" onClick={() => change({ revoke: [item] })} title={revokeTitle(item)}>
                  Revoke
                </button>
              </li>
            ))}
            {(grant.revoked ?? []).map((item) => (
              <li key={item} title={item} className="hl-flow__revoked">
                <span>{describeItem(item)}</span>
                <span className="hl-table__dim"> · revoked</span>
                <span className="hl-flow__spacer" />
                <button className="hl-flow__button" onClick={() => change({ grant: [item] })} title="grant it back">
                  Grant back
                </button>
              </li>
            ))}
          </ul>
          {limits.length > 0 && (
            <table className="hl-table" aria-label="limits">
              <thead>
                <tr>
                  <th>Limit</th>
                  <th>Granted</th>
                  <th>Lower to</th>
                </tr>
              </thead>
              <tbody>
                {limits.map(([name, n]) => {
                  const lowered = grant.lowered?.[name];
                  const typed = lowerTo[name] ?? "";
                  const to = Number(typed);
                  const ok = typed !== "" && Number.isFinite(to) && to >= 0 && to < (lowered ?? n);
                  return (
                    <tr key={name}>
                      <td className="hl-flow__what">{name}</td>
                      <td>
                        {n}
                        {lowered !== undefined && <span className="hl-flow__waiting"> · lowered to {lowered}</span>}
                      </td>
                      <td>
                        <input className="hl-flow__filter hl-flow__number" inputMode="numeric" aria-label={`lower ${name} to`} value={typed} onChange={(e) => setLowerTo({ ...lowerTo, [name]: e.target.value })} />{" "}
                        <button className="hl-flow__button" disabled={!ok} onClick={() => change({ lower: { [name]: to } })} title="lower it without redeploying; a deploy can raise it again">
                          Lower
                        </button>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          )}
          <label className="hl-flow__check" title="when the grant doesn't cover what it needs">
            <input type="checkbox" checked={grant.keep_running ?? false} onChange={(e) => change({ keep_running: e.target.checked })} />
            Keep running when something required is revoked (those calls fail) instead of pausing
          </label>
          {(grant.changes ?? []).length > 0 && (
            <ul className="hl-flow__changes">
              {(grant.changes ?? []).slice(-5).reverse().map((c, i) => (
                <li key={i}>
                  {when(c.at)} · {c.change}
                </li>
              ))}
            </ul>
          )}
          <p className="hl-flow__note">
            Granted {when(grant.granted_at)} by {grant.by}.
          </p>
        </>
      )}
    </section>
  );
}

/** The secrets it reads: each one's version and where it comes from, set anew, its history, and
 *  rolled back to an older version. Values are never shown. */
function Secrets({ slug, names, admin }: { slug: string; names: string[]; admin: boolean }) {
  const [infos, setInfos] = useState<SecretInfo[]>([]);
  const [envNames, setEnvNames] = useState<string[]>([]);
  const reload = () => listSecrets(slug).then(setInfos, () => setInfos([]));
  useEffect(() => {
    void reload();
    if (admin) getEnvSecretNames().then((r) => setEnvNames(r.names), () => setEnvNames([]));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [slug, admin]);
  return (
    <section className="hl-flow__section" aria-label="secrets">
      <h2>Secrets</h2>
      <div className="hl-flow__secrets">
        {names.map((name) => (
          <SecretRow key={name} slug={slug} name={name} info={infos.find((x) => x.name === name)} envNames={envNames} onChanged={() => void reload()} />
        ))}
      </div>
    </section>
  );
}

/** The settings it was deployed with, beyond the defaults: changed in the editor, by a deploy. */
function DeploySettings({ manifest }: { manifest: ManifestReply | null }) {
  const r = manifest?.requirements;
  const rows: [string, string][] = [];
  if (r) {
    for (const [name, n] of Object.entries(r.limits ?? {})) rows.push([name, String(n)]);
    if ((r.plain_http ?? []).length > 0) rows.push(["plain http to", (r.plain_http ?? []).join(", ")]);
    if (r.private_addresses) rows.push(["private addresses", "may reach them"]);
    if ((r.optional ?? []).length > 0) rows.push(["runs without", (r.optional ?? []).map(describeItem).join(", ")]);
    rows.push(["agents may run", (r.agents ?? []).length > 0 ? (r.agents ?? []).join(", ") : "none of its actions"]);
  }
  return (
    <section className="hl-flow__section" aria-label="deploy settings">
      <h2>Deploy settings</h2>
      {rows.length === 0 ? (
        <p className="hl-flow__note">the defaults</p>
      ) : (
        <dl className="hl-flow__facts">
          {rows.map(([name, value]) => (
            <div key={name}>
              <dt>{name}</dt>
              <dd>{value}</dd>
            </div>
          ))}
        </dl>
      )}
      <p className="hl-flow__note">Changed in the editor (⚙), by deploying it again.</p>
    </section>
  );
}

/** Its runs waiting for an approval or an event: what each waits for and until when, and what was delivered
 *  to it so far. Never their tokens (the server doesn't list them). */
function Waiting({ slug, count }: { slug: string; count: number }) {
  const [runs, setRuns] = useState<WaitingRun[] | null>(null);
  useEffect(() => {
    listWaitingRuns(slug).then(setRuns, () => setRuns([]));
  }, [slug, count]);
  return (
    <section className="hl-flow__section" aria-label="waiting runs">
      <h2>Waiting runs</h2>
      <ul className="hl-flow__changes">
        {(runs ?? []).map((r) => (
          <li key={r.run_id} title={`run ${r.run_id}`}>
            <span className="hl-flow__what">{r.route ?? r.kind}</span> · since {when(r.parked_at)} ·{" "}
            {r.waiting.map((w) => `waits for ${isApproval(w) ? approvalSaid(w) : `"${w.name}"`} until ${when(w.expires_at)}`).join("; ")}
            {r.delivered.length > 0 && <span> · delivered: {r.delivered.join(", ")}</span>}
          </li>
        ))}
      </ul>
    </section>
  );
}

/** Its run sessions, newest first, each with the state it ended with; a stopped flow runs again
 *  from one of them. And what it holds, in memory and on disk. */
function Sessions({ entry, onRunFrom }: { entry: DeploymentEntry; onRunFrom: (session: number) => Promise<void> }) {
  const [sessions, setSessions] = useState<SessionEntry[] | null>(null);
  const [holds, setHolds] = useState<Holds | null>(null);
  const [msg, setMsg] = useState("");
  useEffect(() => {
    getSessions(entry.slug).then((s) => setSessions(s.sessions), (e: unknown) => setMsg(say(e)));
    getHolds(entry.slug).then(setHolds, () => setHolds(null));
  }, [entry.slug, entry.status, entry.session, entry.trigger_count]);
  const stopped = entry.status === "stopped";
  const time = (secs: number) => new Date(secs * 1000).toLocaleTimeString();
  return (
    <section className="hl-flow__section" aria-label="sessions">
      <h2>Sessions</h2>
      {msg && <div className="hl-flow__bad">{msg}</div>}
      <table className="hl-table">
        <tbody>
          {(sessions ?? []).slice().reverse().map((s) => (
            <tr key={s.number}>
              <td className="hl-flow__what">#{s.number}</td>
              <td className="hl-table__dim">
                {time(s.started_at)} → {s.ended_at === null ? "now" : time(s.ended_at)}
              </td>
              <td>
                {s.requests} runs · {s.errors} failed
              </td>
              <td className="hl-table__dim" title={`started from ${s.from}`}>
                {s.ended_at === null ? `from ${s.from}` : summarizeState(s.state)}
              </td>
              <td className="hl-flow__cell-end">
                {stopped && s.state !== null && (
                  <button className="hl-flow__button" onClick={() => onRunFrom(s.number).catch((e: unknown) => setMsg(say(e)))} title="run a new session from this one's state">
                    Run from it
                  </button>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {holds && (
        <ul className="hl-flow__changes" aria-label="holds">
          {holdsLines(holds, Date.now()).map((l) => (
            <li key={l}>{l}</li>
          ))}
        </ul>
      )}
    </section>
  );
}

/** Removing it: every session and its state go, which can't be undone; asked by typing its
 *  name, once it is stopped. */
function Remove({ slug, stopped, onRemove }: { slug: string; stopped: boolean; onRemove: () => Promise<void> }) {
  const [typed, setTyped] = useState("");
  const [msg, setMsg] = useState("");
  return (
    <section className="hl-flow__section hl-flow__danger" aria-label="remove">
      <h2>Remove</h2>
      <p className="hl-flow__note">
        Removes the deployment, every session and its state. It can't be undone; its saved flow stays.{stopped ? "" : " Stop it first."}
      </p>
      <div className="hl-flow__bar">
        <input className="hl-flow__filter" aria-label={`type ${slug} to remove it`} placeholder={`type ${slug}`} value={typed} disabled={!stopped} onChange={(e) => setTyped(e.target.value)} />
        <button className="hl-flow__button hl-flow__button--danger" disabled={!stopped || typed !== slug} onClick={() => onRemove().catch((e: unknown) => setMsg(say(e)))}>
          Remove {slug}
        </button>
      </div>
      {msg && <div className="hl-flow__bad">{msg}</div>}
    </section>
  );
}
