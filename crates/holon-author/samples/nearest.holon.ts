// Nearest word: a js node.
//
// Keeps a dictionary of words and answers the closest one to a query, by edit distance. The one
// dense computation is a js node: JavaScript run as a module with no imports, under a fuel
// budget. The flow around it stores, routes and answers.
//
// Try it: deploy it, then add kitten (and the same way sitting and mitten):
//   curl -X POST --data-binary kitten http://127.0.0.1:8117/words
//   curl http://127.0.0.1:8117/nearest/kittne    best "kitten", distance 2
// Before any word is added, a query answers 404.
import { flow, js, route, status } from "holon"
import words from "collection:words"
import { now } from "holon:clock"

flow("nearest", { port: 8117 })

// Finds the candidate closest to a word, and how many edits away it is.
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

// Adds the posted text to the dictionary.
route.post("/words", (req) => {
  words[req.body] = { added_at: now }
  return status(201, { added: req.body })
})

// Answers the dictionary word closest to the one in the path, and its distance.
route.get("/nearest/:word", (req) => {
  const found = closest(req.params.word, Object.keys(words.list()))
  return { query: req.params.word, best: found.best, distance: found.distance }
})
