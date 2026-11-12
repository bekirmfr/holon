// Before the editor: who is using it (holon-server's accounts.rs, access.rs). A new server first
// asks for its admin, with the setup code it printed when it started; an invite link lets its
// holder set their password; otherwise one signs in, with a name and password or, when the
// server has one, through its identity provider (signin.ts). Signed in, the editor shows, with
// the account in its header: who is signed in and what they may decide, change password,
// Getting started and sign out.

import { useCallback, useEffect, useRef, useState, type CSSProperties, type FormEvent, type ReactNode } from "react";
import { acceptInvite, changePassword, getAuthState, getSession, inviteFor, mayI, setUp, signInWith, signOutHere, whoAmI, type Kind, type Role } from "./server";
import { claimsOf, finishSignIn, signedIn, startSignIn, type AuthConfig } from "./signin";
import { useDialog } from "./Dialog";
import { lettersOf } from "./keymap";
import { Kbd } from "./ui/Kbd";
import { keyOf } from "./ui/Keys";

/** Who is signed in, for the editor: a person, or (signed in with its token) an agent or a service. */
export type Me = { name: string; kind: Kind; role: Role };

const card: CSSProperties = {
  background: "var(--bg)",
  border: "1px solid var(--border)",
  borderRadius: 6,
  padding: 20,
  display: "grid",
  gap: 10,
  color: "var(--muted)",
  fontSize: 12,
  width: 340,
  maxWidth: "calc(100vw - 32px)",
};
const field: CSSProperties = {
  font: "inherit",
  fontSize: 13,
  padding: "6px 8px",
  color: "var(--ink)",
  background: "var(--surface-2)",
  border: "1px solid var(--border)",
  borderRadius: 3,
};
const primary: CSSProperties = { font: "inherit", fontSize: 12, padding: "6px 12px", color: "var(--accent-ink)", background: "var(--accent)", border: "none", borderRadius: 3, cursor: "pointer" };
const quiet: CSSProperties = { font: "inherit", fontSize: 12, padding: "6px 12px", color: "var(--muted)", background: "transparent", border: "1px solid var(--border)", borderRadius: 3, cursor: "pointer" };

function Screen({ title, lead, children, onSubmit, error }: { title: string; lead?: ReactNode; children: ReactNode; onSubmit: () => Promise<void>; error: string }) {
  const [busy, setBusy] = useState(false);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    await onSubmit().finally(() => setBusy(false));
  };
  return (
    <div style={{ minHeight: "100vh", display: "flex", alignItems: "center", justifyContent: "center", background: "var(--bg)" }}>
      <form style={card} onSubmit={submit} aria-label={title}>
        <div style={{ color: "var(--ink)", fontSize: 15 }}>
          holon <span style={{ color: "var(--faint)" }}>·</span> {title}
        </div>
        {lead && <div style={{ lineHeight: 1.45 }}>{lead}</div>}
        {children}
        {error && (
          <div role="alert" style={{ color: "var(--bad)" }}>
            {error}
          </div>
        )}
        <button type="submit" style={{ ...primary, opacity: busy ? 0.6 : 1 }} disabled={busy}>
          {title}
        </button>
      </form>
    </div>
  );
}

const Field = ({ label, value, onChange, type = "text", autoComplete }: { label: string; value: string; onChange: (v: string) => void; type?: string; autoComplete?: string }) => (
  <label style={{ display: "grid", gap: 4 }}>
    {label}
    <input style={field} type={type} value={value} autoComplete={autoComplete} onChange={(e) => onChange(e.target.value)} />
  </label>
);

const messageOf = (e: unknown) => (e instanceof Error ? e.message : String(e));

type Stage = { kind: "loading" } | { kind: "unreachable"; message: string } | { kind: "setup" } | { kind: "invite"; token: string; name: string } | { kind: "sign-in"; oidc: AuthConfig | null } | { kind: "in"; me: Me };

