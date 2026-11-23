// Sea battle: a game server that is also the witness of its players.
//
// Two captains hide a fleet each on an 8 x 8 sea and take turns firing. The server never sees a
// fleet. Each browser places its own ships and sends only a seal: the sha256 of the board and a
// random salt, kept in the browser. When a shot lands, the defender's own page answers "hit" or
// "miss", and the server writes the answer down beside the shot. It can't tell whether the answer
// is true, and it doesn't need to: it is the witness. When a fleet is sunk (or a captain has fired
// at every square), both pages reveal their board and salt. The flow checks each board against the
// seal it gave before the first shot, checks the fleet is legal (ships of 4, 3, 3 and 2, each in a
// straight line), and checks every answer the captain gave against that board. A captain who moved
// a ship, or answered "miss" to a hit, is caught and loses, and the page shows the squares they
// lied about. Tick "lie about hits" on the page to watch it happen. A captain who goes silent to
// avoid the reveal (a shot left unanswered, a turn not taken, a board not shown) loses after two
// minutes: the other claims the game, and wins it if their own board is honest.
//
// The players can't cheat the server, and the server can't cheat the players either. What it may
// do is all on its approval: answer requests, keep its games, and send each move to whoever is
// watching. It calls nowhere and holds no secret. Every shot, answer and reveal is a run in its
// log, so holon export sea-battle, then holon verify, replays the game: the dice that named it,
// every seal it checked and every verdict it reached. A game is two runs a shot, so the log keeps
// 5000 runs, about twenty games, where a flow keeps 500 unless it says.
//
// Try it: deploy it, open http://127.0.0.1:8204/ and start a game, then open the link it gives in
// a second browser (or a private window) to join. Or with curl, two seals made up for the
// purpose:
//   curl -s -H 'content-type: application/json' \
//     -d '{"name":"Ada","commit":"'$(printf '%064d' 0)'"}' http://127.0.0.1:8204/games
// answers {"game": "…", "seat": "a", "key": "…"}, and GET /games/<game> the game as anyone sees it.
import {
  flow,
  js,
  length,
  respond,
  route,
  secure_equals,
  sha256,
  slice,
  status,
  text_length,
} from "holon"
import games from "collection:games"
import moves from "channel:moves"
import { now } from "holon:clock"
import { random } from "holon:random"

flow("sea-battle", { port: 8204, keep: { runs: 5000 } })

// The rules, run when a captain reveals: is the board a legal fleet, and which of the answers the
// captain gave to the shots at it were not what the board says.
const judge = js(function rules(board, said) {
  const fleet = [4, 3, 3, 2]
  const cells = String(board).split("")
  let wrong =
    cells.length !== 64 || cells.some((c) => !"01234".includes(c))
      ? "the board is not 64 squares of water and ships"
      : ""
  fleet.forEach((len, i) => {
    const at = []
    cells.forEach((c, k) => {
      if (c === String(i + 1)) at.push(k)
    })
    const across = at.every((k, j) => j === 0 || (k === at[j - 1] + 1 && k % 8 !== 0))
    const down = at.every((k, j) => j === 0 || k === at[j - 1] + 8)
    if (wrong === "" && (at.length !== len || !(across || down))) {
      wrong = "ship " + (i + 1) + " is not a straight line of " + len + " squares"
    }
  })
  const lies =
    wrong === ""
      ? said.filter((s) => (cells[s.cell] !== "0") !== (s.said === "hit")).map((s) => s.cell)
      : []
  return { fleet: wrong, lies }
})

// A captain's seat as the server keeps it: the key that proves who is calling is kept apart from
// everything else, which anyone may see.
function seat(name, commit, key) {
  const kept = {
    name,
    commit,
    key,
    revealed: false,
    board: "",
    salt: "",
    sealed: false,
    fleet: "",
    lies: [],
    honest: false,
  }
  return kept
}

// A seat as anyone sees it: everything but its key.
function shown_seat(p) {
  const shown = {
    name: p.name,
    commit: p.commit,
    revealed: p.revealed,
    board: p.board,
    salt: p.salt,
    sealed: p.sealed,
    fleet: p.fleet,
    lies: p.lies,
    honest: p.honest,
  }
  return shown
}

