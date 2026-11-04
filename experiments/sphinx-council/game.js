// The Sphinx's council: a harness around the chess sample's own `rules` holon, read at startup
// from crates/holon-author/samples/chess.holon (never a copy), so every move goes through the
// same legality checks the flow uses.
//
//   node game.js new                 start a game (state.json)
//   node game.js player <move>       White's move (SAN or coordinates); an illegal one exits 1
//   node game.js sphinx <SAN>        Black's move, held to the contract: a move not among the
//                                    legal moves is "broken", and the rules' fallback is played
//   node game.js show                the board, the game, and (Black to move) each piece's moves
const fs = require("fs");
const path = require("path");

const HOLON = path.join(__dirname, "..", "..", "crates", "holon-author", "samples", "chess.holon");
const lines = fs.readFileSync(HOLON, "utf8").split("\n");
const start = lines.findIndex((l) => l.startsWith("holon rules(board, move) -> next js"));
const end = lines.findIndex((l, i) => i > start && l === "}");
if (start < 0 || end < 0) throw new Error("couldn't find the rules holon in chess.holon");
const rules = new Function("board", "move", lines.slice(start + 1, end).join("\n"));

const STATE = path.join(__dirname, "state.json");
const load = () => JSON.parse(fs.readFileSync(STATE, "utf8"));
const save = (g) => fs.writeFileSync(STATE, JSON.stringify(g, null, 2) + "\n");

const NAMES = { p: "pawn", n: "knight", b: "bishop", r: "rook", q: "queen", k: "king" };
const squares = (fen) => {
  const out = {};
  fen.split(" ")[0].split("/").forEach((row, r) => {
    let f = 0;
    for (const c of row) {
      if (c >= "1" && c <= "8") f += Number(c);
      else { out["abcdefgh"[f] + (8 - r)] = c; f++; }
    }
  });
  return out;
};
const picture = (fen) => {
  const sq = squares(fen), out = [];
  for (let r = 8; r >= 1; r--) {
    let line = r + " ";
    for (const f of "abcdefgh") line += " " + (sq[f + r] || ".");
    out.push(line);
  }
  out.push("   a b c d e f g h");
  return out.join("\n");
};
// Black's pieces with at least one legal move, each with its moves (SAN).
const pieces = (g) => {
  const sq = squares(g.fen), by = {};
  for (const l of g.legal) {
    const from = l.uci.slice(0, 2), c = sq[from];
    if (!c || c !== c.toLowerCase()) continue;
    (by[from] = by[from] || { square: from, piece: NAMES[c], moves: [] }).moves.push(l.san);
  }
  return Object.values(by);
};
const show = (g) => {
  console.log(picture(g.fen));
  console.log(`FEN: ${g.fen}`);
  console.log(`PGN: ${g.pgn}`);
  console.log(`Status: ${g.status}${g.winner ? ` (winner: ${g.winner})` : ""}. Turn: ${g.turn === "w" ? "White" : "Black"}. Material: White ${g.material.white}, Black ${g.material.black}.`);
  const last = (g.turns || [])[(g.turns || []).length - 1];
  if (last) console.log(`The Sphinx's last turn: ${JSON.stringify(last)}`);
  if (g.turn === "b" && g.status === "playing") {
    console.log(`Legal moves: ${g.legal.map((l) => l.san).join(", ")}`);
    console.log("Pieces with legal moves:");
    for (const p of pieces(g)) console.log(`- ${p.piece} ${p.square}: ${p.moves.join(", ")}`);
    console.log(`The rules' fallback: ${g.fallback && g.fallback.san}`);
  }
};

const [cmd, move] = process.argv.slice(2);
if (cmd === "new") {
  const g = rules(null, "");
  g.turns = [];
  save(g);
  show(g);
} else if (cmd === "show") {
  show(load());
} else if (cmd === "player") {
  const g = load();
  if (g.turn !== "w") { console.error("it is the Sphinx's move"); process.exit(1); }
  try { const n = rules(g, move); save(n); show(n); }
  catch (e) { console.error("400: " + e.message); process.exit(1); }
} else if (cmd === "sphinx") {
  const g = load();
  if (g.turn !== "b" || g.status !== "playing") { console.error("it isn't the Sphinx's move"); process.exit(1); }
  const proposed = (move || "").trim();
  const hit = g.legal.find((l) => l.san === proposed.replace(/[+#!?]+$/, ""));
  const played = hit || g.fallback;
  const turn = { ply: g.moves.length + 1, proposed, kept: hit ? "kept" : "broken", played: played.san };
  const n = rules(g, played.uci);
  n.turns = [...(g.turns || []), turn];
  save(n);
  show(n);
} else {
  console.error("usage: node game.js new | player <move> | sphinx <SAN> | show");
  process.exit(2);
}
