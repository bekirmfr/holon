<!-- Written from crates/holon/src/surfaces.rs, the table the tests hold every surface to:
     HOLON_BLESS=1 cargo test -p holon every_surface_is_in_the_manual writes it again. -->
# What reaches a server, and what guards it

A Holon server keeps flows' runs, their state and the lines they log. Everything that reaches them
does so in one of three ways: through the server's routes, held to the grants of whoever signs in
(who may do what: [People, agents and services](../guides/people.md)); through a file on a
machine, guarded by that machine's permissions; or neither, a command that prints text of its own.
This page lists each surface, the `holon` commands and the tools `holon mcp` serves an agent, with
what it reaches and what guards it. A test runs each one against a server and fails on a route it
calls that isn't listed here, and on a command or a tool that isn't.

Three grants hold what a surface can show:

- a run's values (what it was sent, what each step computed): `run.steps` on the flow;
- a flow's state (its values, a cell's changes, a session's snapshot): `state.view`;
- the lines a flow logs: `run.view`, and an agent only where a person let agents read them (the
  deploy settings' "agents may read its logs").

Whoever doesn't hold one is refused, with the grant named, or is shown nothing of it. An agent is
refused, besides, what only a person does: approving, granting, starting, stopping and deleting.

## The run file

Reaches: ~/.holon/run/<port>.json, written by holon serve: a token that signs in as the local admin, which every command on this machine uses when it has no HOLON_TOKEN and no holon login session.

Guarded by: the OS's permissions on the file: only the user who runs holon serve may read it. Whoever reads it is the local admin, and every grant holds for them.

So on the server's own machine, whoever can read the run file is the server's admin, whatever the
grants say: the grants hold the run file to nothing, and keeping the machine's accounts to those
who should be its admin is what guards it. A command run with an agent's or a service's token
(HOLON_TOKEN) or a `holon login` session signs in as that instead, and is held to its grants.
`holon access local` says which account the run file signs in as. An agent's shell running as that
user is such a command: [An agent with a shell](../guides/agents.md#an-agent-with-a-shell).

## An export is a file no grant follows

`holon export`, and the editor's export, write a deployment's runs to a file, with every value
they were sent and computed. The server answers it only to whoever holds `run.steps` and
`run.export` on the flow. Once the file is written, no grant follows it: whoever can read the file
reads every value in it, and `holon verify` checks it with no server at all. Keep an export where
only those who hold `run.steps` on its flow can read it.

## Commands

| Command | Reaches | Guarded by | Reads of what the server keeps, and the grant it takes |
|---|---|---|---|
| `holon serve` | the data folder (the runs, state, secrets and accounts it keeps), the policy file, and the run file it writes | the OS's permissions on the data folder, and the root key it is sealed under | - |
| `holon new` | writes a starter flow or a sample, and holon.d.ts, where it is told | nothing: it is the user's own file | - |
| `holon check` | the flow's file it is given | nothing: it is the user's own file | - |
| `holon show` | the flow's file it is given | nothing: it is the user's own file | - |
| `holon fmt` | the flow's file it is given, rewritten | nothing: it is the user's own file | - |
| `holon types` | writes holon.d.ts (and a tsconfig) in the folder it is given | nothing: it is the user's own file | - |
| `holon deploy` | `POST /unit`, `POST /deploy/preview`, `POST /deploy`, `GET /deployments/{slug}/document`, the flow's file it is given | the grants of whoever signs in; besides, nothing: it is the user's own file | - |
| `holon replay` | `POST /unit`, `GET /deployments/{slug}/replay`, the flow's file it is given | the grants of whoever signs in; besides, nothing: it is the user's own file | a run's values: run.steps |
| `holon editor` | `GET /auth/me`, `GET /auth/me/can`, `GET /deployments`, `GET /flows`, `GET /flows/{slug}`, `POST /flows`, `GET /activity`, `GET /admin/warnings`, `GET /deployments/{slug}/log`, `GET /deployments/{slug}/log/heads`, `GET /deployments/{slug}/document`, `GET /deployments/{slug}/manifest`, `GET /deployments/{slug}/events`, `GET /deployments/{slug}/versions`, `GET /deployments/{slug}/grant`, `POST /deployments/{slug}/grant`, `GET /deployments/{slug}/secrets`, `GET /deployments/{slug}/secrets/{name}`, `POST /deployments/{slug}/secrets/{name}`, `DELETE /deployments/{slug}/secrets/{name}`, `GET /deployments/{slug}/runs`, `GET /runs/waiting`, `POST /deployments/{slug}/runs/{id}/decide`, `GET /proposals`, `GET /proposals/{id}`, `POST /proposals/{id}/decline`, `GET /access/proposals`, `POST /access/proposals/{id}/approve`, `POST /access/proposals/{id}/decline`, `GET /deployments/{slug}/replay`, `POST /deployments/{slug}/settings`, `POST /deployments/{slug}/secrets/{name}/rollback`, `POST /admin/keys/forget`, `GET /deployments/{slug}/holds`, `GET /deployments/{slug}/state`, `GET /deployments/{slug}/sessions`, `GET /deployments/{slug}/collections/{name}`, `GET /deployments/{slug}/changes`, `GET /deployments/{slug}/logs`, `GET /deployments/{slug}/export`, `POST /deployments/{slug}/restore`, `POST /deployments/{slug}/request`, `POST /trigger/{slug}/{name}`, `POST /deployments/{slug}/tick`, `POST /deployments/{slug}/run`, `POST /deployments/{slug}/pause`, `POST /deployments/{slug}/stop`, `DELETE /deployments/{slug}`, `POST /unit`, `POST /deploy/preview`, `POST /deploy`, `GET /admin/secrets/env`, `GET /people`, `POST /people`, `PATCH /people/{name}`, `POST /people/{name}/reset`, `DELETE /people/{name}`, `POST /people/{name}/tokens`, `DELETE /people/{name}/tokens/{id}`, `GET /roles`, `PUT /roles/{name}`, `DELETE /roles/{name}`, `GET /groups`, `PUT /groups/{name}`, `DELETE /groups/{name}`, `GET /bindings`, `POST /bindings`, `DELETE /bindings/{id}`, `GET /access/versions`, `GET /access`, `PUT /access/approvals`, `GET /auth/owned`, `GET /permissions`, `POST /permissions/reload`, `GET /admin/grants`, `GET /admin/oidc`, `POST /admin/oidc`, `GET /admin/host`, `GET /admin/folders`, `POST /admin/folders`, `DELETE /admin/folders/{name}`, `POST /admin/folders/sweep`, `GET /admin/databases`, `POST /admin/databases`, `DELETE /admin/databases/{name}`, `GET /admin/databases/{name}/tables`, `DELETE /admin/mails/{name}`, `DELETE /admin/feeds/{name}`, `DELETE /admin/queues/{name}`, `POST /admin/store/sweep`, `GET /admin/mails`, `POST /admin/mails`, `GET /admin/feeds`, `POST /admin/feeds`, `GET /admin/queues`, `POST /admin/queues`, `GET /admin/secrets`, `GET /admin/secrets/{name}`, `POST /admin/secrets/{name}`, `POST /admin/secrets/{name}/cap`, `POST /admin/secrets/{name}/rollback`, `DELETE /admin/secrets/{name}`, `POST /admin/storage`, `GET /admin/folders/browse`, `GET /summary`, `POST /summary/check`, `GET /deployments/{slug}/feeds`, `GET /deployments/{slug}/queues`, `GET /deployments/{slug}/channels`, `DELETE /flows/{slug}`, the flows' files it opens in tabs, and an export it writes (as holon export) | the grants of whoever signs in; besides, nothing: it is the user's own file | a run's values: run.steps; a flow's state: state.view; log lines: run.view (an agent: "agents may read its logs") |
| `holon verify` | an export (with --files, the files its runs read; with --data and --root-key, a sealed backup's). An export holds the runs' values, and no grant follows a file | nothing for an export: whoever has the file reads it. A sealed backup: the root key | - |
| `holon export` | `GET /deployments/{slug}/export`, writes the runs, with their values, to a file: from then on no grant follows them | the grants of whoever signs in; besides, nothing: it is the user's own file | a run's values: run.steps |
| `holon restore` | `GET /deployments/{slug}/versions`, `POST /deployments/{slug}/restore` | the grants of whoever signs in | a flow's state: state.view |
| `holon history` | `GET /deployments/{slug}/events`, `GET /deployments/{slug}/log/heads`, `GET /deployments/{slug}/versions`, `GET /deployments/{slug}/changes`, `GET /deployments/{slug}/document` | the grants of whoever signs in | a flow's state: state.view |
| `holon deployments` | `GET /deployments`, `GET /deployments/{slug}/runs` | the grants of whoever signs in | a run's values: run.steps |
| `holon runs` | `GET /deployments/{slug}/log/heads`, `GET /deployments/{slug}/runs`, `GET /deployments/{slug}/log`, `GET /deployments/{slug}/document` | the grants of whoever signs in | a run's values: run.steps |
| `holon run` | `POST /deployments/{slug}/run` | the grants of whoever signs in | - |
| `holon pause` | `POST /deployments/{slug}/pause` | the grants of whoever signs in | - |
| `holon stop` | `POST /deployments/{slug}/stop` | the grants of whoever signs in | - |
| `holon remove` | `DELETE /deployments/{slug}`, `POST /admin/keys/forget` | the grants of whoever signs in | - |
| `holon call` | `POST /deployments/{slug}/request` | the grants of whoever signs in | - |
| `holon load` | `GET /deployments`, `GET /deployments/{slug}/log/heads`, the flow's own port (or --to): its routes, sent as any client of the flow sends them | the grants of whoever signs in; besides, the flow: it answers whoever reaches its port, as its routes say | - |
| `holon action` | `POST /trigger/{slug}/{name}` | the grants of whoever signs in | - |
| `holon tick` | `POST /deployments/{slug}/tick` | the grants of whoever signs in | a run's values: run.steps |
| `holon state` | `GET /deployments/{slug}/state`, `GET /deployments/{slug}/collections/{name}`, `GET /deployments/{slug}/sessions` | the grants of whoever signs in | a flow's state: state.view |
| `holon decide` | `GET /deployments/{slug}/runs`, `POST /deployments/{slug}/runs/{id}/decide` | the grants of whoever signs in | a run's values: run.steps |
| `holon logs` | `GET /deployments/{slug}/logs` | the grants of whoever signs in | log lines: run.view (an agent: "agents may read its logs") |
| `holon wait` | `GET /proposals` | the grants of whoever signs in | - |
| `holon grant` | `GET /deployments/{slug}/grant`, `GET /deployments/{slug}/manifest`, `POST /deployments/{slug}/grant` | the grants of whoever signs in | - |
| `holon grants` | `GET /admin/grants` | the grants of whoever signs in | - |
| `holon policy` | `GET /permissions`, `POST /permissions/reload`, `POST /admin/storage` | the grants of whoever signs in | - |
| `holon secret list` | `GET /deployments/{slug}/manifest`, `GET /deployments/{slug}/secrets` | the grants of whoever signs in | - |
| `holon secret history` | `GET /deployments/{slug}/secrets/{name}` | the grants of whoever signs in | - |
| `holon secret set` | `POST /deployments/{slug}/secrets/{name}`, the value piped in, or the variable or vault --from names | the grants of whoever signs in; besides, nothing: it is the user's own file | - |
| `holon secret rollback` | `POST /deployments/{slug}/secrets/{name}/rollback` | the grants of whoever signs in | - |
| `holon secret remove` | `DELETE /deployments/{slug}/secrets/{name}` | the grants of whoever signs in | - |
| `holon secret bind` | `POST /admin/secrets/{name}`, the value piped in, or the variable or vault --from names | the grants of whoever signs in; besides, nothing: it is the user's own file | - |
| `holon secret cap` | `POST /admin/secrets/{name}/cap` | the grants of whoever signs in | - |
| `holon secret bound` | `GET /admin/secrets` | the grants of whoever signs in | - |
| `holon secret unbind` | `DELETE /admin/secrets/{name}` | the grants of whoever signs in | - |
| `holon secret env` | `GET /admin/secrets/env` | the grants of whoever signs in | - |
| `holon flows list` | `GET /deployments`, `GET /flows` | the grants of whoever signs in | - |
| `holon flows show` | `GET /flows/{slug}`, with --out, writes the flow's text to a file | the grants of whoever signs in; besides, nothing: it is the user's own file | - |
| `holon flows save` | `GET /auth/me`, `GET /flows/{slug}`, `POST /flows`, the flow's file it is given | the grants of whoever signs in; besides, nothing: it is the user's own file | - |
| `holon flows remove` | `DELETE /flows/{slug}` | the grants of whoever signs in | - |
| `holon activity` | `GET /activity` | the grants of whoever signs in | - |
| `holon login` | `POST /auth/sign-in`, writes the session to ~/.holon/tokens.json | the grants of whoever signs in; besides, the OS's permissions: tokens.json is readable only by the user | - |
| `holon logout` | `POST /auth/sign-out`, takes the session out of ~/.holon/tokens.json | the grants of whoever signs in; besides, the OS's permissions: tokens.json is readable only by the user | - |
| `holon people list` | `GET /people` | the grants of whoever signs in | - |
| `holon people add` | `POST /people` | the grants of whoever signs in | - |
| `holon people set` | `PATCH /people/{name}` | the grants of whoever signs in | - |
| `holon people reset` | `POST /people/{name}/reset` | the grants of whoever signs in | - |
| `holon people remove` | `DELETE /people/{name}` | the grants of whoever signs in | - |
| `holon people token` | `POST /people/{name}/tokens` | the grants of whoever signs in | - |
| `holon people revoke` | `DELETE /people/{name}/tokens/{id}` | the grants of whoever signs in | - |
| `holon oidc show` | `GET /admin/oidc` | the grants of whoever signs in | - |
| `holon oidc set` | `POST /admin/oidc` | the grants of whoever signs in | - |
| `holon oidc off` | `POST /admin/oidc` | the grants of whoever signs in | - |
| `holon roles list` | `GET /roles` | the grants of whoever signs in | - |
| `holon roles define` | `PUT /roles/{name}` | the grants of whoever signs in | - |
| `holon roles remove` | `DELETE /roles/{name}` | the grants of whoever signs in | - |
| `holon groups list` | `GET /groups` | the grants of whoever signs in | - |
| `holon groups add` | `GET /groups`, `PUT /groups/{name}` | the grants of whoever signs in | - |
| `holon groups drop` | `GET /groups`, `PUT /groups/{name}` | the grants of whoever signs in | - |
| `holon groups remove` | `DELETE /groups/{name}` | the grants of whoever signs in | - |
| `holon bind` | `POST /bindings`, `GET /auth/me/can`, `GET /people`, `GET /bindings`, `GET /roles` | the grants of whoever signs in | - |
| `holon unbind` | `GET /bindings`, `DELETE /bindings/{id}` | the grants of whoever signs in | - |
| `holon access versions` | `GET /access/versions` | the grants of whoever signs in | - |
| `holon access show` | `GET /access` | the grants of whoever signs in | - |
| `holon access approvals` | `GET /access`, `PUT /access/approvals` | the grants of whoever signs in | - |
| `holon access local` | `GET /access/local`, `PUT /access/local` | the grants of whoever signs in | - |
| `holon access proposals` | `GET /access/proposals` | the grants of whoever signs in | - |
| `holon access approve` | `POST /access/proposals/{id}/approve` | the grants of whoever signs in | - |
| `holon access decline` | `POST /access/proposals/{id}/decline` | the grants of whoever signs in | - |
| `holon folder add` | `POST /admin/folders` | the grants of whoever signs in | - |
| `holon folder list` | `GET /admin/folders` | the grants of whoever signs in | - |
| `holon folder remove` | `DELETE /admin/folders/{name}` | the grants of whoever signs in | - |
| `holon folder sweep` | `POST /admin/folders/sweep` | the grants of whoever signs in | - |
| `holon database add` | `POST /admin/databases` | the grants of whoever signs in | - |
| `holon database list` | `GET /admin/databases` | the grants of whoever signs in | - |
| `holon database remove` | `DELETE /admin/databases/{name}` | the grants of whoever signs in | - |
| `holon database tables` | `GET /admin/databases/{name}/tables` | the grants of whoever signs in | - |
| `holon mail add` | `POST /admin/mails` | the grants of whoever signs in | - |
| `holon mail list` | `GET /admin/mails` | the grants of whoever signs in | - |
| `holon mail remove` | `DELETE /admin/mails/{name}` | the grants of whoever signs in | - |
| `holon feed add` | `POST /admin/feeds` | the grants of whoever signs in | - |
| `holon feed list` | `GET /admin/feeds` | the grants of whoever signs in | - |
| `holon feed remove` | `DELETE /admin/feeds/{name}` | the grants of whoever signs in | - |
| `holon queue add` | `POST /admin/queues` | the grants of whoever signs in | - |
| `holon queue list` | `GET /admin/queues` | the grants of whoever signs in | - |
| `holon queue remove` | `DELETE /admin/queues/{name}` | the grants of whoever signs in | - |
| `holon store sweep` | `POST /admin/store/sweep` | the grants of whoever signs in | - |
| `holon key status` | `GET /admin/keys` | the grants of whoever signs in | - |
| `holon key rotate` | `POST /admin/keys/rotate`, with --root, the server stopped: the data folder's keys.json, and the root key where it is kept | the grants of whoever signs in; besides, the OS's permissions on the data folder, and the root key | - |
| `holon key forget` | `POST /admin/keys/forget` | the grants of whoever signs in | - |
| `holon key export` | `GET /admin/keys`, the data folder's keys.json, and the root key where it is kept (with --server and no --root-key, where that server says it reads it), copied to a file of the user's | the grants of whoever signs in; besides, the OS's permissions on the data folder, and the root key | - |
| `holon host` | `GET /admin/host`, `GET /deployments/{slug}/holds` | the grants of whoever signs in | - |
| `holon pick-folder` | a folder dialog on the server's machine, for the admin page's browse | the OS: it runs as the user who runs holon serve | - |
| `holon mcp` | `GET /auth/me`, `GET /people`, `POST /people`, `POST /people/{name}/tokens`, `GET /deployments`, `GET /flows`, `GET /flows/{slug}`, `POST /flows`, `GET /proposals`, `GET /deployments/{slug}/document`, `GET /deployments/{slug}/manifest`, `GET /deployments/{slug}/secrets`, `GET /deployments/{slug}/runs`, `GET /deployments/{slug}/log`, `GET /deployments/{slug}/log/heads`, `GET /deployments/{slug}/logs`, `GET /deployments/{slug}/state`, `GET /deployments/{slug}/events`, `GET /deployments/{slug}/versions`, `GET /deployments/{slug}/changes`, `GET /deployments/{slug}/replay`, `POST /deployments/{slug}/restore`, `POST /deployments/{slug}/request`, `POST /trigger/{slug}/{name}`, `POST /deployments/{slug}/runs/{id}/decide`, `POST /unit`, `POST /deploy/preview`, `POST /deploy`, the agent's token kept in ~/.holon/tokens.json; on a server the user runs, the run file, to make their agent and its token | the grants of whoever signs in; besides, the OS's permissions: tokens.json and the run file are readable only by the user | a run's values: run.steps; a flow's state: state.view; log lines: run.view (an agent: "agents may read its logs") |
| `holon mcp setup` | `GET /auth/me`, `GET /people`, `POST /people`, `POST /people/{name}/tokens`, the agent hosts' settings files, where it writes its entry, and ~/.holon/tokens.json, where it keeps the agent's token; on a server the user runs, the run file | the grants of whoever signs in; besides, the OS's permissions: they are the user's own files | - |
| `holon mcp hosts` | the agent hosts' settings files it lists, and the hosts the user added | nothing: it is the user's own file | - |
| `holon lsp` | the flows' text its editor sends it | nothing: it is the user's own file | - |
| `holon version` | nothing: text of its own | - | - |
| `holon update` | downloads the release and replaces this installation | the OS's permissions on the installation | - |
| `holon migrate` | what an older release left elsewhere (a data folder, key files), moved into ~/.holon | the OS's permissions on the data folder, and the root key it is sealed under | - |
| `holon uninstall` | removes this installation (with --purge, all of ~/.holon) | the OS's permissions: they are the user's own files | - |

## Agent tools

`holon mcp` signs in as an agent: with a person's token, or with none, it refuses every tool
but `reference` and `check`.

| Tool | Reaches | Reads of what the server keeps, and the grant it takes |
|---|---|---|
| `reference` | nothing: text of its own | - |
| `check` | nothing: text of its own | - |
| `save` | `GET /auth/me`, `GET /flows/{slug}`, `POST /flows` | - |
| `deploy` | `POST /unit`, `POST /deploy/preview`, `POST /deploy`, `GET /deployments/{slug}/document`, `GET /proposals` | - |
| `replay` | `POST /unit`, `GET /deployments/{slug}/replay` | a run's values: run.steps |
| `call` | `GET /auth/me`, `GET /deployments/{slug}/log/heads`, `POST /deployments/{slug}/request`, `GET /deployments/{slug}/log`, `GET /deployments/{slug}/document` | a run's values: run.steps |
| `run` | `POST /trigger/{slug}/{name}`, `GET /deployments/{slug}/log`, `GET /deployments/{slug}/document` | a run's values: run.steps |
| `whoami` | `GET /auth/me` | - |
| `deployments` | `GET /auth/me`, `GET /auth/me/can`, `GET /deployments`, `GET /flows`, `GET /proposals`, `GET /deployments/{slug}/document`, `GET /deployments/{slug}/manifest`, `GET /deployments/{slug}/secrets` | - |
| `flows` | `GET /deployments`, `GET /flows` | - |
| `runs` | `GET /deployments/{slug}/log/heads`, `GET /deployments/{slug}/runs` | a run's values: run.steps |
| `decide` | `GET /deployments/{slug}/runs`, `POST /deployments/{slug}/runs/{id}/decide` | a run's values: run.steps |
| `logs` | `GET /deployments/{slug}/logs` | log lines: run.view (an agent: "agents may read its logs") |
| `state` | `GET /deployments/{slug}/state` | a flow's state: state.view |
| `explain` | `GET /deployments/{slug}/log`, `GET /deployments/{slug}/document` | a run's values: run.steps |
| `history` | `GET /deployments/{slug}/events`, `GET /deployments/{slug}/log/heads`, `GET /deployments/{slug}/versions`, `GET /deployments/{slug}/changes` | a flow's state: state.view |
| `restore` | `POST /deployments/{slug}/restore` | a flow's state: state.view |
