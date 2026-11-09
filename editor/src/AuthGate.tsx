// Before the editor: who is using it (holon-server's accounts.rs, access.rs). A new server first
// asks for its admin, with the setup code it printed when it started; an invite link lets its
// holder set their password; otherwise one signs in, with a name and password or, when the
// server has one, through its identity provider (signin.ts). Signed in, the editor shows, with
// an account menu in its header: change password and sign out.

import { useCallback, useEffect, useState, type CSSProperties, type FormEvent, type ReactNode } from "react";
import { acceptInvite, changePassword, getAuthState, getSession, inviteFor, setUp, signInWith, signOutHere, whoAmI, type Role } from "./server";
import { finishSignIn, signedIn, startSignIn, type AuthConfig } from "./signin";
import { useDialog } from "./Dialog";

/** Who is signed in, for the editor. */
export type Me = { name: string; role: Role };

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
        if (me) return setStage({ kind: "in", me: { name: me.name, role: me.role } });
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
  const attempt = async (f: () => Promise<{ name: string; role: Role }>) => {
    try {
      const s = await f();
      setPassword("");
      setAgain("");
      setStage({ kind: "in", me: { name: s.name, role: s.role } });
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

/** The top bar's avatar, at its right: who is signed in; its menu opens the welcome and docs,
 *  changes the password, signs out. The look is the top bar's own. */
export function AccountMenu({ me, onWelcome }: { me: Me; onWelcome: () => void }) {
  const dialog = useDialog();
  const [open, setOpen] = useState(false);
  const change = async () => {
    setOpen(false);
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
  return (
    <span className="hl-user__box">
      <button className="hl-user" onClick={() => setOpen(!open)} aria-haspopup="menu" aria-expanded={open} aria-label={`account: ${me.name}`} title={`signed in as ${me.name}, ${me.role}`}>
        {initials(me.name)}
      </button>
      {open && (
        <div role="menu" className="hl-user__menu">
          <span className="hl-user__who">
            <b>{me.name}</b> · {me.role}
          </span>
          <button
            role="menuitem"
            onClick={() => {
              setOpen(false);
              onWelcome();
            }}
          >
            Welcome and docs
          </button>
          <button role="menuitem" onClick={change}>
            Change password
          </button>
          <button role="menuitem" onClick={() => signOutHere()}>
            Sign out
          </button>
        </div>
      )}
    </span>
  );
}
