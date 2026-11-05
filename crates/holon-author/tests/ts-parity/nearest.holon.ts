// Nearest word (samples/nearest.holon) in TypeScript's syntax: the same unit.
import { flow, js, route, status } from "holon"
import { now } from "holon:clock"
import words from "collection:words"

flow("nearest", { port: 8117 })

const closest = js(function closest(word, candidates) {
  const dist = (a, b) => {
    const d = Array.from({ length: a.length + 1 }, (_, i) => [i]);
    for (let j = 1; j <= b.length; j++) d[0][j] = j;
    for (let i = 1; i <= a.length; i++)
      for (let j = 1; j <= b.length; j++)
        d[i][j] = Math.min(d[i - 1][j] + 1, d[i][j - 1] + 1, d[i - 1][j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1));
    return d[a.length][b.length];
  };
  let found = { best: undefined, distance: Infinity };
  for (const c of candidates) { const k = dist(word, c); if (k < found.distance) found = { best: c, distance: k }; }
  return found;
}, { outputs: ["best", "distance"] })

route.post("/words", (req) => {
  words[req.body] = { added_at: now }
  return status(201, { added: req.body })
})

route.get("/nearest/:word", (req) => {
  const found = closest(req.params.word, Object.keys(words.list()))
  return { query: req.params.word, best: found.best, distance: found.distance }
})
