// Hello API: the smallest server.
//
// One route with a path parameter, answered with text.
//
// Try it: deploy it, then
//   curl http://127.0.0.1:8101/hello/Ada               "Hello, Ada"
import { flow, route } from "holon"

flow("hello-api", { port: 8101 })

// Greets the name in the path.
route.get("/hello/:name", (req) => `Hello, ${req.params.name}`)
