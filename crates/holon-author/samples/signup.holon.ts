// Signup: a try keeps what it did above the line that failed, as JavaScript's does.
//
// Takes a signup as JSON. Inside the try, the attempt is counted first; then the body is read, the
// email checked, and the account kept, each step noted in step. A line that fails ends the block
// there: what was done above it stays done (the count), what is below it isn't (the account), and
// the catch answers the last step reached, which step holds as JavaScript leaves it.
//
// Try it: deploy it, then
//   curl -X POST -d '{"email":"ada@example.com","name":"Ada"}' \
//     http://127.0.0.1:8209/signup                        201 {"step":"kept"}
//   curl -X POST -d 'not json' http://127.0.0.1:8209/signup
//                                                       400 {"failed_after":"counted"}
//   curl -X POST -d '{"email":"ada","name":"Ada"}' http://127.0.0.1:8209/signup
//                                                       400 {"failed_after":"read"}
//   curl http://127.0.0.1:8209/stats                    {"tried":3}, though one account was kept
//   curl http://127.0.0.1:8209/accounts                 Ada's, alone
import { flow, route, status } from "holon"
import accounts from "collection:accounts"
import state from "state"

flow("signup", { port: 8209 })

route.post("/signup", (req) => {
  let step = "counted"
  try {
    state.tried = (state.tried ?? 0) + 1
    const body = JSON.parse(req.body)
    step = "read"
    if (body.email.indexOf("@") < 1) throw new Error("an email has an @")
    step = "checked"
    accounts[body.email] = { name: body.name }
    step = "kept"
  } catch {
    return status(400, { failed_after: step })
  }
  return status(201, { step })
})

// How many signups were tried, each counted whatever became of it.
route.get("/stats", () => ({ tried: state.tried ?? 0 }))

// The accounts kept.
route.get("/accounts", () => accounts.list())
