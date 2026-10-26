// The admin panel (admins only; holon-server's auth.rs), in three tabs (docs/L4s/databases-L4.md
// §9.1): people and sign-in (the people who use this server, the API tokens scripts use, signing
// in through an identity provider); resources (the folders and databases flows read and write:
// folders.rs, databases.rs); and permissions (what this host allows, and who holds what). Below
// each, what was done, by whom.
// Adding someone gives a one-time link for them to set their password; resetting gives a new
// one. The server refuses any change that would leave no admin. A grant changes only on its
// deployment's card: permissions links there.

import { Fragment, useCallback, useEffect, useState, type CSSProperties } from "react";
import {
  changeAccount,
  bindDatabase,
  bindFolder,
  createToken,
  getOidcSettings,
  getPermissions,
  listDatabases,
  listFolders,
  listGrants,
  listTables,
  inviteAccount,
  listAccounts,
  listTokens,
  reloadPermissions,
  removeAccount,
  resetAccount,
  revokeToken,
  setOidcSettings,
  unbindDatabase,
  unbindFolder,
  type AccountEntry,
  type DatabaseBinding,
  type DatabaseEntry,
  type FolderBinding,
  type FolderEntry,
  type HeldGrant,
  type HostPolicy,
  type OidcSettings,
  type Role,
  type TokenEntry,
} from "./server";
import { useDialog } from "./Dialog";
import { FolderPicker } from "./FolderPicker";
import { ActivityList } from "./Pages";
import { describeItem } from "./holon";
import { holdersOf, kindsHeld, othersHeld, usesText, type Holder } from "./holdings";

const ROLES: Role[] = ["viewer", "editor", "admin", "agent"];
const ROLE_TITLE: Record<Role, string> = {
  viewer: "reads flows, deployments and logs",
  editor: "also saves, deploys, runs and deletes",
  admin: "also writes secrets and manages this panel",
  agent: "reads, proposes flows and calls them; never approves, starts, stops or deletes",
};

type Tab = "people" | "resources" | "permissions";
const TABS: { tab: Tab; label: string; about: string }[] = [
  { tab: "people", label: "people and sign-in", about: "who uses this server, and how they get in" },
  { tab: "resources", label: "resources", about: "what exists on this host for flows to use: its folders and databases" },
  { tab: "permissions", label: "permissions", about: "what this host allows, and who holds what" },
];

const small: CSSProperties = { font: "inherit", fontSize: 11, padding: "2px 8px", color: "#9fb4d4", background: "#0e1a2f", border: "1px solid #1b2a45", borderRadius: 3, cursor: "pointer" };
const primary: CSSProperties = { ...small, color: "#0e1a2f", background: "#6fd3e0", border: "none" };
const input: CSSProperties = { font: "inherit", fontSize: 12, padding: "3px 6px", color: "#e6eefb", background: "#0e1a2f", border: "1px solid #1b2a45", borderRadius: 3 };
const head: CSSProperties = { color: "#cfe3ff", fontSize: 14, letterSpacing: "0.04em", margin: "20px 0 6px" };
const row: CSSProperties = { display: "flex", alignItems: "center", gap: 8, padding: "6px 10px", marginBottom: 4, background: "#0e1a2f", border: "1px solid #1b2a45", borderRadius: 4, fontSize: 12 };
const about: CSSProperties = { color: "#9fb4d4", fontSize: 11, marginBottom: 6, lineHeight: 1.45 };
const quiet: CSSProperties = { color: "#7e8ea8", fontSize: 11 };
const tabButton = (on: boolean): CSSProperties => ({
  ...small,
  fontSize: 12,
  padding: "4px 12px",
  color: on ? "#0e1a2f" : "#9fb4d4",
  background: on ? "#6fd3e0" : "transparent",
  border: on ? "1px solid #6fd3e0" : "1px solid #1b2a45",
});
/** What holding a resource for whole runs costs, said where it is set. */
const RUN_LOCK_COST = "every run that writes it goes one at a time, for its whole length, across every deployment bound to it";

