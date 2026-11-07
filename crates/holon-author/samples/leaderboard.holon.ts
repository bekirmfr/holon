// Leaderboard: a list's methods.
//
// Post a round's scores, each a player and their points, and get the board JavaScript's list
// methods make of it: ranked by points with toSorted (a tie keeps the order it came in), the top
// three with their places from map's index, each player once with new Set, the first to reach
// 100 with find, whether anyone scored nothing with some, and the total with reduce. POST
// /board/:player answers one player's points (filter), their best (reduce without a start) and
// the place of their best score (findIndex in the ranking). No list changes in place: toSorted
// gives a new one, and xs.sort() on a list something else holds is refused.
//
// Try it: deploy it, then
//   curl -X POST -d '{"scores":[{"player":"ada","points":90},{"player":"bo","points":120},{"player":"cy","points":0},{"player":"ada","points":130}]}' http://127.0.0.1:8174/board
//     {"any_zero":true,"first_to_100":"bo","players":["ada","bo","cy"],"top":["1. ada 130","2. bo 120","3. ada 90"],"total":340}
//   the same body to http://127.0.0.1:8174/board/ada
//     {"best":130,"place":1,"player":"ada","points":[90,130],"rounds":2}
//   the same body to http://127.0.0.1:8174/board/dee    404
//   a body without scores to /board    400
import { flow, length, route, status } from "holon"

flow("leaderboard", { port: 8174 })

// The board: the top three with their places, the players, the first to 100, a zero, the total.
route.post("/board", (req) => {
  const scores = req.json.scores
  const top = scores.toSorted((a, b) => b.points - a.points).slice(0, 3)
  const board = {
    top: top.map((s, i) => `${i + 1}. ${s.player} ${s.points}`),
    players: [...new Set(scores.map((s) => s.player))].toSorted(),
    first_to_100: scores.find((s) => s.points >= 100)?.player ?? "nobody",
    any_zero: scores.some((s) => s.points === 0),
    total: scores.reduce((sum, s) => sum + s.points, 0),
  }
  return board ?? status(400)
})

// One player's points in the order posted, their best, and where it ranks; 404 for no such player.
route.post("/board/:player", (req) => {
  const scores = req.json.scores
  const player = req.params.player
  const points = scores.filter((s) => s.player === player).map((s) => s.points)
  const ranked = scores.toSorted((a, b) => b.points - a.points)
  const card = {
    player,
    points,
    rounds: length(points),
    best: points.reduce((a, b) => Math.max(a, b)),
    place: ranked.findIndex((s) => s.player === player) + 1,
  }
  return card ?? status(404)
})
