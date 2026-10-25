# Getting started

In about fifteen minutes you will install Holon, start a server, deploy a flow, send it a
request, look at the run, and change the flow. At the end, an agent writes one for you.

You need a terminal and a browser. Nothing else: the editor is built into the server.

## 1. Install

On macOS or Linux:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/bekirmfr/holon/releases/latest/download/holon-installer.sh | sh
```

On Windows, in PowerShell:

```powershell
irm https://github.com/bekirmfr/holon/releases/latest/download/holon-installer.ps1 | iex
```

The script fetches the right build, checks it and puts `holon` in `~/.holon/bin`, on your PATH.
Open a new terminal and check it is there:

```sh
holon version
```

While Holon is in release candidates, `latest` may not have one yet: name the version instead
(`…/releases/download/v0.1.0-rc.21/holon-installer.sh`). See
[the command line reference](reference/cli.md#install) for the installer's options.

## 2. Start a server

```sh
holon serve
```

```
holon: data in /home/you/.holon/data
holon: no accounts yet. Open the editor and set up the first admin with this code: K7QM-4XWP
holon: the editor (built in) at http://127.0.0.1:7420/
holon: listening on http://127.0.0.1:7420 (waiting to be set up; see access.rs)
```

The server keeps everything in `~/.holon/data`. It listens on 127.0.0.1 only: nobody else on the
network can reach it until you say so ([Server configuration](reference/server.md)).

## 3. Create the admin

Open http://127.0.0.1:7420. The editor asks for the **setup code** the server printed, your name
and a password (at least ten characters). The code works once: whoever enters it first is the
server's admin, which is why it is printed only where the server runs.

The editor opens on the **welcome** page. It has the same steps as this page, and short docs.

## 4. Deploy a sample

Press **flows** in the header. Under **samples** there is one flow for each thing Holon can do.
Find **Hello API** and press **open**. It opens in its own tab, as an unsaved copy:

- the **canvas** shows the flow as a graph: a request node, the text it builds, and a respond
  node;
- the **text** pane shows the same flow in the language (press **text on** in the toolbar if
  your window is narrow):

```holon
flow hello-api
port 8101

route GET /hello/:name (req) {
  respond "Hello, {req.params.name}"
}
```

Press **deploy**. Holon works out what the flow may do and asks you to approve it:

> **Deploy "hello-api"?**
> It needs your approval to:
> - receive requests

That list comes from the flow itself, not from anything its author wrote about it. This flow can
only answer requests. Press **Approve and deploy**. The deploy bar now says **● running :8101**.

## 5. Send it a request

```sh
curl http://127.0.0.1:8101/hello/ada
```

```
Hello, ada
```

The flow has its own server, on its own port. You can also send a request from the editor: the
**server** panel at the bottom left lists the routes, and **test** opens a request tester.

## 6. Look at the run

Switch the toolbar from **edit** to **view**. The canvas is now the deployed flow, read only, and
the **run** pane at the bottom lists its runs. Pick the one you just sent:

- each node shows the value it made, beside its outputs;
- the text pane marks the same values beside each expression;
- **◀** and **▶** step through the run one node at a time, in the order it was evaluated;
- **✓ replay reproduces the run** means the editor ran the flow again from what the run log
  recorded, and got the same answer.

Every run is kept like this, with everything it read from outside. That is what makes replay
exact, and what lets anyone check a deployment's runs later ([Checking runs](guides/checking-runs.md)).

## 7. Change it

Switch back to **edit**. In the text pane, change the route to answer a record instead of text:

```holon
flow hello-api
port 8101

route GET /hello/:name (req) {
  respond { greeting: "Hello, {req.params.name}", at: now }
}
```

The canvas follows as you type. A mistake is underlined in the text, and the canvas keeps the
last version that compiled until you fix it.

The deploy button now reads **update •**: the canvas differs from what is deployed. Press it. This
version also reads the clock (`now`), which the first grant didn't cover, so Holon asks for that,
and only that:

> **Update "hello-api"?**
> It needs your approval to:
> - read the clock
>
> Already granted:
> - receive requests

Approve it, and send the request again:

```sh
curl http://127.0.0.1:8101/hello/ada
```

```
{"at":1793001600,"greeting":"Hello, ada"}
```

The flow is now a new version, with a new hash. Each run names the version it ran on, so view
mode shows an older run on the canvas it ran on.

Press **save** to keep the flow on the server; it appears under **saved flows** on the flows page.

## 8. Write a flow as text

Flows are plain text files, and the `holon` command works with them on any machine, with no
server:

```sh
holon new todo.holon
holon check todo.holon
```

`new` writes a starter flow; `holon new todo.holon --sample notes-api` writes a sample instead
(`holon new --help` lists them). `check` compiles it and tells you what it is and what it may do,
or each mistake with its line and column. Edit it in any editor (the
[VS Code extension](guides/editor.md#other-editors) adds highlighting and checks as you type), then
`holon fmt todo.holon` writes it in the canonical form.

To deploy a text file, press **import .holon** on the flows page: it opens as a tab, ready to
deploy.

## 9. Let an agent write one

Holon has a tool surface for agents, over MCP. Add it to your agent's host:

```sh
holon mcp setup claude-code
```

`holon mcp hosts` lists the hosts it knows (Claude Code, Claude Desktop, Cursor, VS Code, Zed and
many more). Restart the agent, and ask it for something:

> Write a Holon flow that keeps a counter per name: POST /count/:name adds one, GET /count/:name
> answers the count. Deploy it.

The agent reads the language, checks its flow until it compiles, and deploys it. It can't
approve what the flow may do, so the first deploy stops there: the flow appears on your **flows**
page, saved by the agent. Open it, read what it asks for, and press **deploy** and **Approve and
deploy** if you agree. After that, a version that asks for nothing more deploys from the agent at
once, and the agent can send it requests and explain its runs. See
[Working with agents](guides/agents.md).

## Next

- [Concepts](concepts.md): what a flow is, what "may do" means, and what the run record holds.
- [A tour of the language](language/tour.md): the language, by example.
- [Stripe to Slack](guides/stripe-to-slack.md): a real webhook, end to end.
