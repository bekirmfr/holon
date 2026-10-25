// The admin panel (admins only; holon-server's auth.rs): the people who use this server, the API
// tokens scripts use, signing in through an identity provider, the folders flows read and write
// (folders.rs), and what was done, by whom.
// Adding someone gives a one-time link for them to set their password; resetting gives a new
// one. The server refuses any change that would leave no admin.

import { useCallback, useEffect, useState, type CSSProperties } from "react";
import {
  changeAccount,
  bindFolder,
  createToken,
  getOidcSettings,
  listFolders,
  inviteAccount,
  listAccounts,
  listTokens,
  removeAccount,
  resetAccount,
  revokeToken,
  setOidcSettings,
  unbindFolder,
  type AccountEntry,
  type FolderBinding,
  type FolderEntry,
  type OidcSettings,
  type Role,
  type TokenEntry,
} from "./server";
import { useDialog } from "./Dialog";
import { ActivityList } from "./Pages";

const ROLES: Role[] = ["viewer", "editor", "admin", "agent"];
const ROLE_TITLE: Record<Role, string> = {
  viewer: "reads flows, deployments and logs",
  editor: "also saves, deploys, runs and deletes",
  admin: "also writes secrets and manages this panel",
  agent: "reads, proposes flows and calls them; never approves, starts, stops or deletes",
};

const small: CSSProperties = { font: "inherit", fontSize: 11, padding: "2px 8px", color: "#9fb4d4", background: "#0e1a2f", border: "1px solid #1b2a45", borderRadius: 3, cursor: "pointer" };
const primary: CSSProperties = { ...small, color: "#0e1a2f", background: "#6fd3e0", border: "none" };
const input: CSSProperties = { font: "inherit", fontSize: 12, padding: "3px 6px", color: "#e6eefb", background: "#0e1a2f", border: "1px solid #1b2a45", borderRadius: 3 };
const head: CSSProperties = { color: "#cfe3ff", fontSize: 14, letterSpacing: "0.04em", margin: "20px 0 6px" };
const row: CSSProperties = { display: "flex", alignItems: "center", gap: 8, padding: "6px 10px", marginBottom: 4, background: "#0e1a2f", border: "1px solid #1b2a45", borderRadius: 4, fontSize: 12 };

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

export function AdminPage({ me }: { me: string }) {
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
  const [folderDraft, setFolderDraft] = useState<FolderBinding>({ name: "", path: "", access: "read-write", keep: "bytes", cap: null, deployment: null });
  // How long a folder keeps what runs read, when it keeps it for a while ("7d").
  const [keepFor, setKeepFor] = useState("");
  const [version, setVersion] = useState(0);

  const refresh = useCallback(() => {
    listAccounts().then(setPeople).catch((e: unknown) => setMsg(messageOf(e)));
    listTokens().then(setTokens).catch((e: unknown) => setMsg(messageOf(e)));
    listFolders().then(setFolders).catch((e: unknown) => setMsg(messageOf(e)));
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

  return (
    <div style={{ maxWidth: 760 }}>
      <div style={{ color: "#cfe3ff", fontSize: 14, letterSpacing: "0.04em", marginBottom: 4 }}>admin</div>
      <div style={{ color: "#9fb4d4", fontSize: 11, marginBottom: 8 }}>who uses this server, and how they get in</div>
      {msg && (
        <div role="alert" style={{ color: "#e0916f", fontSize: 12, marginBottom: 8 }}>
          {msg}
        </div>
      )}

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
      {tokens.length === 0 && <div style={{ color: "#7e8ea8", fontSize: 11, marginBottom: 6 }}>none</div>}
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

      <div style={head}>folders</div>
      <div style={{ color: "#9fb4d4", fontSize: 11, marginBottom: 6, lineHeight: 1.45 }}>
        Files flows read and write on this server&apos;s machine. A flow names a folder (<code>folder inbox</code>), never a path: bind each name to a real folder here.
        Read only: flows may read it, not write it. Reads kept by hash: the run log keeps a hash of what each run read, not its text; for a while (7d, 12h):
        its text for that long, then only its hash. A cap (10M): a write that would take the folder&apos;s files past it doesn&apos;t land. For one deployment:
        that deployment sees this folder by the name, whatever the server&apos;s is, and no other deployment sees it.
      </div>
      {folders.length === 0 && <div style={{ color: "#7e8ea8", fontSize: 11, marginBottom: 6 }}>none</div>}
      {folders.map((f) => (
        <div key={`${f.deployment ?? ""}/${f.name}`} style={row}>
          <span style={{ color: "#6fd3e0", minWidth: 90 }}>{f.name}</span>
          <span style={{ color: "#e6eefb", flex: 1, overflowWrap: "anywhere" }}>{f.path}</span>
          <span style={{ color: "#7e8ea8", fontSize: 11 }}>
            {f.access === "read" ? "read only" : "read and write"}
            {f.keep === "hash" ? " · reads kept by hash" : f.keep.startsWith("bytes for ") ? ` · reads kept ${f.keep.slice("bytes for ".length)}, then by hash` : ""}
            {f.cap != null ? ` · capped at ${sizeText(f.cap)}` : ""}
            {f.deployment ? ` · for ${f.deployment} only` : ""}
          </span>
          <button
            style={small}
            title="unbind the name: the files stay where they are"
            onClick={async () => {
              const whose = f.deployment ? ` for ${f.deployment}` : "";
              if (await dialog.confirm(`Unbind the folder "${f.name}"${whose}? Its files stay where they are; deployments that use it can't read or write it until it is bound again.`, "Unbind"))
                act(() => unbindFolder(f.name, f.deployment));
            }}
          >
            ×
          </button>
        </div>
      ))}
      <form
        style={{ ...row, background: "transparent", flexWrap: "wrap" }}
        onSubmit={(e) => {
          e.preventDefault();
          act(async () => {
            const keep = folderDraft.keep === "bytes for " ? `bytes for ${keepFor.trim()}` : folderDraft.keep;
            const cap = typeof folderDraft.cap === "string" ? folderDraft.cap.trim() || null : folderDraft.cap;
            await bindFolder({ ...folderDraft, name: folderDraft.name.trim(), path: folderDraft.path.trim(), keep: keep as FolderEntry["keep"], cap, deployment: folderDraft.deployment?.trim() || null });
            setFolderDraft({ ...folderDraft, name: "", path: "", cap: null, deployment: null });
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
        <input
          aria-label="for one deployment"
          placeholder="for one deployment (empty: all)"
          value={folderDraft.deployment ?? ""}
          onChange={(e) => setFolderDraft({ ...folderDraft, deployment: e.target.value })}
          style={input}
        />
        <button
          type="submit"
          style={primary}
          disabled={!folderDraft.name.trim() || !folderDraft.path.trim() || (folderDraft.keep === "bytes for " && !keepFor.trim())}
        >
          bind folder
        </button>
      </form>

      <div style={head}>signing in through an identity provider</div>
      <div style={{ color: "#9fb4d4", fontSize: 11, marginBottom: 6, lineHeight: 1.45 }}>
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

      <ActivityList version={String(version)} />
    </div>
  );
}
