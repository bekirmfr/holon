//! Every surface through which a person or an agent reaches what a server keeps (the audit
//! register's R60): each `holon` command and subcommand, each tool `holon mcp` serves, and the run
//! file. Each one has the routes it calls, what it reads or writes on this machine and what guards
//! that, and which of the server's kept values it can show. A command that reaches nothing has
//! neither routes nor a local reach: it prints text of its own.
//!
//! Plain data with no `crate::` in it, so tests/surfaces.rs includes this file as it is. The tests
//! that hold it:
//! - tests/surfaces.rs: the dispatch in main.rs, each command's usage and `holon mcp`'s tool list
//!   have no command, subcommand or tool this table lacks, and the table has none they lack. Each
//!   command and tool is run against a real server behind a recording proxy, and calls only the
//!   routes its row names. Each row that `shows` something is run as a holder, as a person without
//!   run.steps or state.view, and as agents. Each refuses whoever lacks the grant, naming it, or
//!   shows them no canary; to a holder, it shows the canary.
//! - src/tests/run_steps.rs: every route a row names is a route of can.rs's ROUTES, the table
//!   that sweep holds to its grants, and the tools are `holon mcp`'s.
//! - docs/manual/reference/surfaces.md is written from this table (HOLON_BLESS=1).

/// What of a server's keeping a surface reads for its caller, and so which grant it takes: what it
/// shows of it, or what follows from it (a replay says which runs answer otherwise).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shows {
    /// What runs were sent and what each step computed: run.steps on the flow (R55).
    RunValues,
    /// What a flow keeps in its state, a cell's changes, a session's snapshot: state.view (R56).
    State,
    /// What a flow wrote with console.log: run.view, and an agent only where a person let agents
    /// read its logs.
    LogLines,
}

impl Shows {
    pub(crate) fn grant(self) -> &'static str {
        match self {
            Shows::RunValues => "run.steps",
            Shows::State => "state.view",
            Shows::LogLines => "run.view (an agent: \"agents may read its logs\")",
        }
    }
}

/// What a surface reads or writes on this machine, and what guards it.
pub(crate) struct Local {
    pub(crate) reads: &'static str,
    pub(crate) guarded_by: &'static str,
}

pub(crate) struct Surface {
    /// As typed after `holon` (`runs`, `secret set`), or a tool's name.
    pub(crate) name: &'static str,
    /// The routes it calls, as can.rs's ROUTES write them: `GET /deployments/{slug}/runs`.
    pub(crate) routes: &'static [&'static str],
    pub(crate) local: Option<Local>,
    /// What it reads for its caller of what the server keeps.
    pub(crate) shows: &'static [Shows],
}

const OWN: &str = "nothing: it is the user's own file";
const SEALED: &str = "the OS's permissions on the data folder, and the root key it is sealed under";

