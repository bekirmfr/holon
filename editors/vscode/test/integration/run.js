// Runs suite.js inside a real VS Code with this extension loaded, against the `holon` built in
// this repository (target/debug, or CARGO_TARGET_DIR; HOLON_PATH names another). The VS Code is
// downloaded once into .vscode-test, or VSCODE_PATH names one installed.

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { runTests } = require("@vscode/test-electron");

async function main() {
  const extension = path.join(__dirname, "..", "..");
  const target = process.env.CARGO_TARGET_DIR ?? path.join(extension, "..", "..", "target");
  const holon = process.env.HOLON_PATH ?? path.join(target, "debug", process.platform === "win32" ? "holon.exe" : "holon");
  if (!fs.existsSync(holon)) throw new Error(`no holon at ${holon}: cargo build -p holon first, or set HOLON_PATH`);

  // A workspace of its own: one flow with a mistake, and holon.path set to the holon under test.
  // VS Code's own TypeScript formatter is off, so formatting asks `holon lsp`, as a flow's author
  // would set it.
  const workspace = fs.mkdtempSync(path.join(os.tmpdir(), "holon-vscode-"));
  fs.mkdirSync(path.join(workspace, ".vscode"));
  // The console is started at its own port, with its own data (and the built editor, when there is one), so
  // the test's server is not a developer's running one.
  fs.writeFileSync(path.join(workspace, ".vscode", "settings.json"), JSON.stringify({ "holon.path": holon, "holon.consoleUrl": "http://127.0.0.1:7431", "typescript.format.enable": false }));
  const editor = path.join(extension, "..", "..", "editor", "dist");
  const holonEnv = { HOLON_DATA: path.join(workspace, "holon-data"), ...(fs.existsSync(editor) ? { HOLON_EDITOR: editor } : {}) };
  fs.writeFileSync(path.join(workspace, "hello.holon.ts"), "import { flow, route } from 'holon'\n\nflow(\"hello\", { port: 8200 })\n\nroute.get(\"/hello/:name\", (req) =>    nope)\n");

  const failure = path.join(workspace, "failure.txt");
  await runTests({
    vscodeExecutablePath: process.env.VSCODE_PATH,
    extensionDevelopmentPath: extension,
    extensionTestsPath: path.join(__dirname, "suite.js"),
    extensionTestsEnv: holonEnv,
    // Its own user data, so an installed VS Code that is open isn't the one asked.
    launchArgs: [workspace, "--user-data-dir", path.join(workspace, ".user"), "--disable-extensions", "--disable-workspace-trust", "--skip-welcome", "--skip-release-notes"],
  }).catch((e) => {
    if (fs.existsSync(failure)) console.error(`\nthe suite failed:\n${fs.readFileSync(failure, "utf8")}\n`);
    throw e;
  });
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
