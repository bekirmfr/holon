// Tidewheel: a sea nobody hosts, that everyone can check.
//
// A sea of 96 x 96 cells ruled by one law (a cyclic cellular automaton: a cell moves on to the next
// state when enough of the cells around it already have it), whose slow waves curl into whirls and
// swallow each other. The flow does not run the law and never sends a picture. Every browser
// computes the sea itself; what the flow keeps is what no browser could: the order of things.
//
// Its clock ticks once a second. Each tick it takes the touches people planted for that tick
// (POST /touch keeps one for two ticks from now, the input delay of a lockstep game, so that no
// browser is ever told late), chains them onto the hash of the ticks before, and sends on the
// channel live only that: the tick's number, its touches, and the chain. A page steps its own sea
// with the law (GET /law.js, whose sha256 is in the world's first hash, so another law is another
// world), checks each pack against the chain, and draws. Nothing else crosses the network. Even
// the weather comes from the chain: a tick whose hash starts low lets a whirl fall where the next
// bytes of the hash say, so nobody can know it before the tick, and everybody can check it after.
//
// Since the flow cannot see the sea, the browsers vouch for it. Every 60 ticks each page hashes its
// sea and sends the hash to POST /witness; the first to arrive also sends the sea itself, kept as a
// fossil, and every later one is counted: agreeing, or not. A page that opens late takes the newest
// fossil and the touches since (GET /sync), and steps on to now. "Replay" on a fossil does the
// same between two fossils, in your browser: the one before it, the touches in between
// (GET /replay), sixty steps of the law, and the hash that comes out against the one kept. The law
// reads no clock and no dice and does whole-number arithmetic only, so it comes out the same
// everywhere. "Fork" starts a sea of your own from a fossil that nothing you do in reaches the
// world. Every 10 minutes the law changes (an era, swell to terrace) and the sea is carried across.
//
// The witnesses are anonymous pages, one count per open page (a tab's id lives as long as the tab): a page can lie, and several can lie
// together, so the count is evidence and not proof. What no page can change is each tick's run in
// this flow's log (holon export tidewheel, then holon verify, runs each tick again and compares).
//
// Try it: deploy it and open http://127.0.0.1:8199/. Press the sea to plant a whirl; a ring marks it
// until its tick comes. A minute on, the first fossil is kept: open it, replay it, fork it. Open the
// page in a second window and the two seas are the same sea. From a shell,
//   curl -N http://127.0.0.1:8199/live                  each tick's inputs as they happen
//   curl -s http://127.0.0.1:8199/sync                  what a page that opens now is given
//   curl -s http://127.0.0.1:8199/fossils               the fossils kept
//   curl -s -d '{"x":48,"y":48}' http://127.0.0.1:8199/touch
import { every, flow, respond, route, sha256, status, text_length } from "holon"
import state from "state"
import inputs from "collection:inputs"
import fossils from "collection:fossils"
import marks from "collection:marks"
import grids from "collection:grids"
import witnesses from "collection:witnesses"
import live from "channel:live"
import { now } from "holon:clock"
import { random } from "holon:random"

flow("tidewheel", { port: 8199 })

// The world's one number: its first sea comes from it.
const SEED = 20261110

