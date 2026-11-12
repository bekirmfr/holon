// The host's pages (holon-server's auth.rs, gaps-web.md G26–G29), each shown to whoever may view
// it, its forms and buttons disabled, with the server's refusal as their title, for whoever may not
// change what they change (useCan.tsx; can.rs's route table says which action each route needs), each at an
// address of its own: Resources, in tabs (#resources: its databases; #resources.folders, .mail,
// .feeds, .queues, .secrets with the server's own and their history, .host with its storage, .who-may),
// People and access (#people: people, the agents and services they own with their tokens,
// signing in through an identity provider; for a person who may not see everyone, the agents and
// services they own, with their tokens: OwnedPage) and Policy (#policy: what holon.toml allows, its limits
// and ceilings, and the deployments it holds off). A section of People and access has its place in
// the address too (#people.agents). Below each page, what was done, by whom. Adding someone gives a one-time link for them to set their password; resetting gives a
// new one. The server refuses any change that would leave no admin. A grant changes only on its
// deployment's page: who may touch what links there.
import { Fragment, useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import {
  addPrincipal,
  changePrincipal,
  bindDatabase,
  bindMail,
  bindFolder,
  bindServerSecret,
  capServerSecret,
  createToken,
  getOidcSettings,
  getPermissions,
  listDatabases,
  listMails,
  listFeeds,
  bindFeed,
  unbindFeed,
  type FeedBinding,
  type FeedEntry,
  type OnFull,
  listQueues,
  bindQueue,
  unbindQueue,
  type QueueEntry,
  listFolders,
  listGrants,
  listProposals,
  type Proposal,
  listServerSecrets,
  listTables,
  invitePerson,
  listOwned,
  listPrincipals,
  listRoles,
  reloadPermissions,
  removePrincipal,
  resetPerson,
  revokeToken,
  setOidcSettings,
  sweepFolders,
  sweepStore,
  getHost,
  setSweepEvery,
  setStorage,
  type HostView,
  unbindDatabase,
  unbindMail,
  unbindFolder,
  unbindServerSecret,
  type DatabaseBinding,
  type DatabaseEntry,
  type MailBinding,
  type MailEntry,
  type FolderBinding,
  type FolderEntry,
  type HeldGrant,
  type HostPolicy,
  type Kind,
  type OidcSettings,
  type Principal,
  type RoleEntry,
  type SecretInfo,
  type DeploymentEntry,
  ServerError,
} from "./server";
import { Gated, gateOf, useCan, type Answer } from "./useCan";
import { cacheLine, diskFull, diskLine, holdsLines, isEvery, isThreshold, keptLines, reportLine, scheduleLine, sizeText, whenFullLine } from "./storage";
import { useDialog } from "./Dialog";
import { FolderPicker } from "./FolderPicker";
import { ActivityList } from "./Pages";
import { describeItem } from "./holon";
import { databaseMatrix, holdersOf, kindsHeld, othersHeld, usesText, waitingFor, type Holder } from "./holdings";
import { CollectionsTab, DatabaseMatrix, ResourceTable, SecretsTable } from "./ResourceViews";
import { databaseLine, feedLine, folderLine, mailLine, queueLine } from "./resourceRows";
import { seenSaid } from "./lastSeen";
import { PolicyCheck, PolicyText } from "./PolicyViews";
import { limitMeaning } from "./policyMeans";
import { about, input, primary, quiet, row, small } from "./hostStyles";
import { bindingSaid, hostRoleOf, otherBindings, rolesFor, roleTitle } from "./access";

import { format, RESOURCES_LABELS, RESOURCES_TABS, type ResourcesTab } from "./routes";

/** What holding a resource for whole runs costs, said where it is set. */
const RUN_LOCK_COST = "every run that writes it goes one at a time, for its whole length, across every deployment bound to it";

const messageOf = (e: unknown) => (e instanceof Error ? e.message : String(e));
/** A read refused (403) is what the caller may not view: left empty, not said as a failure. */
const refusedRead = (e: unknown) => e instanceof ServerError && e.status === 403;
/** The link someone opens to set their password. */
const linkFor = (token: string) => `${location.origin}${location.pathname}?invite=${encodeURIComponent(token)}`;
/** A connection typed in as a URL (`postgres://user:…@host/shop`), not where one is kept: the page never sends it. */
const isUrl = (s: string) => /^[a-z][a-z0-9+.-]*:\/\//i.test(s.trim());
const keyOf = (b: { name: string; deployment: string | null }) => `${b.deployment ?? ""}/${b.name}`;
/** The deployments that use a binding, as a sentence's end: `orders-api and audit`. */
const named = (holders: Holder[]) => {
  const slugs = holders.map((h) => h.slug);
  return slugs.length <= 1 ? (slugs[0] ?? "") : `${slugs.slice(0, -1).join(", ")} and ${slugs[slugs.length - 1]}`;
};

/** Where a server secret's value is: sealed here, an env variable of the server's, a Vault field. */
type SecretKind = "value" | "env" | "vault";

/** What the host's pages read and do: every list (one page shows a part), what was said, and
 *  `act`, which does a change and reads everything again. `shown`: the databases' tables are
 *  read (Resources). `tab`: the page's tab; picking another (by the address too, where the page
 *  stays mounted) reads everything again, so what was bound elsewhere since shows. */
function useHost(shown = false, tab: string | null = null) {
  const dialog = useDialog();
  const [principals, setPrincipals] = useState<Principal[]>([]);
  const [oidc, setOidc] = useState<OidcSettings | null>(null);
  const [msg, setMsg] = useState("");
  const [link, setLink] = useState<{ name: string; url: string } | null>(null);
  const [secret, setSecret] = useState<{ label: string; token: string } | null>(null);
  // Every role (`GET /roles`), the built-in ones first: what a kind is offered.
  const [roles, setRoles] = useState<RoleEntry[]>([]);
  const [draft, setDraft] = useState({ name: "", role: "editor", email: "" });
  // An agent or a service being added: its owner empty for whoever adds it.
  const [addDraft, setAddDraft] = useState({ name: "", kind: "agent" as Exclude<Kind, "person">, role: "agent", owner: "" });
  const [issuer, setIssuer] = useState("");
  const [clientId, setClientId] = useState("");
  const [folders, setFolders] = useState<FolderEntry[]>([]);
  const [folderDraft, setFolderDraft] = useState<FolderBinding>({ name: "", path: "", access: "read-write", keep: "bytes", cap: null, shared: false, deployment: null });
  // How long a folder keeps what runs read, when it keeps it for a while ("7d").
  const [keepFor, setKeepFor] = useState("");
  // The "browse…" for the folder's path is open.
  const [browsing, setBrowsing] = useState(false);
  const [databases, setDatabases] = useState<DatabaseEntry[]>([]);
  const [databaseDraft, setDatabaseDraft] = useState<DatabaseBinding & { where: string }>({ name: "", where: "", access: "read-write", schema: null, deployment: null });
  // Each database's tables as last read, by binding: the tables, or why they couldn't be read.
  const [tables, setTables] = useState<Record<string, string[] | { error: string }>>({});
  const [mails, setMails] = useState<MailEntry[]>([]);
  const noMail: MailBinding = { name: "", smtp: "", from: "", to: "", imap: "", folder: "", done: "", verdict_from: "", oauth: "", deployment: null };
  const [mailDraft, setMailDraft] = useState<MailBinding>(noMail);
  const [feeds, setFeeds] = useState<FeedEntry[]>([]);
  // A feed to bind, its headers written NAME=SOURCE and its subscription messages one a line, its
  // --only FIELD=VALUE with spaces between.
  const noFeed = {
    name: "",
    from: "url" as "url" | "channel",
    url: "",
    database: "",
    channel: "",
    headers: "",
    buffer: "",
    idle: "",
    deployment: "",
    onFull: "wait" as OnFull,
    key: "",
    sequence: "",
    subscribe: "",
    heartbeat: "",
    every: "",
    id: "",
    only: "",
    resume: "",
  };
  const [feedDraft, setFeedDraft] = useState(noFeed);
  const [queues, setQueues] = useState<QueueEntry[]>([]);
  const noQueue = { name: "", url: "", visibility: "", keepFailed: false, deployment: "" };
  const [queueDraft, setQueueDraft] = useState(noQueue);
  const [serverSecrets, setServerSecrets] = useState<SecretInfo[]>([]);
  // A server secret being bound: its name, its value or where that is kept, and where it may go.
  const [secretDraft, setSecretDraft] = useState({ name: "", value: "", to: "", kind: "value" as SecretKind });
  const [grants, setGrants] = useState<HeldGrant[]>([]);
  // Whether the grants could be read: they are the host's to view (`host.view`), and who uses a
  // resource is read from them.
  const [grantsKnown, setGrantsKnown] = useState(true);
  // The versions waiting for a person: what each would add to a resource (§9.1).
  const [proposals, setProposals] = useState<Proposal[]>([]);
  const [policy, setPolicy] = useState<HostPolicy | null>(null);
  // What reading the policy again said: when, and the deployments it holds off.
  const [reloaded, setReloaded] = useState<{ at: number; attention: [string, string][] } | null>(null);
  // What the last sweep of the store and of the file texts said.
  const [swept, setSwept] = useState<{ store?: string; folders?: string }>({});
  // What the host holds and how it sweeps (`GET /admin/host`), and the sweep setting being edited.
  const [host, setHost] = useState<HostView | null>(null);
  const sweeps = host?.sweeps ?? null;
  const [everyDraft, setEveryDraft] = useState("");
  const [warnDraft, setWarnDraft] = useState("");
  const [fullDraft, setFullDraft] = useState("");
  const [backupDraft, setBackupDraft] = useState<string | null>(null);
  const [kind, setKind] = useState("everything");
  const [version, setVersion] = useState(0);

  const refresh = useCallback(() => {
    const said = (e: unknown) => refusedRead(e) || setMsg(messageOf(e));
    listPrincipals().then(setPrincipals).catch(said);
    listRoles().then(setRoles).catch(said);
    listFolders().then(setFolders).catch(said);
    listDatabases().then(setDatabases).catch(said);
    listMails().then(setMails).catch(said);
    listFeeds().then(setFeeds).catch(said);
    listQueues().then(setQueues).catch(said);
    listServerSecrets().then(setServerSecrets).catch(said);
    listGrants()
      .then((g) => {
        setGrants(g);
        setGrantsKnown(true);
      })
      .catch((e: unknown) => {
        if (refusedRead(e)) setGrantsKnown(false);
        said(e);
      });
    listProposals().then(setProposals).catch(said);
    getPermissions().then(setPolicy).catch(said);
    getHost()
      .then((h) => {
        setHost(h);
        // What the admin is typing stays: a refresh only fills the field when it is empty.
        setEveryDraft((d) => d || h.sweeps.every);
        setWarnDraft((d) => d || h.warn_at);
        setFullDraft((d) => d || h.full_at);
      })
      .catch(said);
    getOidcSettings()
      .then((s) => {
        setOidc(s);
        setIssuer(s?.issuer ?? "");
        setClientId(s?.client_id ?? "");
      })
      .catch(said);
    setVersion((v) => v + 1);
  }, []);
  useEffect(refresh, [refresh, tab]);

  /** Read a database's tables again (after a migration, say). */
  const readTables = useCallback((d: DatabaseEntry) => {
    listTables(d.name, d.deployment)
      .then((t) => setTables((all) => ({ ...all, [keyOf(d)]: t })))
      .catch((e: unknown) => setTables((all) => ({ ...all, [keyOf(d)]: { error: messageOf(e) } })));
  }, []);
  // Each database's tables, read when the resources are shown and for a binding not read yet.
  useEffect(() => {
    if (!shown) return;
    for (const d of databases) if (!(keyOf(d) in tables)) readTables(d);
  }, [shown, databases, tables, readTables]);

  /** Do `f`, say what failed, and read everything again. */
  const act = async (f: () => Promise<unknown>) => {
    setMsg("");
    try {
      await f();
    } catch (e) {
      setMsg(messageOf(e));
    }
    refresh();
  };

  const folderHolders = (f: FolderEntry) => holdersOf("folder", f, folders, grants);
  const databaseHolders = (d: DatabaseEntry) => holdersOf("database", d, databases, grants);
  const mailHolders = (m: MailEntry) => holdersOf("mail", m, mails, grants);
  const feedHolders = (f: FeedEntry) => holdersOf("feed", f, feeds, grants);
  const queueHolders = (q: QueueEntry) => holdersOf("queue", q, queues, grants);
  /** What unbinding (or binding again) does to the deployments that use it, as the server does it now. */
  const unbindingSays = (holders: Holder[]) =>
    holders.length === 0 ? "No deployment uses it." : `${named(holders)} ${holders.length === 1 ? "uses" : "use"} it: they keep running, and their calls to it fail until it is bound again.`;

  return {
    principals, setPrincipals, roles, oidc, setOidc, msg, setMsg, link, setLink, secret,
    setSecret, draft, setDraft, addDraft, setAddDraft, issuer, setIssuer, clientId, setClientId, grantsKnown, proposals,
    folders, setFolders, folderDraft, setFolderDraft, keepFor, setKeepFor, browsing, setBrowsing,
    databases, setDatabases, databaseDraft, setDatabaseDraft, tables, setTables, serverSecrets,
    setServerSecrets, secretDraft, setSecretDraft, grants, setGrants, policy, setPolicy, reloaded,
    setReloaded, swept, setSwept, host, setHost, everyDraft, setEveryDraft, warnDraft, setWarnDraft,
    fullDraft, setFullDraft, backupDraft, setBackupDraft, kind, setKind, version, setVersion,
    dialog, sweeps, refresh, readTables, act, folderHolders, databaseHolders, unbindingSays,
    mails, mailDraft, setMailDraft, noMail, feeds, feedDraft, setFeedDraft, noFeed, queues, queueDraft,
    setQueueDraft, noQueue, mailHolders, feedHolders, queueHolders,
  };
}
type Host = ReturnType<typeof useHost>;

/** A host page: its title, what it is for, what failed, its sections, and what was done. */
function HostPage({ title, about: what, tab, host, children }: { title: string; about: string; tab: string | null; host: Host; children: ReactNode }) {
  const at = useRef<HTMLDivElement>(null);
  // A section named in the address is scrolled to, once it is there.
  useEffect(() => {
    if (tab) at.current?.querySelector(`[data-section="${tab}"]`)?.scrollIntoView({ block: "start" });
  }, [tab, host.version]);
  return (
    <div ref={at} className="hl-flow hl-host-page" aria-label={title}>
      <header className="hl-flow__head">
        <h1 className="hl-flow__name">{title}</h1>
        <span className="hl-flow__status">{what}</span>
      </header>
      {host.msg && (
        <div role="alert" style={{ color: "var(--bad)", fontSize: 12 }}>
          {host.msg}
        </div>
      )}
      <div>{children}</div>
      <ActivityList version={String(host.version)} />
    </div>
  );
}

/** A token just made: whose and what for, and its value, shown once. */
type MadeToken = { label: string; token: string };

/** An agent's or a service's tokens: its owner or an admin makes them, each shown once (`show`). */
async function makeToken(p: Principal, dialog: ReturnType<typeof useDialog>, act: (f: () => Promise<unknown>) => Promise<void>, show: (made: MadeToken) => void) {
  const label = await dialog.prompt(`What is ${p.name}'s new token for? (GitHub CI, a laptop…)`, "", "make token");
  if (!label?.trim()) return;
  act(async () => {
    const r = await createToken(p.name, label.trim());
    show({ label: `${p.name}'s "${r.entry.label}"`, token: r.token });
  });
}

/** A token just made, to copy now: it isn't shown again. */
function MadeTokenBox({ made, onDone }: { made: MadeToken; onDone: () => void }) {
  return (
    <div style={{ ...row, flexDirection: "column", alignItems: "stretch", borderColor: "var(--sel)" }}>
      <span>
        The token {made.label}. Copy it now: it isn&apos;t shown again.
      </span>
      <span style={{ display: "flex", gap: 6 }}>
        <input aria-label="new API token" readOnly value={made.token} style={{ ...input, flex: 1 }} onFocus={(e) => e.target.select()} />
        <button style={small} onClick={() => navigator.clipboard?.writeText(made.token)}>
          copy
        </button>
        <button style={small} onClick={onDone}>
          done
        </button>
      </span>
    </div>
  );
}

/** When a principal was last seen or a token last used (`what`), as a column of its row: how long
 *  ago, the exact time as its title (lastSeen.ts). */
function Seen({ what, at }: { what: "last seen" | "last used"; at: number | null | undefined }) {
  const said = seenSaid(at, Date.now());
  return (
    <span className="hl-seen" title={said.title}>
      {what} {said.text}
    </span>
  );
}

/** `p`'s tokens, each revoked from its row (when `may`), with when each was last used; or that it
 *  has none, so it can't sign in yet. */
function TokenRows({ p, dialog, act, may = true }: { p: Principal; dialog: ReturnType<typeof useDialog>; act: (f: () => Promise<unknown>) => Promise<void>; may?: Answer }) {
  const tokens = p.tokens ?? [];
  if (tokens.length === 0) return <div style={{ ...quiet, margin: "-2px 0 8px 24px" }}>{p.name} has no token: it can&apos;t sign in yet</div>;
  return (
    <>
      {tokens.map((t) => (
        <div key={t.id} style={{ ...row, marginLeft: 24, padding: "6px 14px" }}>
          <span style={{ minWidth: 90 }}>{t.label}</span>
          <span style={{ color: "var(--faint)", fontSize: 11, flex: 1 }}>
            a token of {p.name}, made by {t.created_by}, {new Date(t.created_at * 1000).toLocaleString()}
          </span>
          <Seen what="last used" at={t.last_used} />
          <button
            style={small}
            aria-label={`revoke ${p.name}'s token ${t.label}`}
            {...gateOf(may)}
            onClick={async () => {
              if (await dialog.confirm(`Revoke ${p.name}'s token "${t.label}"? Whatever signs in with it stops working.`, "Revoke")) act(() => revokeToken(p.name, t.id));
            }}
          >
            revoke
          </button>
        </div>
      ))}
    </>
  );
}

/** How an agent connects to this server: `holon mcp`, as an agent, never as you. */
function ConnectingAnAgent() {
  const line = `holon mcp setup claude-code --server ${location.origin}`;
  return (
    <>
      <div style={{ ...quiet, margin: "4px 0 6px" }}>
        Connecting an agent: <code>holon mcp</code> signs in as an agent, never as you. On this server&apos;s machine, it is yours, made for you the first time
        (<i>you</i>-agent, owned by you); from another, give it a token of an agent added here.
      </div>
      <div style={{ ...row, background: "transparent" }}>
        <input aria-label="the holon mcp line" readOnly value={line} style={{ ...input, flex: 1, fontFamily: "ui-monospace, Menlo, Consolas, monospace" }} onFocus={(e) => e.target.select()} />
        <button style={small} onClick={() => navigator.clipboard?.writeText(line)}>
          copy
        </button>
      </div>
      <a href={format({ kind: "docs", page: "guides/agents.md", anchor: null })} style={{ color: "var(--accent)", fontSize: 11, textDecoration: "none" }}>
        working with agents, in the manual ›
      </a>
    </>
  );
}

/** What a person who isn't an admin owns (auth.rs `GET /auth/owned`): their agents and services,
 *  each with its tokens, which they make (each shown once) and revoke. An admin adds an agent or a
 *  service and names its owner; People and access shows an admin everyone's. */
export function OwnedPage() {
  const dialog = useDialog();
  const [owned, setOwned] = useState<Principal[] | null>(null);
  const [msg, setMsg] = useState("");
  const [made, setMade] = useState<MadeToken | null>(null);
  const refresh = useCallback(() => {
    listOwned()
      .then(setOwned)
      .catch((e: unknown) => setMsg(messageOf(e)));
  }, []);
  useEffect(refresh, [refresh]);
  /** Do `f`, say what failed, and read what they own again. */
  const act = async (f: () => Promise<unknown>) => {
    setMsg("");
    try {
      await f();
    } catch (e) {
      setMsg(messageOf(e));
    }
    refresh();
  };
  return (
    <div className="hl-flow hl-host-page" aria-label="Your agents and services">
      <header className="hl-flow__head">
        <h1 className="hl-flow__name">Your agents and services</h1>
        <span className="hl-flow__status">the agents and services you own, and their tokens</span>
      </header>
      {msg && (
        <div role="alert" style={{ color: "var(--bad)", fontSize: 12 }}>
          {msg}
        </div>
      )}
      <div style={about}>
        An agent (an AI, connected through <code>holon mcp</code>) or a service (a script, CI) signs in with tokens of its own, and acts under its own name. You
        own these, so you make and revoke their tokens; an admin adds one for you, and changes its role.
      </div>
      {made && <MadeTokenBox made={made} onDone={() => setMade(null)} />}
      {owned?.length === 0 && <div style={{ ...quiet, marginBottom: 6 }}>You own no agent or service: an admin adds one for you, owned by you, in People and access.</div>}
      {owned?.map((p) => (
        <Fragment key={p.name}>
          <div style={row}>
            <span style={{ color: "var(--accent)", minWidth: 90 }}>{p.name}</span>
            <span style={quiet} title={p.bindings.map(bindingSaid).join(", ")}>
              {p.kind}
              {hostRoleOf(p) !== p.kind && hostRoleOf(p) !== "" && `, ${hostRoleOf(p)}`}
            </span>
            <span style={{ color: "var(--faint)", fontSize: 11, flex: 1 }}>
              {p.disabled && <span style={{ color: "var(--bad)" }}>disabled: its tokens are refused</span>}
            </span>
            <button style={small} aria-label={`make a token for ${p.name}`} title="a new token for it, shown once" onClick={() => makeToken(p, dialog, act, setMade)}>
              make token
            </button>
          </div>
          <TokenRows p={p} dialog={dialog} act={act} />
        </Fragment>
      ))}
      <ConnectingAnAgent />
    </div>
  );
}

/** A principal's own role on the host, to change: the roles its kind may hold, the ones an admin
 *  defined included. None when it holds one only through a group or on a flow (holon bind). */
function RoleSelect({ p, roles, act, may }: { p: Principal; roles: RoleEntry[]; act: (f: () => Promise<unknown>) => void; may: Answer }) {
  const now = hostRoleOf(p);
  return (
    <select aria-label={`${p.name}'s role`} value={now} style={input} {...gateOf(may, { title: roleTitle(roles, now) })} onChange={(e) => act(() => changePrincipal(p.name, { role: e.target.value }))}>
      {now === "" && (
        <option value="" disabled>
          no role on the host
        </option>
      )}
      {rolesFor(roles, p.kind).map((r) => (
        <option key={r.name} value={r.name} title={roleTitle(roles, r.name)}>
          {r.name}
        </option>
      ))}
    </select>
  );
}

/** What else holds for a principal, read only, on a line of its own above the row's notes: a role
 *  on a flow or an approval point, or through a group (`holon bind`, `holon groups`). */
function HeldElsewhere({ p }: { p: Principal }) {
  const others = otherBindings(p);
  if (others.length === 0) return null;
  return (
    <span aria-label={`${p.name}'s bindings`} style={{ display: "block", color: "var(--muted)" }} title="bound with holon bind; holon access show lists every binding">
      {others.map(bindingSaid).join(", ")}
    </span>
  );
}

export function PeoplePage({ me, kind, tab }: { me: string; kind: Kind; tab: string | null }) {
  const h = useHost(false, tab);
  // Every change here is to a principal (`people.manage` on the host, can.rs's routes).
  const can = useCan(["people.manage"]);
  const manage = can.may("people.manage");
  /** An agent's or a service's tokens: an admin's to make, or its owner's (can.rs's owner rule). */
  const tokensOf = (p: Principal): Answer => (kind === "person" && p.owner === me ? true : manage);
  const { principals, roles, oidc, link, setLink, secret, setSecret, draft, setDraft, addDraft, setAddDraft, issuer, setIssuer, clientId, setClientId, dialog, act } = h;
  const people = principals.filter((p) => p.kind === "person");
  const others = principals.filter((p) => p.kind !== "person");
  /** Who may own an agent or a service: the people, and whoever owns it now (the OS user, say). */
  const owners = (now: string | null) => [...new Set([...people.map((p) => p.name), ...(now ? [now] : [])])];
  return (
    <HostPage title="People and access" about="who uses this server: people, the agents and services they own, and how people sign in" tab={tab} host={h}>
      <div className="hl-host-page__head" data-section="accounts">people</div>
      {link && (
        <div style={{ ...row, flexDirection: "column", alignItems: "stretch", borderColor: "var(--sel)" }}>
          <span>
            Send <span style={{ color: "var(--ink)" }}>{link.name}</span> this link: they set their password with it (once, within 7 days).
          </span>
          <span style={{ display: "flex", gap: 6 }}>
            <input aria-label="invite link" readOnly value={link.url} style={{ ...input, flex: 1 }} onFocus={(e) => e.target.select()} />
            <button style={small} onClick={() => navigator.clipboard?.writeText(link.url)}>
              copy
            </button>
            <button style={small} onClick={() => setLink(null)}>
              done
            </button>
          </span>
        </div>
      )}
      {people.map((p) => {
        const owns = others.filter((o) => o.owner === p.name).map((o) => o.name);
        return (
          <div key={p.name} style={row}>
            <span style={{ color: "var(--accent)", minWidth: 90 }}>
              {p.name}
              {p.name === me && <span style={{ color: "var(--faint)" }}> (you)</span>}
            </span>
            <RoleSelect p={p} roles={roles} act={act} may={manage} />
            <span style={{ color: "var(--faint)", fontSize: 11, flex: 1 }}>
              <HeldElsewhere p={p} />
              {p.email ?? ""}
              {owns.length > 0 && <span> · owns {owns.join(", ")}</span>}
              {p.invited && <span style={{ color: "var(--warn)" }}> · link not used yet</span>}
              {p.disabled && <span style={{ color: "var(--bad)" }}> · disabled</span>}
            </span>
            <Seen what="last seen" at={p.last_seen} />
            <button
              style={small}
              {...gateOf(manage, { title: "the email they sign in with through the identity provider" })}
              onClick={async () => {
                const email = await dialog.prompt(`${p.name}'s email, for signing in through the identity provider (empty for none):`, p.email ?? "");
                if (email !== null) act(() => changePrincipal(p.name, { email }));
              }}
            >
              email
            </button>
            <button
              style={small}
              {...gateOf(manage, { title: "a new link to set a new password; their sign-ins end" })}
              onClick={async () => {
                if (!(await dialog.confirm(`Reset ${p.name}'s password? Their sign-ins end, and you get a new link for them.`, "Reset"))) return;
                act(async () => {
                  const r = await resetPerson(p.name);
                  setLink({ name: p.name, url: linkFor(r.invite) });
                });
              }}
            >
              reset
            </button>
            <button style={small} onClick={() => act(() => changePrincipal(p.name, { disabled: !p.disabled }))} {...gateOf(manage, { title: p.disabled ? "let them sign in again" : "stop them signing in; their sign-ins end" })}>
              {p.disabled ? "enable" : "disable"}
            </button>
            <button
              style={small}
              {...gateOf(manage, { title: owns.length > 0 ? "give what they own another owner first" : "remove them" })}
              onClick={async () => {
                if (await dialog.confirm(`Remove ${p.name}? What they deployed stays, and says who did it. This can't be undone.`, "Remove")) act(() => removePrincipal(p.name));
              }}
            >
              ×
            </button>
          </div>
        );
      })}
      <Gated may={manage}>
      <form
        style={{ ...row, background: "transparent" }}
        onSubmit={(e) => {
          e.preventDefault();
          act(async () => {
            const r = await invitePerson(draft.name.trim(), draft.role, draft.email.trim());
            setLink({ name: r.name, url: linkFor(r.invite) });
            setDraft({ name: "", role: draft.role, email: "" });
          });
        }}
      >
        <input aria-label="new person's name" placeholder="name" value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} style={input} />
        <select aria-label="new person's role" value={draft.role} title={roleTitle(roles, draft.role)} onChange={(e) => setDraft({ ...draft, role: e.target.value })} style={input}>
          {rolesFor(roles, "person").map((r) => (
            <option key={r.name} value={r.name} title={roleTitle(roles, r.name)}>
              {r.name}
            </option>
          ))}
        </select>
        <input aria-label="new person's email" placeholder="email (optional)" value={draft.email} onChange={(e) => setDraft({ ...draft, email: e.target.value })} style={{ ...input, flex: 1 }} />
        <button type="submit" style={primary} disabled={!draft.name.trim()}>
          add person
        </button>
      </form>
      </Gated>

      <div className="hl-host-page__head" data-section="agents">agents and services</div>
      <div style={about}>
        An agent (an AI, connected through <code>holon mcp</code>) writes flows, checks them, saves and deploys them, and runs their actions, but never approves what a
        flow may do: that stays with a person. A service is a script or CI, a viewer or an editor, never an admin. Each signs in with tokens of its own (sent as
        Authorization: Bearer &lt;token&gt;) and has an owner, the person answerable for it, who makes and revokes its tokens. Which actions agents may run is each
        deployment&apos;s own setting.
      </div>
      {secret && <MadeTokenBox made={secret} onDone={() => setSecret(null)} />}
      {others.length === 0 && <div style={{ ...quiet, marginBottom: 6 }}>no agent or service yet</div>}
      {others.map((p) => (
        <Fragment key={p.name}>
          <div style={row}>
            <span style={{ color: "var(--accent)", minWidth: 90 }}>{p.name}</span>
            <span style={quiet}>{p.kind}</span>
            <RoleSelect p={p} roles={roles} act={act} may={manage} />
            <select aria-label={`${p.name}'s owner`} value={p.owner ?? ""} style={input} {...gateOf(manage, { title: "the person answerable for it" })} onChange={(e) => act(() => changePrincipal(p.name, { owner: e.target.value }))}>
              {owners(p.owner).map((o) => (
                <option key={o} value={o}>
                  owned by {o}
                </option>
              ))}
            </select>
            <span style={{ color: "var(--faint)", fontSize: 11, flex: 1 }}>
              <HeldElsewhere p={p} />
              {p.owner_disabled && <span style={{ color: "var(--warn)" }}>its owner is disabled</span>}
              {p.disabled && <span style={{ color: "var(--bad)" }}>{p.owner_disabled ? " · " : ""}disabled: its tokens are refused</span>}
            </span>
            <Seen what="last seen" at={p.last_seen} />
            <button style={small} aria-label={`make a token for ${p.name}`} {...gateOf(tokensOf(p), { title: "a new token for it, shown once" })} onClick={() => makeToken(p, dialog, act, setSecret)}>
              make token
            </button>
            <button style={small} onClick={() => act(() => changePrincipal(p.name, { disabled: !p.disabled }))} {...gateOf(manage, { title: p.disabled ? "let its tokens sign in again" : "refuse its tokens" })}>
              {p.disabled ? "enable" : "disable"}
            </button>
            <button
              style={small}
              {...gateOf(manage, { title: `remove the ${p.kind}` })}
              onClick={async () => {
                if (await dialog.confirm(`Remove ${p.name}? Its tokens stop working. This can't be undone.`, "Remove")) act(() => removePrincipal(p.name));
              }}
            >
              ×
            </button>
          </div>
          <TokenRows p={p} dialog={dialog} act={act} may={tokensOf(p)} />
        </Fragment>
      ))}
      <Gated may={manage}>
      <form
        style={{ ...row, background: "transparent" }}
        onSubmit={(e) => {
          e.preventDefault();
          act(async () => {
            await addPrincipal(addDraft.name.trim(), addDraft.kind, addDraft.role, addDraft.owner);
            setAddDraft({ ...addDraft, name: "" });
          });
        }}
      >
        <input aria-label="new agent's or service's name" placeholder="name" value={addDraft.name} onChange={(e) => setAddDraft({ ...addDraft, name: e.target.value })} style={input} />
        <select
          aria-label="new agent's or service's kind"
          value={addDraft.kind}
          onChange={(e) => {
            const kind = e.target.value as Exclude<Kind, "person">;
            setAddDraft({ ...addDraft, kind, role: kind === "agent" ? "agent" : "viewer" });
          }}
          style={input}
        >
          <option value="agent">agent</option>
          <option value="service">service</option>
        </select>
        <select aria-label="new agent's or service's role" value={addDraft.role} title={roleTitle(roles, addDraft.role)} onChange={(e) => setAddDraft({ ...addDraft, role: e.target.value })} style={input}>
          {rolesFor(roles, addDraft.kind).map((r) => (
            <option key={r.name} value={r.name} title={roleTitle(roles, r.name)}>
              {r.name}
            </option>
          ))}
        </select>
        <select aria-label="new agent's or service's owner" value={addDraft.owner} onChange={(e) => setAddDraft({ ...addDraft, owner: e.target.value })} style={{ ...input, flex: 1 }}>
          <option value="">owned by you</option>
          {people
            .filter((p) => p.name !== me)
            .map((p) => (
              <option key={p.name} value={p.name}>
                owned by {p.name}
              </option>
            ))}
        </select>
        <button type="submit" style={primary} disabled={!addDraft.name.trim()}>
          add {addDraft.kind}
        </button>
      </form>
      </Gated>
      <ConnectingAnAgent />

      <div className="hl-host-page__head" data-section="sign-in">signing in through an identity provider</div>
      <div style={about}>
        Optional: people may also sign in through your provider (Keycloak, Okta, Auth0, Entra ID…), registered there as a public client
        with PKCE and this page&apos;s address as its redirect. Someone signing in is matched to the person with their email (set under people).
      </div>
      <Gated may={manage}>
      <div style={{ ...row, background: "transparent", flexWrap: "wrap" }}>
        <input aria-label="issuer" placeholder="issuer (https://id.example.com/realms/x)" value={issuer} onChange={(e) => setIssuer(e.target.value)} style={{ ...input, flex: 2, minWidth: 220 }} />
        <input aria-label="client id" placeholder="client id" value={clientId} onChange={(e) => setClientId(e.target.value)} style={{ ...input, flex: 1, minWidth: 120 }} />
        <button style={primary} disabled={!issuer.trim() || !clientId.trim()} onClick={() => act(() => setOidcSettings({ issuer: issuer.trim(), client_id: clientId.trim() }))}>
          {oidc ? "update" : "turn on"}
        </button>
        {oidc && (
          <button style={small} onClick={() => act(() => setOidcSettings(null))}>
            turn off
          </button>
        )}
      </div>
      </Gated>
      <div style={{ color: "var(--faint)", fontSize: 11 }}>{oidc ? `on: ${oidc.issuer} as ${oidc.client_id}` : "off"}</div>
    </HostPage>
  );
}

