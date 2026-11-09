// Proposal: a version an agent proposes, for a person to approve.
//
// It answers with a fresh id from another API: the flow names the host it calls, so deploying it
// asks for permission to reach that host. An agent that deploys it through holon mcp (its deploy
// tool, with a why) doesn't deploy it: the server keeps it as a proposal, which waits in the
// editor's Approvals with the agent's why and note and what it asks for. A person approves it,
// which deploys it, or declines it with a reason the agent reads back (holon mcp's deployments).
//
// Try it: connect an agent (holon mcp --server http://127.0.0.1:7420), ask it to deploy this
// with a why, then review it under Approvals and approve it, and
//   curl http://127.0.0.1:8184/id
// answers an id httpbin made.
import { flow, route } from "holon"
import httpbin from "host:httpbin.org"

flow("proposal", { port: 8184 })

// Asks httpbin for a uuid, and answers it as the id.
route.get("/id", async () => {
  const made = await httpbin.get("/uuid")
  return { id: JSON.parse(made.body).uuid }
})