const messageOf = (e: unknown) => (e instanceof Error ? e.message : String(e));
/** The link someone opens to set their password. */
const linkFor = (token: string) => `${location.origin}${location.pathname}?invite=${encodeURIComponent(token)}`;
/** A size in bytes as people read it (folders.rs's `size_text`): `512 bytes`, `64 KiB`, `1.5 MiB`. */
function sizeText(bytes: number): string {
  for (const [size, unit] of [
    [2 ** 30, "GiB"],
    [2 ** 20, "MiB"],
    [2 ** 10, "KiB"],
  ] as const) {
    if (bytes >= size) {
      const n = bytes / size;
      return Number.isInteger(n) || n >= 100 ? `${n.toFixed(0)} ${unit}` : `${n.toFixed(1)} ${unit}`;
    }
  }
  return `${bytes} bytes`;
}
/** A connection typed in as a URL (`postgres://user:…@host/shop`), not where one is kept: the page never sends it. */
const isUrl = (s: string) => /^[a-z][a-z0-9+.-]*:\/\//i.test(s.trim());
const keyOf = (b: { name: string; deployment: string | null }) => `${b.deployment ?? ""}/${b.name}`;
/** The deployments that use a binding, as a sentence's end: `orders-api and audit`. */
const named = (holders: Holder[]) => {
  const slugs = holders.map((h) => h.slug);
  return slugs.length <= 1 ? (slugs[0] ?? "") : `${slugs.slice(0, -1).join(", ")} and ${slugs[slugs.length - 1]}`;
};

