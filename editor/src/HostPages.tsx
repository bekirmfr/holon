// The host's pages (admins only; holon-server's auth.rs, gaps-web.md G26–G29), each at an
// address of its own: Resources, in tabs (#resources: its databases; #resources.folders, .mail,
// .feeds, .secrets with the server's own and their history, .host with its storage, .who-may),
// People and access (#people: accounts, API tokens, agents, signing in through an identity
// provider) and Policy (#policy: what holon.toml allows, its limits and ceilings, and the
// deployments it holds off). A section of People and access has its place in the address too
// (#people.tokens). Below each page, what was done, by whom. Adding someone gives a one-time link for them to set their password; resetting gives a
// new one. The server refuses any change that would leave no admin. A grant changes only on its
// deployment's page: who may touch what links there.
import { Fragment, useCallback, useEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import {
  changeAccount,
  bindDatabase,
  bindFolder,
  bindServerSecret,
  capServerSecret,
  createToken,
  getOidcSettings,
  getPermissions,
  listDatabases,
  listFolders,
  listGrants,
  listServerSecrets,
  listTables,
  inviteAccount,
  listAccounts,
  listTokens,
  reloadPermissions,
  removeAccount,
  resetAccount,
  revokeToken,
  setOidcSettings,
  sweepFolders,
  sweepStore,
  getHost,
  setSweepEvery,
  setStorage,
  type HostView,
  unbindDatabase,
  unbindFolder,
  unbindServerSecret,
  type AccountEntry,
  type DatabaseBinding,
  type DatabaseEntry,
  type FolderBinding,
  type FolderEntry,
  type HeldGrant,
  type HostPolicy,
  type OidcSettings,
  type Role,
  type SecretInfo,
  type TokenEntry,
  getSecretHistory,
  previewSecretRollback,
  rollbackSecret,
  listFeeds,
  listMails,
  type DeploymentEntry,
  type SecretHistory,
} from "./server";
import { cacheLine, diskFull, diskLine, holdsLines, isEvery, isThreshold, keptLines, reportLine, scheduleLine, sizeText, whenFullLine } from "./storage";
import { useDialog } from "./Dialog";
import { FolderPicker } from "./FolderPicker";
import { ActivityList } from "./Pages";
import { describeItem } from "./holon";
import { holdersOf, kindsHeld, othersHeld, usesText, type Holder } from "./holdings";

import { format, RESOURCES_LABELS, RESOURCES_TABS, type ResourcesTab } from "./routes";
import { useDryRun } from "./DryRun";
import { secretRollbackReport, sourceLabel } from "./DeployBar";

const ROLES: Role[] = ["viewer", "editor", "admin", "agent"];
const ROLE_TITLE: Record<Role, string> = {
  viewer: "reads flows, deployments and logs",
  editor: "also saves, deploys, runs and deletes",
  admin: "also writes secrets and manages this panel",
  agent: "reads, proposes flows and calls them; never approves, starts, stops or deletes",
};

const small: CSSProperties = { font: "inherit", fontSize: 11, padding: "2px 8px", color: "var(--muted)", background: "var(--surface-2)", border: "1px solid var(--border)", borderRadius: 3, cursor: "pointer" };
const primary: CSSProperties = { ...small, color: "var(--accent-ink)", background: "var(--accent)", border: "none" };
const input: CSSProperties = { font: "inherit", fontSize: 12, padding: "3px 6px", color: "var(--ink)", background: "var(--surface-2)", border: "1px solid var(--border)", borderRadius: 3 };
const head: CSSProperties = { color: "var(--body)", fontSize: 14, letterSpacing: "0.04em", margin: "20px 0 6px" };
const row: CSSProperties = { display: "flex", alignItems: "center", gap: 8, padding: "6px 10px", marginBottom: 4, background: "var(--surface-2)", border: "1px solid var(--border)", borderRadius: 4, fontSize: 12 };
const about: CSSProperties = { color: "var(--muted)", fontSize: 11, marginBottom: 6, lineHeight: 1.45 };
const quiet: CSSProperties = { color: "var(--faint)", fontSize: 11 };
/** What holding a resource for whole runs costs, said where it is set. */
const RUN_LOCK_COST = "every run that writes it goes one at a time, for its whole length, across every deployment bound to it";

const messageOf = (e: unknown) => (e instanceof Error ? e.message : String(e));
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
 *  read (Resources). */
function useHost(shown = false) {
  const dialog = useDialog();
  const [people, setPeople] = useState<AccountEntry[]>([]);
  const [tokens, setTokens] = useState<TokenEntry[]>([]);
  const [oidc, setOidc] = useState<OidcSettings | null>(null);
  const [msg, setMsg] = useState("");
  const [link, setLink] = useState<{ name: string; url: string } | null>(null);
  const [secret, setSecret] = useState<{ label: string; token: string } | null>(null);
  const [draft, setDraft] = useState({ name: "", role: "editor" as Role, email: "" });
  const [tokenDraft, setTokenDraft] = useState({ label: "", role: "editor" as Role });
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
  const [serverSecrets, setServerSecrets] = useState<SecretInfo[]>([]);
  // A server secret being bound: its name, its value or where that is kept, and where it may go.
  const [secretDraft, setSecretDraft] = useState({ name: "", value: "", to: "", kind: "value" as SecretKind });
  const [grants, setGrants] = useState<HeldGrant[]>([]);
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
    listAccounts().then(setPeople).catch((e: unknown) => setMsg(messageOf(e)));
    listTokens().then(setTokens).catch((e: unknown) => setMsg(messageOf(e)));
    listFolders().then(setFolders).catch((e: unknown) => setMsg(messageOf(e)));
    listDatabases().then(setDatabases).catch((e: unknown) => setMsg(messageOf(e)));
    listServerSecrets().then(setServerSecrets).catch((e: unknown) => setMsg(messageOf(e)));
    listGrants().then(setGrants).catch((e: unknown) => setMsg(messageOf(e)));
    getPermissions().then(setPolicy).catch((e: unknown) => setMsg(messageOf(e)));
    getHost()
      .then((h) => {
        setHost(h);
        // What the admin is typing stays: a refresh only fills the field when it is empty.
        setEveryDraft((d) => d || h.sweeps.every);
        setWarnDraft((d) => d || h.warn_at);
        setFullDraft((d) => d || h.full_at);
      })
      .catch((e: unknown) => setMsg(messageOf(e)));
    getOidcSettings()
      .then((s) => {
        setOidc(s);
        setIssuer(s?.issuer ?? "");
        setClientId(s?.client_id ?? "");
      })
      .catch((e: unknown) => setMsg(messageOf(e)));
    setVersion((v) => v + 1);
  }, []);
  useEffect(refresh, [refresh]);

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
  /** Which deployments use it, said under its row. */
  const usedBy = (holders: Holder[]) => (
    <div style={{ ...quiet, padding: "0 10px 6px", marginTop: -2 }}>
      {holders.length === 0 ? (
        "no deployment uses it"
      ) : (
        <>
          used by {named(holders)} (
          <a href={format({ kind: "resources", tab: "who-may" })} style={{ color: "var(--accent)", textDecoration: "none" }}>
            who may touch what
          </a>{" "}
          says how)
        </>
      )}
    </div>
  );
  /** What unbinding (or binding again) does to the deployments that use it, as the server does it now. */
  const unbindingSays = (holders: Holder[]) =>
    holders.length === 0 ? "No deployment uses it." : `${named(holders)} ${holders.length === 1 ? "uses" : "use"} it: they keep running, and their calls to it fail until it is bound again.`;

  return {
    people, setPeople, tokens, setTokens, oidc, setOidc, msg, setMsg, link, setLink, secret,
    setSecret, draft, setDraft, tokenDraft, setTokenDraft, issuer, setIssuer, clientId, setClientId,
    folders, setFolders, folderDraft, setFolderDraft, keepFor, setKeepFor, browsing, setBrowsing,
    databases, setDatabases, databaseDraft, setDatabaseDraft, tables, setTables, serverSecrets,
    setServerSecrets, secretDraft, setSecretDraft, grants, setGrants, policy, setPolicy, reloaded,
    setReloaded, swept, setSwept, host, setHost, everyDraft, setEveryDraft, warnDraft, setWarnDraft,
    fullDraft, setFullDraft, backupDraft, setBackupDraft, kind, setKind, version, setVersion,
    dialog, sweeps, refresh, readTables, act, folderHolders, databaseHolders, usedBy, unbindingSays,
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

export function PeoplePage({ me, tab }: { me: string; tab: string | null }) {
  const h = useHost();
  const { people, tokens, oidc, link, setLink, secret, setSecret, draft, setDraft, tokenDraft, setTokenDraft, issuer, setIssuer, clientId, setClientId, dialog, act } = h;
  return (
    <HostPage title="People and access" about="who uses this server, the tokens scripts and agents use, and how people sign in" tab={tab} host={h}>
      <div style={head} data-section="accounts">people</div>
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
      {people.map((p) => (
        <div key={p.name} style={row}>
          <span style={{ color: "var(--accent)", minWidth: 90 }}>
            {p.name}
            {p.name === me && <span style={{ color: "var(--faint)" }}> (you)</span>}
          </span>
          <select
            aria-label={`${p.name}'s role`}
            value={p.role}
            style={input}
            title={ROLE_TITLE[p.role]}
            onChange={(e) => act(() => changeAccount(p.name, { role: e.target.value as Role }))}
          >
            {ROLES.map((r) => (
              <option key={r} value={r} title={ROLE_TITLE[r]}>
                {r}
              </option>
            ))}
          </select>
          <span style={{ color: "var(--faint)", fontSize: 11, flex: 1 }}>
            {p.email ?? ""}
            {p.invited && <span style={{ color: "var(--warn)" }}> · link not used yet</span>}
            {p.disabled && <span style={{ color: "var(--bad)" }}> · disabled</span>}
          </span>
          <button
            style={small}
            title="the email they sign in with through the identity provider"
            onClick={async () => {
              const email = await dialog.prompt(`${p.name}'s email, for signing in through the identity provider (empty for none):`, p.email ?? "");
              if (email !== null) act(() => changeAccount(p.name, { email }));
            }}
          >
            email
          </button>
          <button
            style={small}
            title="a new link to set a new password; their sign-ins end"
            onClick={async () => {
              if (!(await dialog.confirm(`Reset ${p.name}'s password? Their sign-ins end, and you get a new link for them.`, "Reset"))) return;
              act(async () => {
                const r = await resetAccount(p.name);
                setLink({ name: p.name, url: linkFor(r.invite) });
              });
            }}
          >
            reset
          </button>
          <button style={small} onClick={() => act(() => changeAccount(p.name, { disabled: !p.disabled }))} title={p.disabled ? "let them sign in again" : "stop them signing in; their sign-ins end"}>
            {p.disabled ? "enable" : "disable"}
          </button>
          <button
            style={small}
            title="remove the account"
            onClick={async () => {
              if (await dialog.confirm(`Remove ${p.name}'s account? This can't be undone.`, "Remove")) act(() => removeAccount(p.name));
            }}
          >
            ×
          </button>
        </div>
      ))}
      <form
        style={{ ...row, background: "transparent" }}
        onSubmit={(e) => {
          e.preventDefault();
          act(async () => {
            const r = await inviteAccount(draft.name.trim(), draft.role, draft.email.trim());
            setLink({ name: r.name, url: linkFor(r.invite) });
            setDraft({ name: "", role: draft.role, email: "" });
          });
        }}
      >
        <input aria-label="new person's name" placeholder="name" value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} style={input} />
        <select aria-label="new person's role" value={draft.role} onChange={(e) => setDraft({ ...draft, role: e.target.value as Role })} style={input}>
          {ROLES.map((r) => (
            <option key={r} value={r}>
              {r}
            </option>
          ))}
        </select>
        <input aria-label="new person's email" placeholder="email (optional)" value={draft.email} onChange={(e) => setDraft({ ...draft, email: e.target.value })} style={{ ...input, flex: 1 }} />
        <button type="submit" style={primary} disabled={!draft.name.trim()}>
          add person
        </button>
      </form>

      <div style={head} data-section="tokens">API tokens</div>
      <div style={{ color: "var(--muted)", fontSize: 11, marginBottom: 6 }}>for scripts and CI: sent as Authorization: Bearer &lt;token&gt;, acting with their role</div>
      {secret && (
        <div style={{ ...row, flexDirection: "column", alignItems: "stretch", borderColor: "var(--sel)" }}>
          <span>
            The token for <span style={{ color: "var(--ink)" }}>{secret.label}</span>. Copy it now: it isn't shown again.
          </span>
          <span style={{ display: "flex", gap: 6 }}>
            <input aria-label="new API token" readOnly value={secret.token} style={{ ...input, flex: 1 }} onFocus={(e) => e.target.select()} />
            <button style={small} onClick={() => navigator.clipboard?.writeText(secret.token)}>
              copy
            </button>
            <button style={small} onClick={() => setSecret(null)}>
              done
            </button>
          </span>
        </div>
      )}
      {tokens.length === 0 && <div style={{ color: "var(--faint)", fontSize: 11, marginBottom: 6 }}>no API tokens</div>}
      {tokens.map((t) => (
        <div key={t.id} style={row}>
          <span style={{ color: "var(--accent)", minWidth: 90 }}>{t.label}</span>
          <span>{t.role}</span>
          <span style={{ color: "var(--faint)", fontSize: 11, flex: 1 }}>
            made by {t.created_by}, {new Date(t.created_at * 1000).toLocaleString()}
          </span>
          <button
            style={small}
            onClick={async () => {
              if (await dialog.confirm(`Revoke the token "${t.label}"? Whatever uses it stops working.`, "Revoke")) act(() => revokeToken(t.id));
            }}
          >
            revoke
          </button>
        </div>
      ))}
      <form
        style={{ ...row, background: "transparent" }}
        onSubmit={(e) => {
          e.preventDefault();
          act(async () => {
            const r = await createToken(tokenDraft.label.trim(), tokenDraft.role);
            setSecret({ label: r.entry.label, token: r.token });
            setTokenDraft({ label: "", role: tokenDraft.role });
          });
        }}
      >
        <input aria-label="new token's label" placeholder="what it's for (e.g. ci)" value={tokenDraft.label} onChange={(e) => setTokenDraft({ ...tokenDraft, label: e.target.value })} style={{ ...input, flex: 1 }} />
        <select aria-label="new token's role" value={tokenDraft.role} onChange={(e) => setTokenDraft({ ...tokenDraft, role: e.target.value as Role })} style={input}>
          {ROLES.map((r) => (
            <option key={r} value={r}>
              {r}
            </option>
          ))}
        </select>
        <button type="submit" style={primary} disabled={!tokenDraft.label.trim()}>
          make token
        </button>
      </form>

      <div style={head} data-section="sign-in">signing in through an identity provider</div>
      <div style={about}>
        Optional: people may also sign in through your provider (Keycloak, Okta, Auth0, Entra ID…), registered there as a public client
        with PKCE and this page's address as its redirect. Someone signing in is matched to the account with their email (set under people).
      </div>
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
      <div style={{ color: "var(--faint)", fontSize: 11 }}>{oidc ? `on: ${oidc.issuer} as ${oidc.client_id}` : "off"}</div>

      <div style={head} data-section="agents">agents</div>
      <div style={about}>
        An agent writes flows, checks them, saves and deploys them, and runs their actions, but never approves what a flow may do: that stays with a person. It
        connects through <code>holon mcp</code>. On this server&apos;s machine it signs in as you and asks as the agent role; from another, it uses an API token with
        role agent (make one above). Which actions agents may run is each deployment&apos;s own setting.
      </div>
      <div style={{ ...row, background: "transparent" }}>
        <input aria-label="the holon mcp line" readOnly value={`holon mcp setup claude-code --server ${location.origin}`} style={{ ...input, flex: 1, fontFamily: "ui-monospace, Menlo, Consolas, monospace" }} onFocus={(e) => e.target.select()} />
        <button style={small} onClick={() => navigator.clipboard?.writeText(`holon mcp setup claude-code --server ${location.origin}`)}>
          copy
        </button>
      </div>
      {[
        ...people.filter((p) => p.role === "agent").map((p) => ({ key: `account:${p.name}`, name: p.name, said: "an account" })),
        ...tokens.filter((t) => t.role === "agent").map((t) => ({ key: `token:${t.id}`, name: t.label, said: `a token, made by ${t.created_by}` })),
      ].map((a) => (
        <div key={a.key} style={row}>
          <span style={{ color: "var(--accent)", minWidth: 90 }}>{a.name}</span>
          <span style={quiet}>{a.said}</span>
        </div>
      ))}
      {!people.some((p) => p.role === "agent") && !tokens.some((t) => t.role === "agent") && <div style={{ ...quiet, marginBottom: 6 }}>no agent account or token yet</div>}
      <a href={format({ kind: "docs", page: "guides/agents.md", anchor: null })} style={{ color: "var(--accent)", fontSize: 11, textDecoration: "none" }}>
        working with agents, in the manual ›
      </a>
    </HostPage>
  );
}

