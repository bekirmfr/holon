// Browser key (samples/browser-key.holon) in TypeScript's syntax: the same unit.
import { flow, route } from "holon"
import maps_browser_key from "secret:maps_browser_key"

flow("browser-key", { port: 8163 })

// Hands the browser its maps key.
route.get("/config", () => ({ maps_key: maps_browser_key, version: 1 }))
