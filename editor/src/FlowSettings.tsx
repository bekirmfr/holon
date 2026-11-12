// A flow's Settings tab (FlowPage.tsx, web-console-L4.md §8.4): every setting of a deployed flow
// in one place, changed in place where a change narrows it. What it is granted (revoke, grant
// back, keep running), its secrets, its limits and the runs it keeps (lowered at once; raised by a
// new version, which a person approves), its network, whether agents may propose its versions,
// its waiting runs, its sessions and what it holds; and its lifecycle: pause or resume, stop, and
// remove. Each control follows `can` on the flow (FlowPage.tsx asks it for them all).

import { useEffect, useRef, useState } from "react";
import { SecretRow } from "./DeploySettings";
import { useDialog } from "./Dialog";
import { approvalSaid, isApproval } from "./decisions";
import { summarizeState } from "./diagnostics";
import { limitChange, limitRows, type LimitRow } from "./flowFacts";
import { describeItem, permissionKind } from "./holon";
import { KindBadge } from "./Manifest";
import { holdsLines } from "./storage";
import {
  changeGrant,
  changeSettings,
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
  type HostPolicy,
  type Holds,
  type ManifestReply,
  type SecretInfo,
  type SessionEntry,
  type WaitingRun,
} from "./server";
import { Gated, gateOf, type Answer, type Can } from "./useCan";

const when = (secs: number) => new Date(secs * 1000).toLocaleString();
const say = (e: unknown) => (e instanceof Error ? e.message : String(e));

export function SettingsTab({
  slug,
  entry,
  can,
  focus,
  onChanged,
  onAction,
  onEdit,
  onRemove,
}: {
  slug: string;
  entry: DeploymentEntry;
  /** What whoever is signed in may do to it (FlowPage.tsx's FLOW_ACTIONS). */
  can: Can;
  /** Asked for from the admin page: each new value brings the permissions into view. */
  focus?: number;
  /** Its grant or its settings changed here: the table should be fetched again. */
  onChanged: () => void;
  /** Pause, resume or stop it; run a new session from a stopped one's state. */
  onAction: (action: "run" | "pause" | "stop", from?: "latest" | "fresh" | number) => Promise<void>;
  /** Open it in the editor: a setting raised is a new version, made there. */
  onEdit: () => void;
  /** Remove it, every session and its state. */
  onRemove: () => Promise<void>;
}) {
  const version = `${entry.unit} ${entry.status} ${entry.attention ?? ""}`;
  const [manifest, setManifest] = useState<ManifestReply | null>(null);
  const [view, setView] = useState<GrantView | null>(null);
  const [grantError, setGrantError] = useState("");
  const [policy, setPolicy] = useState<HostPolicy | null>(null);
  useEffect(() => {
    getManifest(slug).then(setManifest, () => setManifest(null));
    getGrant(slug).then(setView, (e: unknown) => setGrantError(say(e)));
  }, [slug, version]);
  useEffect(() => {
    getPermissions().then(setPolicy, () => setPolicy(null));
  }, []);
  // A change to the grant here, or to a setting through it: the grant as it now is.
  const reread = () => {
    getGrant(slug).then(setView, (e: unknown) => setGrantError(say(e)));
    onChanged();
  };
  const secretNames = (manifest?.capabilities ?? []).flatMap((c) => {
    const m = /^SecretRead\("(.*)"\)$/.exec(c);
    return m ? [m[1]] : [];
  });
  const rows = view ? limitRows(view.grant, policy?.defaults ?? {}, policy?.ceilings ?? {}) : [];
  return (
    <div className="hl-flow__settings">
      <p className="hl-flow__note hl-flow__settings-how">
        Each setting changes here. One that narrows what it may do applies at once; one that widens it is a new version, which a person approves.
      </p>
      <Permissions slug={slug} view={view} error={grantError} manifest={manifest} focus={focus} onChanged={(v) => (setView(v), onChanged())} may={can.may("flow.approve-deploy")} />
      {secretNames.length > 0 && <Secrets slug={slug} names={secretNames} may={can.may("secret.set")} envNames={can.holds("resource.view")} />}
      <Limits slug={slug} title="Limits" group="limits" rows={rows} known={policy !== null} may={can.may("flow.deploy")} onChanged={reread} onEdit={onEdit} />
      <Limits slug={slug} title="Runs kept" group="runs kept" rows={rows} known={policy !== null} may={can.may("flow.deploy")} onChanged={reread} onEdit={onEdit} />
      <Network view={view} policy={policy} may={can.may("flow.approve-deploy")} onChange={(item) => changeGrant(slug, { revoke: [item] }).then((v) => (setView(v), onChanged()))} onEdit={onEdit} />
      <Agents slug={slug} entry={entry} agents={manifest?.requirements?.agents ?? []} may={can.may("flow.deploy")} onChanged={onChanged} />
      {(entry.waiting ?? 0) > 0 && <Waiting slug={slug} count={entry.waiting ?? 0} />}
      <Sessions entry={entry} onRunFrom={(n) => onAction("run", n)} may={can.may("flow.lifecycle")} />
      <Lifecycle entry={entry} onAction={onAction} may={can.may("flow.lifecycle")} />
      <Remove slug={slug} stopped={entry.status === "stopped"} onRemove={onRemove} may={can.may("flow.delete")} />
    </div>
  );
}

