// Issue links: every link to one issue, built from its path.
//
// One route builds each link to a GitHub issue from the owner, repo and number in its path:
// thirteen interpolated parts, the same node many times over, each told apart by where its value
// goes. It calls nothing; the links are only text.
//
// Try it: deploy it, then
//   curl http://127.0.0.1:8111/repos/bekirmfr/holon/issues/7
// answers the issue's page, its API address, its comments and events, the repo's clone address,
// and the short form bekirmfr/holon#7.
import { flow, route } from "holon"

flow("issue-links", { port: 8111 })

// Builds each link from the owner, repo and number, and answers them as one record.
route.get("/repos/:owner/:repo/issues/:number", (req) => {
  const owner = req.params.owner
  const repo = req.params.repo
  const number = req.params.number
  const page = `https://github.com/${owner}/${repo}/issues/${number}`
  const api = `https://api.github.com/repos/${owner}/${repo}/issues/${number}`
  const comments = `${api}/comments`
  const events = `${api}/events`
  const clone = `git@github.com:${owner}/${repo}.git`
  const short = `${owner}/${repo}#${number}`
  return { page, api, comments, events, clone, short }
})
