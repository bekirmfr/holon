// Remittance mail: customers' remittance advice, read from a mailbox and matched to invoices.
//
// Customers mail remittances@acme.com a CSV of the invoices they paid, a line each:
// invoice,amount. Each message is read as it comes and moved out of the folder once its run is
// over. Its lines are matched against the open invoices: one paid in full, by the customer it was
// sent to, is closed; any other line is kept to look at.
//
// A From line proves nothing: anyone can write billing@customer.example. m.authenticated is the
// mailbox's own server's word that the domain in From sent the message, so a message that isn't
// authenticated changes nothing, and is kept with what that server said.
//
// Before it works, an admin binds the mailbox, its URL and login in a secret source, and names its
// server, whose verdict on each sender is the one believed:
//   HOLON_SECRET_REMIT_IMAP=imaps://user:password@imap.example.com   (the server's environment)
//   holon mail add remit --imap env:HOLON_SECRET_REMIT_IMAP --done Processed
//     --verdict-from mx.acme.com
//
// Try it: deploy it, then
//   curl -X POST -d '{"id":"INV-1","amount":"100.00","customer":"customer.example"}'
//     http://127.0.0.1:8188/invoices
//   mail the mailbox a CSV with the line INV-1,100.00 from an address at customer.example
//   curl http://127.0.0.1:8188/invoices                     INV-1, paid
//   curl http://127.0.0.1:8188/remittances                  the message, and what it settled
import { csv_parse, decimal_compare, flow, on, route, status } from "holon"
import state from "state"
import remittances from "collection:remittances"
import remit from "mail:remit"

flow("remittance-mail", { port: 8188 })

// An invoice to be paid: its amount, and the domain of the customer it was sent to.
route.post("/invoices", (req) => {
  state.open = {
    ...(state.open ?? {}),
    [req.json.id]: { amount: req.json.amount, customer: req.json.customer },
  }
  return status(201, { opened: req.json.id })
})

route.get("/invoices", () => ({ open: state.open ?? {}, paid: state.paid ?? {} }))

// Whether a remittance's line settles an open invoice: one sent to the sender's domain, paid in
// full.
function settles(line, open, domain) {
  const invoice = open[line.invoice] ?? { customer: "", amount: "0" }
  return invoice.customer === domain && decimal_compare(invoice.amount, line.amount) === 0
}

// A remittance: each line of each CSV attached, matched against the open invoices.
on.mail(remit, (m) => {
  if (!m.authenticated) {
    remittances[m.message_id] = {
      from: m.from ?? m.name,
      subject: m.subject,
      refused: "not authenticated",
      verdict: m.verdict,
    }
  } else {
    const domain = m.from.split("@")[1].toLowerCase()
    const open = state.open ?? {}
    const lines = Object.keys(m.attachments)
      .filter((name) => name.toLowerCase().endsWith(".csv"))
      .flatMap((name) => csv_parse(m.attachments[name]))
    const settled = lines.filter((l) => settles(l, open, domain)).map((l) => l.invoice)
    state.open = Object.fromEntries(Object.entries(open).filter((e) => !settled.includes(e[0])))
    state.paid = {
      ...(state.paid ?? {}),
      ...Object.fromEntries(
        settled.map((invoice) => [invoice, { by: m.from, message: m.message_id, date: m.date }]),
      ),
    }
    remittances[m.message_id] = {
      from: m.from,
      subject: m.subject,
      settled,
      unmatched: lines.filter((l) => !settles(l, open, domain)),
    }
  }
})

route.get("/remittances", () => remittances.list({ limit: 50 }))