// A game as anyone sees it.
function view(g) {
  const shown = {
    id: g.id,
    phase: g.phase,
    turn: g.turn,
    shots: g.shots,
    winner: g.winner,
    why: g.why,
    since: g.since,
    a: shown_seat(g.a),
    b: shown_seat(g.b),
  }
  return shown
}

// Which seat a key is: "a", "b", or "" for anyone else.
function seat_of(g, key) {
  const which = secure_equals(key, g.a.key) ? "a" : secure_equals(key, g.b.key) ? "b" : ""
  return which
}

// What a captain did wrong, in a sentence, or "" when nothing.
function charge(p) {
  const said = !p.sealed
    ? `${p.name} revealed a board that is not the one sealed before the first shot`
    : p.fleet !== ""
      ? `${p.name} sealed a fleet that breaks the rules: ${p.fleet}`
      : length(p.lies) > 0
        ? `${p.name} lied about ${length(p.lies)} shot${length(p.lies) === 1 ? "" : "s"}`
        : ""
  return said
}

// A captain's seat once their board and salt are shown: checked against the seal they gave, the
// fleet against the rules, and the answers they gave (`said`, the shots at them) against the board.
function opened(p, board, salt, said) {
  const verdict = judge(board, said)
  const sealed = sha256(`${board}|${salt}`) === p.commit
  const shown = {
    ...p,
    revealed: true,
    board,
    salt,
    sealed,
    fleet: verdict.fleet,
    lies: verdict.lies,
    honest: sealed && verdict.fleet === "" && length(verdict.lies) === 0,
  }
  return shown
}

// The verdict, once both boards are revealed. Two honest captains: the one who sank the other's
// fleet wins. One honest captain: they win, whatever the score. Two cheats: nobody does.
function finish(g) {
  const sank_a = g.shots.filter((s) => s.by === "a" && s.said === "hit").length === 12
  const winner =
    g.a.honest && g.b.honest ? (sank_a ? "a" : "b") : g.a.honest ? "a" : g.b.honest ? "b" : ""
  const why =
    g.a.honest && g.b.honest
      ? `${g[winner].name} sank the whole fleet, and both boards matched their seals and every answer.`
      : winner !== ""
        ? `${charge(winner === "a" ? g.b : g.a)}, so ${g[winner].name} wins.`
        : `${charge(g.a)}, and ${charge(g.b)}: nobody wins.`
  const done = { ...g, phase: "over", winner, why }
  return done
}

