// Card check: a card's CVC goes to the payment processor and is never kept, however short it is.
//
// A shop posts a card, { number, cvc, amount }. The route sends it to the payment processor
// (httpbin.dev stands in, and echoes what it got) and answers the card's last four digits and what
// the processor said. The flow taints the number, found once and then removed by its text wherever
// its runs have it, and never keeps the CVC: its value is replaced where it stands, under its key,
// whatever its length, in the request, the call's request and answer, every value the run made. A
// CVC of 737 is too short to remove by its text (it is in an amount of 1737 too), so it is guarded
// by its place, and the amount stays as it is. A write or a send that holds the CVC under its key
// (a state, a collection, a file, a mail, another flow's action) is refused; the request to the
// processor is sent. For now a flow that never keeps a key can't wait, and a CVC read out of its key
// into a value of its own (const c = req.json.cvc) is followed no further.
//
// Try it: deploy it, then
//   curl -d '{"number":"4111111111111111","cvc":"737","amount":1737}' http://127.0.0.1:8202/check
//     answers {"last4":"1111","processor":200}
//   holon runs card-check --run 0
//     shows ‹never_keep:cvc› where the CVC was, ‹taint:number› where the number was, and the
//     amount 1737 as it was sent
import { flow, route } from "holon"
import httpbin from "host:httpbin.dev"

flow("card-check", { port: 8202, taint: ["number"], never_keep: ["cvc"] })

// Sends the card to the processor, and answers its last four digits and what the processor said.
route.post("/check", async (req) => {
  const checked = await httpbin.post("/anything", {
    headers: { "content-type": "application/json" },
    body: req.body,
  })
  return { last4: req.json.number.slice(-4), processor: checked.status }
})
