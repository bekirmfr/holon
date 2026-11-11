# People, agents and services

Everyone who acts on a Holon server signs in, and what they do is kept under their name: "deployed
version 2 by ana", "saved by bma-agent", "made a token for ci · by ana". This guide sets up a
server for a team: the first admin, a teammate, an agent of your own, a token for CI, and taking
any of them away.

## Who acts

Each is a **principal** of one of three kinds, each name unique across them:

| Kind | Who | Signs in with | Its role |
|---|---|---|---|
| person | someone on your team | a password, or your identity provider | viewer, editor or admin |
| agent | an AI, through `holon mcp` | tokens of its own | agent or viewer |
| service | a script, CI | tokens of its own | viewer or editor |

A **viewer** reads; an **editor** also saves, deploys and approves, runs, pauses, stops and
deletes; an **admin** also sets secrets, binds resources and manages people. An agent's role,
**agent**, reads, proposes flows and calls them, and never approves, starts, stops or deletes
([what each may do](../reference/server.md#accounts-and-roles)).

An agent or a service has an **owner**, a person, answerable for it: they make and revoke its
tokens, as an admin may. Only a person decides an approval a run waits at.

## The first admin

A new server prints a setup code when it starts (or takes `HOLON_SETUP_CODE`):

```
holon: no accounts yet. Open the editor and set up the first admin with this code: 7KQ2-M9XD
```

Open the editor, enter the code, your name and a password: you are its admin. Until a person
does, nobody signs in, and the code can't be guessed: five wrong in a row refuse setting up for a
minute.

Commands you run on the server's machine (`holon deploy`, `holon people`…) need no sign-in: they
reach it through its run file, as `local:<your user>`, an admin, while that server runs.

## A teammate

In **People and access**, add them with a name and a role (and their email, if they sign in through
your identity provider). You get a link: send it to them, and they choose their password with it,
once, within seven days. Or:

```sh
holon people add ana --person --role editor
```

which prints the link. Later, on the same page (or with `holon people set` and `reset`): change
their role, **reset** their password (a new link; their sign-ins end), **disable** them (they are
signed out, and can't sign in until enabled), or remove them. The server never lets the last
admin go.

## An agent of your own

On the machine where you run `holon serve`:

```sh
holon mcp setup claude-code
```

The first time, the server makes you an agent, `<your user>-agent`, owned by you, and `holon mcp`
signs in as it, never as you:

```
holon mcp signs in to http://127.0.0.1:7420 as bma-agent, an agent owned by bma.
```

It proposes what needs approval, and you approve it; everything it does says *bma-agent*. See
[Working with agents](agents.md).

**From another machine**, the agent needs a token:

1. An admin adds the agent, owned by you: in **People and access**, *agents and services*, or
   `holon people add ana-laptop --agent --owner ana`.
2. You make its token: in the editor, **Your agents and services** (or, for an admin, **People and
   access**) has **make token**. Copy it then: it isn't shown again.
3. On your machine, `holon mcp setup claude-code --server https://holon.example.com` asks for it,
   checks it is an agent's, and keeps it in `~/.holon/tokens.json`, readable only by you.

A person's token, or a service's, is refused there: an agent signs in only as an agent.

## A token for CI

A script or a CI job is a **service**. An editor service deploys, and approves what a flow may do
as any editor does (`holon deploy --approve`); a viewer service reads.

```sh
holon people add ci --service --role editor
holon people token ci --label "CI on GitHub" > ci.token
```

The token is printed once, on stdout, so it goes to the file without being shown. Keep it as the
CI's secret, and give it to `holon` as `HOLON_TOKEN`:

```sh
HOLON_TOKEN="$HOLON_CI_TOKEN" holon deploy shop.holon.ts --server https://holon.example.com
```

The deployment's history says it was deployed *by ci*. A service never decides an approval a run
waits at: that is a person's.

## Taking access away

- **Revoke a token** (its row's **revoke**, or `holon people revoke NAME ID`): whatever signs in with
  it stops at once. Each has its label, who made it and when, so you know which to revoke.
- **Disable** an agent or a service: all its tokens are refused until you enable it. Disabling a
  person signs them out.
- **Remove** an agent or a service: its tokens go with it. A person who still owns one can't be
  removed until it has another owner (**People and access**, or `holon people set NAME --owner
  PERSON`).

What was done stays under the name that did it, after any of these.

## Who did what

- The **Overview**'s *What was done* (and `holon activity`): saves, deploys, grants, people and
  tokens, each by whom.
- A deployment's **history**: each version and who deployed it, and who restored, paused it or
  changed its grant or its secrets, among its runs.
- **Approvals**: who proposed a version and why, and who approved or declined it; for a run waiting
  at an approval, who has approved it so far.

## Signing in through your identity provider

People can also sign in through Keycloak, Okta, Auth0 or Entra ID: an admin registers the server
there and sets it in **People and access** (or `holon oidc set`). Someone signing in is matched to
the person here with the email the provider vouches for, so set each person's email
([Signing in through an identity provider](../reference/server.md#signing-in-through-an-identity-provider)).
