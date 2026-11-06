// Stress test: what each log level costs, under load.
//
// Three routes doing the same small work, recorded three ways. strict, the default: a run's
// record is on disk before it answers, and runs waiting for the disk at the same moment share one
// sync (group commit). lazy: recorded all the same, on disk within a second, so a run answers
// without waiting on the disk. unrecorded: not in the log, only counted, with the last failure;
// a person approves it at deploy. holon load sends each route as many requests as it answers, in
// turn, and puts them side by side: requests a second, latency, and what the server did with the
// runs (recorded, synced, counted).
//
// Try it: deploy it (holon deploy stress-test.holon.ts, approving what it asks), then
//   holon load stress-test                   each route for 10 s, from 8 connections
//   holon load stress-test --for 5 --connections 32 --route "GET /strict"
//   curl http://127.0.0.1:8171/strict        one by hand: "strict ok"
import { flow, route } from "holon"

flow("stress-test", { port: 8171 })

// Recorded, and on disk before it answers.
route.get("/strict", () => "strict ok")

// Recorded, and on disk within a second of its answer.
route.get("/lazy", () => "lazy ok", { log: "lazy" })

// Not recorded: counted.
route.get("/unrecorded", () => "unrecorded ok", { log: "unrecorded" })