// The page: a fleet placed and sealed in the browser, the two seas, and the verdict. Its board and
// salt never leave the browser until the reveal.
const page = `<!doctype html>
<html lang="en">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Sea battle</title>
<style>
:root{color-scheme:dark;--sea:#0b2740;--deep:#06182a;--ink:#dbe9f2;--dim:#86a0b4;--ship:#9fb3c4;--hit:#ff6b4a;--miss:#3d6c8f;--ok:#6fd39a;--bad:#ff8a73;--line:#ffffff14}
*{box-sizing:border-box}
body{margin:0;background:var(--deep);color:var(--ink);font:15px/1.5 system-ui,sans-serif}
main{max-width:860px;margin:0 auto;padding:24px 16px 48px}
h1{margin:0 0 4px;font-size:26px;letter-spacing:.5px}
p{margin:6px 0}
.dim{color:var(--dim)}
.seas{display:flex;flex-wrap:wrap;gap:24px;margin:20px 0}
.sea{flex:1 1 300px;max-width:400px}
.sea h2{font-size:15px;margin:0 0 8px;font-weight:600}
.grid{display:grid;grid-template-columns:repeat(8,1fr);gap:3px;background:var(--line);padding:3px;border-radius:8px}
.grid button{aspect-ratio:1;border:0;border-radius:4px;background:var(--sea);color:var(--ink);font:inherit;font-size:18px;padding:0;cursor:default}
.grid button.ship{background:var(--ship)}
.grid button.miss::after{content:"";display:block;width:26%;aspect-ratio:1;margin:auto;border-radius:50%;background:var(--miss)}
.grid button.hit{background:var(--hit)}
.grid button.wait{outline:2px dashed var(--ink);outline-offset:-4px}
.grid button.lie{box-shadow:inset 0 0 0 3px #ffd54a}
.grid.aim button:not(.hit):not(.miss):not(.wait){cursor:crosshair}
.grid.aim button:not(.hit):not(.miss):not(.wait):hover{background:#174a70}
.seal{font:12px ui-monospace,monospace;color:var(--dim);word-break:break-all}
.bar{display:flex;flex-wrap:wrap;gap:8px;align-items:center;margin:12px 0}
input[type=text]{background:var(--sea);border:1px solid var(--line);color:var(--ink);border-radius:6px;padding:8px 10px;font:inherit}
button.go{background:#2f7fb8;color:#fff;border:0;border-radius:6px;padding:8px 14px;font:inherit;cursor:pointer}
button.go.quiet{background:transparent;border:1px solid var(--line);color:var(--ink)}
#status{font-size:17px;margin-top:14px;min-height:1.5em}
#verdict{display:none;margin:18px 0;padding:14px 16px;border-radius:10px;background:#0f3150}
#verdict.on{display:block}
.ok{color:var(--ok)}.bad{color:var(--bad)}
#link{font:13px ui-monospace,monospace;word-break:break-all}
</style>
<main>
<h1>Sea battle</h1>
<p class="dim">The server never sees your fleet. It keeps a seal of it, writes down every answer you give, and checks them all when the boards are revealed.</p>

<section id="lobby">
<div class="bar"><input type="text" id="name" placeholder="Your name" maxlength="24"><button class="go quiet" id="shuffle">Shuffle fleet</button><button class="go" id="start">Start a game</button></div>
</section>

<p id="status"></p>
<p id="share" style="display:none">Send this link to your opponent: <span id="link"></span></p>
<div class="bar" id="cheat" style="display:none"><label><input type="checkbox" id="lie"> Lie about hits (answer "miss" when they hit me)</label></div>

<div class="seas">
<div class="sea"><h2 id="leftname">Your waters</h2><div class="grid" id="left"></div><p class="seal" id="leftseal"></p></div>
<div class="sea"><h2 id="rightname">Their waters</h2><div class="grid" id="right"></div><p class="seal" id="rightseal"></p></div>
</div>

<p id="claimbar" style="display:none"><button class="go" id="claim"></button></p>
<div id="verdict"></div>
</main>
<script>
var FLEET = [4, 3, 3, 2];
var $ = function (id) { return document.getElementById(id); };
var memory = {};
var load = function (id) { try { return JSON.parse(localStorage.getItem("sea-battle:" + id)) || memory[id] || null; } catch (e) { return memory[id] || null; } };
var save = function (id, v) { memory[id] = v; try { localStorage.setItem("sea-battle:" + id, JSON.stringify(v)); } catch (e) {} };

var hex = function (bytes) { return Array.from(bytes, function (b) { return (b + 256).toString(16).slice(1); }).join(""); };
var sha = function (text) {
  if (window.crypto && crypto.subtle) return crypto.subtle.digest("SHA-256", new TextEncoder().encode(text)).then(function (d) { return hex(new Uint8Array(d)); });
  return Promise.resolve(own(text));
};
// sha256 for a page served over plain http on a network, where the browser offers none.
var own = function (text) {
  var bytes = new TextEncoder().encode(text), primes = [];
  for (var c = 2; primes.length < 64; c++) if (primes.every(function (p) { return c % p; })) primes.push(c);
  var frac = function (x) { return x - Math.floor(x); };
  var K = primes.map(function (p) { return Math.floor(frac(Math.cbrt(p)) * 4294967296); });
  var h = primes.slice(0, 8).map(function (p) { return Math.floor(frac(Math.sqrt(p)) * 4294967296); });
  var size = ((bytes.length + 9 + 63) >> 6) << 6, buf = new Uint8Array(size);
  buf.set(bytes); buf[bytes.length] = 128;
  var view = new DataView(buf.buffer);
  view.setUint32(size - 8, Math.floor(bytes.length / 536870912)); view.setUint32(size - 4, (bytes.length << 3) >>> 0);
  var w = new Array(64), rotr = function (x, n) { return (x >>> n) | (x << (32 - n)); };
  for (var off = 0; off < size; off += 64) {
    for (var i = 0; i < 16; i++) w[i] = view.getUint32(off + i * 4);
    for (i = 16; i < 64; i++) w[i] = (w[i - 16] + (rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >>> 3)) + w[i - 7] + (rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >>> 10))) >>> 0;
    var a = h[0], b = h[1], cc = h[2], d = h[3], e = h[4], f = h[5], g = h[6], hh = h[7];
    for (i = 0; i < 64; i++) {
      var t1 = (hh + (rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25)) + ((e & f) ^ (~e & g)) + K[i] + w[i]) >>> 0;
      var t2 = ((rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22)) + ((a & b) ^ (a & cc) ^ (b & cc))) >>> 0;
      hh = g; g = f; f = e; e = (d + t1) >>> 0; d = cc; cc = b; b = a; a = (t1 + t2) >>> 0;
    }
    [a, b, cc, d, e, f, g, hh].forEach(function (v, i) { h[i] = (h[i] + v) >>> 0; });
  }
  return h.map(function (v) { return (v + 4294967296).toString(16).slice(1); }).join("");
};

// A fleet placed at random: ships 1 to 4, each a straight line, none overlapping.
var shuffle = function () {
  var cells = new Array(64).fill("0");
  FLEET.forEach(function (len, i) {
    for (;;) {
      var across = Math.random() < 0.5;
      var r = Math.floor(Math.random() * (across ? 8 : 9 - len)), c = Math.floor(Math.random() * (across ? 9 - len : 8));
      var at = [];
      for (var k = 0; k < len; k++) at.push((r + (across ? 0 : k)) * 8 + c + (across ? k : 0));
      if (at.every(function (x) { return cells[x] === "0"; })) { at.forEach(function (x) { cells[x] = String(i + 1); }); return; }
    }
  });
  return cells.join("");
};
var salt = function () { var b = new Uint8Array(16); crypto.getRandomValues(b); return hex(b); };

var grids = {};
var makeGrid = function (id, onClick) {
  var el = $(id), cells = [];
  for (var k = 0; k < 64; k++) {
    var btn = document.createElement("button");
    btn.setAttribute("aria-label", "ABCDEFGH"[k % 8] + (Math.floor(k / 8) + 1));
    (function (k) { btn.onclick = function () { if (onClick) onClick(k); }; })(k);
    el.appendChild(btn); cells.push(btn);
  }
  grids[id] = cells;
};
var paint = function (id, cls) { grids[id].forEach(function (b, k) { b.className = cls(k); }); };

var post = function (path, body) {
  return fetch(path, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) })
    .then(function (r) { return r.json().catch(function () { return {}; }).then(function (j) { return { ok: r.ok, body: j }; }); });
};

var gameId = location.hash.slice(1), game = null, mine = null, draft = { board: shuffle(), salt: salt() }, busy = false;

var marks = function (shots, k) {
  var s = shots.find(function (s) { return s.cell === k; });
  return s ? (s.said === "" ? " wait" : " " + s.said) : "";
};
var render = function () {
  var g = game;
  if (!g) {
    paint("left", function (k) { return draft.board[k] !== "0" ? "ship" : ""; });
    paint("right", function () { return ""; });
    $("leftname").textContent = "Your fleet (only you can see it)";
    $("rightname").textContent = "Their waters";
    return;
  }
  var me = mine && mine.seat, them = me === "b" ? "a" : "b";
  var left = me || "a", right = me ? them : "b";
  var atLeft = g.shots.filter(function (s) { return s.by !== left; });
  var atRight = g.shots.filter(function (s) { return s.by === left; });
  var boardOf = function (seat) { return seat === me ? mine.board : g[seat].board; };
  var lies = function (seat) { return g[seat].lies || []; };
  var cls = function (seat, shots) {
    return function (k) {
      var b = boardOf(seat);
      return (b && b[k] !== "0" ? "ship" : "") + marks(shots, k) + (lies(seat).indexOf(k) >= 0 ? " lie" : "");
    };
  };
  paint("left", cls(left, atLeft));
  paint("right", cls(right, atRight));
  $("leftname").textContent = (me ? "Your waters" : (g.a.name || "Captain A") + "'s waters");
  $("rightname").textContent = (g[right].name || "Waiting for a captain") + "'s waters";
  $("leftseal").textContent = "seal " + (g[left].commit || "...");
  $("rightseal").textContent = "seal " + (g[right].commit || "...");
  var aim = me && g.phase === "play" && g.turn === me && !g.shots.some(function (s) { return s.said === ""; });
  $("right").className = "grid" + (aim ? " aim" : "");
  $("lobby").style.display = "none";
  $("share").style.display = me === "a" && g.phase === "waiting" ? "" : "none";
  $("link").textContent = location.href;
  $("cheat").style.display = me && (g.phase === "play" || g.phase === "waiting") ? "" : "none";
  var hits = function (seat) { return g.shots.filter(function (s) { return s.by === seat && s.said === "hit"; }).length; };
  var say = g.phase === "waiting" ? (me ? "Waiting for an opponent to join." : "This game is waiting for a second captain.")
    : g.phase === "play" ? (me ? (g.turn === me ? "Your turn: fire at their waters." : "Their turn.") : g[g.turn].name + " is firing.") + " Hits " + hits(left) + " / 12, taken " + hits(right) + " / 12."
    : g.phase === "reveal" ? "The battle is over. The boards are being revealed and checked."
    : g.winner ? g[g.winner].name + " wins." : "Nobody wins.";
  $("status").textContent = say;
  var v = $("verdict");
  if (g.phase === "over") {
    var line = function (seat) {
      var p = g[seat];
      if (!p.revealed) return "<p><b></b>: <span class=bad>went silent and never revealed a board</span></p>";
      return "<p><b></b>: " + (p.sealed ? "<span class=ok>board matches its seal</span>" : "<span class=bad>board does not match its seal</span>")
        + ", " + (p.fleet === "" ? "<span class=ok>legal fleet</span>" : "<span class=bad>illegal fleet</span>")
        + ", " + (p.lies.length ? "<span class=bad>" + p.lies.length + " false answers (marked in yellow)</span>" : "<span class=ok>every answer true</span>") + "</p>";
    };
    v.innerHTML = "<p id=why></p>" + line("a") + line("b");
    v.querySelector("#why").textContent = g.why;
    v.querySelectorAll("b")[0].textContent = g.a.name;
    v.querySelectorAll("b")[1].textContent = g.b.name;
    v.className = "on";
  } else v.className = "";
  silence();
  act();
};

// What this page does by itself: answer a shot at it, and reveal its board when the battle ends.
var act = function () {
  var g = game, me = mine && mine.seat;
  if (!g || !me || busy) return;
  var waiting = g.shots.find(function (s) { return s.said === "" && s.by !== me; });
  if (g.phase === "play" && waiting) {
    var hit = mine.board[waiting.cell] !== "0";
    busy = true;
    post("/games/" + gameId + "/answer", { key: mine.key, said: hit && !$("lie").checked ? "hit" : "miss" }).then(done);
  } else if (g.phase === "reveal" && !g[me].revealed) {
    busy = true;
    post("/games/" + gameId + "/reveal", { key: mine.key, board: mine.board, salt: mine.salt }).then(done);
  }
};
var done = function (r) { busy = false; if (r.ok && r.body.id) { game = r.body; render(); } };

// When the opponent has kept everyone waiting two minutes, a button to claim the game.
var silence = function () {
  var g = game, me = mine && mine.seat, them = me === "b" ? "a" : "b";
  var pending = g && g.shots.find(function (s) { return s.said === ""; });
  var waitingOn = !g ? "" : g.phase === "reveal" ? (g[them].revealed ? "" : them) : g.phase !== "play" ? "" : pending ? (pending.by === me ? them : me) : g.turn;
  var left = g ? 120 - (Date.now() / 1000 - g.since) : 0;
  var can = me && waitingOn === them && (g.phase === "play" || g[me].revealed) && left <= 0;
  $("claimbar").style.display = can ? "" : "none";
  if (can) $("claim").textContent = g[them].name + " has gone silent. Claim the game";
};
setInterval(function () { if (game) silence(); }, 5000);
var claim = function () {
  if (busy) return;
  busy = true;
  post("/games/" + gameId + "/claim", { key: mine.key, board: mine.board, salt: mine.salt }).then(done);
};

var fire = function (k) {
  var g = game, me = mine && mine.seat;
  if (!g || !me || busy || g.phase !== "play" || g.turn !== me) return;
  busy = true;
  post("/games/" + gameId + "/fire", { key: mine.key, cell: k }).then(done);
};

var refresh = function () {
  return fetch("/games/" + gameId).then(function (r) { return r.ok ? r.json() : null; }).then(function (g) {
    game = g;
    if (!g) { $("status").textContent = "There is no game " + gameId + "."; return; }
    if (!mine && g.phase === "waiting") {
      $("start").textContent = "Join " + (g.a.name || "this game");
      $("lobby").style.display = "";
      $("status").textContent = (g.a.name || "A captain") + " is waiting for an opponent. Shuffle your fleet and join.";
      game = null; render(); game = g;
      return;
    }
    render();
  });
};

var begin = function () {
  var name = $("name").value.trim();
  $("start").disabled = true;
  sha(draft.board + "|" + draft.salt).then(function (commit) {
    return post(gameId ? "/games/" + gameId + "/join" : "/games", { name: name || undefined, commit: commit });
  }).then(function (r) {
    $("start").disabled = false;
    if (!r.ok) { $("status").textContent = r.body.error || "That didn't work."; return; }
    gameId = r.body.game;
    mine = { seat: r.body.seat, key: r.body.key, board: draft.board, salt: draft.salt };
    save(gameId, mine);
    history.replaceState(null, "", "#" + gameId);
    refresh();
  });
};

makeGrid("left");
makeGrid("right", fire);
$("shuffle").onclick = function () { draft = { board: shuffle(), salt: salt() }; render(); };
$("start").onclick = begin;
$("claim").onclick = claim;
new EventSource("/live").onmessage = function (e) {
  var g = JSON.parse(e.data);
  if (g.id === gameId && (mine || g.phase !== "waiting")) { game = g; render(); }
};
if (gameId) { mine = load(gameId); refresh(); } else render();
</script>
</html>`

