# People, agents and services

Everyone who acts on a Holon server signs in, and what they do is kept under their name: "deployed
version 2 by ana", "saved by bma-agent", "made a token for ci · by ana". This guide sets up a
server for a team: the first admin, a teammate, an agent of your own, a token for CI, a role on
one flow or a role of your own, and taking any of them away.

In the editor it is all **People and access**, in tabs: *People*, *Agents and tokens*, *Groups*,
*Roles*, *Bindings* (with the form that binds a role), *History* and *Signing in*, and a page for
each principal, their name a link, saying what they hold and what it lets them do
([The web editor](editor.md#the-hosts-pages)). Each has a command too, shown below.

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
tokens, as an admin may. A service never decides an approval a run waits at, and an agent
takes only a step written for an agent ([Who decides an approval](#who-decides-an-approval)).

## The first admin

A new server prints a setup code when it starts (or takes `HOLON_SETUP_CODE`):

```
holon: no accounts yet. Open the editor and set up the first admin with this code: 7KQ2-M9XD
```

Open the editor, enter the code, your name and a password: you are its admin. Until a person
does, nobody signs in, and the code can't be guessed at speed: five wrong in a row from one address
refuse setting up from there for a minute.

Commands you run on the server's machine (`holon deploy`, `holon people`, `holon decide`…) need
no sign-in: they reach it through its run file, while that server runs, as you, the person who set
it up: holding what is bound to you, and admin on the host. So you are one person whichever way
you sign in: a step bound to you, you take with `holon decide` or in the editor, once; a run you
start, or a version you deploy, from either is yours. The activity log says which entries came
through the run file (`via: local:<your user>`). Before anyone sets the server up they act as
`local:<your user>`, an admin. Someone else running the machine now? `holon access local --as
NAME` ties the run file to them, as a new version of the access document; removing your account
unties it, and while it is disabled, or tied to no one, the run file is the machine's admin and
decides no approval.

## A teammate

In **People and access**, *People*, add them with a name and a role (and their email, if they sign
in through your identity provider). You get a link: send it to them, and they choose their password with it,
once, within seven days. Or:

```sh
holon people add ana --person --role editor
```

which prints the link. Later, on the same page (or with `holon people set` and `reset`): change
their role, **reset** their password (a new link; their sign-ins end), **disable** them (they are
signed out, and can't sign in until enabled), or remove them. The server never lets the last
admin go. While access changes wait for people, someone who decides runs or holds what is an
admin's is reset as [No one takes over someone](#no-one-takes-over-someone) says.

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

1. An admin adds the agent, owned by you: in **People and access**, *Agents and tokens*, or
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
waits at: that is a person's, or an agent's step written for one.

## A role on one flow, and roles of your own

A role is **bound** to a principal where it holds: on the whole host (the role you give someone
when you add them), on one flow, or at one approval point of a flow, until a time if you say.
Bob, a viewer on the host, edits and operates the payout flow, and Dee decides its approvals for
the week Bob is away:

```sh
holon bind editor --to bob --flow payout --why "owns the payout flow"
holon bind editor --to dee --approval payout:ok --until 7d --why "covers for bob"
```

In the editor, **People and access**, *Bindings* (or the person's page, their name a link) binds
the same: to whom, the role, where (the host, a flow, a tag, or an approval point, its flow and
then one of its approvals), until a date or for a while, and why. A binding at an approval point
decides it and nothing else: `holon decide payout` and the editor's Runs waiting list Dee the runs
waiting there, and what each shows, but no other run. The editor lists only the flows one may
view, so to open the flow itself `holon bind` says what to add (`holon bind viewer --to dee --flow
payout`), and the form offers it. A role of your own allows only the actions you choose, from the
catalogue `holon roles define` lists when you get one wrong, or *Roles*' form ticks:

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
for them (`editor on payout via payments`); `holon people` lists the same. *Groups* makes one, and
its page adds and takes out members and binds a role to it. Each principal's page says what their
bindings let them do, as the server decides it: on the host, on every flow, on the flows they
saved, on each tag's flows, and on each flow what more, with the approvals they decide there.
Approvals offers Approve and Decline wherever the server says you may decide.

No one gives more than they hold: you define a role, bind it, or add someone to a group bound it
only holding all it allows there yourself. Nor do you take more than you hold: you change or
unbind a role, or take someone out of a group bound it, only holding what it allows there. And
you change someone (their email, role, owner, password, tokens, whether they are enabled or
there) only holding all they hold, since you could then act as them: a role that manages people
doesn't make its holder an admin. Setting the identity provider says who everyone is, so it takes
holding what each person holds: an admin sets it. Whatever an agent is bound, it never approves, starts,
stops, deletes or restores, sets a secret, decides a person's step of an approval, or manages
people; a service never decides an approval. A binding's **unbind**, which asks why, or `holon
unbind ID` takes it away, and an expired one holds nothing. A binding, a role or a group change
refused says why in the server's words, in the editor as on the command line.

The editor shows each person what their bindings let them: a button the server would refuse them
(**Pause** on a flow they only view, **bind** without `people.assign`) is disabled, and says why in
the server's words, and each of the host's pages is shown to whoever may view it. A change to who
holds what reaches the pages of those it changes within seconds, without signing in again
([The web editor](editor.md#pages-and-tabs)).

## Who decides an approval

An approval's steps say which role takes each (`steps: [{ role: "reviewer" }, { role: "approver"
}]`), and each principal takes one step, once, never of a run they started but a step that says
`starter: true` (`{ role: "editor", starter: true }`: whoever asked for a draft approves it as it
reads), and an agent never of one it started. The host says four things more, for every flow,
kept with the roles and bindings:

```sh
holon access approvals --exclude-deployer on --why "who ships a payout doesn't sign it off"
holon access approvals --agents off --why "no agent decides money yet"
holon access approvals --starter off --why "whoever asks for something never signs it off"
holon access approvals --changes 2 --why "two more sign off whoever is given more"
```

- `--exclude-deployer on`: whoever deployed the version a run is on decides none of its
  approvals, whatever they hold. Off by default.
- `--agents off`: no agent decides an approval. A flow whose approval has a step for an agent
  doesn't deploy, and an agent takes no step of a run already waiting. On by default.
- `--starter off`: whoever started a run takes no step of it, whatever the step says. A flow whose
  approval has a step that lets them (`starter: true`) doesn't deploy, and a run already waiting
  goes by it at once. On by default: a person who started a run may take a step that says so.
- `--changes N`: an access change that gives someone more waits until N people other than its
  author approve it, 1 to 5 ([below](#access-changes-that-wait-for-people)). Off by default: a
  change applies at once.

Each change is a version, with who made it and why; `holon access approvals` alone shows them.

## Access changes that wait for people

With `--changes N`, a change that gives someone more isn't made: it waits as a proposal. It gives
more when it lets someone do what they couldn't, of `run.decide` or of what is an admin's
(people, roles, secrets, the host's resources and operations): binding a role that holds it,
adding someone to a group bound to one, changing a role so that it holds it, setting someone's
role on the host to one; and when it loosens these settings (`--exclude-deployer off`,
`--agents on`, `--starter on`, `--changes` lower or off). A binding that lasts longer than the
one it outlives gives more too.

The server answers such a change 202 with the proposal: what its version will say, what it gives
and to whom, and how many must approve it. N people other than its author approve it, each a
person holding `people.assign` on the host and what the change gives, where it gives it: two who
hold less don't give more together. The last approval makes it, as its author asked it, by them,
its why saying who approved it; it is read again first, and one that no longer applies (its role
removed since, the last admin) is stale, saying why, and its author proposes it again. Anyone who
may approve it declines it with a reason, and its author withdraws it so. Proposals are kept
beside the access document (`access.proposals.json`), never as versions of it: a proposal changes
no one's access. The routes are the server's ([The server](../reference/server.md#accounts-and-roles)).

The command that made it says so (`bound approver to dee on "payout": waits for 1 person other
than you to approve it (access change 5)`), as does the editor's form, and nothing is bound yet.
It is decided in the editor's **Approvals**, under **Access changes**, or from the command line:

```sh
holon access proposals          # what waits, what it gives, and who could act as whom it gives more
holon access approve 5          # bob, another admin
holon access decline 5 --reason "dee is on leave until March"
```

**Access changes** lists each that waits: what it changes and gives, who proposed it, when and why,
who could act as whom it gives more, and who approved it so far, with **Approve…** and
**Decline…** to whoever may (its author has **Withdraw…**), and those decided lately. People and
access › **History** lists those that wait above the versions. An agent never approves one, as it
never approves a deploy.

Taking away applies at once: unbinding, taking someone out of a group, narrowing a role, raising
`--changes`. So does a change that gives no one more, and a change to these settings made on the
server's machine through its run file (`holon access approvals` there): it is how a host whose
approvers are gone gets its access back, and whoever holds that machine holds the data folder
anyway. A new person or agent is never made waiting, as an invitation's link is shown once, to
whoever makes it: one with a role that would wait is refused (409), saying to invite or add them
as a viewer and then give them the role, which waits.

## No one takes over someone

While `--changes` is on, no one makes a way in to someone else who decides runs or holds anything
of an admin's (an editor, an admin, or a role of yours that does): their password **reset**, their
**email** set (a provider matches people by it), a **token** made of their agent or service by
anyone but its owner, another **owner** given to it, or the **identity provider** set, which says
who everyone is. Whoever did it could sign in as them, and count as a second person wherever
people are counted. The server refuses it (403), saying so; with the setting off, an admin does
it as before.

It is still done three ways. They do it themselves: a person changes their own password or
email, and an agent's owner makes its tokens. It is done on the server's machine, through its
run file (`holon people reset bob` there): the machine holds the data folder, so it is the root
here, and no account is. Or what they hold is taken away first, which applies at once, the way in
made, and what they held given back, which waits like any change that gives more. For a
forgotten password:

```sh
holon people set bob --role viewer     # at once
holon people reset bob                 # a new link, for bob
holon people set bob --role admin      # waits for another admin to approve it
```

A proposal says whom it gives more and who could act as them: "ada made bob's reset link".
Whoever made someone's way in (their invite or reset link, their email; an agent's or a
service's owner, or one who made a token of it) doesn't approve giving them more, whoever
proposed it, and a way in made to one of them while it waits makes it stale. The People page
says, for each person, who made their way in.

Each decision is kept with what let it: the binding its principal took the step through, by its
id, and the version of the access document it was read in. The run is given them with the
decision, so its log and its export carry them: an auditor checks who took each step, in which
order and through what, offline, against `holon access show --version N`. The editor shows them
too: on hover over a step in Runs waiting, and on the run's page, a line each. The
`approved-payout` sample binds an agent, a person and you on its tag, one to each of its three
steps; `reviewed-draft` binds an agent to draft and you to approve the draft of a run you start.

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
- **Who held which role**: each change to the roles, groups, bindings and approval settings is a
  version kept in `access.versions.jsonl` in the data folder, with who made it, when, what it
  changed and why:
  `holon access` lists them, and `holon access show --version N` prints one whole, for an auditor
  ([the server reference](../reference/server.md#accounts-and-roles)). In the editor, **People and
  access**, *History* lists them, and a version's page says what it changed from the one before:
  each binding, role and group added, removed or changed, and the approval settings.

## Signing in through your identity provider

People can also sign in through Keycloak, Okta, Auth0 or Entra ID: an admin registers the server
there and sets it in **People and access**, *Signing in* (or `holon oidc set`). The first time someone signs in
there, they are matched to the person here with the email the provider vouches for, so set each
person's email; from then on they are who the provider names them by, whatever their email
becomes. Setting someone's email again matches them anew the next time
([Signing in through an identity provider](../reference/server.md#signing-in-through-an-identity-provider)).
