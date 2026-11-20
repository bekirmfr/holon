// Loyalty points: a run says which writes it held and didn't make, and why.
//
// A purchase adds points to its member, a tenth of the amount rounded down, and answers
// "recorded" whatever became of the write, as a fire-and-forget endpoint does. A purchase for a
// member nobody added, or one whose points can't be worked out, makes no write. The run's record
// says so: each write it held and didn't make, and where that began (a read that found nothing, a
// field the request lacked, a step that failed). `holon runs loyalty --run N` and the editor's run
// view show it on the line that would have made the write. A write the flow's own `if` left out
// isn't said: that was the flow's choice.
//
// Try it: deploy it (holon deploy loyalty.holon.ts), then
//   curl -X POST http://127.0.0.1:8203/members/ada -d '{"name":"Ada"}'
//   curl -X POST http://127.0.0.1:8203/purchases -d '{"member":"ada","amount":120}'
//   curl http://127.0.0.1:8203/members/ada
//   curl -X POST http://127.0.0.1:8203/purchases -d '{"member":"bob","amount":50}'
//   holon runs loyalty --run 3
import { flow, route } from "holon"
import members from "collection:members"

flow("loyalty", { port: 8203 })

// A tenth of the amount, rounded down. An amount that isn't more than 0 is refused.
function earned(amount) {
  if (amount <= 0) throw new Error("an amount is more than 0")
  return Math.floor(amount / 10)
}

route.post("/members/:id", (req) => {
  members[req.params.id] = { name: req.json.name, points: 0 }
  return "added"
})

// Adds the purchase's points to its member, and answers "recorded" whatever the write did.
route.post("/purchases", (req) => {
  const member = members[req.json.member]
  members[req.json.member] = { ...member, points: member.points + earned(req.json.amount) }
  return "recorded"
})

route.get("/members/:id", (req) => members[req.params.id] ?? "no such member")