const fn routes(name: &'static str, routes: &'static [&'static str], shows: &'static [Shows]) -> Surface {
    Surface { name, routes, local: None, shows }
}

const fn local(name: &'static str, reads: &'static str, guarded_by: &'static str) -> Surface {
    Surface { name, routes: &[], local: Some(Local { reads, guarded_by }), shows: &[] }
}

const fn both(name: &'static str, routes: &'static [&'static str], reads: &'static str, guarded_by: &'static str, shows: &'static [Shows]) -> Surface {
    Surface { name, routes, local: Some(Local { reads, guarded_by }), shows }
}

const fn text(name: &'static str) -> Surface {
    Surface { name, routes: &[], local: None, shows: &[] }
}

use Shows::{LogLines, RunValues, State};

/// The run file, apart from every command: `holon serve` writes it (`~/.holon/run/<port>.json`,
/// or HOLON_RUN_DIR), and a command on the same machine with no HOLON_TOKEN and no `holon login`
/// session signs in with its token as the local admin (`holon access local` says which account).
/// The grants hold it to nothing, so a sweep's callers without a grant don't apply to it. What
/// guards it is the OS: only the user who ran `holon serve` may read it.
pub(crate) const RUN_FILE: Surface = Surface {
    name: "the run file",
    routes: &[],
    local: Some(Local {
        reads: "~/.holon/run/<port>.json, written by holon serve: a token that signs in as the local admin, which every command on this machine uses when it has no HOLON_TOKEN and no holon login session",
        guarded_by: "the OS's permissions on the file: only the user who runs holon serve may read it. Whoever reads it is the local admin, and every grant holds for them",
    }),
    shows: &[RunValues, State, LogLines],
};

/// Every command and subcommand, in the order of `holon --help`.
pub(crate) const COMMANDS: &[Surface] = &[
    local("serve", "the data folder (the runs, state, secrets and accounts it keeps), the policy file, and the run file it writes", SEALED),
    local("new", "writes a starter flow or a sample, and holon.d.ts, where it is told", OWN),
    local("check", "the flow's file it is given", OWN),
    local("show", "the flow's file it is given", OWN),
    local("fmt", "the flow's file it is given, rewritten", OWN),
    local("types", "writes holon.d.ts (and a tsconfig) in the folder it is given", OWN),
    both("deploy", &["POST /unit", "POST /deploy/preview", "POST /deploy", "GET /deployments/{slug}/document"], "the flow's file it is given", OWN, &[]),
    both("replay", &["POST /unit", "GET /deployments/{slug}/replay"], "the flow's file it is given", OWN, &[RunValues]),
    both(
        "editor",
        &[
            "GET /auth/me", "GET /auth/me/can", "GET /deployments", "GET /flows", "GET /flows/{slug}", "POST /flows", "GET /activity", "GET /admin/warnings",
            "GET /deployments/{slug}/log", "GET /deployments/{slug}/log/heads", "GET /deployments/{slug}/document", "GET /deployments/{slug}/manifest", "GET /deployments/{slug}/events",
            "GET /deployments/{slug}/versions", "GET /deployments/{slug}/grant", "POST /deployments/{slug}/grant", "GET /deployments/{slug}/secrets",
            "GET /deployments/{slug}/secrets/{name}", "POST /deployments/{slug}/secrets/{name}", "DELETE /deployments/{slug}/secrets/{name}",
            "GET /deployments/{slug}/runs", "GET /runs/waiting", "POST /deployments/{slug}/runs/{id}/decide", "GET /proposals", "GET /proposals/{id}", "POST /proposals/{id}/decline",
            "GET /access/proposals", "POST /access/proposals/{id}/approve", "POST /access/proposals/{id}/decline", "GET /deployments/{slug}/replay",
            "POST /deployments/{slug}/settings", "POST /deployments/{slug}/secrets/{name}/rollback", "POST /admin/keys/forget", "GET /deployments/{slug}/holds", "GET /deployments/{slug}/state", "GET /deployments/{slug}/sessions",
            "GET /deployments/{slug}/collections/{name}", "GET /deployments/{slug}/changes", "GET /deployments/{slug}/logs",
            "GET /deployments/{slug}/export", "POST /deployments/{slug}/restore", "POST /deployments/{slug}/request", "POST /trigger/{slug}/{name}",
            "POST /deployments/{slug}/tick", "POST /deployments/{slug}/run", "POST /deployments/{slug}/pause", "POST /deployments/{slug}/stop",
            "DELETE /deployments/{slug}", "POST /unit", "POST /deploy/preview", "POST /deploy", "GET /admin/secrets/env", "GET /people", "POST /people",
            "PATCH /people/{name}", "POST /people/{name}/reset", "DELETE /people/{name}", "POST /people/{name}/tokens", "DELETE /people/{name}/tokens/{id}",
            "GET /roles", "PUT /roles/{name}", "DELETE /roles/{name}", "GET /groups", "PUT /groups/{name}", "DELETE /groups/{name}", "GET /bindings", "POST /bindings",
            "DELETE /bindings/{id}", "GET /access/versions", "GET /access", "PUT /access/approvals", "GET /auth/owned", "GET /permissions", "POST /permissions/reload", "GET /admin/grants", "GET /admin/oidc", "POST /admin/oidc", "GET /admin/host",
            "GET /admin/folders", "POST /admin/folders", "DELETE /admin/folders/{name}", "POST /admin/folders/sweep", "GET /admin/databases",
            "POST /admin/databases", "DELETE /admin/databases/{name}", "GET /admin/databases/{name}/tables", "DELETE /admin/mails/{name}",
            "DELETE /admin/feeds/{name}", "DELETE /admin/queues/{name}", "POST /admin/store/sweep", "GET /admin/mails", "POST /admin/mails", "GET /admin/feeds",
            "POST /admin/feeds", "GET /admin/queues", "POST /admin/queues", "GET /admin/secrets", "GET /admin/secrets/{name}", "POST /admin/secrets/{name}",
            "POST /admin/secrets/{name}/cap", "POST /admin/secrets/{name}/rollback", "DELETE /admin/secrets/{name}", "POST /admin/storage", "GET /admin/folders/browse",
            "GET /summary", "POST /summary/check", "GET /deployments/{slug}/feeds", "GET /deployments/{slug}/queues", "GET /deployments/{slug}/channels", "DELETE /flows/{slug}",
        ],
        "the flows' files it opens in tabs, and an export it writes (as holon export)",
        OWN,
        &[RunValues, State, LogLines],
    ),
    local("verify", "an export (with --files, the files its runs read; with --data and --root-key, a sealed backup's). An export holds the runs' values, and no grant follows a file", "nothing for an export: whoever has the file reads it. A sealed backup: the root key"),
    both("export", &["GET /deployments/{slug}/export"], "writes the runs, with their values, to a file: from then on no grant follows them", OWN, &[RunValues]),
    routes("restore", &["GET /deployments/{slug}/versions", "POST /deployments/{slug}/restore"], &[State]),
    routes(
        "history",
        &["GET /deployments/{slug}/events", "GET /deployments/{slug}/log/heads", "GET /deployments/{slug}/versions", "GET /deployments/{slug}/changes", "GET /deployments/{slug}/document"],
        &[State],
    ),
    routes("deployments", &["GET /deployments", "GET /deployments/{slug}/runs"], &[RunValues]),
    routes("runs", &["GET /deployments/{slug}/log/heads", "GET /deployments/{slug}/runs", "GET /deployments/{slug}/log", "GET /deployments/{slug}/document"], &[RunValues]),
    routes("run", &["POST /deployments/{slug}/run"], &[]),
    routes("pause", &["POST /deployments/{slug}/pause"], &[]),
    routes("stop", &["POST /deployments/{slug}/stop"], &[]),
    routes("remove", &["DELETE /deployments/{slug}", "POST /admin/keys/forget"], &[]),
    routes("call", &["POST /deployments/{slug}/request"], &[]),
    both(
        "load",
        &["GET /deployments", "GET /deployments/{slug}/log/heads"],
        "the flow's own port (or --to): its routes, sent as any client of the flow sends them",
        "the flow: it answers whoever reaches its port, as its routes say",
        &[],
    ),
    routes("action", &["POST /trigger/{slug}/{name}"], &[]),
    routes("tick", &["POST /deployments/{slug}/tick"], &[RunValues]),
    routes("state", &["GET /deployments/{slug}/state", "GET /deployments/{slug}/collections/{name}", "GET /deployments/{slug}/sessions"], &[State]),
    routes("decide", &["GET /deployments/{slug}/runs", "POST /deployments/{slug}/runs/{id}/decide"], &[RunValues]),
    routes("logs", &["GET /deployments/{slug}/logs"], &[LogLines]),
    routes("wait", &["GET /proposals"], &[]),
    routes("grant", &["GET /deployments/{slug}/grant", "GET /deployments/{slug}/manifest", "POST /deployments/{slug}/grant"], &[]),
    routes("grants", &["GET /admin/grants"], &[]),
    routes("policy", &["GET /permissions", "POST /permissions/reload", "POST /admin/storage"], &[]),
    routes("secret list", &["GET /deployments/{slug}/manifest", "GET /deployments/{slug}/secrets"], &[]),
    routes("secret history", &["GET /deployments/{slug}/secrets/{name}"], &[]),
    both("secret set", &["POST /deployments/{slug}/secrets/{name}"], "the value piped in, or the variable or vault --from names", OWN, &[]),
    routes("secret rollback", &["POST /deployments/{slug}/secrets/{name}/rollback"], &[]),
    routes("secret remove", &["DELETE /deployments/{slug}/secrets/{name}"], &[]),
    both("secret bind", &["POST /admin/secrets/{name}"], "the value piped in, or the variable or vault --from names", OWN, &[]),
    routes("secret cap", &["POST /admin/secrets/{name}/cap"], &[]),
    routes("secret bound", &["GET /admin/secrets"], &[]),
    routes("secret unbind", &["DELETE /admin/secrets/{name}"], &[]),
    routes("secret env", &["GET /admin/secrets/env"], &[]),
    routes("flows list", &["GET /deployments", "GET /flows"], &[]),
    both("flows show", &["GET /flows/{slug}"], "with --out, writes the flow's text to a file", OWN, &[]),
    both("flows save", &["GET /auth/me", "GET /flows/{slug}", "POST /flows"], "the flow's file it is given", OWN, &[]),
    routes("flows remove", &["DELETE /flows/{slug}"], &[]),
    routes("activity", &["GET /activity"], &[]),
    both("login", &["POST /auth/sign-in"], "writes the session to ~/.holon/tokens.json", "the OS's permissions: tokens.json is readable only by the user", &[]),
    both("logout", &["POST /auth/sign-out"], "takes the session out of ~/.holon/tokens.json", "the OS's permissions: tokens.json is readable only by the user", &[]),
    routes("people list", &["GET /people"], &[]),
    routes("people add", &["POST /people"], &[]),
    routes("people set", &["PATCH /people/{name}"], &[]),
    routes("people reset", &["POST /people/{name}/reset"], &[]),
    routes("people remove", &["DELETE /people/{name}"], &[]),
    routes("people token", &["POST /people/{name}/tokens"], &[]),
    routes("people revoke", &["DELETE /people/{name}/tokens/{id}"], &[]),
    routes("oidc show", &["GET /admin/oidc"], &[]),
    routes("oidc set", &["POST /admin/oidc"], &[]),
    routes("oidc off", &["POST /admin/oidc"], &[]),
    routes("roles list", &["GET /roles"], &[]),
    routes("roles define", &["PUT /roles/{name}"], &[]),
    routes("roles remove", &["DELETE /roles/{name}"], &[]),
    routes("groups list", &["GET /groups"], &[]),
    routes("groups add", &["GET /groups", "PUT /groups/{name}"], &[]),
    routes("groups drop", &["GET /groups", "PUT /groups/{name}"], &[]),
    routes("groups remove", &["DELETE /groups/{name}"], &[]),
    routes("bind", &["POST /bindings", "GET /auth/me/can", "GET /people", "GET /bindings", "GET /roles"], &[]),
    routes("unbind", &["GET /bindings", "DELETE /bindings/{id}"], &[]),
    routes("access versions", &["GET /access/versions"], &[]),
    routes("access show", &["GET /access"], &[]),
    routes("access approvals", &["GET /access", "PUT /access/approvals"], &[]),
    routes("access local", &["GET /access/local", "PUT /access/local"], &[]),
    routes("access proposals", &["GET /access/proposals"], &[]),
    routes("access approve", &["POST /access/proposals/{id}/approve"], &[]),
    routes("access decline", &["POST /access/proposals/{id}/decline"], &[]),
    routes("folder add", &["POST /admin/folders"], &[]),
    routes("folder list", &["GET /admin/folders"], &[]),
    routes("folder remove", &["DELETE /admin/folders/{name}"], &[]),
    routes("folder sweep", &["POST /admin/folders/sweep"], &[]),
    routes("database add", &["POST /admin/databases"], &[]),
    routes("database list", &["GET /admin/databases"], &[]),
    routes("database remove", &["DELETE /admin/databases/{name}"], &[]),
    routes("database tables", &["GET /admin/databases/{name}/tables"], &[]),
    routes("mail add", &["POST /admin/mails"], &[]),
    routes("mail list", &["GET /admin/mails"], &[]),
    routes("mail remove", &["DELETE /admin/mails/{name}"], &[]),
    routes("feed add", &["POST /admin/feeds"], &[]),
    routes("feed list", &["GET /admin/feeds"], &[]),
    routes("feed remove", &["DELETE /admin/feeds/{name}"], &[]),
    routes("queue add", &["POST /admin/queues"], &[]),
    routes("queue list", &["GET /admin/queues"], &[]),
    routes("queue remove", &["DELETE /admin/queues/{name}"], &[]),
    routes("store sweep", &["POST /admin/store/sweep"], &[]),
    routes("key status", &["GET /admin/keys"], &[]),
    both(
        "key rotate",
        &["POST /admin/keys/rotate"],
        "with --root, the server stopped: the data folder's keys.json, and the root key where it is kept",
        "the OS's permissions on the data folder, and the root key",
        &[],
    ),
    routes("key forget", &["POST /admin/keys/forget"], &[]),
    routes("host", &["GET /admin/host", "GET /deployments/{slug}/holds"], &[]),
    local("pick-folder", "a folder dialog on the server's machine, for the admin page's browse", "the OS: it runs as the user who runs holon serve"),
    both(
        "mcp",
        &[
            "GET /auth/me", "GET /people", "POST /people", "POST /people/{name}/tokens", "GET /deployments", "GET /flows", "GET /flows/{slug}", "POST /flows",
            "GET /proposals", "GET /deployments/{slug}/document", "GET /deployments/{slug}/manifest", "GET /deployments/{slug}/secrets",
            "GET /deployments/{slug}/runs", "GET /deployments/{slug}/log", "GET /deployments/{slug}/log/heads", "GET /deployments/{slug}/logs",
            "GET /deployments/{slug}/state", "GET /deployments/{slug}/events", "GET /deployments/{slug}/versions", "GET /deployments/{slug}/changes",
            "GET /deployments/{slug}/replay", "POST /deployments/{slug}/restore", "POST /deployments/{slug}/request", "POST /trigger/{slug}/{name}",
            "POST /deployments/{slug}/runs/{id}/decide", "POST /unit", "POST /deploy/preview", "POST /deploy",
        ],
        "the agent's token kept in ~/.holon/tokens.json; on a server the user runs, the run file, to make their agent and its token",
        "the OS's permissions: tokens.json and the run file are readable only by the user",
        &[RunValues, State, LogLines],
    ),
    both(
        "mcp setup",
        &["GET /auth/me", "GET /people", "POST /people", "POST /people/{name}/tokens"],
        "the agent hosts' settings files, where it writes its entry, and ~/.holon/tokens.json, where it keeps the agent's token; on a server the user runs, the run file",
        "the OS's permissions: they are the user's own files",
        &[],
    ),
    local("mcp hosts", "the agent hosts' settings files it lists, and the hosts the user added", OWN),
    local("lsp", "the flows' text its editor sends it", OWN),
    text("version"),
    local("update", "downloads the release and replaces this installation", "the OS's permissions on the installation"),
    local("migrate", "what an older release left elsewhere (a data folder, key files), moved into ~/.holon", SEALED),
    local("uninstall", "removes this installation (with --purge, all of ~/.holon)", "the OS's permissions: they are the user's own files"),
];

/// Every tool `holon mcp` serves an agent, signed in as that agent: a person's token, or none,
/// and it refuses each but `reference` and `check`.
pub(crate) const TOOLS: &[Surface] = &[
    text("reference"),
    text("check"),
    routes("save", &["GET /auth/me", "GET /flows/{slug}", "POST /flows"], &[]),
    routes("deploy", &["POST /unit", "POST /deploy/preview", "POST /deploy", "GET /deployments/{slug}/document", "GET /proposals"], &[]),
    routes("replay", &["POST /unit", "GET /deployments/{slug}/replay"], &[RunValues]),
    routes(
        "call",
        &["GET /auth/me", "GET /deployments/{slug}/log/heads", "POST /deployments/{slug}/request", "GET /deployments/{slug}/log", "GET /deployments/{slug}/document"],
        &[RunValues],
    ),
    routes("run", &["POST /trigger/{slug}/{name}", "GET /deployments/{slug}/log", "GET /deployments/{slug}/document"], &[RunValues]),
    routes("whoami", &["GET /auth/me"], &[]),
    routes(
        "deployments",
        &[
            "GET /auth/me", "GET /auth/me/can", "GET /deployments", "GET /flows", "GET /proposals", "GET /deployments/{slug}/document", "GET /deployments/{slug}/manifest",
            "GET /deployments/{slug}/secrets",
        ],
        &[],
    ),
    routes("flows", &["GET /deployments", "GET /flows"], &[]),
    routes("runs", &["GET /deployments/{slug}/log/heads", "GET /deployments/{slug}/runs"], &[RunValues]),
    routes("decide", &["GET /deployments/{slug}/runs", "POST /deployments/{slug}/runs/{id}/decide"], &[RunValues]),
    routes("logs", &["GET /deployments/{slug}/logs"], &[LogLines]),
    routes("state", &["GET /deployments/{slug}/state"], &[State]),
    routes("explain", &["GET /deployments/{slug}/log", "GET /deployments/{slug}/document"], &[RunValues]),
    routes(
        "history",
        &["GET /deployments/{slug}/events", "GET /deployments/{slug}/log/heads", "GET /deployments/{slug}/versions", "GET /deployments/{slug}/changes"],
        &[State],
    ),
    routes("restore", &["POST /deployments/{slug}/restore"], &[State]),
];

/// Whether `called` (`METHOD /path?query`, as sent) is a call of `route` (`METHOD /path/{param}`,
/// as ROUTES writes it): each `{…}` is one segment, never empty.
pub(crate) fn calls(route: &str, called: &str) -> bool {
    let (Some((method, path)), Some((sent, sent_path))) = (route.split_once(' '), called.split_once(' ')) else { return false };
    let sent_path = sent_path.split('?').next().unwrap_or_default();
    let (want, got): (Vec<&str>, Vec<&str>) = (path.split('/').collect(), sent_path.split('/').collect());
    method == sent && want.len() == got.len() && want.iter().zip(&got).all(|(w, g)| if w.starts_with('{') { !g.is_empty() } else { w == g })
}
