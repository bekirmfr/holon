// People and access (#people; access-and-approvals §12), in tabs: People, Agents and tokens,
// Groups, Roles, Bindings, History and Signing in. Each principal has a page of its own
// (#people.who.<name>): what it is, its tokens, its groups, its bindings, and what they let it do
// on the host and on each flow (access.ts givesOf, as `can` decides). So has each group
// (#people.groups.<name>: its members and what is bound to it) and each version of the access
// document (#people.history.v<n>: what it changed from the one before). A binding is made with one
// form (who, role, where, until when and why: `holon bind` in the editor), under the bindings it
// adds to, and taken away from its row, saying why. The server refuses, in its own words, what its
// author doesn't hold. Shown to whoever may view people (an admin, or a role holding people.view
// on the host); a person who may not sees the agents and services they own (OwnedPage). Adding
// someone gives a one-time link for them to set their password; resetting gives a new one. The
// server refuses any change that would leave no admin.

import { Fragment, useCallback, useEffect, useId, useState, type CSSProperties, type ReactNode } from "react";
import {
  allowsSaid,
  approvalsOf,
  bindingSaid,
  catalogueOf,
  changesOf,
  givesOf,
  heldBy,
  hostRoleOf,
  otherBindings,
  rolesFor,
  roleTitle,
  scopeSaid,
  seesFlow,
  subjectSaid,
  untilOf,
  utc,
  type Gives,
  type Tagged,
} from "./access";
import { useDialog } from "./Dialog";
import { gate, useCan } from "./can";
import { about, HostPage, input, messageOf, primary, quiet, row, small } from "./HostPages";
import { format, PEOPLE_LABELS, PEOPLE_TABS, type PeopleTab, type Route } from "./routes";
import {
  addPrincipal,
  bind,
  changePrincipal,
  createToken,
  defineRole,
  getAccessVersion,
  getManifest,
  getOidcSettings,
  invitePerson,
  listAccessVersions,
  listBindings,
  listDeployments,
  listGroups,
  listOwned,
  listPrincipals,
  listRoles,
  putGroup,
  removeGroup,
  removePrincipal,
  removeRole,
  resetPerson,
  revokeToken,
  ServerError,
  setOidcSettings,
  unbind,
  type AccessDocument,
  type AccessVersion,
  type BindingEntry,
  type GroupEntry,
  type Kind,
  type OidcSettings,
  type Principal,
  type RoleEntry,
  type Scope,
  type Subject,
} from "./server";

/** The link someone opens to set their password. */
const linkFor = (token: string) => `${location.origin}${location.pathname}?invite=${encodeURIComponent(token)}`;
const link = { color: "var(--accent)", textDecoration: "none" } as const;

/** What People and access reads, and `act`: it does a change, says what failed in the server's
 *  words, and reads everything again. A list the server won't show whoever is signed in (403) is
 *  left empty: they see what they may. */
function useAccess() {
  const dialog = useDialog();
  const [principals, setPrincipals] = useState<Principal[]>([]);
  // Whether the people were read (or refused), so a page can say no one is named so.
  const [loaded, setLoaded] = useState(false);
  // Every role (`GET /roles`), the built-in ones first: what a kind is offered.
  const [roles, setRoles] = useState<RoleEntry[]>([]);
  const [groups, setGroups] = useState<GroupEntry[]>([]);
  const [bindings, setBindings] = useState<BindingEntry[]>([]);
  // The deployed flows and their tags: where a tag's binding holds.
  const [flows, setFlows] = useState<Tagged[]>([]);
  const [versions, setVersions] = useState<AccessVersion[]>([]);
  const [oidc, setOidc] = useState<OidcSettings | null>(null);
  const [msg, setMsg] = useState("");
  const [version, setVersion] = useState(0);

  const refresh = useCallback(() => {
    const failed = (e: unknown) => {
      if (!(e instanceof ServerError && e.status === 403)) setMsg(messageOf(e));
    };
    listPrincipals()
      .then(setPrincipals, failed)
      .finally(() => setLoaded(true));
    listRoles().then(setRoles, failed);
    listGroups().then(setGroups, failed);
    listBindings().then(setBindings, failed);
    listDeployments().then((ds) => setFlows(ds.map((d) => ({ slug: d.slug, tags: d.tags ?? [] }))), failed);
    listAccessVersions().then(setVersions, failed);
    getOidcSettings().then(setOidc, failed);
    setVersion((v) => v + 1);
  }, []);
  useEffect(refresh, [refresh]);

  /** Do `f`, say what failed, and read everything again: whether it was done. */
  const act = async (f: () => Promise<unknown>): Promise<boolean> => {
    setMsg("");
    try {
      await f();
      return true;
    } catch (e) {
      setMsg(messageOf(e));
      return false;
    } finally {
      refresh();
    }
  };
  return { dialog, principals, loaded, roles, groups, bindings, flows, versions, oidc, msg, version, refresh, act };
}
type Access = ReturnType<typeof useAccess>;
type Act = (f: () => Promise<unknown>) => Promise<unknown>;

/** A token just made: whose and what for, and its value, shown once. */
type MadeToken = { label: string; token: string };