// The law: what a sea does in a tick, and the sea it starts as. Plain JavaScript, served as
// /law.js for the page to run. Whole numbers only, no clock, no dice: the same touches give the
// same sea in any browser, which is all a replay needs.
const law = `var Tidewheel = (function () {
  var W = 96, H = 96, GENS = 3, ERA = 600, FOSSIL = 60;
  var LAWS = [
    { name: "swell", S: 5, R: 3, T: 4 },
    { name: "terrace", S: 12, R: 2, T: 2 }
  ];
  function lawAt(n) { return LAWS[Math.floor(n / ERA) % LAWS.length]; }
  function offsetsOf(law) {
    var out = [];
    for (var dy = -law.R; dy <= law.R; dy++) {
      for (var dx = -law.R; dx <= law.R; dx++) {
        if ((dx !== 0 || dy !== 0) && Math.abs(dx) + Math.abs(dy) <= law.R) out.push([dx, dy]);
      }
    }
    return out;
  }
  function evolve(cells, law, offsets) {
    var S = law.S, next = new Uint8Array(W * H);
    for (var y = 0; y < H; y++) {
      for (var x = 0; x < W; x++) {
        var s = cells[y * W + x], up = s + 1 === S ? 0 : s + 1, count = 0;
        for (var o = 0; o < offsets.length; o++) {
          if (cells[((y + offsets[o][1] + H) % H) * W + ((x + offsets[o][0] + W) % W)] === up) count++;
        }
        next[y * W + x] = count >= law.T ? up : s;
      }
    }
    return next;
  }
  function octant(dx, dy) {
    var ax = Math.abs(dx), ay = Math.abs(dy);
    if (dx >= 0 && dy < 0) return ax < ay ? 0 : 1;
    if (dx > 0 && dy >= 0) return ay < ax ? 2 : 3;
    if (dx <= 0 && dy > 0) return ax < ay ? 4 : 5;
    return ay < ax ? 6 : 7;
  }
  function unpack(text) {
    var cells = new Uint8Array(W * H);
    for (var i = 0; i < W * H; i++) cells[i] = text.charCodeAt(i) - 48;
    return cells;
  }
  function pack(cells) {
    var out = [];
    for (var i = 0; i < W * H; i++) out.push(String.fromCharCode(48 + cells[i]));
    return out.join("");
  }
  function genesis(seed) {
    var law = LAWS[0], offsets = offsetsOf(law), x = seed >>> 0, cells = new Uint8Array(W * H);
    for (var i = 0; i < W * H; i++) {
      x = (Math.imul(x, 1664525) + 1013904223) >>> 0;
      cells[i] = (x >>> 16) % law.S;
    }
    for (var g = 0; g < 150; g++) cells = evolve(cells, law, offsets);
    return pack(cells);
  }
  function plant(cells, x, y, base, S) {
    for (var dy = -3; dy <= 3; dy++) {
      for (var dx = -3; dx <= 3; dx++) {
        if (Math.abs(dx) + Math.abs(dy) > 3) continue;
        var at = ((y + dy + H) % H) * W + ((x + dx + W) % W);
        cells[at] = dx === 0 && dy === 0 ? base : (base + Math.floor(octant(dx, dy) * S / 8)) % S;
      }
    }
  }
  function step(grid, touches, n, chain) {
    var law = lawAt(n), was = lawAt(n - 1), S = law.S, cells = unpack(grid);
    if (was.S !== S) for (var i = 0; i < W * H; i++) cells[i] = Math.floor(cells[i] * S / was.S);
    var planted = Array.isArray(touches) ? touches.slice(0, 6) : [];
    for (var k = 0; k < planted.length; k++) {
      var x = Math.floor(planted[k].x), y = Math.floor(planted[k].y);
      if (x >= 0 && x < W && y >= 0 && y < H) plant(cells, x, y, (n + k) % S, S);
    }
    if (parseInt(chain.slice(0, 2), 16) < 24) {
      plant(cells, parseInt(chain.slice(2, 4), 16) % W, parseInt(chain.slice(4, 6), 16) % H, parseInt(chain.slice(6, 8), 16) % S, S);
    }
    var offsets = offsetsOf(law);
    for (var g = 0; g < GENS; g++) cells = evolve(cells, law, offsets);
    return pack(cells);
  }
  return { W: W, H: H, GENS: GENS, ERA: ERA, FOSSIL: FOSSIL, LAWS: LAWS, genesis: genesis, step: step };
})();`