// The game's page.
route.get("/", () => respond(page, { headers: { "content-type": "text/html; charset=utf-8" } }))

// Every move of every game as it happens, as server-sent events.
route.get("/live", () => moves.subscribe())

// A game as anyone sees it: the seals, the shots and their answers, and, at the end, the boards.
route.get("/games/:id", (req) => view(games[req.params.id]) ?? status(404))

// Starts a game with the first captain's seal, and answers its name and the captain's key.
route.post("/games", (req) => {
  const commit = req.json.commit ?? ""
  if (text_length(commit) !== 64) {
    return status(400, { error: "a seal is the sha256 of the board and a salt, in hex" })
  }
  const dice = sha256(`${random}|${now}`)
  const id = slice(dice, 0, 8)
  const key = slice(dice, 8, 40)
  const name = slice(req.json.name ?? "Captain A", 0, 24)
  games[id] = {
    id,
    phase: "waiting",
    turn: "a",
    shots: [],
    winner: "",
    why: "",
    at: now,
    since: now,
    a: seat(name, commit, key),
    b: seat("", "", ""),
  }
  return status(201, { game: id, seat: "a", key })
})

// The second captain joins with their seal; the first fires first.
route.post("/games/:id/join", (req) => {
  const g = games[req.params.id]
  const commit = req.json.commit ?? ""
  if (!(g.phase === "waiting" && text_length(commit) === 64)) {
    return status(409, { error: "this game has its captains, or that is not a seal" })
  }
  const key = slice(sha256(`${random}|${g.id}|b`), 0, 32)
  const next = {
    ...g,
    phase: "play",
    since: now,
    b: seat(slice(req.json.name ?? "Captain B", 0, 24), commit, key),
  }
  games[g.id] = next
  moves.send(view(next))
  return { game: g.id, seat: "b", key }
})

