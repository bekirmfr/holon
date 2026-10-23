// Before the editor: who is using it (holon-server's accounts.rs, access.rs). A new server first
// asks for its admin, with the setup code it printed when it started; an invite link lets its
// holder set their password; otherwise one signs in, with a name and password or, when the
// server has one, through its identity provider (signin.ts). Signed in, the editor shows, with
// an account menu in its header: change password, the admin panel (admins), sign out.

import { useCallback, useEffect, useState, type CSSProperties, type FormEvent, type ReactNode } from "react";
import { acceptInvite, changePassword, getAuthState, getSession, inviteFor, setUp, signInWith, signOutHere, whoAmI, type Role } from "./server";
import { finishSignIn, signedIn, startSignIn, type AuthConfig } from "./signin";
import { useDialog } from "./Dialog";

/** Who is signed in, for the editor. */
export type Me = { name: string; role: Role };

const card: CSSProperties = {
  background: "#0b1527",
  border: "1px solid #1b2a45",
  borderRadius: 6,
  padding: 20,
  display: "grid",
  gap: 10,
  color: "#9fb4d4",
  fontSize: 12,
  width: 340,
  maxWidth: "calc(100vw - 32px)",
};
const field: CSSProperties = {
  font: "inherit",
  fontSize: 13,
  padding: "6px 8px",
  color: "#e6eefb",
  background: "#0e1a2f",
  border: "1px solid #1b2a45",
  borderRadius: 3,
};
const primary: CSSProperties = { font: "inherit", fontSize: 12, padding: "6px 12px", color: "#0e1a2f", background: "#6fd3e0", border: "none", borderRadius: 3, cursor: "pointer" };
const quiet: CSSProperties = { font: "inherit", fontSize: 12, padding: "6px 12px", color: "#9fb4d4", background: "transparent", border: "1px solid #1b2a45", borderRadius: 3, cursor: "pointer" };

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
        <div style={{ color: "#e6eefb", fontSize: 15 }}>
          holon <span style={{ color: "#7e8ea8" }}>·</span> {title}
        </div>
        {lead && <div style={{ lineHeight: 1.45 }}>{lead}</div>}
        {children}
        {error && (
          <div role="alert" style={{ color: "#e0916f" }}>
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
      return <div style={{ padding: 24, color: "#9fb4d4" }}>connecting…</div>;
    case "unreachable":
      return (
        <div style={{ padding: 24, color: "#e0916f", display: "grid", gap: 10, maxWidth: 520 }}>
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
          lead="This server has no accounts yet. Create its admin: the setup code was printed where the server started (holon-server: …set up the first admin with this code)."
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
              You were invited as <span style={{ color: "#e6eefb" }}>{stage.name}</span>. Set your password to sign in.
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

/** The header's account menu: who, the welcome and docs, change password, the admin panel
 *  (admins), sign out. */
export function AccountMenu({ me, style, onAdmin, adminOpen, onWelcome }: { me: Me; style: CSSProperties; onAdmin: () => void; adminOpen: boolean; onWelcome: () => void }) {
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
    <span style={{ position: "relative", display: "inline-flex", gap: 6 }}>
      {me.role === "admin" && (
        <button style={{ ...style, ...(adminOpen ? { color: "#0e1a2f", background: "#6fd3e0" } : {}) }} onClick={onAdmin}>
          admin
        </button>
      )}
      <button style={style} onClick={() => setOpen(!open)} aria-haspopup="menu" title={`signed in as ${me.name}, ${me.role}`}>
        {me.name} ▾
      </button>
      {open && (
        <div role="menu" style={{ position: "absolute", right: 0, top: "calc(100% + 4px)", zIndex: 50, background: "#0b1527", border: "1px solid #1b2a45", borderRadius: 4, padding: 6, display: "grid", gap: 4, minWidth: 180 }}>
          <span style={{ color: "#7e8ea8", fontSize: 11, padding: "2px 6px" }}>
            {me.name} · {me.role}
          </span>
          <button
            role="menuitem"
            style={{ ...style, textAlign: "left" }}
            onClick={() => {
              setOpen(false);
              onWelcome();
            }}
          >
            welcome and docs
          </button>
          <button role="menuitem" style={{ ...style, textAlign: "left" }} onClick={change}>
            change password
          </button>
          <button role="menuitem" style={{ ...style, textAlign: "left" }} onClick={() => signOutHere()}>
            sign out
          </button>
        </div>
      )}
    </span>
  );
}
