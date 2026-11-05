// Hello API (samples/hello-api.holon) in TypeScript's syntax: the same unit.
import { flow, route } from "holon"

flow("hello-api", { port: 8101 })

// Greets the name in the path.
route.get("/hello/:name", (req) => `Hello, ${req.params.name}`)