// A captain fires at a square (0 to 63) on their turn, once the last shot has its answer.
route.post("/games/:id/fire", (req) => {
  const g = games[req.params.id]
  const me = seat_of(g, req.json.key ?? "")
  const cell = req.json.cell
  const waiting = g.shots.some((s) => s.said === "")
  const again = g.shots.some((s) => s.by === me && s.cell === cell)
  const fair =
    g.phase === "play" &&
    me !== "" &&
    g.turn === me &&
    !waiting &&
    !again &&
    Math.floor(cell) === cell &&
    cell >= 0 &&
    cell < 64
  if (!(fair ?? false)) {
    return status(409, { error: "not your turn, or not a square you can fire at" })
  }
  const next = { ...g, since: now, shots: [...g.shots, { by: me, cell, said: "" }] }
  games[g.id] = next
  moves.send(view(next))
  return view(next)
})

// The captain fired at says what the shot found, "hit" or "miss". The server writes it down and
// can't check it yet: that is what the reveal is for. The other captain fires next.
route.post("/games/:id/answer", (req) => {
  const g = games[req.params.id]
  const me = seat_of(g, req.json.key ?? "")
  const said = req.json.said
  const i = g.shots.findIndex((s) => s.said === "")
  const shot = g.shots[i]
  const fair =
    g.phase === "play" && me !== "" && shot.by !== me && (said === "hit" || said === "miss")
  if (!(fair ?? false)) {
    return status(409, { error: "no shot at you is waiting for an answer" })
  }
  const shots = g.shots.with(i, { ...shot, said })
  const hits = shots.filter((s) => s.by === shot.by && s.said === "hit").length
  const fired = shots.filter((s) => s.by === shot.by).length
  const next = {
    ...g,
    since: now,
    shots,
    turn: me,
    phase: hits === 12 || fired === 64 ? "reveal" : "play",
  }
  games[g.id] = next
  moves.send(view(next))
  return view(next)
})