/** An agent's or a service's tokens: its owner or an admin makes them, each shown once (`show`). */
async function makeToken(p: Principal, dialog: ReturnType<typeof useDialog>, act: Act, show: (made: MadeToken) => void) {
  const label = await dialog.prompt(`What is ${p.name}'s new token for? (GitHub CI, a laptop…)`, "", "make token");
  if (!label?.trim()) return;
  void act(async () => {
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

/** `p`'s tokens, each revoked from its row; or that it has none, so it can't sign in yet. */
function TokenRows({ p, dialog, act }: { p: Principal; dialog: ReturnType<typeof useDialog>; act: Act }) {
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
          <button
            style={small}
            aria-label={`revoke ${p.name}'s token ${t.label}`}
            onClick={async () => {
              if (await dialog.confirm(`Revoke ${p.name}'s token "${t.label}"? Whatever signs in with it stops working.`, "Revoke")) void act(() => revokeToken(p.name, t.id));
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
      <a href={format({ kind: "docs", page: "guides/agents.md", anchor: null })} style={{ ...link, fontSize: 11 }}>
        working with agents, in the manual ›
      </a>
    </>
  );
}

/** What a person who may not view people owns (auth.rs `GET /auth/owned`): their agents and
 *  services, each with its tokens, which they make (each shown once) and revoke. An admin adds an
 *  agent or a service and names its owner; People and access shows an admin everyone's. */
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
 *  defined included. None when it holds one only through a group or elsewhere (Bindings). */
function RoleSelect({ p, roles, act }: { p: Principal; roles: RoleEntry[]; act: Act }) {
  const now = hostRoleOf(p);
  return (
    <select aria-label={`${p.name}'s role`} value={now} style={input} title={roleTitle(roles, now)} onChange={(e) => void act(() => changePrincipal(p.name, { role: e.target.value }))}>
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
 *  on a flow or an approval point, or through a group (its page lists them, to change). */
function HeldElsewhere({ p }: { p: Principal }) {
  const others = otherBindings(p);
  if (others.length === 0) return null;
  return (
    <span aria-label={`${p.name}'s bindings`} style={{ display: "block", color: "var(--muted)" }} title="their page lists each binding, to unbind or add to">
      {others.map(bindingSaid).join(", ")}
    </span>
  );
}

/** A principal's name, linking to its page. */
const Who = ({ name, style }: { name: string; style?: CSSProperties }) => (
  <a href={format({ kind: "principal", name })} style={{ ...link, ...style }}>
    {name}
  </a>
);

// ── the tabs ──────────────────────────────────────────────────────────────────────────────────

/** People and access: its tabs, and the page `route` names (a tab, a principal, a group, or a
 *  version of the access document). */
export function PeoplePage({ me, route }: { me: string; route: Route }) {
  const a = useAccess();
  const kindOf = (name: string) => a.principals.find((p) => p.name === name)?.kind;
  const tab: PeopleTab =
    route.kind === "people"
      ? route.tab
      : route.kind === "group"
        ? "groups"
        : route.kind === "access-version"
          ? "history"
          : route.kind === "principal" && (kindOf(route.name) ?? "person") !== "person"
            ? "agents"
            : "people";
  return (
    <HostPage title="People and access" about="who uses this server, what each may do and where, and every change to that" tab={null} host={a}>
      <nav className="hl-flow__tabs" aria-label="people tabs">
        {PEOPLE_TABS.map((t) => (
          <a key={t} href={format({ kind: "people", tab: t })} aria-current={t === tab ? "page" : undefined}>
            {PEOPLE_LABELS[t]}
          </a>
        ))}
      </nav>
      <div style={{ marginTop: 12 }}>
        {route.kind === "principal" ? (
          <PrincipalView key={route.name} name={route.name} me={me} a={a} />
        ) : route.kind === "group" ? (
          <GroupView key={route.name} name={route.name} a={a} />
        ) : route.kind === "access-version" ? (
          <VersionView n={route.version} latest={a.versions[0]?.version ?? null} />
        ) : tab === "people" ? (
          <PeopleList me={me} a={a} />
        ) : tab === "agents" ? (
          <AgentsList me={me} a={a} />
        ) : tab === "groups" ? (
          <GroupsList a={a} />
        ) : tab === "roles" ? (
          <RolesList a={a} />
        ) : tab === "bindings" ? (
          <>
            <div style={about}>
              A binding gives a role somewhere: on this host (every flow, and the host&apos;s own pages), on one flow, on the flows deployed with a tag (those deployed
              later too), or at one approval point of a flow, to decide it. It goes to a principal or a group, until it is taken away or the moment it ends. A
              binding gives only what whoever makes it holds there, and is taken away only by someone who holds what it gives. Every change is a version, in History.
            </div>
            <BindingTable rows={a.bindings} a={a} label="bindings" />
            <BindForm a={a} />
          </>
        ) : tab === "history" ? (
          <HistoryList a={a} />
        ) : (
          <SignIn a={a} />
        )}
      </div>
    </HostPage>
  );
}

function PeopleList({ me, a }: { me: string; a: Access }) {
  const { principals, roles, dialog, act } = a;
  const [invite, setInvite] = useState<{ name: string; url: string } | null>(null);
  const [draft, setDraft] = useState({ name: "", role: "editor", email: "" });
  const people = principals.filter((p) => p.kind === "person");
  const others = principals.filter((p) => p.kind !== "person");
  return (
    <>
      {invite && (
        <div style={{ ...row, flexDirection: "column", alignItems: "stretch", borderColor: "var(--sel)" }}>
          <span>
            Send <span style={{ color: "var(--ink)" }}>{invite.name}</span> this link: they set their password with it (once, within 7 days).
          </span>
          <span style={{ display: "flex", gap: 6 }}>
            <input aria-label="invite link" readOnly value={invite.url} style={{ ...input, flex: 1 }} onFocus={(e) => e.target.select()} />
            <button style={small} onClick={() => navigator.clipboard?.writeText(invite.url)}>
              copy
            </button>
            <button style={small} onClick={() => setInvite(null)}>
              done
            </button>
          </span>
        </div>
      )}
      {people.map((p) => {
        const owns = others.filter((o) => o.owner === p.name).map((o) => o.name);
        return (
          <div key={p.name} style={row}>
            <span style={{ minWidth: 90 }}>
              <Who name={p.name} />
              {p.name === me && <span style={{ color: "var(--faint)" }}> (you)</span>}
            </span>
            <RoleSelect p={p} roles={roles} act={act} />
            <span style={{ color: "var(--faint)", fontSize: 11, flex: 1 }}>
              <HeldElsewhere p={p} />
              {p.email ?? ""}
              {owns.length > 0 && <span> · owns {owns.join(", ")}</span>}
              {p.invited && <span style={{ color: "var(--warn)" }}> · link not used yet</span>}
              {p.disabled && <span style={{ color: "var(--bad)" }}> · disabled</span>}
            </span>
            <button
              style={small}
              title="the email they sign in with through the identity provider"
              onClick={async () => {
                const email = await dialog.prompt(`${p.name}'s email, for signing in through the identity provider (empty for none):`, p.email ?? "");
                if (email !== null) void act(() => changePrincipal(p.name, { email }));
              }}
            >
              email
            </button>
            <button
              style={small}
              title="a new link to set a new password; their sign-ins end"
              onClick={async () => {
                if (!(await dialog.confirm(`Reset ${p.name}'s password? Their sign-ins end, and you get a new link for them.`, "Reset"))) return;
                void act(async () => {
                  const r = await resetPerson(p.name);
                  setInvite({ name: p.name, url: linkFor(r.invite) });
                });
              }}
            >
              reset
            </button>
            <button style={small} onClick={() => void act(() => changePrincipal(p.name, { disabled: !p.disabled }))} title={p.disabled ? "let them sign in again" : "stop them signing in; their sign-ins end"}>
              {p.disabled ? "enable" : "disable"}
            </button>
            <button
              style={small}
              title={owns.length > 0 ? "give what they own another owner first" : "remove them"}
              onClick={async () => {
                if (await dialog.confirm(`Remove ${p.name}? What they deployed stays, and says who did it. This can't be undone.`, "Remove")) void act(() => removePrincipal(p.name));
              }}
            >
              ×
            </button>
          </div>
        );
      })}
      <form
        style={{ ...row, background: "transparent" }}
        onSubmit={(e) => {
          e.preventDefault();
          void act(async () => {
            const r = await invitePerson(draft.name.trim(), draft.role, draft.email.trim());
            setInvite({ name: r.name, url: linkFor(r.invite) });
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
    </>
  );
}

function AgentsList({ me, a }: { me: string; a: Access }) {
  const { principals, roles, dialog, act } = a;
  const [made, setMade] = useState<MadeToken | null>(null);
  // An agent or a service being added: its owner empty for whoever adds it.
  const [draft, setDraft] = useState({ name: "", kind: "agent" as Exclude<Kind, "person">, role: "agent", owner: "" });
  const people = principals.filter((p) => p.kind === "person");
  const others = principals.filter((p) => p.kind !== "person");
  /** Who may own an agent or a service: the people, and whoever owns it now (the OS user, say). */
  const owners = (now: string | null) => [...new Set([...people.map((p) => p.name), ...(now ? [now] : [])])];
  return (
    <>
      <div style={about}>
        An agent (an AI, connected through <code>holon mcp</code>) writes flows, checks them, saves and deploys them, and runs their actions, but never approves what a
        flow may do: that stays with a person. A service is a script or CI, a viewer or an editor, never an admin. Each signs in with tokens of its own (sent as
        Authorization: Bearer &lt;token&gt;) and has an owner, the person answerable for it, who makes and revokes its tokens. Which actions agents may run is each
        deployment&apos;s own setting.
      </div>
      {made && <MadeTokenBox made={made} onDone={() => setMade(null)} />}
      {others.length === 0 && <div style={{ ...quiet, marginBottom: 6 }}>no agent or service yet</div>}
      {others.map((p) => (
        <Fragment key={p.name}>
          <div style={row}>
            <Who name={p.name} style={{ minWidth: 90 }} />
            <span style={quiet}>{p.kind}</span>
            <RoleSelect p={p} roles={roles} act={act} />
            <select aria-label={`${p.name}'s owner`} value={p.owner ?? ""} style={input} title="the person answerable for it" onChange={(e) => void act(() => changePrincipal(p.name, { owner: e.target.value }))}>
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
            <button style={small} aria-label={`make a token for ${p.name}`} title="a new token for it, shown once" onClick={() => makeToken(p, dialog, act, setMade)}>
              make token
            </button>
            <button style={small} onClick={() => void act(() => changePrincipal(p.name, { disabled: !p.disabled }))} title={p.disabled ? "let its tokens sign in again" : "refuse its tokens"}>
              {p.disabled ? "enable" : "disable"}
            </button>
            <button
              style={small}
              title={`remove the ${p.kind}`}
              onClick={async () => {
                if (await dialog.confirm(`Remove ${p.name}? Its tokens stop working. This can't be undone.`, "Remove")) void act(() => removePrincipal(p.name));
              }}
            >
              ×
            </button>
          </div>
          <TokenRows p={p} dialog={dialog} act={act} />
        </Fragment>
      ))}
      <form
        style={{ ...row, background: "transparent" }}
        onSubmit={(e) => {
          e.preventDefault();
          void act(async () => {
            await addPrincipal(draft.name.trim(), draft.kind, draft.role, draft.owner);
            setDraft({ ...draft, name: "" });
          });
        }}
      >
        <input aria-label="new agent's or service's name" placeholder="name" value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} style={input} />
        <select
          aria-label="new agent's or service's kind"
          value={draft.kind}
          onChange={(e) => {
            const kind = e.target.value as Exclude<Kind, "person">;
            setDraft({ ...draft, kind, role: kind === "agent" ? "agent" : "viewer" });
          }}
          style={input}
        >
          <option value="agent">agent</option>
          <option value="service">service</option>
        </select>
        <select aria-label="new agent's or service's role" value={draft.role} title={roleTitle(roles, draft.role)} onChange={(e) => setDraft({ ...draft, role: e.target.value })} style={input}>
          {rolesFor(roles, draft.kind).map((r) => (
            <option key={r.name} value={r.name} title={roleTitle(roles, r.name)}>
              {r.name}
            </option>
          ))}
        </select>
        <select aria-label="new agent's or service's owner" value={draft.owner} onChange={(e) => setDraft({ ...draft, owner: e.target.value })} style={{ ...input, flex: 1 }}>
          <option value="">owned by you</option>
          {people
            .filter((p) => p.name !== me)
            .map((p) => (
              <option key={p.name} value={p.name}>
                owned by {p.name}
              </option>
            ))}
        </select>
        <button type="submit" style={primary} disabled={!draft.name.trim()}>
          add {draft.kind}
        </button>
      </form>
      <ConnectingAnAgent />
    </>
  );
}

function SignIn({ a }: { a: Access }) {
  const [issuer, setIssuer] = useState("");
  const [clientId, setClientId] = useState("");
  useEffect(() => {
    setIssuer(a.oidc?.issuer ?? "");
    setClientId(a.oidc?.client_id ?? "");
  }, [a.oidc]);
  return (
    <>
      <div className="hl-host-page__head">signing in through an identity provider</div>
      <div style={about}>
        Optional: people may also sign in through your provider (Keycloak, Okta, Auth0, Entra ID…), registered there as a public client with PKCE and this
        page&apos;s address as its redirect. Someone signing in is matched to the person with their email (set under People).
      </div>
      <div style={{ ...row, background: "transparent", flexWrap: "wrap" }}>
        <input aria-label="issuer" placeholder="issuer (https://id.example.com/realms/x)" value={issuer} onChange={(e) => setIssuer(e.target.value)} style={{ ...input, flex: 2, minWidth: 220 }} />
        <input aria-label="client id" placeholder="client id" value={clientId} onChange={(e) => setClientId(e.target.value)} style={{ ...input, flex: 1, minWidth: 120 }} />
        <button style={primary} disabled={!issuer.trim() || !clientId.trim()} onClick={() => void a.act(() => setOidcSettings({ issuer: issuer.trim(), client_id: clientId.trim() }))}>
          {a.oidc ? "update" : "turn on"}
        </button>
        {a.oidc && (
          <button style={small} onClick={() => void a.act(() => setOidcSettings(null))}>
            turn off
          </button>
        )}
      </div>
      <div style={{ color: "var(--faint)", fontSize: 11 }}>{a.oidc ? `on: ${a.oidc.issuer} as ${a.oidc.client_id}` : "off"}</div>
    </>
  );
}

// ── bindings: the table and the form ──────────────────────────────────────────────────────────

/** A binding as its row shows it; on a principal's page, `via` the group it holds through. */
type Row = BindingEntry & { via?: string | null };
const rowsOf = (p: Principal): Row[] =>
  p.bindings.map((b) => ({ id: b.id, who: b.group ? { group: b.group } : { principal: p.name }, role: b.role, scope: b.scope, by: b.by, at: b.at, expires_at: b.expires_at, expired: b.expired, via: b.group }));
/** Whom a binding is to, as a link to their page. */
const SubjectLink = ({ who }: { who: Subject }) => (
  <a href={format("principal" in who ? { kind: "principal", name: who.principal } : { kind: "group", name: who.group })}>{subjectSaid(who)}</a>
);

/** Bindings, a row each: to whom (a link to their page), the role (what it allows, on hover),
 *  where, who bound it and when, until when; and unbind, which asks why and keeps it in History.
 *  On a principal's page (`own`), the group a binding holds through instead of whom. */
function BindingTable({ rows, a, label, own }: { rows: Row[]; a: Access; label: string; own?: boolean }) {
  // Unbinding, to whoever the server would let (can.ts).
  const can = useCan();
  if (rows.length === 0) return <div style={{ ...quiet, marginBottom: 8 }}>no binding</div>;
  const unbindIt = async (b: Row) => {
    const what = `${b.role} from ${subjectSaid(b.who)} ${scopeSaid(b.scope)}`;
    const many = own && b.via ? ` It holds through the group ${b.via}: unbinding takes it from every member.` : "";
    const why = await a.dialog.prompt(`Unbind ${what}?${many}\n\nWhy: History keeps it.`, "", "Unbind", { required: "Unbinding says why: type a reason." });
    if (why !== null) void a.act(() => unbind(b.id, why));
  };
  return (
    <table className="hl-table" aria-label={label} style={{ marginBottom: 10 }}>
      <thead>
        <tr>
          <th>{own ? "through" : "to"}</th>
          <th>role</th>
          <th>where</th>
          <th>bound by</th>
          <th>until</th>
          <th />
        </tr>
      </thead>
      <tbody>
        {rows.map((b) => (
          <tr key={b.id} className={b.expired ? "hl-table__dim" : undefined}>
            <td>{own ? b.via ? <SubjectLink who={{ group: b.via }} /> : <span className="hl-table__dim">its own</span> : <SubjectLink who={b.who} />}</td>
            <td title={roleTitle(a.roles, b.role)}>{b.role}</td>
            <td>{scopeSaid(b.scope)}</td>
            <td>
              {b.by || "—"}, {utc(b.at)}
            </td>
            <td>{b.expires_at ? `${utc(b.expires_at)}${b.expired ? " (expired)" : ""}` : "—"}</td>
            <td>
              <button style={small} aria-label={`unbind ${b.role} from ${subjectSaid(b.who)} ${scopeSaid(b.scope)}`} onClick={() => void unbindIt(b)} {...gate(can, "unbind")}>
                unbind
              </button>
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/** Where a binding is made, as the form asks it. */
type Where = "host" | "flow" | "tag" | "approval";
const keyOf = (who: Subject) => ("principal" in who ? `principal:${who.principal}` : `group:${who.group}`);
const subjectOf = (key: string): Subject | null => {
  const at = key.indexOf(":");
  const [kind, name] = [key.slice(0, at), key.slice(at + 1)];
  return at < 0 || name === "" ? null : kind === "group" ? { group: name } : { principal: name };
};

/** The binding form (`holon bind` in the editor): to whom (a principal or a group; fixed on their
 *  own page), the role (those its kind may hold), where (this host, a flow by its slug, the flows
 *  with a tag, or one approval point of a flow, from its deployed version), until when and why. The
 *  server refuses, in its own words, a role it doesn't know, one that holds nothing there, and
 *  more than its author holds there. Bound at an approval point, it says when the holder can't see
 *  the flow, as `holon bind` does, and offers to bind viewer there too. */
function BindForm({ a, who: fixed }: { a: Access; who?: Subject }) {
  const id = useId();
  // Binding, to whoever the server would let (can.ts): anyone else is told why on the button.
  const can = useCan();
  const [who, setWho] = useState(fixed ? keyOf(fixed) : "");
  const [role, setRole] = useState("");
  const [where, setWhere] = useState<Where>("flow");
  const [flow, setFlow] = useState("");
  const [tag, setTag] = useState("");
  const [point, setPoint] = useState("");
  const [points, setPoints] = useState<string[]>([]);
  const [until, setUntil] = useState("");
  const [why, setWhy] = useState("");
  const [said, setSaid] = useState<{ done?: string; error?: string; blind?: { who: Subject; flow: string; point: string } } | null>(null);
  const subject = subjectOf(fixed ? keyOf(fixed) : who);
  const kind = subject && "principal" in subject ? a.principals.find((p) => p.name === subject.principal)?.kind : undefined;
  const offered = kind ? rolesFor(a.roles, kind) : a.roles;
  const slug = flow.trim();
  const deployed = a.flows.some((f) => f.slug === slug);
  // An approval point is one of the flow's, as its deployed version has them.
  useEffect(() => {
    if (where !== "approval" || !deployed) return setPoints([]);
    let stale = false;
    getManifest(slug).then(
      (m) => !stale && setPoints(approvalsOf(m.items)),
      () => !stale && setPoints([]),
    );
    return () => {
      stale = true;
    };
  }, [where, slug, deployed]);
  const ends = untilOf(until, Math.floor(Date.now() / 1000));
  const scope: Scope | null =
    where === "host"
      ? "host"
      : where === "flow"
        ? slug
          ? { flows: { slug } }
          : null
        : where === "tag"
          ? tag.trim()
            ? { flows: { tag: tag.trim() } }
            : null
          : slug && point.trim()
            ? { approval: { flow: { slug }, name: point.trim() } }
            : null;
  const ready = subject !== null && role !== "" && scope !== null && !(ends && "error" in ends);
  const submit = async () => {
    if (!subject || !scope || !ready) return;
    setSaid(null);
    try {
      const r = await bind({ who: subject, role, scope, ...(ends && "at" in ends ? { expires_at: ends.at } : {}), ...(why.trim() ? { why: why.trim() } : {}) });
      const at = scope !== "host" && "approval" in scope ? scope.approval : null;
      const blind = at && !seesFlow(subject, at.flow.slug, a.principals, a.bindings, a.roles, a.flows) ? { who: subject, flow: at.flow.slug, point: at.name } : undefined;
      const until = r.binding.expires_at ? `, until ${utc(r.binding.expires_at)}` : "";
      setSaid({ done: `Bound ${role} to ${subjectSaid(subject)} ${scopeSaid(scope)}${until}: binding ${r.binding.id}, version ${r.version}.`, blind });
      setWhy("");
      setUntil("");
    } catch (e) {
      setSaid({ error: messageOf(e) });
    }
    a.refresh();
  };
  const people = a.principals.filter((p) => p.kind === "person");
  const others = a.principals.filter((p) => p.kind !== "person");
  const tags = [...new Set(a.flows.flatMap((f) => f.tags))].sort();
  return (
    <form
      aria-label={fixed ? `bind a role to ${subjectSaid(fixed)}` : "bind a role"}
      style={{ ...row, background: "transparent", flexDirection: "column", alignItems: "stretch" }}
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <div style={{ display: "flex", gap: 6, flexWrap: "wrap", alignItems: "center" }}>
        {!fixed && (
          <select aria-label="bind: to whom" value={who} onChange={(e) => setWho(e.target.value)} style={input}>
            <option value="" disabled>
              to whom
            </option>
            <optgroup label="people">
              {people.map((p) => (
                <option key={p.name} value={keyOf({ principal: p.name })}>
                  {p.name}
                </option>
              ))}
            </optgroup>
            <optgroup label="agents and services">
              {others.map((p) => (
                <option key={p.name} value={keyOf({ principal: p.name })}>
                  {p.name} ({p.kind})
                </option>
              ))}
            </optgroup>
            <optgroup label="groups">
              {a.groups.map((g) => (
                <option key={g.name} value={keyOf({ group: g.name })}>
                  the group {g.name}
                </option>
              ))}
            </optgroup>
          </select>
        )}
        <select aria-label="bind: role" value={role} title={roleTitle(a.roles, role)} onChange={(e) => setRole(e.target.value)} style={input}>
          <option value="" disabled>
            role
          </option>
          {offered.map((r) => (
            <option key={r.name} value={r.name} title={roleTitle(a.roles, r.name)}>
              {r.name}
            </option>
          ))}
        </select>
        <select aria-label="bind: where" value={where} onChange={(e) => setWhere(e.target.value as Where)} style={input}>
          <option value="host">on this host</option>
          <option value="flow">on a flow</option>
          <option value="tag">on the flows with a tag</option>
          <option value="approval">at one approval of a flow</option>
        </select>
        {(where === "flow" || where === "approval") && (
          <input aria-label="bind: flow" list={`${id}flows`} placeholder="flow" value={flow} onChange={(e) => setFlow(e.target.value)} style={{ ...input, width: 160 }} />
        )}
        {where === "tag" && <input aria-label="bind: tag" list={`${id}tags`} placeholder="tag" value={tag} onChange={(e) => setTag(e.target.value)} style={{ ...input, width: 140 }} />}
        {where === "approval" && (
          <input
            aria-label="bind: approval"
            list={`${id}points`}
            placeholder={deployed && points.length === 0 ? "its version has none" : "approval"}
            value={point}
            onChange={(e) => setPoint(e.target.value)}
            style={{ ...input, width: 140 }}
          />
        )}
        <input aria-label="bind: until" placeholder="until (7d, 2026-12-01)" title="empty: until it is taken away" value={until} onChange={(e) => setUntil(e.target.value)} style={{ ...input, width: 170 }} />
        <input aria-label="bind: why" placeholder="why (History keeps it)" value={why} onChange={(e) => setWhy(e.target.value)} style={{ ...input, flex: 1, minWidth: 140 }} />
        <button type="submit" style={primary} {...gate(can, "bind", !ready)}>
          bind
        </button>
      </div>
      <datalist id={`${id}flows`}>
        {a.flows.map((f) => (
          <option key={f.slug} value={f.slug} />
        ))}
      </datalist>
      <datalist id={`${id}tags`}>
        {tags.map((t) => (
          <option key={t} value={t} />
        ))}
      </datalist>
      <datalist id={`${id}points`}>
        {points.map((p) => (
          <option key={p} value={p} />
        ))}
      </datalist>
      {ends && "error" in ends && <div style={{ color: "var(--bad)", fontSize: 12 }}>{ends.error}</div>}
      {said?.error && (
        <div role="alert" style={{ color: "var(--bad)", fontSize: 12 }}>
          {said.error}
        </div>
      )}
      {said?.done && (
        <div role="status" style={quiet}>
          {said.done}
        </div>
      )}
      {said?.blind && (
        <div style={{ ...quiet, color: "var(--warn)", display: "flex", gap: 8, alignItems: "center" }}>
          <span>
            {subjectSaid(said.blind.who)} decides {said.blind.flow}&apos;s approval &quot;{said.blind.point}&quot; in Runs waiting, but the editor lists the flow
            itself only to those who may view it.
          </span>
          <button
            type="button"
            style={small}
            onClick={() => {
              const at = said.blind?.flow ?? "";
              setRole("viewer");
              setWhere("flow");
              setFlow(at);
              setSaid(null);
            }}
          >
            bind viewer on {said.blind.flow} too
          </button>
        </div>
      )}
    </form>
  );
}

// ── a principal's page ────────────────────────────────────────────────────────────────────────

/** A principal's page: what it is, its tokens (an agent's or a service's), the groups it is in,
 *  its bindings with the form to add one, and what they let it do. */
function PrincipalView({ name, me, a }: { name: string; me: string; a: Access }) {
  const [made, setMade] = useState<MadeToken | null>(null);
  const p = a.principals.find((x) => x.name === name);
  if (!p) return <div style={quiet}>{a.loaded ? `No one here is named ${name}.` : "reading…"}</div>;
  const groups = a.groups.filter((g) => g.members.includes(p.name)).map((g) => g.name);
  const owns = a.principals.filter((o) => o.owner === p.name).map((o) => o.name);
  const facts = [
    p.kind,
    p.owner && `owned by ${p.owner}`,
    p.email,
    owns.length > 0 && `owns ${owns.join(", ")}`,
    p.invited && "invited, link not used yet",
    p.disabled && "disabled",
    p.owner_disabled && "its owner is disabled",
    `added ${utc(p.created_at)}${p.created_by ? ` by ${p.created_by}` : ""}`,
  ].filter(Boolean);
  return (
    <section aria-label={`${p.name}'s page`}>
      <div className="hl-host-page__head">
        {p.name}
        {p.name === me && <span style={{ color: "var(--faint)" }}> (you)</span>}
      </div>
      <div style={about}>
        {facts.join(" · ")}
        {groups.length > 0 && (
          <>
            {" · in "}
            {groups.map((g, i) => (
              <Fragment key={g}>
                {i > 0 && ", "}
                <a href={format({ kind: "group", name: g })} style={link}>
                  {g}
                </a>
              </Fragment>
            ))}
          </>
        )}
      </div>
      {p.kind !== "person" && (
        <>
          <div className="hl-host-page__head">tokens</div>
          {made && <MadeTokenBox made={made} onDone={() => setMade(null)} />}
          <div style={{ ...row, background: "transparent" }}>
            <button style={small} aria-label={`make a token for ${p.name}`} title="a new token for it, shown once" onClick={() => makeToken(p, a.dialog, a.act, setMade)}>
              make token
            </button>
          </div>
          <TokenRows p={p} dialog={a.dialog} act={a.act} />
        </>
      )}
      <div className="hl-host-page__head">bindings</div>
      <BindingTable rows={rowsOf(p)} a={a} label={`what is bound to ${p.name}`} own />
      <BindForm a={a} who={{ principal: p.name }} />
      <div className="hl-host-page__head">what {p.name} may do</div>
      <MayDo gives={givesOf(p.kind, p.bindings, a.roles, a.flows)} roles={a.roles} />
    </section>
  );
}

/** What a principal may do (access.ts `givesOf`), a line each: on this host, on every flow, on the
 *  flows it saved, and on each tag's flows and each flow what it holds there beyond every flow's,
 *  with the approvals it decides there. */
function MayDo({ gives, roles }: { gives: Gives; roles: RoleEntry[] }) {
  const lines: [ReactNode, string][] = [];
  if (gives.host.length > 0) lines.push(["on this host", allowsSaid(gives.host, roles)]);
  if (gives.everyFlow.length > 0) lines.push(["on every flow", allowsSaid(gives.everyFlow, roles)]);
  if (gives.own.length > 0) lines.push(["on the flows it saved", allowsSaid(gives.own, roles)]);
  for (const t of gives.tags) {
    const more = t.actions.filter((x) => !gives.everyFlow.includes(x));
    if (more.length > 0) lines.push([`on the flows tagged ${t.tag}, those deployed later too`, allowsSaid(more, roles)]);
  }
  for (const f of gives.flows) {
    const more = f.actions.filter((x) => !gives.everyFlow.includes(x));
    const decides = f.actions.includes("run.decide") ? [] : f.decides;
    const said = [more.length > 0 ? allowsSaid(more, roles) : "", decides.length > 0 ? `decides ${decides.map((d) => JSON.stringify(d)).join(", ")}` : ""].filter(Boolean).join(" · ");
    if (said)
      lines.push([
        <a key={f.slug} href={format({ kind: "flow", slug: f.slug, tab: "status" })} style={link}>
          on {f.slug}
        </a>,
        said,
      ]);
  }
  if (lines.length === 0) return <div style={{ ...quiet, marginBottom: 8 }}>nothing: no binding holds for it</div>;
  return (
    <dl aria-label="what it may do" style={{ margin: "0 0 12px", fontSize: "var(--fs-sm)" }}>
      {lines.map(([where, what], i) => (
        <div key={i} style={{ display: "flex", gap: 12, padding: "3px 0" }}>
          <dt style={{ minWidth: 220, color: "var(--muted)" }}>{where}</dt>
          <dd style={{ margin: 0 }}>{what}</dd>
        </div>
      ))}
    </dl>
  );
}

// ── groups ────────────────────────────────────────────────────────────────────────────────────

function GroupsList({ a }: { a: Access }) {
  const [draft, setDraft] = useState({ name: "", about: "" });
  return (
    <>
      <div style={about}>
        A group is a named set of principals, bound as one: each member holds what is bound to the group, where its role allows the member&apos;s kind. Groups
        don&apos;t nest.
      </div>
      {a.groups.length === 0 && <div style={{ ...quiet, marginBottom: 6 }}>no group yet</div>}
      {a.groups.map((g) => (
        <div key={g.name} style={row}>
          <a href={format({ kind: "group", name: g.name })} style={{ ...link, minWidth: 120 }}>
            {g.name}
          </a>
          <span style={{ color: "var(--faint)", fontSize: 11, flex: 1 }}>
            {[g.about, g.members.length > 0 ? g.members.join(", ") : "no one", `${g.bound} ${g.bound === 1 ? "binding" : "bindings"}`].filter(Boolean).join(" · ")}
          </span>
          <button
            style={small}
            aria-label={`remove the group ${g.name}`}
            onClick={async () => {
              const why = await a.dialog.prompt(`Remove the group ${g.name}?\n\nWhy (History keeps it):`, "", "Remove", "optional");
              if (why !== null) void a.act(() => removeGroup(g.name, why));
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
          const name = draft.name.trim();
          void a.act(() => putGroup(name, { about: draft.about.trim() || null, members: [] })).then((made) => {
            if (made) location.hash = format({ kind: "group", name });
          });
        }}
      >
        <input aria-label="new group's name" placeholder="name (letters, digits, - and _)" value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} style={input} />
        <input aria-label="new group's about" placeholder="what it is for (optional)" value={draft.about} onChange={(e) => setDraft({ ...draft, about: e.target.value })} style={{ ...input, flex: 1 }} />
        <button type="submit" style={primary} disabled={!draft.name.trim()}>
          make group
        </button>
      </form>
    </>
  );
}

/** A group's page: its members, each taken out from its row and added from the list of everyone
 *  else (`PUT /groups/<name>` takes them all), and what is bound to it, with the form to add to it. */
function GroupView({ name, a }: { name: string; a: Access }) {
  const [adding, setAdding] = useState("");
  const g = a.groups.find((x) => x.name === name);
  if (!g) return <div style={quiet}>{a.loaded ? `No group here is named ${name}.` : "reading…"}</div>;
  const set = (members: string[]) => a.act(() => putGroup(name, { about: g.about, members }));
  const kindOf = (m: string) => a.principals.find((p) => p.name === m)?.kind ?? "";
  return (
    <section aria-label={`the group ${name}`}>
      <div className="hl-host-page__head">{name}</div>
      {g.about && <div style={about}>{g.about}</div>}
      <div className="hl-host-page__head">members</div>
      {g.members.length === 0 && <div style={{ ...quiet, marginBottom: 6 }}>no one yet</div>}
      {g.members.map((m) => (
        <div key={m} style={row}>
          <Who name={m} style={{ flex: 1 }} />
          <span style={quiet}>{kindOf(m)}</span>
          <button
            style={small}
            aria-label={`take ${m} out of ${name}`}
            onClick={async () => {
              if (await a.dialog.confirm(`Take ${m} out of ${name}? They lose what is bound to it.`, "Take out")) void set(g.members.filter((x) => x !== m));
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
          if (adding) void set([...g.members, adding]).then(() => setAdding(""));
        }}
      >
        <select aria-label={`add to ${name}`} value={adding} onChange={(e) => setAdding(e.target.value)} style={{ ...input, flex: 1 }}>
          <option value="">someone to add</option>
          {a.principals
            .filter((p) => !g.members.includes(p.name))
            .map((p) => (
              <option key={p.name} value={p.name}>
                {p.name} ({p.kind})
              </option>
            ))}
        </select>
        <button type="submit" style={primary} disabled={!adding}>
          add
        </button>
      </form>
      <div className="hl-host-page__head">bindings</div>
      <BindingTable rows={a.bindings.filter((b) => "group" in b.who && b.who.group === name)} a={a} label={`what is bound to ${name}`} />
      <BindForm a={a} who={{ group: name }} />
    </section>
  );
}

// ── roles ─────────────────────────────────────────────────────────────────────────────────────

function RolesList({ a }: { a: Access }) {
  const [editing, setEditing] = useState<RoleEntry | null>(null);
  return (
    <>
      <div style={about}>
        A role is a set of the catalogue&apos;s actions and the kinds of principal who may hold it. The built-in ones are fixed; an admin defines others, each
        allowing only what its author holds on this host. A binding gives what its role allows where it is made.
      </div>
      <table className="hl-table" aria-label="roles" style={{ marginBottom: 10 }}>
        <thead>
          <tr>
            <th>role</th>
            <th>held by</th>
            <th>allows</th>
            <th>bound to</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {a.roles.map((r) => (
            <tr key={r.name}>
              <td title={r.about ?? ""}>
                {r.name}
                {r.builtin && <span className="hl-table__dim"> · built in</span>}
              </td>
              <td>{r.kinds.join(", ")}</td>
              <td>{allowsSaid(r.allows, a.roles)}</td>
              <td>{heldBy(a.bindings, r.name) || <span className="hl-table__dim">no one</span>}</td>
              <td style={{ whiteSpace: "nowrap" }}>
                {!r.builtin && (
                  <>
                    <button style={small} aria-label={`change the role ${r.name}`} onClick={() => setEditing(r)}>
                      change
                    </button>{" "}
                    <button
                      style={small}
                      aria-label={`remove the role ${r.name}`}
                      onClick={async () => {
                        const why = await a.dialog.prompt(`Remove the role ${r.name}? A role still bound can't be.\n\nWhy (History keeps it):`, "", "Remove", "optional");
                        if (why !== null) void a.act(() => removeRole(r.name, why));
                      }}
                    >
                      ×
                    </button>
                  </>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <RoleForm key={editing?.name ?? ""} a={a} role={editing} onDone={() => setEditing(null)} />
    </>
  );
}

const KINDS: Kind[] = ["person", "agent", "service"];

/** A role defined or changed (`PUT /roles/<name>`): its name, what it is for, who may hold it, and
 *  what it allows, a checkbox for each of the catalogue's actions by what it acts on. */
function RoleForm({ a, role, onDone }: { a: Access; role: RoleEntry | null; onDone: () => void }) {
  const [name, setName] = useState(role?.name ?? "");
  const [what, setWhat] = useState(role?.about ?? "");
  const [kinds, setKinds] = useState<Kind[]>(role?.kinds ?? ["person"]);
  const [allows, setAllows] = useState<string[]>(role?.allows ?? []);
  const [why, setWhy] = useState("");
  const [error, setError] = useState("");
  const toggle = <T,>(list: T[], x: T) => (list.includes(x) ? list.filter((y) => y !== x) : [...list, x]);
  const submit = async () => {
    setError("");
    try {
      await defineRole(name.trim(), { about: what.trim() || null, allows, kinds, ...(why.trim() ? { why: why.trim() } : {}) });
      setWhy("");
      if (role) onDone();
      else {
        setName("");
        setWhat("");
        setAllows([]);
      }
    } catch (e) {
      setError(messageOf(e));
    }
    a.refresh();
  };
  return (
    <form
      aria-label={role ? `change the role ${role.name}` : "define a role"}
      style={{ ...row, background: "transparent", flexDirection: "column", alignItems: "stretch" }}
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
        <input aria-label="role's name" placeholder="name (letters, digits, - and _)" value={name} disabled={role !== null} onChange={(e) => setName(e.target.value)} style={input} />
        <input aria-label="role's about" placeholder="what it is for (optional)" value={what} onChange={(e) => setWhat(e.target.value)} style={{ ...input, flex: 1, minWidth: 160 }} />
        <span style={quiet}>held by</span>
        {KINDS.map((k) => (
          <label key={k} style={quiet}>
            <input type="checkbox" aria-label={`held by ${k}`} checked={kinds.includes(k)} onChange={() => setKinds(toggle(kinds, k))} /> {k}
          </label>
        ))}
      </div>
      <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(190px, 1fr))", gap: 8 }}>
        {catalogueOf(a.roles).map((r) => (
          <fieldset key={r.of} style={{ border: "1px solid var(--border)", borderRadius: "var(--r-sm)", padding: "4px 10px", margin: 0 }}>
            <legend style={quiet}>{r.of}</legend>
            {r.actions.map((x) => (
              <label key={x} style={{ display: "block", fontSize: "var(--fs-xs)" }}>
                <input type="checkbox" aria-label={x} checked={allows.includes(x)} onChange={() => setAllows(toggle(allows, x))} /> {x.slice(r.of.length + 1)}
              </label>
            ))}
          </fieldset>
        ))}
      </div>
      <div style={{ display: "flex", gap: 6 }}>
        <input aria-label="role's why" placeholder="why (History keeps it)" value={why} onChange={(e) => setWhy(e.target.value)} style={{ ...input, flex: 1 }} />
        <button type="submit" style={primary} disabled={!name.trim() || allows.length === 0 || kinds.length === 0}>
          {role ? "change role" : "define role"}
        </button>
        {role && (
          <button type="button" style={small} onClick={onDone}>
            cancel
          </button>
        )}
      </div>
      {error && (
        <div role="alert" style={{ color: "var(--bad)", fontSize: 12 }}>
          {error}
        </div>
      )}
    </form>
  );
}

// ── history ───────────────────────────────────────────────────────────────────────────────────

function HistoryList({ a }: { a: Access }) {
  return (
    <>
      <div style={about}>
        Every change to who may do what is a version of the access document, kept: who made it, when, what it changed and why. Open one to see what it changed
        from the one before; <code>holon access show --version N</code> prints it whole.
      </div>
      {a.versions.length === 0 ? (
        <div style={quiet}>no version yet</div>
      ) : (
        <table className="hl-table" aria-label="access history">
          <thead>
            <tr>
              <th>version</th>
              <th>when</th>
              <th>by</th>
              <th>what</th>
              <th>why</th>
            </tr>
          </thead>
          <tbody>
            {a.versions.map((v) => (
              <tr key={v.version}>
                <td>
                  <a href={format({ kind: "access-version", version: v.version })}>version {v.version}</a>
                </td>
                <td>{utc(v.at)}</td>
                <td>{v.by || "—"}</td>
                <td>{v.what}</td>
                <td>{v.why ?? ""}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </>
  );
}

/** A version of the access document beside the one before it: what it changed (access.ts
 *  `changesOf`), with links to the versions on either side. */
function VersionView({ n, latest }: { n: number; latest: number | null }) {
  const [read, setRead] = useState<{ after: AccessDocument; before: AccessDocument | null } | string | null>(null);
  useEffect(() => {
    let stale = false;
    setRead(null);
    Promise.all([getAccessVersion(n), n > 1 ? getAccessVersion(n - 1).catch(() => null) : Promise.resolve(null)])
      .then(([after, before]) => !stale && setRead({ after, before }))
      .catch((e: unknown) => !stale && setRead(messageOf(e)));
    return () => {
      stale = true;
    };
  }, [n]);
  if (read === null) return <div style={quiet}>reading version {n}…</div>;
  if (typeof read === "string")
    return (
      <div role="alert" style={{ color: "var(--bad)", fontSize: 12 }}>
        {read}
      </div>
    );
  const { after, before } = read;
  const changes = changesOf(before, after);
  return (
    <section aria-label={`version ${n}`}>
      <div className="hl-host-page__head">version {n}</div>
      <div style={about}>
        {after.what}
        {after.by ? ` · by ${after.by}` : ""} · {utc(after.at)}
        {after.why ? ` · why: ${after.why}` : ""}
      </div>
      <div style={{ display: "flex", gap: 12, marginBottom: 8, fontSize: "var(--fs-xs)" }}>
        {n > 1 && (
          <a href={format({ kind: "access-version", version: n - 1 })} style={link}>
            ‹ version {n - 1}
          </a>
        )}
        {latest !== null && n < latest && (
          <a href={format({ kind: "access-version", version: n + 1 })} style={link}>
            version {n + 1} ›
          </a>
        )}
      </div>
      {before === null && n > 1 && <div style={{ ...quiet, marginBottom: 6 }}>version {n - 1} isn&apos;t kept: everything this one holds is listed as added</div>}
      {changes.length === 0 ? (
        <div style={quiet}>it changed no binding, role, group or setting</div>
      ) : (
        <table className="hl-table" aria-label="what it changed">
          <thead>
            <tr>
              <th>what</th>
              <th>change</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {changes.map((c, i) => (
              <tr key={i}>
                <td>{c.of}</td>
                <td>{c.change}</td>
                <td>{c.said}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}
