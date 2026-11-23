// fmt's input: a flow written any which way (lang/format.rs's tests lay it out).
import { flow, route, action, event, status, respond, js, every } from "holon"
import state from "state"
import notes from "collection:notes"
import { charge } from "flow:billing"

flow("messy", { port: 8000 })

// a function
function fee(total: number, vip) {
  if (total > 100 && vip === true) {
    return 0
  } else if (total > 50) return 1
  else {
    return 5
  }
}
route.post(
  "/order",
  async (req) => {
    const { items, who } = req.json
    let total = req.json.base
    switch (who) {
      case "vip":
        total = total * 2
        break
      // the rest
      default:
        total = total + 2
    }
    if (
      req.json.coupon === "X-COUPON-ONE" ||
      req.json.coupon === "X-COUPON-TWO" ||
      req.json.coupon === "X-THREE"
    )
      total = total - 1
    const paid = await charge({
      total: total + fee(total, who === "vip"),
      items: items,
      who: who,
      note: "a long enough note to break",
    })
    return (
      (paid.answer ?? 0) > 0 &&
      paid.run > 0 &&
      paid.message === "none" &&
      total > 0 &&
      who !== "nobody"
    )
  },
  { log: "lazy" },
)
export const pay = action(async (order) => {
  const paid = await event("paid", { expires: "1d" })
  return paid.method
})
every("1m", () => {
  state.ticks = (state.ticks ?? 0) + 1
})
// the page
const page = js(function html(x) {
  return `<style>a{color:#777}</style>${x}` // #not a holon comment
})
const shown = js(function html(count) {
  return `<p>${count}</p>`
})
const loose = js(function alike(a, b) {
  var c = a
  return c == b
})
route.get("/p", (req) => {
  return page(req.query.x ?? "y")
  // answered
})
route.get("/shown", () =>
  respond(shown(state.ticks ?? 0), {
    headers: { "content-type": "text/html; charset=utf-8" },
    status: 200,
  }),
)
route.get("/loose", (req) => loose(req.query.a, req.query.b))
route.get("/slug", (req) => {
  const slug = req.query.title
    .toLowerCase()
    .split(" ")
    .map((word) => word.toUpperCase())
    .join("-")
    .split("--")
    .join("-")
  return slug
})
route.get("/short", (req) => req.query.title.toLowerCase().split(" "))
// The count.
// Two lines.
route.get("/count", () => state.count ?? 0) // on its line

route.post("/count", (req) => {
  // before
  state.count = req.json.n // after

  /* block */ return "ok"
  // last
})
route.post("/c", (req) => {
  state.count = add(req.json.n, /* two */ 2)
  return "ok"
})
function add(a, b) {
  return a + b
}
route.post("/v", (req) => {
  const a = req.json.a

  let b = 1
  if (a === 2) b = 3
  return a !== b
})
route.get("/kept", () => ({
  a: 1,
  b: 2,
}))
route.get("/joined", () => ({ a: 1, b: 2 }))
route.get("/quoted", () => ({ "$in": [1], b: 2 }))
