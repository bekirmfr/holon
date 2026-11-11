# People, agents and services

Everyone who acts on a Holon server signs in, and what they do is kept under their name: "deployed
version 2 by ana", "saved by bma-agent", "made a token for ci · by ana". This guide sets up a
server for a team: the first admin, a teammate, an agent of your own, a token for CI, a role on
one flow or a role of your own, and taking any of them away.

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
([what each may do](../reference/server.md#accounts-and-roles)). That is a role on the whole
host; a role can also hold on one flow only, and an admin can define roles of their own
([below](#a-role-on-one-flow-and-roles-of-your-own)).

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

## A role on one flow, and roles of your own

A role is **bound** to a principal where it holds: on the whole host (the role you give someone
when you add them), on one flow, or at one approval point of a flow, until a time if you say.
Bob, a viewer on the host, edits and operates the payout flow, and Dee decides its approvals for
the week Bob is away:

```sh
holon bind editor --to bob --flow payout --why "owns the payout flow"
holon bind editor --to dee --approval payout:ok --until 7d --why "covers for bob"
```

A binding at an approval point decides it and nothing else, not even viewing the flow's runs, so
`holon bind` says what to add (`holon bind viewer --to dee --flow payout`). A role of your own
allows only the actions you choose, from the catalogue `holon roles define` lists when you get one
wrong:

```sh
holon roles define payout-approver --allow run.view,run.decide --about "decides payouts"
holon groups add payments bob dee
holon bind payout-approver --group payments --flow payout
```

A flow's text can **tag** it, `flow("payout", { port: 8153, tags: ["payments"] })`, and a role
bound on the tag holds on every flow deployed with it, while it is: a version without the tag
takes the flow out of the binding's reach, and deleting it does too.

```sh
holon bind editor --to bob --tag payments --why "on the payments team"
```

Who may act on a flow is part of what its deploy approves: a version that adds a tag asks for it
(`Tag("payments")`), and its review says who the tag lets in (`joins payments: bob as editor`);
the host's policy never grants one by itself. Bob deploys a new flow tagged `payments`, and a new
version of one, but neither tags a flow he holds nothing on (taking it over) nor takes the tag
off one (taking it out of his own reach, and his team's): a host admin does those. An agent's
version that adds a tag waits as a proposal, as any change it can't approve does.

A role bound to a **group** holds for each member whose kind it allows. **People and access**
offers roles of your own where it offers a role, and lists beside each principal what else holds
for them (`editor on payout via payments`); `holon people` lists the same. Approvals offers
Approve and Decline wherever the server says you may decide.

No one gives more than they hold: you define a role, bind it, or add someone to a group bound it
only holding all it allows there yourself. Whatever an agent is bound, it never approves, starts,
stops, deletes or restores, sets a secret, decides an approval, or manages people; a service never
decides an approval. `holon unbind ID` takes a binding away, and an expired one holds nothing.

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
- **Who held which role**: each change to the roles, groups and bindings is a version kept in
  `access.versions.jsonl` in the data folder, with who made it, when, what it changed and why:
  `holon access` lists them, and `holon access show --version N` prints one whole, for an auditor
  ([the server reference](../reference/server.md#accounts-and-roles)).

## Signing in through your identity provider

People can also sign in through Keycloak, Okta, Auth0 or Entra ID: an admin registers the server
there and sets it in **People and access** (or `holon oidc set`). Someone signing in is matched to
the person here with the email the provider vouches for, so set each person's email
([Signing in through an identity provider](../reference/server.md#signing-in-through-an-identity-provider)).
