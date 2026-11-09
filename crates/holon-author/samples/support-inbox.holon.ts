// Support inbox: customers' requests, read from a Microsoft 365 mailbox and answered from it.
//
// Each message that comes to support@acme.com opens a ticket, numbered, and its sender is answered
// with the number, through the same mail. One binding receives and sends, and the host logs in to
// both servers with an OAuth token it asks Microsoft for, since Microsoft 365 takes no password
// over IMAP or SMTP. Nothing in the flow says so: how the host logs in is the binding's business.
//
// Only a sender the mailbox's own server authenticated is answered: answering a forged From would
// mail whoever it names. A message that isn't authenticated is kept, unanswered, with its verdict.
//
// Before it works, an admin registers an app in Microsoft Entra that may read the mailbox over
// IMAP and send from it over SMTP, and binds the mail, each URL in a secret source:
//   HOLON_SECRET_SUPPORT_OAUTH=https://<app id>:<secret>@login.microsoftonline.com/<tenant>
//     /oauth2/v2.0/token?scope=https://outlook.office365.com/.default
//   HOLON_SECRET_SUPPORT_IMAP=imaps://support%40acme.com@outlook.office365.com
//   HOLON_SECRET_SUPPORT_SMTP=smtp://support%40acme.com@smtp.office365.com:587
//   holon mail add support --imap env:HOLON_SECRET_SUPPORT_IMAP --done Answered
//     --smtp env:HOLON_SECRET_SUPPORT_SMTP --from "Acme support <support@acme.com>" --to "*"
//     --oauth env:HOLON_SECRET_SUPPORT_OAUTH --verdict-from <the name its server writes>
//
// Try it: deploy it, then mail support@acme.com from an address whose domain checks out
//   curl http://127.0.0.1:8189/tickets                      T-1, and who asked
//   the sender is mailed "[T-1] …" back
import { flow, on, route } from "holon"
import state from "state"
import tickets from "collection:tickets"
import support from "mail:support"

flow("support-inbox", { port: 8189 })

// A request: a ticket, and an answer with its number, when its sender is authenticated.
on.mail(support, (m) => {
  if (m.authenticated) {
    const n = (state.next ?? 0) + 1
    state.next = n
    const id = `T-${n}`
    tickets[id] = { from: m.from, subject: m.subject, text: m.text, date: m.date }
    support.send({
      to: m.from,
      subject: `[${id}] ${m.subject}`,
      text: `We have your request, ${id}, and will answer it from this address.\n`,
    })
  } else {
    tickets[`unanswered:${m.message_id}`] = {
      from: m.from ?? m.name,
      subject: m.subject,
      verdict: m.verdict,
    }
  }
})

route.get("/tickets", () => tickets.list({ limit: 50 }))