export function ResourcesPage({ tab, onOpenDeployment }: { tab: ResourcesTab; onOpenDeployment: (slug: string) => void }) {
  const h = useHost(true);
  const { setMsg, folders, folderDraft, setFolderDraft, keepFor, setKeepFor, browsing, setBrowsing, databases, databaseDraft, setDatabaseDraft, tables, setTables, serverSecrets, secretDraft, setSecretDraft, grants, swept, setSwept, host, setHost, everyDraft, setEveryDraft, warnDraft, setWarnDraft, fullDraft, setFullDraft, backupDraft, setBackupDraft, kind, setKind, dialog, sweeps, refresh, readTables, act, folderHolders, databaseHolders, usedBy, unbindingSays } = h;
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
          {databases.length === 0 && <div style={{ color: "var(--faint)", fontSize: 11, marginBottom: 6 }}>no databases bound</div>}
          {databases.map((d) => {
            const read = tables[keyOf(d)];
            return (
              <Fragment key={keyOf(d)}>
                <div style={row}>
                  <span style={{ color: "var(--accent)", minWidth: 90 }}>database {d.name}</span>
                  <span style={{ color: "var(--ink)", flex: 1, overflowWrap: "anywhere" }}>{d.connection ? `${d.address ?? ""} (from ${d.connection})` : d.path}</span>
                  <span style={{ color: "var(--faint)", fontSize: 11 }}>
                    {d.engine} · {d.access === "read" ? "read only" : "read and write"}
                    {d.lock === "run" ? " · held for whole runs" : ""}
                    {d.deployment ? ` · for ${d.deployment} only` : ""}
                  </span>
                  <button
                    style={small}
                    title="unbind the name: the database stays where it is"
                    onClick={async () => {
                      const whose = d.deployment ? ` for ${d.deployment}` : "";
                      if (await dialog.confirm(`Unbind the database "${d.name}"${whose}? It stays where it is. ${unbindingSays(databaseHolders(d))}`, "Unbind"))
                        act(() => unbindDatabase(d.name, d.deployment));
                    }}
                  >
                    ×
                  </button>
                </div>
                <div aria-label={`${d.name}'s tables`} style={{ ...quiet, padding: "0 10px 2px", marginTop: -2, display: "flex", gap: 8, alignItems: "center" }}>
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
                {usedBy(databaseHolders(d))}
              </Fragment>
            );
          })}
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
          {folders.length === 0 && <div style={{ color: "var(--faint)", fontSize: 11, marginBottom: 6 }}>no folders bound</div>}
          {folders.map((f) => (
            <Fragment key={keyOf(f)}>
              <div style={row}>
                <span style={{ color: "var(--accent)", minWidth: 90 }}>folder {f.name}</span>
                <span style={{ color: "var(--ink)", flex: 1, overflowWrap: "anywhere" }}>{f.path}</span>
                <span style={{ color: "var(--faint)", fontSize: 11 }}>
                  {f.access === "read" ? "read only" : "read and write"}
                  {f.keep === "hash" ? " · reads kept by hash" : f.keep.startsWith("bytes for ") ? ` · reads kept ${f.keep.slice("bytes for ".length)}, then by hash` : ""}
                  {f.cap != null ? ` · capped at ${sizeText(f.cap)}` : ""}
                  {f.shared ? " · other servers bind it too" : ""}
                  {f.lock === "run" ? " · held for whole runs" : ""}
                  {f.deployment ? ` · for ${f.deployment} only` : ""}
                </span>
                <button
                  style={small}
                  title="unbind the name: the files stay where they are"
                  onClick={async () => {
                    const whose = f.deployment ? ` for ${f.deployment}` : "";
                    if (await dialog.confirm(`Unbind the folder "${f.name}"${whose}? Its files stay where they are. ${unbindingSays(folderHolders(f))}`, "Unbind"))
                      act(() => unbindFolder(f.name, f.deployment));
                  }}
                >
                  ×
                </button>
              </div>
              {usedBy(folderHolders(f))}
            </Fragment>
          ))}
          <form
            style={{ ...row, background: "transparent", flexWrap: "wrap" }}
            onSubmit={async (e) => {
              e.preventDefault();
              const name = folderDraft.name.trim();
              const deployment = folderDraft.deployment?.trim() || null;
              const there = folders.find((f) => f.name === name && f.deployment === deployment);
              if (there && !(await dialog.confirm(`Bind "${name}" again? It replaces the binding to ${there.path}. ${unbindingSays(folderHolders(there)).replace(/: they keep running.*/, ": their next runs use the new one.")}`, "Bind again")))
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

      {tab === "mail" && <MailTab />}

      {tab === "feeds" && <FeedsTab />}

      {tab === "secrets" && (
        <>
          <div style={about}>
            The server&apos;s own secrets: one bound here is read by every deployment that reads a secret of that name and has none of its own (one set for a deployment,
            under its ⚙, overrides it). Where it may go: the hosts (api.stripe.com) and databases (db:shop) it may be sent to. A deploy whose flow sends it elsewhere is
            refused, and so is a request a run makes with it; a deployment with its own isn&apos;t held to it. Empty: wherever its flows&apos; approvals show.
          </div>
          {serverSecrets.length === 0 && <div style={{ color: "var(--faint)", fontSize: 11, marginBottom: 6 }}>no secrets bound</div>}
          {serverSecrets.map((s) => (
            <Fragment key={s.name}>
            <div style={row} aria-label={`server secret ${s.name}`}>
              <span style={{ color: "var(--accent)", minWidth: 90 }}>secret {s.name}</span>
              <span style={{ color: "var(--faint)", fontSize: 11, flex: 1 }}>
                v{s.version} · {s.source === "local" ? "a value" : s.source}
                {s.problem ? ` · can't be read: ${s.problem}` : ""}
                {" · "}
                {s.to ? (s.to.length ? `may go only to ${s.to.map((d) => (d.startsWith("db:") ? `the database "${d.slice(3)}"` : `https://${d}`)).join(", ")}` : "may go nowhere") : "may go wherever its flows' approvals show"}
              </span>
              <button
                style={small}
                title="change where it may go"
                onClick={async () => {
                  const typed = await dialog.prompt(`Where may the server's secret "${s.name}" go? Hosts and db:NAME databases, separated by commas; empty: wherever its flows' approvals show.`, (s.to ?? []).join(", "));
                  if (typed === null) return;
                  const to = typed.split(",").map((d) => d.trim()).filter(Boolean);
                  act(() => capServerSecret(s.name, to.length ? to : null));
                }}
              >
                where…
              </button>
              <button
                style={small}
                title="remove the server's secret, every version"
                onClick={async () => {
                  if (await dialog.confirm(`Remove the server's secret "${s.name}"? Deployments that read it and have none of their own read nothing for it from their next run.`, "Remove"))
                    act(() => unbindServerSecret(s.name));
                }}
              >
                ×
              </button>
            </div>
            <ServerSecretHistory name={s.name} onChanged={refresh} />
            </Fragment>
          ))}
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
        </>
      )}

      {tab === "host" && (
        <>
          {host && (
            <>
              <div style={head}>the host</div>
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
              <div style={{ ...row, flexDirection: "column", alignItems: "stretch", gap: 4 }}>
                {keptLines(host.kept, Date.now()).map((l, i) => (
                  <div key={l} style={i === 0 ? quiet : undefined}>
                    {l}
                  </div>
                ))}
              </div>
            </>
          )}

          <div style={head}>storage</div>
          <div style={about}>
            What runs leave on this host that nothing kept names any more: states of runs the logs no longer keep, and the texts of files no kept run read or wrote. Anything put in
            the last hour, and code, is never swept. The same as <code>holon store sweep</code> and <code>holon folder sweep</code>.
          </div>
          {sweeps && (
            <>
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
              <div style={{ ...row, ...quiet }}>
                last: the store {reportLine(sweeps.store, "values", Date.now())}; file texts {reportLine(sweeps.folders, "texts", Date.now())}
              </div>
            </>
          )}
          <div style={row}>
            <span style={{ flex: 1 }}>the store: states and values{swept.store && <span style={quiet}> · {swept.store}</span>}</span>
            <button
              style={small}
              title="count what a sweep would remove, then ask"
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
              title="remove the file texts nothing kept names"
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
            Each resource, and the deployments granted it, by table for a database and by how for a folder: read from each deployment&apos;s grant. Nothing
            is changed here: revoking and granting back are on the deployment&apos;s card, which each one opens.
          </div>
          <label style={{ ...quiet, display: "flex", gap: 6, alignItems: "center", marginBottom: 6 }}>
            show
            <select aria-label="show what" value={kind} onChange={(e) => setKind(e.target.value)} style={input}>
              <option value="everything">everything</option>
              {/* Databases and folders always: they're listed as resources, held or not. */}
              {["databases", "folders", ...kindsHeld(grants).filter((k) => k !== "databases" && k !== "folders")].map((k) => (
                <option key={k} value={k}>
                  {k}
                </option>
              ))}
            </select>
          </label>
          {(kind === "everything" || kind === "databases") &&
            databases.map((d) => (
              <Resource key={`db:${keyOf(d)}`} title={`database ${d.name}`} detail={`${d.engine}${d.deployment ? ` · for ${d.deployment} only` : ""}`} holders={databaseHolders(d)} onOpen={onOpenDeployment} />
            ))}
          {(kind === "everything" || kind === "folders") &&
            folders.map((f) => (
              <Resource key={`folder:${keyOf(f)}`} title={`folder ${f.name}`} detail={`${f.path}${f.deployment ? ` · for ${f.deployment} only` : ""}`} holders={folderHolders(f)} onOpen={onOpenDeployment} />
            ))}
          {othersHeld(grants, folders, databases)
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
  const { policy, setPolicy, reloaded, setReloaded, act } = h;
  return (
    <HostPage title="Policy" about="what this host allows at all, from its holon.toml" tab={null} host={h}>
      <div style={head} data-section="policy">the policy</div>
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
          <table aria-label="limits" style={{ fontSize: 11, color: "var(--muted)", borderCollapse: "collapse" }}>
            <thead>
              <tr style={{ color: "var(--faint)", textAlign: "left" }}>
                <th style={{ fontWeight: "normal", paddingRight: 16 }}>limit</th>
                <th style={{ fontWeight: "normal", paddingRight: 16 }}>default</th>
                <th style={{ fontWeight: "normal" }}>at most</th>
              </tr>
            </thead>
            <tbody>
              {Object.keys({ ...policy.defaults, ...policy.ceilings })
                .sort()
                .map((name) => (
                  <tr key={name}>
                    <td style={{ color: "var(--ink)", paddingRight: 16 }}>{name}</td>
                    <td style={{ paddingRight: 16 }}>{policy.defaults[name] ?? "—"}</td>
                    <td>{policy.ceilings[name] ?? "no ceiling"}</td>
                  </tr>
                ))}
            </tbody>
          </table>
        </div>
      )}
      <div style={{ display: "flex", gap: 8, alignItems: "center", marginTop: 4 }}>
        <button
          style={small}
          title="read holon.toml again and hold every deployment to it"
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
        {reloaded && (
          <span style={quiet}>
            read again at {new Date(reloaded.at).toLocaleTimeString()}:{" "}
            {reloaded.attention.length === 0 ? "every deployment is covered" : `${reloaded.attention.map(([s]) => s).join(", ")} can't run as granted`}
          </span>
        )}
      </div>

      <div style={head} data-section="held-off">held off</div>
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

/** A server secret's versions and changes, folded under its row; an older version is made
 *  current again after a dry run, as a deployment's is (DeployBar.tsx's SecretRow). Never a
 *  value: versions are told apart by their fingerprints. */
function ServerSecretHistory({ name, onChanged }: { name: string; onChanged: () => void }) {
  const [history, setHistory] = useState<SecretHistory | null>(null);
  const [said, setSaid] = useState("");
  const dryRun = useDryRun();
  const load = () => getSecretHistory(null, name).then(setHistory).catch((e: unknown) => setSaid(messageOf(e)));
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
  const when = (epoch: number) => new Date(epoch * 1000).toLocaleString([], { dateStyle: "short", timeStyle: "short" });
  return (
    <div style={{ padding: "0 10px 6px", marginTop: -2, fontSize: 11 }}>
      <button style={{ ...small, padding: "0 6px" }} aria-expanded={history !== null} aria-label={`versions of ${name}`} onClick={() => (history ? setHistory(null) : void load())}>
        {history ? "▾" : "▸"} versions and changes
      </button>
      {said && <span style={{ ...quiet, marginLeft: 8 }}>{said}</span>}
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
                <button style={{ ...small, width: 64 }} onClick={() => void rollback(v.version)} title={`make version ${v.version} current again`}>
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

/** A list read once its tab is shown: undefined while it's read, null where this server doesn't serve it. */
function useListed<T>(read: () => Promise<T[] | null>): { list: T[] | null | undefined; failed: string | null } {
  const [list, setList] = useState<T[] | null | undefined>(undefined);
  const [failed, setFailed] = useState<string | null>(null);
  useEffect(() => {
    read().then(setList, (e) => setFailed(messageOf(e)));
  }, [read]);
  return { list, failed };
}

/** What a tab says where its list isn't there: still read, failed, not served, or empty. */
function ListedState({ list, failed, what }: { list: unknown[] | null | undefined; failed: string | null; what: string }) {
  if (failed) return <div role="alert" style={{ color: "var(--bad)", fontSize: 12 }}>{failed}</div>;
  if (list === undefined) return <div style={quiet}>reading…</div>;
  if (list === null) return <div style={quiet}>this server binds no {what} yet: it is older than this editor (holon update)</div>;
  return list.length === 0 ? <div style={quiet}>no {what} bound</div> : null;
}

const forWhom = (deployment: string | null) => (deployment ? ` · for ${deployment} only` : "");

/** Mail: the names flows send and read mail through (`"mail:ops"`), each an SMTP server, an IMAP mailbox, or both. */
function MailTab() {
  const { list, failed } = useListed(listMails);
  return (
    <>
      <div style={about}>
        The names flows send and read mail through. A flow imports a name (<code>&quot;mail:ops&quot;</code>), never a server: each is bound here to an SMTP server it sends
        through, an IMAP mailbox it reads, or both. Its grant says to whom it may send (<code>MailSend(&quot;ops/*&quot;)</code>) and whether it hears what arrives
        (<code>MailArrival(&quot;ops&quot;)</code>). Logins are where their source says, never here.
      </div>
      <ListedState list={list} failed={failed} what="mail" />
      {list?.map((m) => (
        <div key={`${m.deployment ?? ""}/${m.name}`} style={{ ...row, flexDirection: "column", alignItems: "stretch", gap: 2 }} aria-label={`mail ${m.name}`}>
          <div style={{ display: "flex", gap: 8 }}>
            <span style={{ color: "var(--accent)", minWidth: 90 }}>mail {m.name}</span>
            <span style={{ ...quiet, flex: 1 }}>{forWhom(m.deployment).slice(3)}</span>
          </div>
          {m.smtp && (
            <div style={{ color: "var(--ink)", overflowWrap: "anywhere" }}>
              sends through {m.address ?? "an SMTP server"}
              {m.from ? `, from ${m.from}` : ""}
              {m.to && m.to.length > 0 ? `, only to ${m.to.join(", ")}` : ""}
              <span style={quiet}> · {sourceLabel(m.smtp)}</span>
            </div>
          )}
          {m.mailbox && (
            <div style={{ color: "var(--ink)", overflowWrap: "anywhere" }}>
              reads {m.mailbox.address}, folder {m.mailbox.folder}
              {m.mailbox.every ? `, every ${m.mailbox.every}` : ""}
              {m.mailbox.done ? `; what's read goes to ${m.mailbox.done}` : ""}
              <span style={quiet}> · {sourceLabel(m.mailbox.imap)}</span>
            </div>
          )}
          {m.oauth && (
            <div style={quiet}>
              logs in with OAuth at {m.oauth.address} · {sourceLabel(m.oauth.source)}
            </div>
          )}
        </div>
      ))}
    </>
  );
}

/** Feeds: the streams of events flows listen to (`"feed:prices"`), each at an address. */
function FeedsTab() {
  const { list, failed } = useListed(listFeeds);
  return (
    <>
      <div style={about}>
        The streams of events flows listen to. A flow names a feed (<code>&quot;feed:prices&quot;</code>), never a URL: each is bound here to where its events come from, and its
        grant says it hears them (<code>FeedEvent(&quot;prices&quot;)</code>).
      </div>
      <ListedState list={list} failed={failed} what="feeds" />
      {list?.map((f) => (
        <div key={`${f.deployment ?? ""}/${f.name}`} style={row} aria-label={`feed ${f.name}`}>
          <span style={{ color: "var(--accent)", minWidth: 90 }}>feed {f.name}</span>
          <span style={{ color: "var(--ink)", flex: 1, overflowWrap: "anywhere" }}>{f.address}</span>
          <span style={quiet}>
            {sourceLabel(f.url)}
            {f.headers && Object.keys(f.headers).length > 0 ? ` · headers ${Object.keys(f.headers).join(", ")}` : ""}
            {f.buffer != null ? ` · keeps ${f.buffer} events` : ""}
            {f.idle ? ` · idle after ${f.idle}` : ""}
            {forWhom(f.deployment)}
          </span>
        </div>
      ))}
    </>
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