export function ResourcesPage({ tab, deployments, onOpenDeployment }: { tab: ResourcesTab; deployments: DeploymentEntry[]; onOpenDeployment: (slug: string) => void }) {
  const h = useHost(true, tab);
  // Binding (and a server secret's where and roll back), unbinding, and the host's storage and
  // sweeps (can.rs's routes: resource.bind, resource.unbind, host.operate).
  const can = useCan(["resource.bind", "resource.unbind", "host.operate"]);
  const [bind, unbind, operate] = [can.may("resource.bind"), can.may("resource.unbind"), can.may("host.operate")];
  // Where a database or a folder is (its path, where its connection is kept) is shown to whoever
  // may bind one, not to whoever only views them (web-console-L4.md §12 Q4, the narrower answer):
  // the server leaves it out of their lists.
  const seesWhere = can.holds("resource.bind");
  const secretValue = useRef<HTMLInputElement>(null);
  const { setMsg, folders, folderDraft, setFolderDraft, keepFor, setKeepFor, browsing, setBrowsing, databases, databaseDraft, setDatabaseDraft, tables, setTables, serverSecrets, secretDraft, setSecretDraft, grants, swept, setSwept, host, setHost, everyDraft, setEveryDraft, warnDraft, setWarnDraft, fullDraft, setFullDraft, backupDraft, setBackupDraft, kind, setKind, dialog, sweeps, refresh, readTables, act, folderHolders, databaseHolders, unbindingSays, mails, mailDraft, setMailDraft, noMail, feeds, feedDraft, setFeedDraft, noFeed, queues, queueDraft, setQueueDraft, noQueue, mailHolders, feedHolders, queueHolders } = h;
  return (
    <HostPage title="Resources" about="what exists on this host for flows to use, and who may touch what" tab={null} host={h}>
      <nav className="hl-flow__tabs" aria-label="resources tabs">
        {RESOURCES_TABS.map((t) => (
          <a key={t} href={format({ kind: "resources", tab: t })} aria-current={t === tab ? "page" : undefined}>
            {RESOURCES_LABELS[t]}
          </a>
        ))}
      </nav>

      {tab === "databases" && (
        <>
          <div style={about}>
            Databases flows read and write: a SQLite file on this server&apos;s machine, or a Postgres, MySQL or MongoDB database whose URL the server fetches
            from where it is kept, as a secret&apos;s (<code>env:HOLON_SECRET_SHOP_URL</code>, <code>vault:…</code>), never typed in here, stored or shown. A
            flow names a database (<code>database shop</code>), never where it is, and is granted each table its SQL reads or writes (each collection its
            calls name). A schema (create table …) is run when it is bound, creating a SQLite file if there is none; flows run no DDL. Read only: flows may
            read it, not write it. A writable database gets a table holon_commits, where each run&apos;s commit marks it done. Held for whole runs: each run
            that touches it holds it from before it starts until it commits, fails or waits, so every run that writes it goes one at a time, for its whole
            length, across every deployment bound to it (and, on Postgres, MySQL or MongoDB, every server); one that waits past its claim_wait is turned
            away (503). For what a commit&apos;s check can&apos;t cover: a sequence&apos;s next value, a queue, or contention so high that running again costs
            more than waiting.
          </div>
          <ResourceTable
            kind="database"
            empty="no databases bound"
            known={h.grantsKnown}
            rows={databases.map((d) => {
              const read = tables[keyOf(d)];
              const holders = databaseHolders(d);
              const waiting = waitingFor("database", d, databases, h.proposals);
              return {
                ...databaseLine(d, seesWhere),
                holders,
                waiting,
                actions: (
                  <button
                    style={small}
                    aria-label={`unbind ${d.name}`}
                    {...gateOf(unbind, { title: "unbind the name: the database stays where it is" })}
                    onClick={async () => {
                      const whose = d.deployment ? ` for ${d.deployment}` : "";
                      if (await dialog.confirm(`Unbind the database "${d.name}"${whose}? It stays where it is. ${unbindingSays(holders)}`, "Unbind"))
                        act(() => unbindDatabase(d.name, d.deployment));
                    }}
                  >
                    ×
                  </button>
                ),
                below: (
                  <>
                    <div aria-label={`${d.name}'s tables`} style={{ ...quiet, display: "flex", gap: 8, alignItems: "center", marginBottom: 6 }}>
                      <span style={{ flex: 1, overflowWrap: "anywhere" }}>
                        {read === undefined
                          ? "reading its tables…"
                          : "error" in read
                            ? <span style={{ color: "var(--bad)" }}>its tables couldn&apos;t be read: {read.error}</span>
                            : read.length === 0
                              ? `no ${d.engine === "mongodb" ? "collections" : "tables"} yet`
                              : `${d.engine === "mongodb" ? "collections" : "tables"}: ${read.join(", ")}`}
                      </span>
                      <button style={small} title="read its tables again, after a migration" onClick={() => readTables(d)}>
                        read the schema again
                      </button>
                    </div>
                    <DatabaseMatrix name={d.name} matrix={databaseMatrix(Array.isArray(read) ? read : [], holders, waiting)} known={h.grantsKnown} />
                  </>
                ),
              };
            })}
          />
          <Gated may={bind}>
          <form
            style={{ ...row, background: "transparent", flexWrap: "wrap" }}
            onSubmit={async (e) => {
              e.preventDefault();
              const { where, ...d } = databaseDraft;
              const at = where.trim();
              if (isUrl(at)) {
                setMsg("Give where the URL is kept (env:HOLON_SECRET_SHOP_URL or vault:…), not the URL: the server fetches it from there, and this page never sends it.");
                return;
              }
              const name = d.name.trim();
              const deployment = d.deployment?.trim() || null;
              const there = databases.find((b) => b.name === name && b.deployment === deployment);
              if (there && !(await dialog.confirm(`Bind "${name}" again? It replaces its binding. ${unbindingSays(databaseHolders(there)).replace(/: they keep running.*/, ": their next runs use the new one.")}`, "Bind again")))
                return;
              act(async () => {
                const target = at.startsWith("env:") || at.startsWith("vault:") ? { connection: at } : { path: at };
                await bindDatabase({ ...d, ...target, name, schema: d.schema?.trim() || null, deployment });
                setTables((all) => Object.fromEntries(Object.entries(all).filter(([k]) => k !== keyOf({ name, deployment }))));
                setDatabaseDraft({ ...databaseDraft, name: "", where: "", schema: null, lock: "commit", deployment: null });
              });
            }}
          >
            <input aria-label="database name" placeholder="name (shop)" value={databaseDraft.name} onChange={(e) => setDatabaseDraft({ ...databaseDraft, name: e.target.value })} style={input} />
            <input
              aria-label="database path"
              placeholder="a SQLite file's whole path on this server's machine, or where a Postgres, MySQL or MongoDB URL is kept (env:HOLON_SECRET_SHOP_URL, vault:…)"
              value={databaseDraft.where}
              onChange={(e) => setDatabaseDraft({ ...databaseDraft, where: e.target.value })}
              style={{ ...input, flex: 1, minWidth: 220 }}
            />
            <select
              aria-label="database access"
              value={databaseDraft.access}
              onChange={(e) => setDatabaseDraft({ ...databaseDraft, access: e.target.value as DatabaseEntry["access"] })}
              style={input}
            >
              <option value="read-write">read and write</option>
              <option value="read">read only</option>
            </select>
            <label style={{ color: "var(--muted)", fontSize: 11, display: "flex", alignItems: "center", gap: 4 }} title={`each run that touches it holds it for its whole length: ${RUN_LOCK_COST}`}>
              <input
                type="checkbox"
                aria-label="database held for whole runs"
                checked={databaseDraft.lock === "run"}
                onChange={(e) => setDatabaseDraft({ ...databaseDraft, lock: e.target.checked ? "run" : "commit" })}
              />
              held for whole runs
            </label>
            <input
              aria-label="database for one deployment"
              placeholder="for one deployment (empty: all)"
              value={databaseDraft.deployment ?? ""}
              onChange={(e) => setDatabaseDraft({ ...databaseDraft, deployment: e.target.value })}
              style={input}
            />
            {databaseDraft.lock === "run" && <div style={{ ...quiet, color: "var(--warn)", flexBasis: "100%" }}>held for whole runs: {RUN_LOCK_COST}</div>}
            {isUrl(databaseDraft.where) && (
              <div role="alert" style={{ ...quiet, color: "var(--bad)", flexBasis: "100%" }}>
                That is a URL: give where it is kept instead (env:HOLON_SECRET_SHOP_URL, vault:…). It isn&apos;t sent.
              </div>
            )}
            <textarea
              aria-label="database schema"
              placeholder="schema, run when it is bound (create table orders (id integer primary key, …)); empty: the database is there already (a MongoDB has none)"
              value={databaseDraft.schema ?? ""}
              onChange={(e) => setDatabaseDraft({ ...databaseDraft, schema: e.target.value })}
              rows={2}
              style={{ ...input, flexBasis: "100%", fontFamily: "monospace" }}
            />
            <button type="submit" style={primary} disabled={!databaseDraft.name.trim() || !databaseDraft.where.trim() || isUrl(databaseDraft.where)}>
              bind database
            </button>
          </form>
          </Gated>
        </>
      )}

      {tab === "folders" && (
        <>
          <div style={about}>
            Files flows read and write on this server&apos;s machine. A flow names a folder (<code>folder inbox</code>), never a path: bind each name to a real folder here.
            Read only: flows may read it, not write it. Reads kept by hash: the run log keeps a hash of what each run read, not its text; for a while (7d, 12h):
            its text for that long, then only its hash. A cap (10M): a write that would take the folder&apos;s files past it doesn&apos;t land. For one deployment:
            that deployment sees this folder by the name, whatever the server&apos;s is, and no other deployment sees it. Other servers bind it too (a network share):
            their runs&apos; commits lock a file in it, so they never write over each other. Held for whole runs: each run that touches it holds it from before
            it starts until it commits, fails or waits, so every run that writes it goes one at a time, for its whole length, across every deployment bound to
            it; one that waits past its claim_wait is turned away (503). For files a program expects one writer for.
          </div>
          <ResourceTable
            kind="folder"
            empty="no folders bound"
            known={h.grantsKnown}
            rows={folders.map((f) => ({
              ...folderLine(f, seesWhere),
              holders: folderHolders(f),
              waiting: waitingFor("folder", f, folders, h.proposals),
              actions: (
                <button
                  style={small}
                  aria-label={`unbind ${f.name}`}
                  {...gateOf(unbind, { title: "unbind the name: the files stay where they are" })}
                  onClick={async () => {
                    const whose = f.deployment ? ` for ${f.deployment}` : "";
                    if (await dialog.confirm(`Unbind the folder "${f.name}"${whose}? Its files stay where they are. ${unbindingSays(folderHolders(f))}`, "Unbind"))
                      act(() => unbindFolder(f.name, f.deployment));
                  }}
                >
                  ×
                </button>
              ),
            }))}
          />
          <Gated may={bind}>
          <form
            style={{ ...row, background: "transparent", flexWrap: "wrap" }}
            onSubmit={async (e) => {
              e.preventDefault();
              const name = folderDraft.name.trim();
              const deployment = folderDraft.deployment?.trim() || null;
              const there = folders.find((f) => f.name === name && f.deployment === deployment);
              if (there && !(await dialog.confirm(`Bind "${name}" again? It replaces the binding${there.path ? ` to ${there.path}` : ""}. ${unbindingSays(folderHolders(there)).replace(/: they keep running.*/, ": their next runs use the new one.")}`, "Bind again")))
                return;
              act(async () => {
                const keep = folderDraft.keep === "bytes for " ? `bytes for ${keepFor.trim()}` : folderDraft.keep;
                const cap = typeof folderDraft.cap === "string" ? folderDraft.cap.trim() || null : folderDraft.cap;
                await bindFolder({ ...folderDraft, name, path: folderDraft.path.trim(), keep: keep as FolderEntry["keep"], cap, deployment });
                setFolderDraft({ ...folderDraft, name: "", path: "", cap: null, shared: false, lock: "commit", deployment: null });
              });
            }}
          >
            <input aria-label="folder name" placeholder="name (inbox)" value={folderDraft.name} onChange={(e) => setFolderDraft({ ...folderDraft, name: e.target.value })} style={input} />
            <input
              aria-label="folder path"
              placeholder="the whole path on this server's machine"
              value={folderDraft.path}
              onChange={(e) => setFolderDraft({ ...folderDraft, path: e.target.value })}
              style={{ ...input, flex: 1, minWidth: 220 }}
            />
            <button type="button" style={small} title="choose the folder on the server's machine" onClick={() => setBrowsing(true)}>
              browse…
            </button>
            <select aria-label="folder access" value={folderDraft.access} onChange={(e) => setFolderDraft({ ...folderDraft, access: e.target.value as FolderEntry["access"] })} style={input}>
              <option value="read-write">read and write</option>
              <option value="read">read only</option>
            </select>
            <select aria-label="what the log keeps" value={folderDraft.keep} onChange={(e) => setFolderDraft({ ...folderDraft, keep: e.target.value as FolderEntry["keep"] })} style={input}>
              <option value="bytes">log keeps what was read</option>
              <option value="bytes for ">log keeps what was read, for a while</option>
              <option value="hash">log keeps a hash</option>
            </select>
            {folderDraft.keep === "bytes for " && (
              <input aria-label="keep for how long" placeholder="for how long (7d, 12h)" value={keepFor} onChange={(e) => setKeepFor(e.target.value)} style={{ ...input, width: 130 }} />
            )}
            <input
              aria-label="folder cap"
              placeholder="cap (10M; empty: none)"
              value={folderDraft.cap ?? ""}
              onChange={(e) => setFolderDraft({ ...folderDraft, cap: e.target.value })}
              style={{ ...input, width: 150 }}
            />
            <label style={{ color: "var(--muted)", fontSize: 11, display: "flex", alignItems: "center", gap: 4 }} title="a network share other Holon servers bind too: a commit also locks a file in it">
              <input type="checkbox" checked={folderDraft.shared ?? false} onChange={(e) => setFolderDraft({ ...folderDraft, shared: e.target.checked })} />
              other servers bind it too
            </label>
            <label style={{ color: "var(--muted)", fontSize: 11, display: "flex", alignItems: "center", gap: 4 }} title={`each run that touches it holds it for its whole length: ${RUN_LOCK_COST}`}>
              <input
                type="checkbox"
                aria-label="folder held for whole runs"
                checked={folderDraft.lock === "run"}
                onChange={(e) => setFolderDraft({ ...folderDraft, lock: e.target.checked ? "run" : "commit" })}
              />
              held for whole runs
            </label>
            <input
              aria-label="for one deployment"
              placeholder="for one deployment (empty: all)"
              value={folderDraft.deployment ?? ""}
              onChange={(e) => setFolderDraft({ ...folderDraft, deployment: e.target.value })}
              style={input}
            />
            {folderDraft.lock === "run" && <div style={{ ...quiet, color: "var(--warn)", flexBasis: "100%" }}>held for whole runs: {RUN_LOCK_COST}</div>}
            <button
              type="submit"
              style={primary}
              disabled={!folderDraft.name.trim() || !folderDraft.path.trim() || (folderDraft.keep === "bytes for " && !keepFor.trim())}
            >
              bind folder
            </button>
          </form>
          </Gated>
          {/* Outside the form: its own buttons and form would submit the binding. */}
          {browsing && (
            <FolderPicker
              start={folderDraft.path}
              onChoose={(path) => {
                setFolderDraft((d) => ({ ...d, path }));
                setBrowsing(false);
              }}
              onClose={() => setBrowsing(false)}
            />
          )}
        </>
      )}

      {tab === "mail" && (
        <>
          <div style={about}>
            The mail flows send: each name an SMTP server whose URL, with its login, the server fetches from where it is kept, as a secret&apos;s
            (<code>env:HOLON_SECRET_OPS_SMTP</code>, <code>vault:…</code>), never typed in here, stored or shown; it logs in when it is bound. A flow names
            the mail (<code>import ops from &quot;mail:ops&quot;</code>), never the server, and never chooses the sender: from is the binding&apos;s. To is
            every address it may ever send to (addresses, *@domain, or * for anyone): a flow is granted each recipient its text writes, or any address it
            computes within these. Mail goes once its run is over, and is tried again for a day while the server can&apos;t take it. A mail bound to a
            mailbox as well (or instead: an IMAP server&apos;s URL kept the same way) starts a run of <code>on.mail(…)</code> for each message that comes to
            its folder, then moves it to done; whose verdict counts is the name the mailbox&apos;s own server writes in its Authentication-Results header
            (without one, no message is authenticated). A server that takes no password (Microsoft 365, Google Workspace) logs in with a token: its URL names
            the user and no password, and the OAuth source keeps the token request (https://id:secret@host/…/token?scope=…), which the server sends to get one.
          </div>
          <ResourceTable
            kind="mail"
            empty="no mail bound"
            known={h.grantsKnown}
            rows={mails.map((m) => ({
              ...mailLine(m),
              holders: mailHolders(m),
              waiting: waitingFor("mail", m, mails, h.proposals),
              actions: (
                <button
                  style={small}
                  aria-label={`unbind ${m.name}`}
                  {...gateOf(unbind, { title: "unbind the name: mail on its way through it is given up" })}
                  onClick={async () => {
                    const whose = m.deployment ? ` for ${m.deployment}` : "";
                    if (await dialog.confirm(`Unbind the mail "${m.name}"${whose}? Mail on its way through it is given up. ${unbindingSays(mailHolders(m))}`, "Unbind"))
                      act(() => unbindMail(m.name, m.deployment));
                  }}
                >
                  ×
                </button>
              ),
            }))}
          />
          <Gated may={bind}>
          <form
            style={{ ...row, background: "transparent", flexWrap: "wrap" }}
            onSubmit={async (e) => {
              e.preventDefault();
              const name = mailDraft.name.trim();
              const deployment = mailDraft.deployment?.trim() || null;
              const there = mails.find((b) => b.name === name && b.deployment === deployment);
              if (there && !(await dialog.confirm(`Bind "${name}" again? It replaces its binding. ${unbindingSays(mailHolders(there)).replace(/: they keep running.*/, ": their next mail goes through the new one.")}`, "Bind again")))
                return;
              act(async () => {
                const t = (s: string | undefined) => (s ?? "").trim();
                await bindMail({ name, smtp: t(mailDraft.smtp), from: t(mailDraft.from), to: t(mailDraft.to), imap: t(mailDraft.imap), folder: t(mailDraft.folder), done: t(mailDraft.done), verdict_from: t(mailDraft.verdict_from), oauth: t(mailDraft.oauth), deployment });
                setMailDraft(noMail);
              });
            }}
          >
            <input aria-label="mail name" placeholder="name (ops)" value={mailDraft.name} onChange={(e) => setMailDraft({ ...mailDraft, name: e.target.value })} style={input} />
            <input
              aria-label="mail server"
              placeholder="where the SMTP server's URL is kept (env:HOLON_SECRET_OPS_SMTP, vault:…)"
              value={mailDraft.smtp}
              onChange={(e) => setMailDraft({ ...mailDraft, smtp: e.target.value })}
              style={{ ...input, flex: 1, minWidth: 220 }}
            />
            <input aria-label="mail sender" placeholder="from (Treasury <treasury@acme.com>)" value={mailDraft.from} onChange={(e) => setMailDraft({ ...mailDraft, from: e.target.value })} style={input} />
            <input
              aria-label="mail recipients"
              placeholder="to: every address it may send to (*@acme.com, bank@example.com)"
              value={mailDraft.to}
              onChange={(e) => setMailDraft({ ...mailDraft, to: e.target.value })}
              style={{ ...input, flex: 1, minWidth: 220 }}
            />
            <input
              aria-label="mailbox"
              placeholder="to receive: where the IMAP server's URL is kept (env:HOLON_SECRET_REMIT_IMAP)"
              value={mailDraft.imap ?? ""}
              onChange={(e) => setMailDraft({ ...mailDraft, imap: e.target.value })}
              style={{ ...input, flex: 1, minWidth: 220 }}
            />
            <input aria-label="mailbox folder" placeholder="folder (INBOX)" value={mailDraft.folder ?? ""} onChange={(e) => setMailDraft({ ...mailDraft, folder: e.target.value })} style={input} />
            <input aria-label="mailbox done" placeholder="moved to (Processed)" value={mailDraft.done ?? ""} onChange={(e) => setMailDraft({ ...mailDraft, done: e.target.value })} style={input} />
            <input
              aria-label="mailbox verdict from"
              placeholder="whose verdict counts (mx.acme.com)"
              value={mailDraft.verdict_from ?? ""}
              onChange={(e) => setMailDraft({ ...mailDraft, verdict_from: e.target.value })}
              style={input}
            />
            <input
              aria-label="mail oauth"
              placeholder="OAuth: where the token request is kept (env:HOLON_SECRET_SUPPORT_OAUTH)"
              value={mailDraft.oauth ?? ""}
              onChange={(e) => setMailDraft({ ...mailDraft, oauth: e.target.value })}
              style={{ ...input, flex: 1, minWidth: 220 }}
            />
            <input
              aria-label="mail for one deployment"
              placeholder="for one deployment (empty: all)"
              value={mailDraft.deployment ?? ""}
              onChange={(e) => setMailDraft({ ...mailDraft, deployment: e.target.value })}
              style={input}
            />
            {(isUrl(mailDraft.smtp) || isUrl(mailDraft.imap ?? "") || isUrl(mailDraft.oauth ?? "")) && (
              <div role="alert" style={{ ...quiet, color: "var(--bad)", flexBasis: "100%" }}>
                That is a URL: give where it is kept instead (env:HOLON_SECRET_OPS_SMTP, vault:…). It isn&apos;t sent.
              </div>
            )}
            <button
              type="submit"
              style={primary}
              disabled={
                !mailDraft.name.trim() ||
                (!mailDraft.smtp.trim() && !(mailDraft.imap ?? "").trim()) ||
                (!!mailDraft.smtp.trim() && (!mailDraft.from.trim() || !mailDraft.to.trim())) ||
                isUrl(mailDraft.smtp) ||
                isUrl(mailDraft.imap ?? "") ||
                isUrl(mailDraft.oauth ?? "")
              }
            >
              bind mail
            </button>
          </form>
          </Gated>
        </>
      )}

      {tab === "feeds" && (
        <>
          <div style={about}>
            The feeds flows listen to: each name a feed of server-sent events, or a WebSocket (wss://), whose URL the server fetches from where it is kept, as a secret&apos;s
            (<code>env:HOLON_SECRET_PRICES_URL</code>, <code>vault:…</code>), never typed in here, stored or shown, and so are the values of the headers each
            request sends (Authorization=env:HOLON_SECRET_PRICES_AUTH, one a line). It is bound once a request to it answers as a feed does. A flow names the
            feed (<code>import prices from &quot;feed:prices&quot;</code>) and runs <code>on.feed(prices, (e) =&gt; …)</code> for each event it sends, one at
            a time, in the order they came; the server keeps the connection and where it is, opens it again from there when it is lost, and passes over an
            event it ran already. How many may wait for their run (1000), and what comes when that many do: it waits, it is dropped (the next event&apos;s{" "}
            <code>e.missed</code> says how many were), or it replaces the waiting event of its key (<code>e.replaced</code>); and how long the feed may say
            nothing before it is opened again (90s). A WebSocket is also sent its subscription when it opens (a message as written, or a source for one with a
            key in it) and a heartbeat to keep it; the fields of a message that are its id (product_id,sequence) say where it is, and only the messages whose
            field has a value given (type=ticker) run. A WebSocket that replays is opened again from its id where the binding says the feed takes it:
            query:cursor, header:NAME, or message:… sent in place of the subscription, <code>{"{position}"}</code> in it the id. A feed whose JSON messages
            a whole number counts one by one (trade_id), each key on its own (product_id), says on each event how many numbers were skipped before it (
            <code>e.gap</code>): what came while the server wasn&apos;t reading, for the flow to fetch. A feed may also be a Postgres channel: the server
            listens on it (<code>LISTEN</code>) in a database bound here, the one each deployment sees by that name, and each notification
            (<code>pg_notify</code>, from a trigger) is an event; what is sent while it isn&apos;t listening is lost, for the flow to read from the table.
          </div>
          <ResourceTable
            kind="feed"
            empty="no feed bound"
            known={h.grantsKnown}
            rows={feeds.map((f) => ({
              ...feedLine(f),
              holders: feedHolders(f),
              waiting: waitingFor("feed", f, feeds, h.proposals),
              actions: (
                <button
                  style={small}
                  aria-label={`unbind ${f.name}`}
                  {...gateOf(unbind, { title: "unbind the name: its connections close" })}
                  onClick={async () => {
                    const whose = f.deployment ? ` for ${f.deployment}` : "";
                    if (await dialog.confirm(`Unbind the feed "${f.name}"${whose}? Its connections close. ${unbindingSays(feedHolders(f))}`, "Unbind")) act(() => unbindFeed(f.name, f.deployment));
                  }}
                >
                  ×
                </button>
              ),
            }))}
          />
          <Gated may={bind}>
          <form
            style={{ ...row, background: "transparent", flexWrap: "wrap" }}
            onSubmit={async (e) => {
              e.preventDefault();
              const name = feedDraft.name.trim();
              const deployment = feedDraft.deployment.trim() || null;
              const there = feeds.find((b) => b.name === name && b.deployment === deployment);
              if (there && !(await dialog.confirm(`Bind "${name}" again? It replaces its binding. ${unbindingSays(feedHolders(there)).replace(/: they keep running.*/, ": their connections open again to the new one.")}`, "Bind again")))
                return;
              act(async () => {
                const headers: Record<string, string> = {};
                for (const line of feedDraft.headers.split("\n").map((l) => l.trim()).filter(Boolean)) {
                  const at = line.indexOf("=");
                  if (at < 1) throw new Error(`"${line}" isn't a header: NAME=SOURCE (Authorization=env:HOLON_SECRET_PRICES_AUTH)`);
                  headers[line.slice(0, at).trim()] = line.slice(at + 1).trim();
                }
                const channel = feedDraft.from === "channel";
                const binding: FeedBinding = channel
                  ? { name, database: feedDraft.database.trim(), channel: feedDraft.channel.trim(), deployment }
                  : { name, url: feedDraft.url.trim(), headers, deployment };
                if (feedDraft.buffer.trim()) binding.buffer = Number(feedDraft.buffer.trim());
                if (feedDraft.idle.trim()) binding.idle = feedDraft.idle.trim();
                if (feedDraft.onFull !== "wait") binding.on_full = feedDraft.onFull;
                if (feedDraft.key.trim()) binding.key = feedDraft.key.trim();
                if (feedDraft.sequence.trim()) binding.sequence = feedDraft.sequence.trim();
                const subscribe = feedDraft.subscribe.split("\n").map((l) => l.trim()).filter(Boolean);
                if (subscribe.length > 0 && !channel) binding.subscribe = subscribe;
                if (feedDraft.heartbeat.trim() && !channel) binding.heartbeat = feedDraft.heartbeat.trim();
                if (feedDraft.every.trim() && !channel) binding.every = feedDraft.every.trim();
                if (feedDraft.id.trim()) binding.id = feedDraft.id.trim();
                const only = feedDraft.only.split(/\s+/).filter(Boolean);
                if (only.length > 0) binding.only = only;
                const resume = feedDraft.resume.split("\n").map((l) => l.trim()).filter(Boolean);
                if (resume.length > 0 && !channel) binding.resume = resume;
                await bindFeed(binding);
                setFeedDraft(noFeed);
              });
            }}
          >
            <input aria-label="feed name" placeholder="name (prices)" value={feedDraft.name} onChange={(e) => setFeedDraft({ ...feedDraft, name: e.target.value })} style={input} />
            <select aria-label="feed from" value={feedDraft.from} onChange={(e) => setFeedDraft({ ...feedDraft, from: e.target.value as "url" | "channel" })} style={input}>
              <option value="url">from a URL</option>
              <option value="channel">a Postgres channel</option>
            </select>
            {feedDraft.from === "url" ? (
              <input
                aria-label="feed url"
                placeholder="where the feed's URL is kept (env:HOLON_SECRET_PRICES_URL, vault:…)"
                value={feedDraft.url}
                onChange={(e) => setFeedDraft({ ...feedDraft, url: e.target.value })}
                style={{ ...input, flex: 1, minWidth: 220 }}
              />
            ) : (
              <>
                <select aria-label="feed database" value={feedDraft.database} onChange={(e) => setFeedDraft({ ...feedDraft, database: e.target.value })} style={input}>
                  <option value="">its database</option>
                  {[...new Set(databases.filter((d) => d.engine === "postgres").map((d) => d.name))].map((n) => (
                    <option key={n} value={n}>
                      {n}
                    </option>
                  ))}
                </select>
                <input
                  aria-label="feed channel"
                  placeholder="channel (ledger_entries)"
                  value={feedDraft.channel}
                  onChange={(e) => setFeedDraft({ ...feedDraft, channel: e.target.value })}
                  style={{ ...input, flex: 1, minWidth: 160 }}
                />
              </>
            )}
            <input aria-label="feed buffer" placeholder="may wait (1000)" value={feedDraft.buffer} onChange={(e) => setFeedDraft({ ...feedDraft, buffer: e.target.value })} style={input} />
            <select aria-label="feed on full" value={feedDraft.onFull} onChange={(e) => setFeedDraft({ ...feedDraft, onFull: e.target.value as OnFull })} style={input}>
              <option value="wait">when full: wait</option>
              <option value="skip">when full: skip</option>
              <option value="latest">latest of each key</option>
            </select>
            <input
              aria-label="feed sequence"
              placeholder="numbered by (trade_id)"
              value={feedDraft.sequence}
              onChange={(e) => setFeedDraft({ ...feedDraft, sequence: e.target.value })}
              style={input}
            />
            {(feedDraft.onFull === "latest" || feedDraft.sequence.trim() !== "") && (
              <input aria-label="feed key" placeholder="its key (product_id)" value={feedDraft.key} onChange={(e) => setFeedDraft({ ...feedDraft, key: e.target.value })} style={input} />
            )}
            <input aria-label="feed idle" placeholder="quiet for at most (90s)" value={feedDraft.idle} onChange={(e) => setFeedDraft({ ...feedDraft, idle: e.target.value })} style={input} />
            <input
              aria-label="feed for one deployment"
              placeholder="for one deployment (empty: all)"
              value={feedDraft.deployment}
              onChange={(e) => setFeedDraft({ ...feedDraft, deployment: e.target.value })}
              style={input}
            />
            {feedDraft.from === "url" && (
              <textarea
                aria-label="feed headers"
                placeholder="headers each request sends, NAME=SOURCE, one a line (Authorization=env:HOLON_SECRET_PRICES_AUTH)"
                value={feedDraft.headers}
                onChange={(e) => setFeedDraft({ ...feedDraft, headers: e.target.value })}
                rows={2}
                style={{ ...input, flexBasis: "100%", fontFamily: "monospace" }}
              />
            )}
            {feedDraft.from === "channel" && (
              <>
                <input aria-label="feed id" placeholder="its id fields (id)" value={feedDraft.id} onChange={(e) => setFeedDraft({ ...feedDraft, id: e.target.value })} style={input} />
                <input aria-label="feed only" placeholder="runs only (kind=credit)" value={feedDraft.only} onChange={(e) => setFeedDraft({ ...feedDraft, only: e.target.value })} style={input} />
              </>
            )}
            {feedDraft.from === "url" && (/^\s*wss?:/.test(feedDraft.url) || /^\s*(env|vault):/.test(feedDraft.url)) ? (
              <>
                <textarea
                  aria-label="feed subscribe"
                  placeholder={'a WebSocket\'s: what it is sent when it opens, one a line, as written or a source ({"type":"subscribe","channels":["ticker"]})'}
                  value={feedDraft.subscribe}
                  onChange={(e) => setFeedDraft({ ...feedDraft, subscribe: e.target.value })}
                  rows={2}
                  style={{ ...input, flexBasis: "100%", fontFamily: "monospace" }}
                />
                <input aria-label="feed id" placeholder="its id fields (product_id,sequence)" value={feedDraft.id} onChange={(e) => setFeedDraft({ ...feedDraft, id: e.target.value })} style={input} />
                <input aria-label="feed only" placeholder="runs only (type=ticker)" value={feedDraft.only} onChange={(e) => setFeedDraft({ ...feedDraft, only: e.target.value })} style={input} />
                <textarea
                  aria-label="feed resume"
                  placeholder="resumes from its id with (query:cursor, header:NAME, or message:… one a line)"
                  value={feedDraft.resume}
                  onChange={(e) => setFeedDraft({ ...feedDraft, resume: e.target.value })}
                  rows={1}
                  style={{ ...input, fontFamily: "monospace" }}
                />
                <input aria-label="feed heartbeat" placeholder="heartbeat (ping)" value={feedDraft.heartbeat} onChange={(e) => setFeedDraft({ ...feedDraft, heartbeat: e.target.value })} style={input} />
                {feedDraft.heartbeat.trim() && (
                  <input aria-label="feed every" placeholder="every (30s)" value={feedDraft.every} onChange={(e) => setFeedDraft({ ...feedDraft, every: e.target.value })} style={input} />
                )}
              </>
            ) : null}
            {feedDraft.from === "url" && isUrl(feedDraft.url) && (
              <div role="alert" style={{ ...quiet, color: "var(--bad)", flexBasis: "100%" }}>
                That is a URL: give where it is kept instead (env:HOLON_SECRET_PRICES_URL, vault:…). It isn&apos;t sent.
              </div>
            )}
            <button
              type="submit"
              style={primary}
              disabled={!feedDraft.name.trim() || (feedDraft.from === "url" ? !feedDraft.url.trim() || isUrl(feedDraft.url) : !feedDraft.database || !feedDraft.channel.trim())}
            >
              bind feed
            </button>
          </form>
          </Gated>
        </>
      )}

      {tab === "queues" && (
        <>
          <div style={about}>
            The queues flows take messages from and send to: each name an Amazon SQS queue, whose URL with its access key
            (https://&lt;key id&gt;:&lt;secret&gt;@sqs.&lt;region&gt;.amazonaws.com/&lt;account&gt;/&lt;queue&gt;) the server fetches from where it is kept, as a
            secret&apos;s (<code>env:HOLON_SECRET_EVENTS_URL</code>, <code>vault:…</code>), never typed in here, stored or shown. It is bound once the queue
            answers. A flow names the queue (<code>import payout_events from &quot;queue:payout_events&quot;</code>) and runs{" "}
            <code>on.queue(payout_events, (m) =&gt; …)</code> for each message it holds, one at a time, in the order received: the message is deleted once its
            run&apos;s record is on disk, so a crash runs it again rather than losing it, and one handed out again is passed over. One deployment listens to a
            queue. A flow sends with <code>payout_retries.send({"{ data, group }"})</code>: the message goes once the run is over and its writes have landed,
            tried again for a day if the queue can&apos;t take it yet, a FIFO queue&apos;s in the group given and deduplicated by its run. How long a message
            received is kept out of sight at first (5m, kept so while it runs); and whether a failed run&apos;s message is left in the queue, for its redrive
            policy to move, rather than deleted.
          </div>
          <ResourceTable
            kind="queue"
            empty="no queue bound"
            known={h.grantsKnown}
            rows={queues.map((q) => ({
              ...queueLine(q),
              holders: queueHolders(q),
              waiting: waitingFor("queue", q, queues, h.proposals),
              actions: (
                <button
                  style={small}
                  aria-label={`unbind ${q.name}`}
                  {...gateOf(unbind, { title: "unbind the name: its listeners stop" })}
                  onClick={async () => {
                    const whose = q.deployment ? ` for ${q.deployment}` : "";
                    if (await dialog.confirm(`Unbind the queue "${q.name}"${whose}? Its listeners stop, and what it holds stays in it. ${unbindingSays(queueHolders(q))}`, "Unbind"))
                      act(() => unbindQueue(q.name, q.deployment));
                  }}
                >
                  ×
                </button>
              ),
            }))}
          />
          <Gated may={bind}>
          <form
            style={{ ...row, background: "transparent", flexWrap: "wrap" }}
            onSubmit={async (e) => {
              e.preventDefault();
              const name = queueDraft.name.trim();
              const deployment = queueDraft.deployment.trim() || null;
              const there = queues.find((b) => b.name === name && b.deployment === deployment);
              if (there && !(await dialog.confirm(`Bind "${name}" again? It replaces its binding. ${unbindingSays(queueHolders(there)).replace(/: they keep running.*/, ": their listeners start again on the new one.")}`, "Bind again")))
                return;
              act(async () => {
                const visibility = queueDraft.visibility.trim();
                await bindQueue({ name, url: queueDraft.url.trim(), deployment, ...(visibility ? { visibility } : {}), ...(queueDraft.keepFailed ? { keep_failed: true } : {}) });
                setQueueDraft(noQueue);
              });
            }}
          >
            <input aria-label="queue name" placeholder="name (payout_events)" value={queueDraft.name} onChange={(e) => setQueueDraft({ ...queueDraft, name: e.target.value })} style={input} />
            <input
              aria-label="queue url"
              placeholder="where the queue's URL is kept (env:HOLON_SECRET_EVENTS_URL, vault:…)"
              value={queueDraft.url}
              onChange={(e) => setQueueDraft({ ...queueDraft, url: e.target.value })}
              style={{ ...input, flex: 1, minWidth: 220 }}
            />
            <input
              aria-label="queue visibility"
              placeholder="out of sight for (5m)"
              value={queueDraft.visibility}
              onChange={(e) => setQueueDraft({ ...queueDraft, visibility: e.target.value })}
              style={input}
            />
            <label style={{ ...quiet, display: "flex", gap: 4, alignItems: "center" }}>
              <input aria-label="queue keep failed" type="checkbox" checked={queueDraft.keepFailed} onChange={(e) => setQueueDraft({ ...queueDraft, keepFailed: e.target.checked })} />
              leave a failed run&apos;s message
            </label>
            <input
              aria-label="queue for one deployment"
              placeholder="for one deployment (empty: all)"
              value={queueDraft.deployment}
              onChange={(e) => setQueueDraft({ ...queueDraft, deployment: e.target.value })}
              style={input}
            />
            {isUrl(queueDraft.url) && (
              <div role="alert" style={{ ...quiet, color: "var(--bad)", flexBasis: "100%" }}>
                That is a URL: give where it is kept instead (env:HOLON_SECRET_EVENTS_URL, vault:…). It isn&apos;t sent.
              </div>
            )}
            <button type="submit" style={primary} disabled={!queueDraft.name.trim() || !queueDraft.url.trim() || isUrl(queueDraft.url)}>
              bind queue
            </button>
          </form>
          </Gated>
        </>
      )}

      {tab === "collections" && (
        <>
          <div style={about}>
            What each flow keeps on this host by name, with nothing to bind: its collections (<code>import orders from &quot;collection:orders&quot;</code>), each
            with how many items it holds now, and the channels it sends on (<code>import updates from &quot;channel:updates&quot;</code>), which keep their
            newest packs for whoever subscribes. Each flow&apos;s State shows them item by item, with the runs that changed them.
          </div>
          <CollectionsTab deployments={deployments} grants={grants} />
        </>
      )}

      {tab === "secrets" && (
        <>
          <div style={about}>
            The server&apos;s own secrets: one bound here is read by every deployment that reads a secret of that name and has none of its own (one set for a deployment,
            in its Settings in the editor, overrides it). Where it may go: the hosts (api.stripe.com) and databases (db:shop) it may be sent to. A deploy whose flow sends it elsewhere is
            refused, and so is a request a run makes with it; a deployment with its own isn&apos;t held to it. Empty: wherever its flows&apos; approvals show.
          </div>
          <SecretsTable
            secrets={serverSecrets}
            grants={grants}
            known={h.grantsKnown}
            proposals={h.proposals}
            bind={bind}
            unbind={unbind}
            onSetNew={(s) => {
              // The bind form below takes it, its value typed where it is never shown.
              setSecretDraft({ ...secretDraft, name: s.name, value: "", kind: s.source === "local" ? "value" : s.source.startsWith("vault:") ? "vault" : "env" });
              secretValue.current?.scrollIntoView({ block: "center" });
              secretValue.current?.focus();
            }}
            onWhere={async (s) => {
              const typed = await dialog.prompt(`Where may the server's secret "${s.name}" go? Hosts and db:NAME databases, separated by commas; empty: wherever its flows' approvals show.`, (s.to ?? []).join(", "));
              if (typed === null) return;
              const to = typed.split(",").map((d) => d.trim()).filter(Boolean);
              act(() => capServerSecret(s.name, to.length ? to : null));
            }}
            onRemove={async (s) => {
              if (await dialog.confirm(`Remove the server's secret "${s.name}"? Deployments that read it and have none of their own read nothing for it from their next run.`, "Remove"))
                act(() => unbindServerSecret(s.name));
            }}
            onChanged={refresh}
          />
          <Gated may={bind}>
          <form
            style={{ ...row, background: "transparent", flexWrap: "wrap" }}
            onSubmit={(e) => {
              e.preventDefault();
              const to = secretDraft.to.split(",").map((d) => d.trim()).filter(Boolean);
              act(async () => {
                const kept = secretDraft.kind === "value" ? { value: secretDraft.value } : { from: `${secretDraft.kind}:${secretDraft.value.trim()}` };
                await bindServerSecret(secretDraft.name.trim(), { ...kept, ...(to.length ? { to } : {}) });
                setSecretDraft({ ...secretDraft, name: "", value: "", to: "" });
              });
            }}
          >
            <input aria-label="secret name" placeholder="name (stripe_key)" value={secretDraft.name} onChange={(e) => setSecretDraft({ ...secretDraft, name: e.target.value })} style={input} />
            <select
              aria-label="where its value is kept"
              value={secretDraft.kind}
              onChange={(e) => setSecretDraft({ ...secretDraft, kind: e.target.value as SecretKind, value: "" })}
              style={input}
            >
              <option value="value">a value</option>
              <option value="env">an env variable</option>
              <option value="vault">a Vault field</option>
            </select>
            <input
              ref={secretValue}
              aria-label="secret value"
              type={secretDraft.kind === "value" ? "password" : "text"}
              autoComplete={secretDraft.kind === "value" ? "new-password" : "off"}
              data-1p-ignore
              data-lpignore="true"
              placeholder={secretDraft.kind === "value" ? "its value (sealed here, never shown)" : secretDraft.kind === "env" ? "HOLON_SECRET_STRIPE_KEY" : "secret/path#field"}
              value={secretDraft.value}
              onChange={(e) => setSecretDraft({ ...secretDraft, value: e.target.value })}
              style={input}
            />
            <input
              aria-label="where the secret may go"
              placeholder="where it may go (api.stripe.com, db:shop)"
              value={secretDraft.to}
              onChange={(e) => setSecretDraft({ ...secretDraft, to: e.target.value })}
              style={{ ...input, flex: 1, minWidth: 200 }}
            />
            <button type="submit" style={primary} disabled={!secretDraft.name.trim() || !secretDraft.value}>
              bind secret
            </button>
          </form>
          </Gated>
        </>
      )}

      {tab === "host" && (
        <>
          {host && (
            <>
              <div className="hl-host-page__head">the host</div>
              {host.warnings.length > 0 && (
                <div role="alert" aria-label="what the host warns of" style={{ ...row, flexDirection: "column", alignItems: "stretch", gap: 4, borderColor: "var(--bad)" }}>
                  {host.warnings.map((w) => (
                    <div key={w.key}>
                      <div style={{ color: "var(--bad-ink)" }}>⚠ {w.says}</div>
                      <div style={quiet}>{w.todo}</div>
                    </div>
                  ))}
                </div>
              )}
              <div style={about}>
                What this server holds: its memory, each disk it keeps things on, and what it keeps there. The store and the file texts are counted when the
                server starts and after each round of sweeps; the rest is read now. The same as <code>holon host</code>.
              </div>
              <div style={{ ...row, flexDirection: "column", alignItems: "stretch", gap: 4 }}>
                <div>
                  memory: {host.memory.resident === null ? <span style={quiet}>not told on this system</span> : `${sizeText(host.memory.resident)} (at most ${sizeText(host.memory.peak ?? 0)} so far)`}
                </div>
                <div style={quiet}>flow cache: {cacheLine(host.memory.flow_cache, "flows")}</div>
                <div style={quiet}>value cache: {cacheLine(host.memory.value_cache, "values")}</div>
              </div>
              {host.disks.map((d) => (
                <div key={d.path} style={{ ...row, flexDirection: "column", alignItems: "stretch", gap: 4 }} title={d.path}>
                  <div>disk: {diskLine(d)}</div>
                  <div role="meter" aria-label={`disk ${d.path} in use`} aria-valuenow={diskFull(d)} aria-valuemin={0} aria-valuemax={100} style={{ height: 6, background: "var(--border)", borderRadius: 3 }}>
                    <div style={{ width: `${diskFull(d)}%`, height: "100%", borderRadius: 3, background: host.warnings.some((w) => w.key === `disk:${d.path}`) ? "var(--bad)" : "var(--accent)" }} />
                  </div>
                </div>
              ))}
              {host.deployments.each.length > 0 && (
                <div style={{ ...row, flexDirection: "column", alignItems: "stretch", gap: 2 }} aria-label="what each flow holds">
                  <div style={quiet}>what each flow holds, the most first (a run kept in memory takes about 3 times its line)</div>
                  {host.deployments.each.map((h) => {
                    const [memory, disk] = holdsLines(h, Date.now());
                    const most = Math.max(1, ...host.deployments.each.map((x) => x.memory + x.disk));
                    return (
                      <div key={h.slug} style={{ display: "flex", gap: 8, alignItems: "center" }} title={`${memory}\n${disk}`}>
                        <span style={{ width: 140, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{h.slug}</span>
                        <span style={{ width: 90 }}>~{sizeText(h.memory)}</span>
                        <span style={{ width: 90 }}>{sizeText(h.disk)}</span>
                        <div style={{ flex: 1, height: 6, background: "var(--border)", borderRadius: 3 }}>
                          <div style={{ width: `${((h.memory + h.disk) * 100) / most}%`, height: "100%", borderRadius: 3, background: "var(--accent)" }} />
                        </div>
                      </div>
                    );
                  })}
                </div>
              )}
              <Gated may={operate}>
              <form
                style={row}
                onSubmit={(e) => {
                  e.preventDefault();
                  act(async () => {
                    const set = await setStorage({ warn_at: warnDraft.trim() });
                    setHost((h) => (h ? { ...h, warn_at: warnDraft.trim(), sweeps: set.sweeps, warnings: set.warnings } : h));
                  });
                }}
              >
                <span style={{ flex: 1 }}>
                  it warns when a disk is <span style={quiet}>· {host.warn_at.endsWith("%") ? `${host.warn_at} full for the server` : `down to ${host.warn_at} free`}</span>
                </span>
                <input
                  aria-label="when the host warns its disk is full"
                  title="how full for the server (85%), or how much is left (5G, 500M, 1T); written into the server's holon.toml [storage]"
                  value={warnDraft}
                  onChange={(e) => setWarnDraft(e.target.value)}
                  style={{ ...input, width: 80 }}
                />
                <button type="submit" style={small} disabled={!isThreshold(warnDraft) || warnDraft.trim() === host.warn_at}>
                  set
                </button>
              </form>
              <form
                style={{ ...row, flexWrap: "wrap" }}
                onSubmit={(e) => {
                  e.preventDefault();
                  act(async () => {
                    const set = await setStorage({ full_at: fullDraft.trim() });
                    setHost((h) => (h ? { ...h, full_at: fullDraft.trim(), sweeps: set.sweeps, warnings: set.warnings } : h));
                  });
                }}
              >
                <span style={{ flex: 1, minWidth: 260 }}>
                  when a disk is full <span style={quiet}>· {whenFullLine(host.full_at, host.when_full)}</span>
                  {host.acting.paused.length > 0 && <div style={{ color: "var(--bad-ink)" }}>paused for the full disk: {host.acting.paused.join(", ")}</div>}
                  {host.acting.rolled.length > 0 && <div style={{ color: "var(--bad-ink)" }}>logs cut for the full disk: {host.acting.rolled.join(", ")}</div>}
                </span>
                <input
                  aria-label="when the host acts on a full disk"
                  title="past where it warns: how full for the server (95%), or how much is left (1G); written into the server's holon.toml [storage]"
                  value={fullDraft}
                  onChange={(e) => setFullDraft(e.target.value)}
                  style={{ ...input, width: 80 }}
                />
                <button type="submit" style={small} disabled={!isThreshold(fullDraft) || fullDraft.trim() === host.full_at}>
                  set
                </button>
                <select
                  aria-label="what the host does when its disk is full"
                  value={host.when_full}
                  onChange={(e) => {
                    const when_full = e.target.value;
                    act(async () => {
                      const set = await setStorage({ when_full });
                      setHost((h) => (h ? { ...h, when_full: when_full as HostView["when_full"], sweeps: set.sweeps, warnings: set.warnings } : h));
                    });
                  }}
                  style={input}
                >
                  <option value="warn">only warn</option>
                  <option value="pause">pause what records runs</option>
                  <option value="roll">cut the oldest runs</option>
                </select>
              </form>
              <form
                style={{ ...row, flexWrap: "wrap" }}
                onSubmit={(e) => {
                  e.preventDefault();
                  const backup = (backupDraft ?? "").trim() || "none";
                  act(async () => {
                    await setStorage({ backup });
                    setBackupDraft(null);
                    refresh();
                  });
                }}
              >
                <span style={{ flex: 1, minWidth: 260 }}>
                  backup{" "}
                  <span style={quiet}>
                    ·{" "}
                    {host.backup
                      ? `the runs logs let go of are kept in ${host.backup.dir}: ${host.backup.exports} exports, ${sizeText(host.backup.bytes)}${host.backup.waiting ? `, ${host.backup.waiting} to make` : ""}`
                      : "none: the runs logs let go of go"}
                  </span>
                </span>
                <input
                  aria-label="where the host backs up the runs logs let go of"
                  title="a folder's whole path on the server's machine (an external drive, a folder a cloud client keeps in sync); empty: none. Object storage comes next, Holon Cloud is coming"
                  placeholder="/mnt/archive"
                  value={backupDraft ?? host.backup?.dir ?? ""}
                  onChange={(e) => setBackupDraft(e.target.value)}
                  style={{ ...input, width: 220 }}
                />
                <button type="submit" style={small} disabled={backupDraft === null}>
                  set
                </button>
              </form>
              </Gated>
              <div style={{ ...row, flexDirection: "column", alignItems: "stretch", gap: 4 }}>
                {keptLines(host.kept, Date.now()).map((l, i) => (
                  <div key={l} style={i === 0 ? quiet : undefined}>
                    {l}
                  </div>
                ))}
              </div>
            </>
          )}

          <div className="hl-host-page__head">storage</div>
          <div style={about}>
            What runs leave on this host that nothing kept names any more: states of runs the logs no longer keep, and the texts of files no kept run read or wrote. Anything put in
            the last hour, and code, is never swept. The same as <code>holon store sweep</code> and <code>holon folder sweep</code>.
          </div>
          {sweeps && (
            <>
              <Gated may={operate}>
              <form
                style={row}
                onSubmit={(e) => {
                  e.preventDefault();
                  act(async () => {
                    const set = await setSweepEvery(everyDraft.trim());
                    setHost((h) => (h ? { ...h, sweeps: set.sweeps } : h));
                    setEveryDraft(set.sweeps.every);
                  });
                }}
              >
                <span style={{ flex: 1 }}>
                  the server sweeps on its own <span style={quiet}>· {scheduleLine(sweeps, Date.now())}</span>
                </span>
                <input
                  aria-label="how often the server sweeps"
                  title="how often: 90s, 10m, 2h, 1d (a minute at the least), or off; written into the server's holon.toml [storage]"
                  value={everyDraft}
                  onChange={(e) => setEveryDraft(e.target.value)}
                  style={{ ...input, width: 80 }}
                />
                <button type="submit" style={small} disabled={!isEvery(everyDraft) || everyDraft.trim() === sweeps.every}>
                  set
                </button>
              </form>
              </Gated>
              <div style={{ ...row, ...quiet }}>
                last: the store {reportLine(sweeps.store, "values", Date.now())}; file texts {reportLine(sweeps.folders, "texts", Date.now())}
              </div>
            </>
          )}
          <div style={row}>
            <span style={{ flex: 1 }}>the store: states and values{swept.store && <span style={quiet}> · {swept.store}</span>}</span>
            <button
              style={small}
              {...gateOf(operate, { title: "count what a sweep would remove, then ask" })}
              onClick={() =>
                act(async () => {
                  const would = await sweepStore(true);
                  const keeps = `it keeps ${would.kept.values} values (${sizeText(would.kept.bytes)}), and spares ${would.spared.values} put in the last hour`;
                  if (would.swept.values === 0) {
                    setSwept((s) => ({ ...s, store: `nothing to sweep: ${keeps}` }));
                    return;
                  }
                  const removes = `${would.swept.values} values (${sizeText(would.swept.bytes)}) nothing kept names`;
                  if (!(await dialog.confirm(`Sweep the store? It removes ${removes}; ${keeps}. A run whose state is removed can't be restored to.`, "Sweep"))) return;
                  const done = await sweepStore(false);
                  setSwept((s) => ({ ...s, store: `swept ${done.swept.values} values, ${sizeText(done.swept.bytes)} freed` }));
                })
              }
            >
              sweep…
            </button>
          </div>
          <div style={row}>
            <span style={{ flex: 1 }}>file texts: what runs read and wrote in folders{swept.folders && <span style={quiet}> · {swept.folders}</span>}</span>
            <button
              style={small}
              {...gateOf(operate, { title: "remove the file texts nothing kept names" })}
              onClick={async () => {
                if (!(await dialog.confirm("Sweep the file texts no live run, kept run or folder names? A run whose files are removed can't be shown with them again.", "Sweep"))) return;
                act(async () => {
                  const done = await sweepFolders();
                  setSwept((s) => ({ ...s, folders: `swept ${done.texts} texts, ${sizeText(done.bytes)} freed; it keeps ${done.kept.texts} texts (${sizeText(done.kept.bytes)})` }));
                });
              }}
            >
              sweep…
            </button>
          </div>
        </>
      )}

      {tab === "who-may" && (
        <>
          <div style={about}>
            Each resource, and the deployments granted it, by how for a folder and a feed and by recipient for a mail: read from each deployment&apos;s grant. Nothing
            is changed here: revoking and granting back are on the deployment&apos;s card, which each one opens. A database&apos;s, table by table, is its matrix on{" "}
            <a href={format({ kind: "resources", tab: "databases" })} style={{ color: "var(--accent)", textDecoration: "none" }}>
              Databases
            </a>
            .
          </div>
          <label style={{ ...quiet, display: "flex", gap: 6, alignItems: "center", marginBottom: 6 }}>
            show
            <select aria-label="show what" value={kind} onChange={(e) => setKind(e.target.value)} style={input}>
              <option value="everything">everything</option>
              {/* Folders, mail, feeds and queues always: they're listed as resources, held or not. Databases are their matrix's. */}
              {["folders", "mail", "feeds", "queues", ...kindsHeld(grants).filter((k) => !["databases", "folders", "mail", "feeds", "queues"].includes(k))].map((k) => (
                <option key={k} value={k}>
                  {k}
                </option>
              ))}
            </select>
          </label>
          {(kind === "everything" || kind === "folders") &&
            folders.map((f) => (
              <Resource key={`folder:${keyOf(f)}`} title={`folder ${f.name}`} detail={`${seesWhere ? f.path : ""}${f.deployment ? `${seesWhere ? " · " : ""}for ${f.deployment} only` : ""}`} holders={folderHolders(f)} onOpen={onOpenDeployment} />
            ))}
          {(kind === "everything" || kind === "mail") &&
            mails.map((m) => (
              <Resource
                key={`mail:${keyOf(m)}`}
                title={`mail ${m.name}`}
                detail={`${[m.smtp ? `to ${m.to.join(", ")}` : "", m.mailbox ? `receives in ${m.mailbox.folder}` : ""].filter(Boolean).join(" · ")}${m.deployment ? ` · for ${m.deployment} only` : ""}`}
                holders={mailHolders(m)}
                onOpen={onOpenDeployment}
              />
            ))}
          {(kind === "everything" || kind === "feeds") &&
            feeds.map((f) => (
              <Resource key={`feed:${keyOf(f)}`} title={`feed ${f.name}`} detail={`${f.address}${f.deployment ? ` · for ${f.deployment} only` : ""}`} holders={feedHolders(f)} onOpen={onOpenDeployment} />
            ))}
          {(kind === "everything" || kind === "queues") &&
            queues.map((q) => (
              <Resource
                key={`queue:${keyOf(q)}`}
                title={`queue ${q.name}`}
                detail={`${q.address}${q.fifo ? " · FIFO" : ""}${q.deployment ? ` · for ${q.deployment} only` : ""}`}
                holders={queueHolders(q)}
                onOpen={onOpenDeployment}
              />
            ))}
          {othersHeld(grants, folders, databases, mails, feeds, queues)
            .filter((o) => kind === "everything" || o.kind === kind)
            .map((o) => (
              <div key={o.item} style={{ ...row, flexWrap: "wrap" }} title={o.item}>
                <span style={{ color: "var(--ink)", flex: 1, minWidth: 200 }}>{describeItem(o.item)}</span>
                {o.holders.map((h) => (
                  <button key={h.slug} style={{ ...small, textDecoration: h.revoked ? "line-through" : "none" }} title={h.revoked ? `${h.slug}: revoked; open its card` : `open ${h.slug}'s card`} onClick={() => onOpenDeployment(h.slug)}>
                    {h.slug} →
                  </button>
                ))}
              </div>
            ))}
          {grants.length === 0 && <div style={quiet}>nothing deployed: no deployment holds anything</div>}
        </>
      )}
    </HostPage>
  );
}

export function PolicyPage({ deployments, onOpenDeployment }: { deployments: DeploymentEntry[]; onOpenDeployment: (slug: string) => void }) {
  const h = useHost();
  // Reading holon.toml again (`POST /permissions/reload`) is policy.edit's.
  const can = useCan(["policy.edit"]);
  const { policy, setPolicy, reloaded, setReloaded, act } = h;
  // Check flows against it: shown once asked, read from the deployments and proposals as they are.
  const [checking, setChecking] = useState(false);
  return (
    <HostPage title="Policy" about="what this host allows at all, from its holon.toml" tab={null} host={h}>
      <div className="hl-host-page__head" data-section="policy">the policy</div>
      <div style={about}>
        What this host allows at all, from its <code>holon.toml</code> (edited there, on the server&apos;s machine). Reading it again holds every deployment
        to it at once: one a lowered ceiling or a withdrawn exception no longer covers pauses, unless it is set to keep running.
      </div>
      {policy && (
        <div style={{ ...row, flexDirection: "column", alignItems: "stretch", gap: 4 }}>
          <div>
            granted without asking:{" "}
            {policy.auto.length === 0 ? <span style={quiet}>nothing</span> : policy.auto.map((i) => describeItem(i)).join("; ")}
          </div>
          <div>
            plain HTTP {policy.allow_plain_http ? "may be granted" : "is never granted"} · private addresses {policy.allow_private_addresses ? "may be granted" : "are never granted"}
          </div>
          <div>
            agents{" "}
            {policy.agents_propose === false ? (
              <>
                may not propose: a version of theirs that needs a person is refused <span style={quiet}>(agents.propose = false)</span>
              </>
            ) : (
              "may propose: a version of theirs that needs a person waits in Approvals"
            )}
          </div>
          <div className="hl-matrix-wrap">
          <table aria-label="limits" className="hl-table">
            <thead>
              <tr>
                <th>limit</th>
                <th>default</th>
                <th>at most</th>
                <th>what it means</th>
              </tr>
            </thead>
            <tbody>
              {Object.keys({ ...policy.defaults, ...policy.ceilings })
                .sort()
                .map((name) => (
                  <tr key={name}>
                    <td>{name}</td>
                    <td className="hl-table__num">{policy.defaults[name] ?? "—"}</td>
                    <td className="hl-table__num">{policy.ceilings[name] ?? "no ceiling"}</td>
                    <td className="hl-policy__means">{limitMeaning(name) ?? ""}</td>
                  </tr>
                ))}
            </tbody>
          </table>
          </div>
        </div>
      )}
      <div style={{ display: "flex", gap: 8, alignItems: "center", marginTop: 4 }}>
        <button
          style={small}
          {...can.gate("policy.edit", { title: "read holon.toml again and hold every deployment to it" })}
          onClick={() =>
            act(async () => {
              const r = await reloadPermissions();
              setPolicy(r);
              setReloaded({ at: Date.now(), attention: Object.entries(r.attention).filter((e): e is [string, string] => e[1] !== null) });
            })
          }
        >
          read the policy again
        </button>
        <button style={small} aria-pressed={checking} title="which deployments run within it, and each version waiting for a person against it" onClick={() => setChecking(!checking)}>
          check flows against it
        </button>
        {reloaded && (
          <span style={quiet}>
            read again at {new Date(reloaded.at).toLocaleTimeString()}:{" "}
            {reloaded.attention.length === 0 ? "every deployment is covered" : `${reloaded.attention.map(([s]) => s).join(", ")} can't run as granted`}
          </span>
        )}
      </div>

      {checking && policy && <PolicyCheck policy={policy} deployments={deployments} proposals={h.proposals} />}

      <div className="hl-host-page__head" data-section="policy-file">
        the policy file
      </div>
      <div style={about}>
        <code>holon.toml</code> as the server read it, at its start or when it was last read again: an edit on the server&apos;s machine holds once it is.
      </div>
      <PolicyText text={policy?.text} />

      <div className="hl-host-page__head" data-section="held-off">held off</div>
      <div style={about}>
        The deployments that can&apos;t run as they were granted: under a ceiling lowered or an exception withdrawn since, or for want of something they need. Each
        one&apos;s page says what to change.
      </div>
      {deployments
        .filter((d) => d.attention)
        .map((d) => (
          <div key={d.slug} style={row} aria-label={`held off: ${d.slug}`}>
            <span style={{ color: "var(--accent)", minWidth: 110 }}>{d.slug}</span>
            <span style={{ color: "var(--warn)", flex: 1, fontSize: 11 }}>{d.attention}</span>
            <button style={small} onClick={() => onOpenDeployment(d.slug)}>
              open →
            </button>
          </div>
        ))}
      {!deployments.some((d) => d.attention) && <div style={quiet}>none: every deployment runs as granted</div>}
    </HostPage>
  );
}

/** A resource in who may touch what: the deployments granted it, how, and each one's card. */
function Resource({ title, detail, holders, onOpen }: { title: string; detail: string; holders: Holder[]; onOpen: (slug: string) => void }) {
  return (
    <div aria-label={title} style={{ ...row, flexDirection: "column", alignItems: "stretch", gap: 4 }}>
      <div style={{ display: "flex", gap: 8 }}>
        <span style={{ color: "var(--accent)" }}>{title}</span>
        <span style={{ ...quiet, flex: 1, overflowWrap: "anywhere" }}>{detail}</span>
      </div>
      {holders.length === 0 && <div style={quiet}>no deployment is granted it</div>}
      {holders.map((h) => (
        <div key={h.slug} style={{ display: "flex", gap: 8, alignItems: "center", paddingLeft: 10, fontSize: 11 }}>
          <span style={{ color: "var(--ink)", minWidth: 110 }}>{h.slug}</span>
          <span style={{ color: h.status === "running" ? "var(--ok)" : "var(--warn)", minWidth: 54 }} title={h.attention ?? h.status}>
            {h.status}
          </span>
          <span style={{ flex: 1, color: "var(--muted)" }}>
            {usesText(h.uses).map((u, i) => (
              <Fragment key={i}>
                {i > 0 && " · "}
                <span style={u.revoked ? { textDecoration: "line-through", color: "var(--bad)" } : undefined} title={u.revoked ? "revoked" : undefined}>
                  {u.text}
                </span>
              </Fragment>
            ))}
          </span>
          <button style={small} title={`open ${h.slug}'s card, where its grant changes`} onClick={() => onOpen(h.slug)}>
            card →
          </button>
        </div>
      ))}
    </div>
  );
}