// The page: the sea on a canvas, drawn from the packs, with the fossils below it. It steps its
// own sea with /law.js, checks the chain, plants touches, vouches for the sea at each fossil, and
// replays or forks a fossil without asking the flow to compute anything.
const page = `<!doctype html>
<html lang="en">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Tidewheel</title>
<link rel="icon" href="data:image/svg+xml,%3Csvg xmlns=%27http://www.w3.org/2000/svg%27 viewBox=%270 0 32 32%27%3E%3Crect width=%2732%27 height=%2732%27 rx=%276%27 fill=%27%23071a33%27/%3E%3Cpath d=%27M16 16m0 0a2 2 0 1 1 4 0a5 5 0 1 1-10 0a8 8 0 1 1 16 0a11 11 0 1 1-22 0%27 fill=%27none%27 stroke=%27%237fd8e8%27 stroke-width=%273%27 stroke-linecap=%27round%27/%3E%3C/svg%3E">
<style>
:root{color-scheme:dark;--ink:#d6e6ee;--dim:#8196a4;--line:#ffffff1c;--glass:#0a1118d8;--accent:#7fd8e8}
*{box-sizing:border-box}
html,body{margin:0;min-height:100%;background:#03060b;color:var(--ink);font:14px/1.45 ui-sans-serif,system-ui,sans-serif}
body{display:grid;grid-template-rows:auto 1fr auto;min-height:100dvh;background:radial-gradient(120% 90% at 50% 40%,#0a1624 0%,#03060b 70%)}
header{display:flex;align-items:baseline;gap:.9em;padding:14px 18px 0;z-index:3}
h1{margin:0;font:italic 400 28px/1 ui-serif,"Iowan Old Style",Georgia,serif;letter-spacing:.01em}
header p{margin:0;color:var(--dim);font-size:13px;flex:1;min-width:0;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
button{font:inherit;color:var(--ink);background:#ffffff0d;border:1px solid var(--line);border-radius:999px;padding:5px 13px;cursor:pointer}
button:hover{background:#ffffff1c}
#stagewrap{display:grid;place-items:center;padding:10px;min-height:0}
#stage{position:relative;width:max(260px,min(100%,calc(100dvh - 230px)));aspect-ratio:1;max-width:820px;border-radius:22px;overflow:hidden;box-shadow:0 0 0 1px var(--line),0 30px 90px #000a,0 0 120px #1c6a8a33;touch-action:none;cursor:crosshair;background:#040a14}
#stage canvas{position:absolute;inset:0;width:100%;height:100%}
#sea{filter:blur(1.7px) saturate(1.25) contrast(1.08);transform:scale(1.025)}
#glow{filter:blur(16px) brightness(1.5) saturate(1.4);mix-blend-mode:screen;opacity:.55;pointer-events:none}
#rings{position:absolute;inset:0;pointer-events:none}
.ring{position:absolute;width:14%;aspect-ratio:1;margin:-7% 0 0 -7%;border-radius:50%;border:2px solid #fffd;animation:ping 1.6s ease-out infinite}
@keyframes ping{from{transform:scale(.2);opacity:1}to{transform:scale(1);opacity:0}}
#banner{display:none;gap:.5em .8em;align-items:center;flex-wrap:wrap;padding:8px 14px;border-radius:14px;background:var(--glass);border:1px solid var(--line);max-width:820px;width:100%;margin:0 auto}
#banner.on{display:flex}
#banner .sp{flex:1;min-width:1em}
#verdict{flex-basis:100%;order:9;font-size:13px;font-variant-numeric:tabular-nums}
#verdict:empty{display:none}
.ok{color:#8fe6b0}.bad{color:#ff8a8a}
#hint{position:absolute;left:0;right:0;bottom:14px;text-align:center;color:#ffffffb0;font-size:13px;text-shadow:0 1px 8px #000;pointer-events:none;transition:opacity 1.2s}
footer{padding:0 14px 12px;z-index:3;display:grid;gap:8px}
#status{display:flex;flex-wrap:wrap;gap:4px 18px;align-items:center;justify-content:center;color:var(--dim);font-size:13px;font-variant-numeric:tabular-nums}
#status b{color:var(--ink);font-weight:500}
#dot{display:inline-block;width:9px;height:9px;border-radius:50%;background:#667;margin-right:6px}
#dot.in{background:#6fe3a0;box-shadow:0 0 10px #6fe3a0aa}
#dot.out{background:#ff7b7b;box-shadow:0 0 10px #ff7b7baa}
#fossils{display:flex;gap:8px;overflow-x:auto;padding:2px 2px 6px;scrollbar-width:thin;min-height:30px}
#fossils:empty::before{content:"The first fossil is kept a minute after the sea begins.";color:var(--dim);font-size:13px}
.fossil{flex:none;width:76px;border:1px solid var(--line);border-radius:10px;padding:0;overflow:hidden;background:#000}
.fossil canvas{display:block;width:100%;aspect-ratio:1}
.fossil small{display:block;padding:2px 0 3px;color:var(--dim);font-size:10.5px;text-align:center;font-variant-numeric:tabular-nums}
.fossil.on{outline:2px solid var(--accent)}
#about{position:fixed;inset:0;display:none;place-items:center;background:#000a;z-index:9;padding:20px}
#about.on{display:grid}
#card{max-width:540px;max-height:90dvh;overflow:auto;background:#0c141d;border:1px solid var(--line);border-radius:18px;padding:22px 24px;box-shadow:0 30px 80px #000}
#card h2{margin:0 0 8px;font:italic 400 22px ui-serif,Georgia,serif}
#card p{margin:.55em 0;color:#b9cad4}
#card code{background:#ffffff12;border-radius:5px;padding:0 5px;font-size:12.5px}
a{color:var(--accent)}
@media (max-width:640px){header p{display:none}}
@media (prefers-reduced-motion:reduce){.ring{animation:none;opacity:.7}}
</style>
<header>
<h1>Tidewheel</h1>
<p>A sea nobody hosts, that everyone can check.</p>
<button id="why">How it works</button>
</header>
<div id="stagewrap">
<div id="stage">
<canvas id="glow" width="96" height="96"></canvas>
<canvas id="sea" width="96" height="96" role="img" aria-label="The sea: waves that turn into whirls where people press"></canvas>
<div id="rings"></div>
<div id="hint">Press the sea to plant a whirl.</div>
</div>
</div>
<footer>
<div id="banner"></div>
<div id="status"><span><i id="dot"></i><b id="sync">joining</b></span><span>tick <b id="tick">-</b></span><span>era of <b id="era">-</b>, turns in <b id="turns">-</b></span><span id="agreed"></span><span>chain <b id="chainhash" style="font-family:ui-monospace,monospace;font-size:12px">-</b></span></div>
<div id="fossils"></div>
</footer>
<div id="about"><div id="card">
<h2>Nobody sends you the sea.</h2>
<p>It is 9,216 cells obeying one rule, and your browser is computing it now. The server only keeps the order of things: once a second it sends what was done that tick, which is the touches people planted and a hash chained onto the ticks before it. Anyone with the same law and the same touches gets the same sea.</p>
<p>The server cannot see the sea, so the pages vouch for it. Every 60 ticks each one hashes its sea and tells the server; the first also sends the sea itself, which becomes a <b>fossil</b>, and the rest are counted as agreeing or not. The count is evidence: anonymous pages can lie.</p>
<p><b>Replay</b> a fossil and your browser takes the one before it and the touches in between, runs the law 60 times and compares its hash with the one kept. <b>Fork</b> starts a sea of your own from it.</p>
<p>The law is <a href="/law.js" target="_blank">/law.js</a>. This page runs the law whose sha256 is <code id="lawhash">...</code> and the server says the world's is <code id="lawhash2">...</code>.</p>
<p><button id="close">Back to the sea</button></p>
</div></div>
<script src="/law.js"></script>
<script>
const T = Tidewheel;
const { W, H, GENS, ERA, FOSSIL, LAWS } = T;
const TICK_MS = 1000;
const STOPS = [
  ["#040a1c", "#0a2349", "#12508a", "#2f9bb0", "#e9fbf0"],
  ["#10051f", "#3b0f4f", "#7a1b5e", "#c2325a", "#ef6a4a", "#ffb25b", "#ffe8a8", "#9fe0c0", "#3fb3c2", "#2a6fb0", "#2a3a8c", "#1a1650"],
];
const hex = (h) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16));
const PAL = LAWS.map((law, li) => {
  const st = STOPS[li].map(hex);
  return Array.from({ length: law.S }, (_, s) => {
    const p = (s / (law.S - 1)) * (st.length - 1);
    const a = Math.floor(p);
    const b = Math.min(a + 1, st.length - 1);
    const f = p - a;
    return st[a].map((v, k) => v + (st[b][k] - v) * f);
  });
});
const lawIndex = (n) => Math.floor(n / ERA) % LAWS.length;
const $ = (id) => document.getElementById(id);
// sha256 of a text, hex: the browser's, or a small one of our own where it has none (a page not
// served from localhost or https has no crypto.subtle).
const own = (text) => {
  const bytes = new TextEncoder().encode(text);
  const primes = [];
  for (let c = 2; primes.length < 64; c++) if (primes.every((p) => c % p)) primes.push(c);
  const frac = (x) => x - Math.floor(x);
  const K = primes.map((p) => Math.floor(frac(Math.cbrt(p)) * 4294967296));
  const h = primes.slice(0, 8).map((p) => Math.floor(frac(Math.sqrt(p)) * 4294967296));
  const size = ((bytes.length + 9 + 63) >> 6) << 6;
  const buf = new Uint8Array(size);
  buf.set(bytes);
  buf[bytes.length] = 0x80;
  const view = new DataView(buf.buffer);
  view.setUint32(size - 8, Math.floor(bytes.length / 536870912));
  view.setUint32(size - 4, (bytes.length << 3) >>> 0);
  const w = new Array(64);
  const rotr = (x, n) => (x >>> n) | (x << (32 - n));
  for (let off = 0; off < size; off += 64) {
    for (let i = 0; i < 16; i++) w[i] = view.getUint32(off + i * 4);
    for (let i = 16; i < 64; i++) {
      const s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >>> 3);
      const s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >>> 10);
      w[i] = (w[i - 16] + s0 + w[i - 7] + s1) >>> 0;
    }
    let [a, b, c, d, e, f, g, hh] = h;
    for (let i = 0; i < 64; i++) {
      const t1 = (hh + (rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25)) + ((e & f) ^ (~e & g)) + K[i] + w[i]) >>> 0;
      const t2 = ((rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22)) + ((a & b) ^ (a & c) ^ (b & c))) >>> 0;
      hh = g; g = f; f = e; e = (d + t1) >>> 0; d = c; c = b; b = a; a = (t1 + t2) >>> 0;
    }
    [a, b, c, d, e, f, g, hh].forEach((v, i) => { h[i] = (h[i] + v) >>> 0; });
  }
  return h.map((v) => v.toString(16).padStart(8, "0")).join("");
};
const sha = async (text) => {
  if (!(window.crypto && crypto.subtle)) return own(text);
  const d = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return Array.from(new Uint8Array(d), (b) => b.toString(16).padStart(2, "0")).join("");
};
const colors = (grid, n) => {
  const pal = PAL[lawIndex(n)];
  const out = new Float32Array(W * H * 3);
  for (let i = 0; i < W * H; i++) {
    const c = pal[grid.charCodeAt(i) - 48] || pal[0];
    out[i * 3] = c[0]; out[i * 3 + 1] = c[1]; out[i * 3 + 2] = c[2];
  }
  return out;
};
const paint = (cv, rgb) => {
  const ctx = cv.getContext("2d");
  const img = ctx.createImageData(W, H);
  for (let i = 0; i < W * H; i++) {
    img.data[i * 4] = rgb[i * 3]; img.data[i * 4 + 1] = rgb[i * 3 + 1]; img.data[i * 4 + 2] = rgb[i * 3 + 2]; img.data[i * 4 + 3] = 255;
  }
  ctx.putImageData(img, 0, 0);
};
// What is drawn: the world's sea (live), a fossil (frozen), or a sea of one's own (fork).
let mode = "live", shown = null, fork = null, forkTimer = 0;
let from = null, to = null, since = 0;
let current = null;
const show = (grid, n, soft) => {
  const next = colors(grid, n);
  from = soft && current ? current : next;
  to = next;
  since = performance.now();
};
const frame = (t) => {
  if (to) {
    const e = Math.min(1, (t - since) / (TICK_MS - 20));
    const k = e * e * (3 - 2 * e);
    current = to.map((v, i) => from[i] + (v - from[i]) * k);
    paint($("sea"), current);
    paint($("glow"), current);
  }
  requestAnimationFrame(frame);
};
requestAnimationFrame(frame);
const clock = (s) => Math.floor(s / 60) + ":" + String(s % 60).padStart(2, "0");
const setSync = (kind, text) => { $("dot").className = kind; $("sync").textContent = text; };
// The world's sea, as this page has computed it: its tick, its hash, its cells.
let sea = null, chain = "", seaBase = "";
const hud = () => {
  const n = mode === "fork" ? fork.n : mode === "fossil" ? shown.n : sea ? sea.n : 0;
  $("tick").textContent = n.toLocaleString();
  $("era").textContent = LAWS[lawIndex(n)].name;
  $("turns").textContent = clock(Math.round(((ERA - (n % ERA)) * TICK_MS) / 1000));
  if (chain && mode !== "fork") $("chainhash").textContent = chain.slice(0, 8);
};
const groupBy = (items) => {
  const out = {};
  Object.keys(items || {}).sort().forEach((k) => { (out[Number(k.slice(0, 8))] = out[Number(k.slice(0, 8))] || []).push(items[k]); });
  return out;
};
let queue = [], joining = false, lawText = "", lawHash = "";
const join = async () => {
  joining = true;
  setSync("", "joining");
  const sync = await (await fetch("/sync")).json();
  lawText = lawText || (await (await fetch("/law.js")).text());
  lawHash = await sha(lawText);
  seaBase = await sha("sea|" + sync.law + "|" + sync.seed);
  let n = 0, grid = null, h = seaBase, ch = sync.chain0;
  if (sync.fossil.n > 0) { n = sync.fossil.n; grid = sync.fossil.grid; h = sync.fossil.h; ch = sync.fossil.chain; } else { grid = T.genesis(sync.seed); }
  const byTick = groupBy(sync.inputs);
  for (let t = n + 1; t <= sync.n; t++) {
    const ts = byTick[t] || [];
    ch = await sha(ch + "|" + t + "|" + JSON.stringify(ts));
    grid = T.step(grid, ts, t, ch);
    h = await sha(h + "|" + t + "|" + grid);
    if (t % 25 === 0) setSync("", "catching up, " + (sync.n - t).toLocaleString() + " ticks to go");
  }
  sea = { n: sync.n, h, grid };
  chain = ch;
  joining = false;
  const sound = lawHash === sync.law && ch === sync.h;
  setSync(sound ? "in" : "out", sound ? "in the world" : lawHash !== sync.law ? "this is not the world's law" : "the touches do not add up to the chain");
  if (mode === "live") show(sea.grid, sea.n, false);
  hud();
  const waiting = queue; queue = [];
  waiting.forEach(onPack);
  loadFossils();
};
// A join that fails (the server restarting, say) is tried again, the packs waiting meanwhile.
const rejoin = () => join().catch(() => { joining = false; setSync("", "retrying"); setTimeout(rejoin, 2000); });
let steps = Promise.resolve();
const onPack = (pack) => {
  steps = steps.then(async () => {
    if (joining || !sea) { queue.push(pack); return; }
    if (pack.n <= sea.n) return;
    if (pack.n !== sea.n + 1) { await rejoin(); return; }
    const expect = await sha(chain + "|" + pack.n + "|" + JSON.stringify(pack.t));
    chain = pack.h;
    const grid = T.step(sea.grid, pack.t, pack.n, pack.h);
    const h = await sha(sea.h + "|" + pack.n + "|" + grid);
    sea = { n: pack.n, h, grid };
    pending = pending.filter((r) => r.tick > pack.n || (r.el.remove(), false));
    if (expect !== pack.h) setSync("out", "the server's chain broke at " + pack.n);
    if (mode === "live") show(grid, pack.n, true);
    hud();
    if (pack.n % FOSSIL === 0) witness(sea);
  });
};
// Vouch for the sea at a fossil tick: the hash first, and the sea itself if nobody has sent it.
const id = (() => { try { return sessionStorage.tidewheel || (sessionStorage.tidewheel = Math.random().toString(36).slice(2, 14)); } catch (e) { return Math.random().toString(36).slice(2, 14); } })();
const witness = async (s) => {
  const post = (body) => fetch("/witness", { method: "POST", body: JSON.stringify(body) }).then((r) => r.json());
  let said = await post({ id, n: s.n, h: s.h });
  if (said.need) said = await post({ id, n: s.n, h: s.h, grid: s.grid });
  const at = "at #" + s.n.toLocaleString();
  $("agreed").innerHTML = !said.mine ? '<b class="bad">' + at + ", your sea is not the fossil's</b>" : said.dissent ? at + " <b>" + said.agree + "</b> agree, <b class=bad>" + said.dissent + "</b> disagree" : at + " <b>" + said.agree + "</b> other " + (said.agree === 1 ? "browser agrees" : "browsers agree");
  setTimeout(loadFossils, 600);
};
const open = () => {
  const es = new EventSource("/live");
  es.onmessage = (e) => onPack(JSON.parse(e.data));
  es.onerror = () => setSync("", "reconnecting");
  es.onopen = () => { if (!sea) rejoin(); };
};
// Touches: kept for two ticks from now. A ring marks one until its tick has come.
let pending = [], lastTouch = 0, down = false;
const ring = (x, y, ms) => {
  const el = document.createElement("div");
  el.className = "ring";
  el.style.left = ((x + 0.5) / W) * 100 + "%";
  el.style.top = ((y + 0.5) / H) * 100 + "%";
  $("rings").append(el);
  if (ms) setTimeout(() => el.remove(), ms);
  return el;
};
const plant = async (ev) => {
  const r = $("stage").getBoundingClientRect();
  const x = Math.floor(((ev.clientX - r.left) / r.width) * W);
  const y = Math.floor(((ev.clientY - r.top) / r.height) * H);
  if (x < 0 || y < 0 || x >= W || y >= H) return;
  $("hint").style.opacity = 0;
  if (mode === "fork") { fork.touches.push({ x, y }); ring(x, y, 1200); return; }
  if (mode !== "live") return;
  const t = performance.now();
  if (t - lastTouch < 240) return;
  lastTouch = t;
  const res = await fetch("/touch", { method: "POST", body: JSON.stringify({ x, y }) });
  if (res.ok) { const k = await res.json(); pending.push({ tick: k.tick, el: ring(x, y, 0) }); }
};
$("stage").addEventListener("pointerdown", (e) => { down = true; plant(e); });
$("stage").addEventListener("pointermove", (e) => { if (down) plant(e); });
addEventListener("pointerup", () => { down = false; });
// Fossils.
const cache = new Map();
const fossil = async (n) => {
  if (!cache.has(n)) {
    const res = await fetch("/fossil/" + n);
    if (!res.ok) return null;
    cache.set(n, await res.json());
  }
  return cache.get(n);
};
const loadFossils = async () => {
  const list = await (await fetch("/fossils")).json();
  const box = $("fossils");
  for (const f of list) {
    const old = box.querySelector('[data-n="' + f.n + '"]');
    if (old) continue;
    const full = await fossil(f.n);
    if (!full) continue;
    const b = document.createElement("button");
    b.className = "fossil"; b.dataset.n = f.n;
    const cv = document.createElement("canvas"); cv.width = W; cv.height = H;
    paint(cv, colors(full.grid, f.n));
    const label = document.createElement("small"); label.textContent = "#" + f.n.toLocaleString();
    b.append(cv, label);
    b.onclick = () => openFossil(f.n);
    box.insertBefore(b, box.firstChild);
  }
};
const banner = (html) => { $("banner").innerHTML = html; $("banner").classList.toggle("on", !!html); };
const openFossil = async (n) => {
  clearInterval(forkTimer);
  mode = "fossil";
  cache.delete(n);
  shown = await fossil(n);
  document.querySelectorAll(".fossil").forEach((b) => b.classList.toggle("on", Number(b.dataset.n) === n));
  show(shown.grid, shown.n, true);
  hud();
  banner('<span>Fossil <b>#' + n.toLocaleString() + '</b>, kept ' + new Date(shown.at * 1000).toLocaleTimeString() + ', ' + shown.agree + ' agree' + (shown.dissent ? ', ' + shown.dissent + ' disagree' : '') + '</span><span class="sp"></span><span id="verdict"></span><button id="replay">Replay</button><button id="forkit">Fork</button><button id="back">Back to the sea</button>');
  $("replay").onclick = replay;
  $("forkit").onclick = () => startFork(shown);
  $("back").onclick = backToLive;
};
const backToLive = () => {
  clearInterval(forkTimer);
  mode = "live"; fork = null;
  document.querySelectorAll(".fossil").forEach((b) => b.classList.remove("on"));
  banner("");
  if (sea) show(sea.grid, sea.n, true);
  hud();
};
const replay = async () => {
  const v = $("verdict");
  v.className = ""; v.textContent = "replaying...";
  const t0 = performance.now();
  const before = await fossil(shown.n - FOSSIL);
  if (!before) { v.className = "bad"; v.textContent = "the fossil before it is not kept"; return; }
  const log = await (await fetch("/replay?from=" + before.n + "&to=" + shown.n)).json();
  const byTick = groupBy(log);
  let grid = before.grid, h = before.h, ch = before.chain, planted = 0;
  for (let t = before.n + 1; t <= shown.n; t++) {
    const ts = byTick[t] || [];
    planted += ts.length;
    ch = await sha(ch + "|" + t + "|" + JSON.stringify(ts));
    grid = T.step(grid, ts, t, ch);
    h = await sha(h + "|" + t + "|" + grid);
  }
  const ms = Math.round(performance.now() - t0);
  if (h === shown.h && grid === shown.grid && ch === shown.chain) { v.className = "ok"; v.textContent = "Same sea: " + FOSSIL + " ticks and " + planted + " touches replayed here in " + ms + " ms; the sea's hash " + h.slice(0, 10) + "... and the server's chain " + ch.slice(0, 10) + "... both match."; }
  else { v.className = "bad"; v.textContent = "Not the same: replayed " + h.slice(0, 10) + "... (chain " + ch.slice(0, 10) + "...), the world kept " + shown.h.slice(0, 10) + "... (chain " + shown.chain.slice(0, 10) + "...)"; }
};
const startFork = (f) => {
  mode = "fork";
  fork = { n: f.n, grid: f.grid, touches: [] };
  banner('<span>Your own sea, forked at <b>#' + f.n.toLocaleString() + '</b>. Press it to plant whirls: nothing here reaches the world.</span><span class="sp"></span><button id="back">Back to the sea</button>');
  $("back").onclick = backToLive;
  clearInterval(forkTimer);
  forkTimer = setInterval(() => {
    fork.n += 1;
    fork.grid = T.step(fork.grid, fork.touches, fork.n, ((Math.imul(fork.n, 2654435761) >>> 0).toString(16).padStart(8, "0")));
    fork.touches = [];
    show(fork.grid, fork.n, true);
    hud();
  }, TICK_MS / 2);
};
$("why").onclick = async () => {
  $("about").classList.add("on");
  $("lawhash").textContent = lawHash.slice(0, 12) + "...";
  const sync = await (await fetch("/sync")).json();
  $("lawhash2").textContent = sync.law.slice(0, 12) + "...";
};
$("close").onclick = () => $("about").classList.remove("on");
// The blur that melts the cells into waves grows with the cells' size on screen.
const fit = () => { $("sea").style.filter = "blur(" + (($("stage").getBoundingClientRect().width / W) * 0.3).toFixed(2) + "px) saturate(1.25) contrast(1.08)"; };
addEventListener("resize", fit);
fit();
open();
setInterval(hud, 1000);
setTimeout(() => { $("hint").style.opacity = 0; }, 15000);
</script>
</html>`