/** Whether agents may propose its versions (proposals.rs): off, an agent's deploy of it is
 *  refused, neither proposed nor saved. Changing it is a deployer's (`flow.deploy`). Which of its
 *  actions agents may run is part of a version, approved with it. */
function Agents({ slug, entry, agents, may, onChanged }: { slug: string; entry: DeploymentEntry; agents: string[]; may: Answer; onChanged: () => void }) {
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
      <label className="hl-flow__check" title={may === true ? undefined : may}>
        <input type="checkbox" checked={on} {...gateOf(may)} onChange={(e) => change(e.target.checked)} />
        agents may propose versions
      </label>
      <p className="hl-flow__note">{on ? "An agent's version that asks for nothing new deploys at once; one that does waits in Approvals, for a person." : "An agent's deploy of it is refused."}</p>
      <dl className="hl-flow__facts">
        <div>
          <dt>agents may run</dt>
          <dd>{agents.length > 0 ? agents.join(", ") : "none of its actions"}</dd>
        </div>
      </dl>
      {on && !hostTakes && (
        <p className="hl-flow__note" role="note">
          This host takes no proposals from agents (its policy's agents.propose = false): an agent's version that asks for something new is refused, whatever this says.
        </p>
      )}
    </section>
  );
}

/** What it is granted, each item with revoke; what was taken back, with grant back; whether an
 *  uncovered grant pauses it; and the last changes. Changing its grant is approving what it may do
 *  (`flow.approve-deploy`). */
