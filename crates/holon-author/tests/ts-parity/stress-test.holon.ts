// Stress test (samples/stress-test.holon) in TypeScript's syntax: the same unit.
import { flow, route } from "holon"

flow("stress-test", { port: 8171 })

route.get("/strict", () => "strict ok")

route.get("/lazy", () => "lazy ok", { log: "lazy" })

route.get("/unrecorded", () => "unrecorded ok", { log: "unrecorded" })