/** The editor, once someone is signed in; the right screen before. */
export function AuthGate({ children }: { children: (me: Me) => ReactNode }) {
  const [stage, setStage] = useState<Stage>({ kind: "loading" });
  const [error, setError] = useState("");
  const [name, setName] = useState("");
  const [password, setPassword] = useState("");
  const [again, setAgain] = useState("");
  const [code, setCode] = useState("");

  const decide = useCallback(async () => {
    setError("");
    try {
      const state = await getAuthState();
      if (state.setup) return setStage({ kind: "setup" });
      const invite = new URLSearchParams(location.search).get("invite");
      if (invite) {
        const forWhom = await inviteFor(invite).catch((e: unknown) => {
          setError(messageOf(e));
          return null;
        });
        history.replaceState(null, "", location.pathname);
        if (forWhom) return setStage({ kind: "invite", token: invite, name: forWhom.name });
      }
      // Back from the identity provider, or a session kept from before.
      const back = await finishSignIn();
      if (typeof back === "string") setError(back);
      if (getSession() || signedIn()) {
        const me = await whoAmI().catch((e: unknown) => {
          setError(messageOf(e));
          return null;
        });
        if (me) return setStage({ kind: "in", me: { name: me.name, kind: me.kind, role: me.role } });
      }
      setStage({ kind: "sign-in", oidc: state.oidc });
    } catch (e) {
      setStage({ kind: "unreachable", message: messageOf(e) });
    }
  }, []);
  useEffect(() => {
    decide();
    // Signed out (here, or the server said the sign-in ended): back to signing in.
    const onSession = () => {
      if (!getSession() && !signedIn()) decide();
    };
    window.addEventListener("holon:session", onSession);
    return () => window.removeEventListener("holon:session", onSession);
  }, [decide]);

  const matched = () => {
    if (password !== again) throw new Error("the two passwords differ");
  };
  const attempt = async (f: () => Promise<{ name: string; kind: Kind; role: Role }>) => {
    try {
      const s = await f();
      setPassword("");
      setAgain("");
      setStage({ kind: "in", me: { name: s.name, kind: s.kind, role: s.role } });
    } catch (e) {
      setError(messageOf(e));
    }
  };

  switch (stage.kind) {
    case "loading":
      return <div style={{ padding: 24, color: "var(--muted)" }}>connecting…</div>;
    case "unreachable":
      return (
        <div style={{ padding: 24, color: "var(--bad)", display: "grid", gap: 10, maxWidth: 520 }}>
          <div>The server can't be reached: {stage.message}</div>
          <button style={quiet} onClick={decide}>
            try again
          </button>
        </div>
      );
    case "setup":
      return (
        <Screen
          title="set up"
          lead="This server has no accounts yet. Create its admin: the setup code was printed where the server started (holon: …set up the first admin with this code)."
          error={error}
          onSubmit={() =>
            attempt(async () => {
              matched();
              return setUp(code, name, password);
            })
          }
        >
          <Field label="setup code" value={code} onChange={setCode} autoComplete="off" />
          <Field label="your name" value={name} onChange={setName} autoComplete="username" />
          <Field label="password (at least 10 characters)" value={password} onChange={setPassword} type="password" autoComplete="new-password" />
          <Field label="the password again" value={again} onChange={setAgain} type="password" autoComplete="new-password" />
        </Screen>
      );
    case "invite":
      return (
        <Screen
          title="join"
          lead={
            <>
              You were invited as <span style={{ color: "var(--ink)" }}>{stage.name}</span>. Set your password to sign in.
            </>
          }
          error={error}
          onSubmit={() =>
            attempt(async () => {
              matched();
              return acceptInvite(stage.token, password);
            })
          }
        >
          <Field label="password (at least 10 characters)" value={password} onChange={setPassword} type="password" autoComplete="new-password" />
          <Field label="the password again" value={again} onChange={setAgain} type="password" autoComplete="new-password" />
        </Screen>
      );
    case "sign-in":
      return (
        <Screen title="sign in" error={error} onSubmit={() => attempt(() => signInWith(name, password))}>
          <Field label="name" value={name} onChange={setName} autoComplete="username" />
          <Field label="password" value={password} onChange={setPassword} type="password" autoComplete="current-password" />
          {stage.oidc && (
            <button type="button" style={quiet} onClick={() => startSignIn(stage.oidc!)}>
              sign in through {new URL(stage.oidc.issuer).host}
            </button>
          )}
        </Screen>
      );
    case "in":
      return <>{children(stage.me)}</>;
  }
}

/** A name's initials, for the avatar: the first letters of its first two words, or a one-word
 *  name's first two letters. */
export function initials(name: string): string {
  const words = name.split(/[\s._-]+/).filter(Boolean);
  const two = words.length > 1 ? words[0][0] + words[1][0] : (words[0] ?? "?").slice(0, 2);
  return two.toUpperCase();
}

/** The top bar's avatar, at its right: who is signed in. It opens the account dialog. */
export function AccountButton({ me, onStart }: { me: Me; onStart: () => void }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button className="hl-user" onClick={() => setOpen(true)} aria-haspopup="dialog" aria-expanded={open} aria-label={`account: ${me.name}`} title={`signed in as ${me.name}, ${me.role}`}>
        {initials(me.name)}
      </button>
      {open && <AccountDialog me={me} onStart={onStart} onClose={() => setOpen(false)} />}
    </>
  );
}

/** What the account dialog asks the host whether they may do, and how it says each. */
const DECIDES: [string, string][] = [
  ["flow.approve-deploy", "approve a version's deploy"],
  ["run.decide", "decide a run's approval"],
];

/** How this browser signed in, and since when, as far as it knows: a session's (with the time
 *  it was kept, when it was noted), or an identity provider's ID token (its issuer, issued at and
 *  good until). */