function Permissions({ slug, view, error, manifest, focus, onChanged, may }: { slug: string; view: GrantView | null; error: string; manifest: ManifestReply | null; focus?: number; onChanged: (v: GrantView) => void; may: Answer }) {
  const [msg, setMsg] = useState("");
  const here = useRef<HTMLElement>(null);
  useEffect(() => {
    if (focus !== undefined) here.current?.scrollIntoView({ block: "start" });
  }, [focus]);
  const change = (c: Parameters<typeof changeGrant>[1]) => {
    setMsg("");
    changeGrant(slug, c)
      .then(onChanged)
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
  return (
    <section ref={here} className="hl-flow__section" aria-label="permissions">
      <h2>What it may do</h2>
      {(msg || error) && <div className="hl-flow__bad">{msg || error}</div>}
      {view?.attention && <div className="hl-flow__banner hl-flow__bad">⚠ {view.attention}</div>}
      {grant && (
        <>
          <ul className="hl-flow__grant">
            {grant.items
              .filter((item) => !item.startsWith("Limit("))
              .map((item) => (
                <li key={item} title={item}>
                  <span className="hl-flow__what">✓ {describeItem(item)}</span>
                  <KindBadge item={item} optional={optional} trigger={trigger} />
                  <span className="hl-flow__spacer" />
                  <button className="hl-flow__button" onClick={() => change({ revoke: [item] })} {...gateOf(may, { title: revokeTitle(item) })}>
                    Revoke
                  </button>
                </li>
              ))}
            {(grant.revoked ?? []).map((item) => (
              <li key={item} title={item} className="hl-flow__revoked">
                <span>{describeItem(item)}</span>
                <span className="hl-table__dim"> · revoked</span>
                <span className="hl-flow__spacer" />
                <button className="hl-flow__button" onClick={() => change({ grant: [item] })} {...gateOf(may, { title: "grant it back" })}>
                  Grant back
                </button>
              </li>
            ))}
          </ul>
          <label className="hl-flow__check" title={may === true ? "when the grant doesn't cover what it needs" : may}>
            <input type="checkbox" checked={grant.keep_running ?? false} disabled={may !== true} onChange={(e) => change({ keep_running: e.target.checked })} />
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
            Granted {when(grant.granted_at)} by {grant.by}. Something new it may do is a new version, which a person approves.
          </p>
        </>
      )}
    </section>
  );
}

const valueSaid = (n: number, unit: string) => `${n.toLocaleString()}${unit ? ` ${unit}` : ""}`;

/** Its limits, or the runs its log keeps: each as it holds now, and where that comes from (asked
 *  for, the host's default, lowered, the policy's ceiling). A lower one applies at once
 *  (`POST …/settings`, a deployer's); a higher one widens what it may do, so it is a new version,
 *  made in the editor and approved by a person. */
function Limits({ slug, title, group, rows, known, may, onChanged, onEdit }: { slug: string; title: string; group: LimitRow["group"]; rows: LimitRow[]; known: boolean; may: Answer; onChanged: () => void; onEdit: () => void }) {
  const [typed, setTyped] = useState<Record<string, string>>({});
  const [msg, setMsg] = useState<{ text: string; bad: boolean } | null>(null);
  const shown = rows.filter((r) => r.group === group);
  const lower = (r: LimitRow, to: number) => {
    setMsg(null);
    changeSettings(slug, { lower: { [r.name]: to } }).then(
      () => {
        setTyped((t) => ({ ...t, [r.name]: "" }));
        setMsg({ text: `${r.label} lowered to ${valueSaid(to, r.unit)}: it holds from the next run`, bad: false });
        onChanged();
      },
      (e: unknown) => setMsg({ text: say(e), bad: true }),
    );
  };
  return (
    <section className="hl-flow__section" aria-label={title.toLowerCase()}>
      <h2>{title}</h2>
      {msg && <div className={msg.bad ? "hl-flow__bad" : "hl-flow__said"} role="status">{msg.text}</div>}
      {shown.length === 0 ? (
        <p className="hl-flow__note">{known ? "the host's defaults" : "…"}</p>
      ) : (
        <table className="hl-table hl-flow__limits" aria-label={`${title.toLowerCase()} table`}>
          <thead>
            <tr>
              <th>Setting</th>
              <th>Now</th>
              <th>From</th>
              <th>Change to</th>
            </tr>
          </thead>
          <tbody>
            {shown.map((r) => {
              const t = typed[r.name] ?? "";
              const how = limitChange(r, t);
              const from = [
                r.lowered !== null && r.lowered <= r.base ? `lowered from ${valueSaid(r.base, r.unit)}` : r.asked !== null && r.asked === r.base ? "asked for" : "the host's default",
                r.ceiling !== null && r.ceiling < Math.min(r.base, r.lowered ?? Infinity) ? `held to the policy's ceiling` : null,
              ]
                .filter(Boolean)
                .join(" · ");
              return (
                <tr key={r.name}>
                  <td className="hl-flow__what" title={r.name}>
                    {r.label}
                  </td>
                  <td className="hl-table__num">{valueSaid(r.now, r.unit)}</td>
                  <td className="hl-table__dim">{from}</td>
                  <td>
                    <input className="hl-flow__filter hl-flow__number" inputMode="numeric" aria-label={`change ${r.label} to`} value={t} {...gateOf(may)} onChange={(e) => setTyped({ ...typed, [r.name]: e.target.value })} />{" "}
                    {how === "raise" ? (
                      <button className="hl-flow__button" onClick={onEdit} {...gateOf(may, { title: "a higher limit widens what it may do: change it in the editor's Settings and deploy it, a new version a person approves" })}>
                        Raise in a new version…
                      </button>
                    ) : (
                      <button className="hl-flow__button" onClick={() => lower(r, Number(t))} {...gateOf(may, { disabled: how !== "lower", title: how === "bad" ? "a whole number, 0 or more" : "lower it now, without a new version; the next runs are held to it" })}>
                        Lower
                      </button>
                    )}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </section>
  );
}

/** Where it may call: over HTTPS only, but for the hosts it was granted plain HTTP to; private
 *  addresses or not. Each exception granted is revoked here (`flow.approve-deploy`); one more is a
 *  new version, which a person approves. */
function Network({ view, policy, may, onChange, onEdit }: { view: GrantView | null; policy: HostPolicy | null; may: Answer; onChange: (item: string) => Promise<unknown>; onEdit: () => void }) {
  const [msg, setMsg] = useState("");
  const grant = view?.grant;
  const plain = [...(grant?.requirements.plain_http ?? [])];
  const privateAsked = grant?.requirements.private_addresses === true;
  const held = (item: string) => grant?.items.includes(item) === true;
  const revoke = (item: string) => {
    setMsg("");
    onChange(item).catch((e: unknown) => setMsg(say(e)));
  };
  const hosts = (grant?.items ?? []).flatMap((i) => /^HttpRequest\("(.*)"\)$/.exec(i)?.[1] ?? []);
  return (
    <section className="hl-flow__section" aria-label="network">
      <h2>Network</h2>
      {msg && <div className="hl-flow__bad">{msg}</div>}
      <ul className="hl-flow__grant">
        <li>
          <span className="hl-flow__what">calls {hosts.length > 0 ? hosts.join(", ") : "no host"}</span>
          <span className="hl-table__dim"> · over HTTPS{plain.length > 0 ? ", but where below" : ""}</span>
        </li>
        {plain.map((h) => {
          const item = `PlainHttp(${JSON.stringify(h)})`;
          return (
            <li key={h} title={item} className={held(item) ? undefined : "hl-flow__revoked"}>
              <span className="hl-flow__what">plain HTTP to {h}</span>
              {!held(item) && <span className="hl-table__dim"> · revoked</span>}
              {policy && !policy.allow_plain_http && <span className="hl-flow__waiting"> · this host allows none</span>}
              <span className="hl-flow__spacer" />
              {held(item) && (
                <button className="hl-flow__button" onClick={() => revoke(item)} {...gateOf(may, { title: "HTTPS only to it from now on" })}>
                  Revoke
                </button>
              )}
            </li>
          );
        })}
        <li title={privateAsked ? "PrivateAddresses" : undefined} className={privateAsked && !held("PrivateAddresses") ? "hl-flow__revoked" : undefined}>
          <span className="hl-flow__what">{privateAsked && held("PrivateAddresses") ? "may reach private addresses" : "no private addresses"}</span>
          {privateAsked && !held("PrivateAddresses") && <span className="hl-table__dim"> · revoked</span>}
          <span className="hl-flow__spacer" />
          {privateAsked && held("PrivateAddresses") && (
            <button className="hl-flow__button" onClick={() => revoke("PrivateAddresses")} {...gateOf(may, { title: "no private addresses from now on" })}>
              Revoke
            </button>
          )}
        </li>
      </ul>
      <p className="hl-flow__note">
        A host more, plain HTTP or private addresses is a new version, which a person approves:{" "}
        <button className="hl-flow__link" onClick={onEdit}>
          open it in the editor
        </button>
        .
      </p>
    </section>
  );
}

/** The secrets it reads: each one's version and where it comes from, set anew, its history, and
 *  rolled back to an older version (`secret.set`, `may`). Values are never shown. The server's
 *  env variables' names are offered to whoever may view the host's resources (`envNames`). */
function Secrets({ slug, names, may, envNames: offered }: { slug: string; names: string[]; may: Answer; envNames: boolean }) {
  const [infos, setInfos] = useState<SecretInfo[]>([]);
  const [envNames, setEnvNames] = useState<string[]>([]);
  const reload = () => listSecrets(slug).then(setInfos, () => setInfos([]));
  useEffect(() => {
    void reload();
    if (offered) getEnvSecretNames().then((r) => setEnvNames(r.names), () => setEnvNames([]));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [slug, offered]);
  return (
    <section className="hl-flow__section" aria-label="secrets">
      <h2>Secrets</h2>
      <div className="hl-flow__secrets">
        <Gated may={may}>
          {names.map((name) => (
            <SecretRow key={name} slug={slug} name={name} info={infos.find((x) => x.name === name)} envNames={envNames} onChanged={() => void reload()} />
          ))}
        </Gated>
      </div>
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
 *  from one of them (`flow.lifecycle`). And what it holds, in memory and on disk. */
function Sessions({ entry, onRunFrom, may }: { entry: DeploymentEntry; onRunFrom: (session: number) => Promise<void>; may: Answer }) {
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
                  <button className="hl-flow__button" onClick={() => onRunFrom(s.number).catch((e: unknown) => setMsg(say(e)))} {...gateOf(may, { title: "run a new session from this one's state" })}>
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

/** Pause or resume it, and stop it: a stop ends its session, saves its state and releases its
 *  port, and asks first, saying how many waiting runs would end (`flow.lifecycle`). */
function Lifecycle({ entry, onAction, may }: { entry: DeploymentEntry; onAction: (action: "run" | "pause" | "stop") => Promise<void>; may: Answer }) {
  const dialog = useDialog();
  const [msg, setMsg] = useState("");
  const act = (action: "run" | "pause" | "stop") => {
    setMsg("");
    onAction(action).catch((e: unknown) => setMsg(say(e)));
  };
  const waiting = entry.waiting ?? 0;
  const stop = async () => {
    const ends = waiting === 0 ? "No run waits: none ends." : waiting === 1 ? "1 waiting run ends: what it waits for won't come." : `${waiting} waiting runs end: what they wait for won't come.`;
    if (await dialog.confirm(`Stop ${entry.slug}?`, "Stop", <p>It stops answering, its session ends, its state is saved and its port released. {ends} Run it again from its state later.</p>)) act("stop");
  };
  return (
    <section className="hl-flow__section" aria-label="lifecycle">
      <h2>Lifecycle</h2>
      {msg && <div className="hl-flow__bad">{msg}</div>}
      <div className="hl-flow__bar">
        {entry.status === "running" && (
          <button className="hl-flow__button" onClick={() => act("pause")} {...gateOf(may, { title: "stop answering, keep the port and the session" })}>
            Pause
          </button>
        )}
        {entry.status === "paused" && (
          <button className="hl-flow__button" onClick={() => act("run")} {...gateOf(may, { title: "continue the same session" })}>
            Resume
          </button>
        )}
        {entry.status === "stopped" ? (
          <button className="hl-flow__button" onClick={() => act("run")} {...gateOf(may, { title: "a new session, from the state the last one left" })}>
            Run
          </button>
        ) : (
          <button className="hl-flow__button hl-flow__button--danger" onClick={() => void stop()} {...gateOf(may, { title: "end the session, save its state, release the port" })}>
            Stop…
          </button>
        )}
      </div>
      <p className="hl-flow__note">{entry.status === "stopped" ? "Stopped: its state is kept, and a run starts a new session from it." : waiting > 0 ? `${waiting} run${waiting === 1 ? "" : "s"} wait${waiting === 1 ? "s" : ""}: a stop ends them.` : "No run waits."}</p>
    </section>
  );
}

/** Removing it: every session, its state and its log go, which can't be undone (its runs exported
 *  first keep their record). Asked by typing its name, once it is stopped, by whoever may delete
 *  it (`flow.delete`). */
function Remove({ slug, stopped, onRemove, may }: { slug: string; stopped: boolean; onRemove: () => Promise<void>; may: Answer }) {
  const [typed, setTyped] = useState("");
  const [msg, setMsg] = useState("");
  return (
    <section className="hl-flow__section hl-flow__danger" aria-label="remove">
      <h2>Remove</h2>
      <p className="hl-flow__note">
        Removes the deployment, every session, its state and its log. It can't be undone; its saved flow stays, and its runs exported first (Runs, Export) keep their record.{stopped ? "" : " Stop it first."}
      </p>
      <div className="hl-flow__bar">
        <input className="hl-flow__filter" aria-label={`type ${slug} to remove it`} placeholder={`type ${slug}`} value={typed} {...gateOf(may, { disabled: !stopped })} onChange={(e) => setTyped(e.target.value)} />
        <button className="hl-flow__button hl-flow__button--danger" {...gateOf(may, { disabled: !stopped || typed !== slug })} onClick={() => onRemove().catch((e: unknown) => setMsg(say(e)))}>
          Remove {slug}
        </button>
      </div>
      {msg && <div className="hl-flow__bad">{msg}</div>}
    </section>
  );
}
