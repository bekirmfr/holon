# Sending mail

A flow can send mail, but only through a mail an admin has bound to a name, from the sender the
admin chose, and to addresses the admin allows. The flow says `import ops from "mail:ops"` and
`ops.send({ … })`; the admin decides that `ops` is the company's SMTP server, logged in as whom,
sending as `Treasury <treasury@acme.com>`, to anyone at acme.com. The flow never sees the server
or its login, and can't send as anyone else.

## Binding a mail

An admin binds mail from the editor's **admin** page (**resources**, **mail**), or on the
server's machine. The SMTP server's URL, with its login, is kept where a secret's value is: one of
the server's environment variables named `HOLON_SECRET_…` (or a line of its `--dotenv` file), or
a field in Vault.

```sh
export HOLON_SECRET_OPS_SMTP='smtps://treasury%40acme.com:…@smtp.example.com'
holon serve
holon mail add ops --smtp env:HOLON_SECRET_OPS_SMTP --from "Treasury <treasury@acme.com>" --to "*@acme.com,treasury@bank.example"
holon mail list
holon mail remove ops
```

| Option | |
|---|---|
| `--smtp SOURCE` | where the server reads the URL: `env:HOLON_SECRET_…` or `vault:<mount>/<path>#<field>` |
| `--from SENDER` | who the mail is from: an address, or a name and one (`Treasury <treasury@acme.com>`) |
| `--to LIST` | every address it may ever send to, with commas: addresses, `*@domain`, or `*` for anyone |
| `--for SLUG` | for that deployment only |

- The URL is `smtps://user:password@host` (TLS from the first byte, port 465 unless given) or
  `smtp://user:password@host:587` (STARTTLS, which the server must offer: one that doesn't is
  refused, so a login never crosses in clear). Plain `smtp://` only to a relay on the same
  machine (`smtp://127.0.0.1:25`). An `@`, `:` or `%` in the login is written `%40`, `%3A`,
  `%25`. Certificates are checked against the public CAs.
- The server logs in with AUTH PLAIN or LOGIN, whichever the SMTP server offers, and with none
  when the URL has no login. It logs in when the mail is bound, so a binding that can't says why
  then.
- The URL is never stored, shown or logged: the binding keeps `env:HOLON_SECRET_OPS_SMTP`, and
  where it points without its login (`smtps://smtp.example.com:465`), which is what the admin page
  and `holon mail list` show.
- `--to` is required: a binding that sends anywhere says so (`--to "*"`).
- Removing a binding gives up the mail on its way through it, and none goes through it until it
  is bound again.

## Sending

```ts
import { flow, route, status } from "holon"
import ops from "mail:ops"

flow("contact", { port: 8190 })

// Each message sent to the form is mailed to support, who answers its sender.
route.post("/contact", (req) => {
  ops.send({
    to: "support@acme.com",
    subject: `Contact from ${req.json.name}`,
    text: `${req.json.name} (${req.json.email}) writes:\n\n${req.json.message}\n`,
  })
  return status(202)
})
```

`ops.send` takes one record:

| Field | |
|---|---|
| `to` | an address or a list of them |
| `cc` | the same, optional |
| `subject` | a text, on one line |
| `text` | the message, a text |
| `html` | optional: the same message as HTML, sent beside the text for a reader that shows it |
| `attachments` | optional: a record of a file name to its text or bytes, as `zip` takes one |

A message that isn't one has no value, so nothing is sent, as with any write built from a value
that didn't arrive: a field it doesn't know (`bcc`, `reply_to`), no recipient or more than 50, an
address that isn't one, a subject with a line break in it, an attachment that isn't a text or
bytes or whose name isn't a file's, or more than 10 MiB in all. Where the call writes its record
out, `holon check` finds the mistakes it can see first: a field it doesn't know, no `to`, `subject`
or `text`, a quoted recipient that isn't an address, more than 50.

## Who it may mail

The recipients are part of what the flow may do. Each one written as text in quotes is granted by
name; one the flow computes (from a request, a database) is granted as "an address it computes",
which the binding's `--to` holds:

```
it may:
  send mail through "ops" to support@acme.com
  send mail through "ops" to an address it computes, within its binding's
```

- A deploy is refused while the mail isn't bound, or while a recipient the text names is outside
  the binding's `--to`.
- Every recipient of a mail must be one the grant names (or the grant has "an address it
  computes"), or the mail isn't sent; and, as it goes, within the binding's `--to` as it is then,
  or it is given up, whole, and the deployment's timeline says so.
- No `Bcc` or `Reply-To`: both are what a mail pretending to be someone else uses.
- A message holding a secret, or computed from one, isn't sent.

## When it goes

Mail goes once its run is over: after the run's writes have landed and its record is on disk,
never from a run that failed. A mail sent from a run that waits (`await event(…)`) goes once the
run is over, after the event; a link to the event is mailed by the route that starts the run,
which knows its token (`started.waiting.approved`), as the approval-mail sample does.

- Each binding has a queue, sent in order, so a slow SMTP server holds up no run.
- A failure that may pass (no connection, a `4xx` answer) is tried again after a minute, then 5,
  then 30, then every hour for a day, with the same `Message-ID`, so a receiver can tell a mail
  sent twice. A refusal (`5xx`), or a day of failures, gives up.
- Each mail sent or given up is on the deployment's timeline: `mailed "Payout p1 approved" to
  sam@acme.com through ops`.
- Mail on its way when the server stops goes when it starts again, if its run's record is on
  disk. A run whose record a crash lost sent nothing.
- A run logged as `unrecorded` sends its mail all the same.

The message is the server's to write: `From` is the binding's, `Date` the run's clock, and the
`Message-ID` comes from the run, so `holon verify` writes the same message again.

## A link in a mail

The server doesn't know an address of its own the world reaches, so a flow writes the start of a
link it mails as text (`const site = "https://pay.acme.com"`). A link that does something, such as
approving a payout, opens a page whose button does it: a mail scanner (Outlook's, Gmail's) opens
every link in a mail it is handed, and must not approve anything. The page's route is
`{ log: "unrecorded" }`: its path holds the token, which anyone who can read the log could approve
with. The button's route, the one that `resumes`, keeps the token out of the log itself.

## Limits

50 recipients and 10 MiB a message, attachments included, and 100 mails a run. A run that sends
more fails, and none of its mail goes.

## What the run log keeps

Each message as the flow wrote it: its recipients, subject, text and HTML, and its attachments by
size and hash, as a written file is kept.
