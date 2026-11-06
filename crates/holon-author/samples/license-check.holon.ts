// A secret in a query goes to the database, and the approval says so.
//
// A product's licence key, issued by its vendor, is kept as a secret; the vendor's table of
// licences, in a database an admin binds, says what each key unlocks. The flow asks the database
// which plan its key unlocks: the key is the query's $1, never in its text, and goes to the
// database's server with it. Its approval says so: "read the secret "license_key", and send it to
// the database "vendor"", and the canvas marks the read. The row it answers is what the database
// said, not the key, so answering with it isn't answering with the secret.
//
// Before it works, an admin binds the database, with a licence in it:
//   holon database add vendor ~/holon-vendor.db --schema "create table if not exists licenses
//     (license_key text primary key, plan text not null, seats integer not null);
//     insert or ignore into licenses values ('lic-demo-1234', 'team', 5)"
// and the deployment's settings (the gear) set the secret:
//   license_key   lic-demo-1234, the licence the table holds
//
// Try it: deploy it, set the key, then
//   curl http://127.0.0.1:8165/license
// answers {"plan": "team", "seats": 5}.
import { flow, route } from "holon"
import license_key from "secret:license_key"
import vendor from "database:vendor"

flow("license-check", { port: 8165 })

// What this deployment's licence unlocks.
route.get("/license", () =>
  vendor.one("select plan, seats from licenses where license_key = $1", license_key),
)