export function signInSaid(kind: Kind | undefined): string {
  const at = (ms: number) => new Date(ms).toLocaleString();
  const session = getSession();
  if (session) return `${kind && kind !== "person" ? "with its token" : "with a name and password"}${session.at ? `, ${at(session.at)}` : ""}`;
  const oidc = signedIn();
  if (oidc) {
    const claims = claimsOf(oidc.idToken);
    const iss = typeof claims.iss === "string" ? hostOf(claims.iss) : "an identity provider";
    const iat = typeof claims.iat === "number" ? `, ${at(claims.iat * 1000)}` : "";
    return `through ${iss}${iat}, until ${at(oidc.exp * 1000)}`;
  }
  return "not known here";
}

const hostOf = (url: string) => {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
};

/** The account, in a dialog: who is signed in (their name, kind and role, an agent's owner), what
 *  they may decide on the host, as `/auth/me/can` answers, how they signed in and since when; and
 *  their password changed, Getting started, or signing out. Each button has a letter. */
export function AccountDialog({ me, onStart, onClose }: { me: Me; onStart: () => void; onClose: () => void }) {
  const dialog = useDialog();
  const [who, setWho] = useState<{ display?: string | null; email?: string | null; owner?: string | null; kind?: Kind } | null>(null);
  const [may, setMay] = useState<Record<string, true | string> | string | null>(null);
  useEffect(() => {
    let on = true;
    whoAmI().then(
      (w) => on && setWho(w),
      () => undefined,
    );
    mayI(DECIDES.map(([a]) => a)).then(
      (m) => on && setMay(m),
      (e: unknown) => on && setMay(messageOf(e)),
    );
    return () => {
      on = false;
    };
  }, []);
  const kind = who?.kind ?? me.kind;
  const change = async () => {
    onClose();
    const current = await dialog.prompt("Your current password:");
    if (!current) return;
    const next = await dialog.prompt("A new password (at least 10 characters):");
    if (!next) return;
    try {
      await changePassword(current, next);
      await dialog.confirm("Your password is changed.", "OK");
    } catch (e) {
      await dialog.confirm(`Not changed: ${messageOf(e)}`, "OK");
    }
  };
  // s and d are Getting started and the docs elsewhere: not this dialog's letters.
  const buttons: [string, () => void][] = [
    ...(kind === undefined || kind === "person" ? ([["Change password", () => void change()]] as [string, () => void][]) : []),
    [
      "Getting started",
      () => {
        onClose();
        onStart();
      },
    ],
    ["Sign out", () => void signOutHere()],
  ];
  const letters = lettersOf(
    buttons.map(([label]) => label),
    ["s", "d"],
  );
  // Capturing, as the keys dialog does: its letters and Esc are its own while it is open.
  const keys = useRef({ buttons, letters, onClose });
  keys.current = { buttons, letters, onClose };
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const k = keyOf(e);
      const { buttons, letters, onClose } = keys.current;
      const i = letters.indexOf(k);
      const act = k === "Escape" ? onClose : i >= 0 && k !== "" ? buttons[i][1] : null;
      if (!act || document.querySelectorAll('[role="dialog"]').length > 1) return;
      e.preventDefault();
      e.stopPropagation();
      act();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);
  const name = who?.display || me.name;
  return (
    <div className="hl-modal" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="hl-modal__box hl-account" role="dialog" aria-modal="true" aria-label="account">
        <div className="hl-modal__head">
          <span>Account</span>
          <button className="hl-modal__close" aria-label="close" onClick={onClose}>
            ×
          </button>
        </div>
        <dl className="hl-account__facts">
          <div>
            <dt>name</dt>
            <dd>
              <b>{name}</b>
              {name !== me.name && <span className="hl-account__dim"> ({me.name})</span>}
              {who?.email && <span className="hl-account__dim"> · {who.email}</span>}
            </dd>
          </div>
          <div>
            <dt>is</dt>
            <dd>
              {kind ?? "person"} · {me.role}
              {kind === "agent" && who?.owner && <span className="hl-account__dim"> · owned by {who.owner}</span>}
            </dd>
          </div>
          <div>
            <dt>may</dt>
            <dd>
              {may === null ? (
                "…"
              ) : typeof may === "string" ? (
                <span className="hl-account__dim">the host didn't say: {may}</span>
              ) : (
                <ul className="hl-account__may" aria-label="may decide">
                  {DECIDES.map(([action, said]) => (
                    <li key={action} title={action}>
                      {may[action] === true ? (
                        <>✓ {said}</>
                      ) : (
                        <span className="hl-account__dim">
                          ✗ {said}
                          {typeof may[action] === "string" ? `: ${may[action]}` : ""}
                        </span>
                      )}
                    </li>
                  ))}
                </ul>
              )}
            </dd>
          </div>
          <div>
            <dt>signed in</dt>
            <dd>{signInSaid(kind)}</dd>
          </div>
        </dl>
        <div className="hl-account__do">
          {buttons.map(([label, act], i) => (
            <button key={label} className="hl-flow__button" onClick={act} aria-keyshortcuts={letters[i] ? letters[i].toUpperCase() : undefined}>
              {letters[i] && <Kbd k={letters[i]} />}
              {label}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
