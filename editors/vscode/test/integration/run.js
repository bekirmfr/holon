// Runs suite.js inside a real VS Code with this extension loaded, against the `holon` built in
// this repository (target/debug, or CARGO_TARGET_DIR; HOLON_PATH names another). The VS Code is
// downloaded once into .vscode-test, or VSCODE_PATH names one installed. A server of its own runs
// for the sidebar's checks, with its own data, set up as the first admin, and a flow saved on it.

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { execFileSync, spawn } = require("node:child_process");
const { runTests } = require("@vscode/test-electron");

const SERVER = "http://127.0.0.1:7431";
const SETUP_CODE = "test-setup-code";
const PASSWORD = "a long test password";

const SAVED_FLOW = 'import { flow, route } from "holon"\n\nflow("hello", { port: 8200 })\n\nroute.get("/hello/:name", (req) => `Hello, ${req.params.name}`)\n';

async function waitForHealth(url) {
  for (let i = 0; i < 150; i++) {
    try {
      if ((await fetch(new URL("/health", url))).ok) return;
    } catch {
      // not up yet
    }
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`no server answered at ${url}`);
}

async function main() {
  const extension = path.join(__dirname, "..", "..");
  const target = process.env.CARGO_TARGET_DIR ?? path.join(extension, "..", "..", "target");
  const holon = process.env.HOLON_PATH ?? path.join(target, "debug", process.platform === "win32" ? "holon.exe" : "holon");
  if (!fs.existsSync(holon)) throw new Error(`no holon at ${holon}: cargo build -p holon first, or set HOLON_PATH`);

  // A workspace of its own: one flow with a mistake, and holon.path set to the holon under test.
  // VS Code's own TypeScript formatter is off, so formatting asks `holon lsp`, as a flow's author
  // would set it. The sidebar's server is the test's own, at 7431.
  const workspace = fs.mkdtempSync(path.join(os.tmpdir(), "holon-vscode-"));
  fs.mkdirSync(path.join(workspace, ".vscode"));
  fs.writeFileSync(path.join(workspace, ".vscode", "settings.json"), JSON.stringify({ "holon.path": holon, "holon.consoleUrl": SERVER, "typescript.format.enable": false }));
  fs.writeFileSync(path.join(workspace, "hello.holon.ts"), "import { flow, route } from 'holon'\n\nflow(\"hello\", { port: 8200 })\n\nroute.get(\"/hello/:name\", (req) =>    nope)\n");

  const log = fs.openSync(path.join(workspace, "server.log"), "w");
  const serve = spawn(holon, ["serve", "--at", "127.0.0.1:7431"], {
    env: { ...process.env, HOLON_DATA: path.join(workspace, "holon-data"), HOLON_SETUP_CODE: SETUP_CODE },
    stdio: ["ignore", log, log],
  });
  try {
    await waitForHealth(SERVER);
    // First admin, as the setup code allows; its token is what the sidebar's commands sign in with.
    const setup = await (
      await fetch(new URL("/auth/setup", SERVER), {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ code: SETUP_CODE, name: "tester", password: PASSWORD }),
      })
    ).json();
    const savedDir = fs.mkdtempSync(path.join(os.tmpdir(), "holon-flow-"));
    const saved = path.join(savedDir, "hello.holon.ts");
    fs.writeFileSync(saved, SAVED_FLOW);
    execFileSync(holon, ["flows", "save", saved, "--server", SERVER], { env: { ...process.env, HOLON_TOKEN: setup.token }, stdio: "inherit" });

    const failure = path.join(workspace, "failure.txt");
    await runTests({
      vscodeExecutablePath: process.env.VSCODE_PATH,
      extensionDevelopmentPath: extension,
      extensionTestsPath: path.join(__dirname, "suite.js"),
      extensionTestsEnv: { HOLON_TOKEN: setup.token },
      // Its own user data, so an installed VS Code that is open isn't the one asked.
      launchArgs: [workspace, "--user-data-dir", path.join(workspace, ".user"), "--disable-extensions", "--disable-workspace-trust", "--skip-welcome", "--skip-release-notes"],
    }).catch((e) => {
      if (fs.existsSync(failure)) console.error(`\nthe suite failed:\n${fs.readFileSync(failure, "utf8")}\n`);
      throw e;
    });
  } finally {
    serve.kill();
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