// The page.
route.get("/", () => respond(page, { headers: { "content-type": "text/html; charset=utf-8" } }))

// The law, for the page to run.
route.get("/law.js", () =>
  respond(law, { headers: { "content-type": "text/javascript; charset=utf-8" } }),
)

// Each tick's inputs as they happen, as server-sent events.
route.get("/live", () => live.subscribe())

// Plant a whirl at a cell {x, y}, kept for the tick two from now: the answer says which.
route.post("/touch", (req) => {
  const x = Math.floor(req.json.x)
  const y = Math.floor(req.json.y)
  const inside = x >= 0 && x < 96 && y >= 0 && y < 96
  const k = (state.n ?? 0) + 2
  if (inside ?? false) {
    inputs[`${String(k).padStart(8, "0")}-${Math.floor(random * 1000000000)}`] = { x, y }
    return status(202, { tick: k })
  }
  return status(400)
})

// What a page that opens now is given: the tick and its chain, the newest fossil (tick 0 when none
// is kept yet) with the touches since, the world's seed and the hash of its law. Where the newest
// fossil is, the witnesses keep in state, so a world nobody watched for hours still has one.
route.get("/sync", () => {
  const n = state.n ?? 0
  const newest = fossils[String(state.latest ?? 0).padStart(8, "0")] ?? { n: 0, h: "" }
  const since = newest.n
  return {
    n,
    h: state.h ?? "",
    seed: SEED,
    law: sha256(law),
    chain0: sha256(`${sha256(law)}|${SEED}`),
    fossil: {
      n: since,
      h: newest.h,
      grid: grids[String(since).padStart(8, "0")] ?? "",
      chain: marks[String(since).padStart(8, "0")] ?? "",
    },
    inputs: inputs.list({ after: `${String(since).padStart(8, "0")}~`, limit: 1000 }),
  }
})