export function AdminPage({ me, onOpenDeployment }: { me: string; onOpenDeployment: (slug: string) => void }) {
  const dialog = useDialog();
  const [tab, setTab] = useState<Tab>("people");
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
  const [grants, setGrants] = useState<HeldGrant[]>([]);
  const [policy, setPolicy] = useState<HostPolicy | null>(null);
  // What reading the policy again said: when, and the deployments it holds off.
  const [reloaded, setReloaded] = useState<{ at: number; attention: [string, string][] } | null>(null);
  const [kind, setKind] = useState("everything");
  const [version, setVersion] = useState(0);

  const refresh = useCallback(() => {
    listAccounts().then(setPeople).catch((e: unknown) => setMsg(messageOf(e)));
    listTokens().then(setTokens).catch((e: unknown) => setMsg(messageOf(e)));
    listFolders().then(setFolders).catch((e: unknown) => setMsg(messageOf(e)));
    listDatabases().then(setDatabases).catch((e: unknown) => setMsg(messageOf(e)));
    listGrants().then(setGrants).catch((e: unknown) => setMsg(messageOf(e)));
    getPermissions().then(setPolicy).catch((e: unknown) => setMsg(messageOf(e)));
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
    if (tab !== "resources") return;
    for (const d of databases) if (!(keyOf(d) in tables)) readTables(d);
  }, [tab, databases, tables, readTables]);

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
      {holders.length === 0 ? "no deployment uses it" : <>used by {named(holders)} (permissions says how)</>}
    </div>
  );
  /** What unbinding (or binding again) does to the deployments that use it, as the server does it now. */
  const unbindingSays = (holders: Holder[]) =>
    holders.length === 0 ? "No deployment uses it." : `${named(holders)} ${holders.length === 1 ? "uses" : "use"} it: they keep running, and their calls to it fail until it is bound again.`;

  const current = TABS.find((t) => t.tab === tab) ?? TABS[0];
  return (
    <div style={{ maxWidth: 760 }}>
      <div style={{ color: "#cfe3ff", fontSize: 14, letterSpacing: "0.04em", marginBottom: 6 }}>admin</div>
      <div role="tablist" aria-label="admin sections" style={{ display: "flex", gap: 6, marginBottom: 6 }}>
        {TABS.map((t) => (
          <button key={t.tab} role="tab" aria-selected={t.tab === tab} style={tabButton(t.tab === tab)} onClick={() => setTab(t.tab)}>
            {t.label}
          </button>
        ))}
      </div>
      <div style={{ color: "#9fb4d4", fontSize: 11, marginBottom: 8 }}>{current.about}</div>
      {msg && (
        <div role="alert" style={{ color: "#e0916f", fontSize: 12, marginBottom: 8 }}>
          {msg}
        </div>
      )}

      {tab === "people" && (
        <>
          <div style={head}>people</div>
          {link && (
            <div style={{ ...row, flexDirection: "column", alignItems: "stretch", borderColor: "#2a5f68" }}>
              <span>
                Send <span style={{ color: "#e6eefb" }}>{link.name}</span> this link: they set their password with it (once, within 7 days).
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
              <span style={{ color: "#6fd3e0", minWidth: 90 }}>
                {p.name}
                {p.name === me && <span style={{ color: "#7e8ea8" }}> (you)</span>}
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
              <span style={{ color: "#7e8ea8", fontSize: 11, flex: 1 }}>
                {p.email ?? ""}
                {p.invited && <span style={{ color: "#e0c36f" }}> · link not used yet</span>}
                {p.disabled && <span style={{ color: "#e0916f" }}> · disabled</span>}
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

          <div style={head}>API tokens</div>
          <div style={{ color: "#9fb4d4", fontSize: 11, marginBottom: 6 }}>for scripts and CI: sent as Authorization: Bearer &lt;token&gt;, acting with their role</div>
          {secret && (
            <div style={{ ...row, flexDirection: "column", alignItems: "stretch", borderColor: "#2a5f68" }}>
              <span>
                The token for <span style={{ color: "#e6eefb" }}>{secret.label}</span>. Copy it now: it isn't shown again.
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
          {tokens.length === 0 && <div style={{ color: "#7e8ea8", fontSize: 11, marginBottom: 6 }}>no API tokens</div>}
          {tokens.map((t) => (
            <div key={t.id} style={row}>
              <span style={{ color: "#6fd3e0", minWidth: 90 }}>{t.label}</span>
              <span>{t.role}</span>
              <span style={{ color: "#7e8ea8", fontSize: 11, flex: 1 }}>
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

          <div style={head}>signing in through an identity provider</div>
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
          <div style={{ color: "#7e8ea8", fontSize: 11 }}>{oidc ? `on: ${oidc.issuer} as ${oidc.client_id}` : "off"}</div>
        </>
      )}

      {tab === "resources" && (
        <>
          <div style={head}>folders</div>
          <div style={about}>
            Files flows read and write on this server&apos;s machine. A flow names a folder (<code>folder inbox</code>), never a path: bind each name to a real folder here.
            Read only: flows may read it, not write it. Reads kept by hash: the run log keeps a hash of what each run read, not its text; for a while (7d, 12h):
            its text for that long, then only its hash. A cap (10M): a write that would take the folder&apos;s files past it doesn&apos;t land. For one deployment:
            that deployment sees this folder by the name, whatever the server&apos;s is, and no other deployment sees it. Other servers bind it too (a network share):
            their runs&apos; commits lock a file in it, so they never write over each other. Held for whole runs: each run that touches it holds it from before
            it starts until it commits, fails or waits, so every run that writes it goes one at a time, for its whole length, across every deployment bound to
            it; one that waits past its claim_wait is turned away (503). For files a program expects one writer for.
          </div>
          {folders.length === 0 && <div style={{ color: "#7e8ea8", fontSize: 11, marginBottom: 6 }}>no folders bound</div>}
          {folders.map((f) => (
            <Fragment key={keyOf(f)}>
              <div style={row}>
                <span style={{ color: "#6fd3e0", minWidth: 90 }}>folder {f.name}</span>
                <span style={{ color: "#e6eefb", flex: 1, overflowWrap: "anywhere" }}>{f.path}</span>
                <span style={{ color: "#7e8ea8", fontSize: 11 }}>
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
            <label style={{ color: "#9fb4d4", fontSize: 11, display: "flex", alignItems: "center", gap: 4 }} title="a network share other Holon servers bind too: a commit also locks a file in it">
              <input type="checkbox" checked={folderDraft.shared ?? false} onChange={(e) => setFolderDraft({ ...folderDraft, shared: e.target.checked })} />
              other servers bind it too
            </label>
            <label style={{ color: "#9fb4d4", fontSize: 11, display: "flex", alignItems: "center", gap: 4 }} title={`each run that touches it holds it for its whole length: ${RUN_LOCK_COST}`}>
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
            {folderDraft.lock === "run" && <div style={{ ...quiet, color: "#e0c36f", flexBasis: "100%" }}>held for whole runs: {RUN_LOCK_COST}</div>}
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

          <div style={head}>databases</div>
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
          {databases.length === 0 && <div style={{ color: "#7e8ea8", fontSize: 11, marginBottom: 6 }}>no databases bound</div>}
          {databases.map((d) => {
            const read = tables[keyOf(d)];
            return (
              <Fragment key={keyOf(d)}>
                <div style={row}>
                  <span style={{ color: "#6fd3e0", minWidth: 90 }}>database {d.name}</span>
                  <span style={{ color: "#e6eefb", flex: 1, overflowWrap: "anywhere" }}>{d.connection ? `${d.address ?? ""} (from ${d.connection})` : d.path}</span>
                  <span style={{ color: "#7e8ea8", fontSize: 11 }}>
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
                        ? <span style={{ color: "#e0916f" }}>its tables couldn&apos;t be read: {read.error}</span>
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
            <label style={{ color: "#9fb4d4", fontSize: 11, display: "flex", alignItems: "center", gap: 4 }} title={`each run that touches it holds it for its whole length: ${RUN_LOCK_COST}`}>
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
            {databaseDraft.lock === "run" && <div style={{ ...quiet, color: "#e0c36f", flexBasis: "100%" }}>held for whole runs: {RUN_LOCK_COST}</div>}
            {isUrl(databaseDraft.where) && (
              <div role="alert" style={{ ...quiet, color: "#e0916f", flexBasis: "100%" }}>
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

      {tab === "permissions" && (
        <>
          <div style={head}>the policy</div>
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
              <table aria-label="limits" style={{ fontSize: 11, color: "#9fb4d4", borderCollapse: "collapse" }}>
                <thead>
                  <tr style={{ color: "#7e8ea8", textAlign: "left" }}>
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
                        <td style={{ color: "#e6eefb", paddingRight: 16 }}>{name}</td>
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

          <div style={head}>who may touch what</div>
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
                <span style={{ color: "#e6eefb", flex: 1, minWidth: 200 }}>{describeItem(o.item)}</span>
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

      <ActivityList version={String(version)} />
    </div>
  );
}

/** A resource in who may touch what: the deployments granted it, how, and each one's card. */
function Resource({ title, detail, holders, onOpen }: { title: string; detail: string; holders: Holder[]; onOpen: (slug: string) => void }) {
  return (
    <div aria-label={title} style={{ ...row, flexDirection: "column", alignItems: "stretch", gap: 4 }}>
      <div style={{ display: "flex", gap: 8 }}>
        <span style={{ color: "#6fd3e0" }}>{title}</span>
        <span style={{ ...quiet, flex: 1, overflowWrap: "anywhere" }}>{detail}</span>
      </div>
      {holders.length === 0 && <div style={quiet}>no deployment is granted it</div>}
      {holders.map((h) => (
        <div key={h.slug} style={{ display: "flex", gap: 8, alignItems: "center", paddingLeft: 10, fontSize: 11 }}>
          <span style={{ color: "#e6eefb", minWidth: 110 }}>{h.slug}</span>
          <span style={{ color: h.status === "running" ? "#9ad29f" : "#e0c36f", minWidth: 54 }} title={h.attention ?? h.status}>
            {h.status}
          </span>
          <span style={{ flex: 1, color: "#9fb4d4" }}>
            {usesText(h.uses).map((u, i) => (
              <Fragment key={i}>
                {i > 0 && " · "}
                <span style={u.revoked ? { textDecoration: "line-through", color: "#e0916f" } : undefined} title={u.revoked ? "revoked" : undefined}>
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