// After the last shot, each captain reveals their board and salt. The flow checks them against the
// seal, the fleet against the rules, and every answer against the board; once both have revealed,
// it gives its verdict.
route.post("/games/:id/reveal", (req) => {
  const g = games[req.params.id]
  const me = seat_of(g, req.json.key ?? "")
  const board = req.json.board ?? ""
  const salt = req.json.salt ?? ""
  const fair = g.phase === "reveal" && me !== "" && !g[me].revealed
  if (!(fair ?? false)) {
    return status(409, { error: "nothing to reveal now" })
  }
  const other = me === "a" ? "b" : "a"
  const shown = opened(
    g[me],
    board,
    salt,
    g.shots.filter((s) => s.by === other),
  )
  const both = { ...g, since: now, [me]: shown }
  const next = g[other].revealed ? finish(both) : both
  games[g.id] = next
  moves.send(view(next))
  return view(next)
})

// A captain whose opponent has gone silent for two minutes (a shot at them left unanswered, their
// turn not taken, their board not revealed) claims the game, showing their own board as a reveal
// does unless they already have. It is theirs only if their own board is honest: a cheat can't win
// by waiting for the other to leave.
route.post("/games/:id/claim", (req) => {
  const g = games[req.params.id]
  const me = seat_of(g, req.json.key ?? "")
  const other = me === "a" ? "b" : "a"
  const i = g.shots.findIndex((s) => s.said === "")
  const silent =
    g.phase === "reveal" ? !g[other].revealed : i >= 0 ? g.shots[i].by === me : g.turn === other
  const fair =
    (g.phase === "play" || (g.phase === "reveal" && g[me].revealed)) &&
    me !== "" &&
    silent &&
    now - g.since >= 120
  if (!(fair ?? false)) {
    return status(409, { error: "your opponent still has time" })
  }
  const mine = g[me].revealed
    ? g[me]
    : opened(
        g[me],
        req.json.board ?? "",
        req.json.salt ?? "",
        g.shots.filter((s) => s.by === other),
      )
  const next = {
    ...g,
    [me]: mine,
    phase: "over",
    since: now,
    winner: mine.honest ? me : "",
    why: mine.honest
      ? `${g[other].name} went silent for two minutes, and ${mine.name}'s board matched its seal and every answer, so ${mine.name} wins.`
      : `${g[other].name} went silent, and ${charge(mine)}: nobody wins.`,
  }
  games[g.id] = next
  moves.send(view(next))
  return view(next)
})