// A page's vouching for its sea at a fossil tick: its hash, and, for the first, the sea itself.
// The first becomes the fossil (a sea kept is never replaced: `??` keeps what is there); every page,
// the first included, leaves a vote under its page id, which a later vote of its own replaces. The
// answer counts the other pages' votes: agreeing with the fossil, or not. Asked for a tick that is
// no fossil tick, or with no sea to keep when none is kept, it answers what is missing.
route.post("/witness", (req) => {
  const n = req.json.n
  const who = req.json.id
  const hash = req.json.h
  const key = String(n).padStart(8, "0")
  const asked =
    n % 60 === 0 && n <= (state.n ?? 0) && n > (state.n ?? 0) - 3000 && text_length(hash) === 64
  if (asked ?? false) {
    const others = Object.values(witnesses.list({ prefix: `${key}-`, limit: 500 })).filter(
      (v) => v.id !== who,
    )
    const fresh = text_length(req.json.grid) === 9216 ? { n, h: hash, at: now } : undefined
    const kept = fossils[key] ?? fresh
    const want = kept.h
    fossils[key] = kept
    state.latest = Math.max(state.latest ?? 0, kept.n)
    grids[key] = grids[key] ?? req.json.grid
    witnesses[`${key}-${who}`] = { id: who, h: hash, n }
    return (
      {
        n,
        mine: want === hash,
        agree: others.filter((v) => v.h === want).length,
        dissent: others.filter((v) => v.h !== want).length,
      } ?? { need: true }
    )
  }
  return status(400)
})

