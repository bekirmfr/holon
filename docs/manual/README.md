# The Holon manual

Holon runs small services (APIs, webhooks, scheduled jobs, runs that wait for a person) written as
**flows**. Agents or people write them; Holon shows what each flow may do before it runs, runs
only that, and keeps a record of every run that anyone can check.

This manual is for people who use Holon: to write flows, run a server, approve what agents
propose, and find out what happened. For how Holon is built, see the design documents listed in
[`docs/README.md`](../README.md).

## Start here

- **[Getting started](getting-started.md):** install Holon, run a server, deploy a flow, watch it
  run, and have an agent write one. About fifteen minutes.
- **[Concepts](concepts.md):** what a flow is, how permissions work, what the run record holds,
  and why every run ends.

## The language

- **[A tour](language/tour.md):** the language by example, from a one-line server to webhooks,
  state, durable runs and JavaScript.
- **[Reference](language/reference.md):** every construct: files and headers, statements,
  expressions, the outside world, routes, holons, effects, types and errors.
- **[Leaves](language/leaves.md):** every building block in the catalog, with its inputs,
  outputs and what it does. Generated from the code.

## Guides

- **[The web editor](guides/editor.md):** the canvas, the text pane, deploying, and replaying a
  run.
- **[Deploying and permissions](guides/deploying.md):** deploy settings, approving what a flow
  may do, secrets, changing a grant, new versions.
- **[Actions: runs by hand](guides/actions.md):** an `action` block, running it from the editor,
  a script or an agent, and who may.
- **[Working with agents](guides/agents.md):** connecting an agent to Holon, what it can and
  can't do, and approving what it proposes.
- **[Stripe to Slack](guides/stripe-to-slack.md):** a real webhook end to end, against Stripe and
  Slack, including finding and fixing a failure.
- **[A public HTTPS address](guides/public-https.md):** putting a reverse proxy or a tunnel in
  front of a deployed flow.
- **[Files on the host](guides/files.md):** named folders: binding them, reading and writing
  files, big files by range.
- **[Databases on the host](guides/databases.md):** named SQLite databases: binding them with a
  schema, SQL in quotes and values after it, `expect:`, and runs that touch the same rows.
- **[Checking runs](guides/checking-runs.md):** exporting a deployment's runs, verifying them
  offline, and witnesses.

## Reference

- **[Command line](reference/cli.md):** every `holon` command and option.
- **[Server configuration](reference/server.md):** where the server keeps things, its options,
  the policy file, accounts and tokens.
- **[Permissions](reference/permissions.md):** every permission a flow can ask for, and how the
  policy treats it.

## Conventions

Code blocks marked `holon` are whole flows: a test compiles every one of them, so they work as
written. Blocks of output (from `holon check` and the like) are real output, with long hashes
shortened.