// The fossils kept lately, oldest first: tick, hash and when.
route.get("/fossils", () =>
  Object.values(
    fossils.list({ after: String(Math.max(0, (state.n ?? 0) - 3000)).padStart(8, "0"), limit: 60 }),
  ),
)

// One fossil: the sea at that tick, with its hash, and how many pages agree with it, or not.
route.get("/fossil/:n", (req) => {
  const key = req.params.n.padStart(8, "0")
  const fossil = fossils[key]
  const votes = Object.values(witnesses.list({ prefix: `${key}-`, limit: 500 }))
  return {
    ...fossil,
    grid: grids[key],
    chain: marks[key] ?? "",
    agree: votes.filter((v) => v.h === fossil.h).length,
    dissent: votes.filter((v) => v.h !== fossil.h).length,
  }
})

// What a replay needs: the touches of the ticks after `from`, up to `to`, by key.
route.get("/replay", (req) => {
  const upto = req.query.to.padStart(8, "0")
  const kept = inputs.list({ after: `${req.query.from.padStart(8, "0")}~`, limit: 1000 })
  return Object.fromEntries(Object.entries(kept).filter((e) => e[0].slice(0, 8) <= upto))
})

// Each second: the touches planted for this tick go onto the chain, and out. Every 60 ticks the
// chain is marked, so that a replay has where to start from.
every("1s", () => {
  const n = (state.n ?? 0) + 1
  const touches = Object.values(inputs.list({ prefix: `${String(n).padStart(8, "0")}-` }))
  const h = sha256(`${state.h ?? sha256(`${sha256(law)}|${SEED}`)}|${n}|${JSON.stringify(touches)}`)
  state.n = n
  state.h = h
  marks[String(n).padStart(8, "0")] = n % 60 === 0 ? h : undefined
  live.send({ n, h, t: touches })
})
