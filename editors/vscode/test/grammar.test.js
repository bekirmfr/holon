// The grammar, through VS Code's own TextMate engine: every sample tokenizes, and each kind of word
// gets its scope (docs/L4s/language-server-L4.md).

const test = require("node:test");
const assert = require("node:assert");
const fs = require("node:fs");
const path = require("node:path");
const oniguruma = require("vscode-oniguruma");
const { Registry, INITIAL } = require("vscode-textmate");

const samples = path.join(__dirname, "..", "..", "..", "crates", "holon-author", "samples");

async function grammar() {
  await oniguruma.loadWASM(fs.readFileSync(require.resolve("vscode-oniguruma/release/onig.wasm")).buffer);
  const registry = new Registry({
    onigLib: Promise.resolve({ createOnigScanner: (p) => new oniguruma.OnigScanner(p), createOnigString: (s) => new oniguruma.OnigString(s) }),
    loadGrammar: async () => {
      const file = path.join(__dirname, "..", "syntaxes", "holon.tmLanguage.json");
      return JSON.parse(fs.readFileSync(file, "utf8"));
    },
  });
  return registry.loadGrammar("source.holon");
}

/** Each token of `text` as [its text, its innermost scope], line by line. */
function tokenize(g, text) {
  let stack = INITIAL;
  const out = [];
  for (const line of text.split("\n")) {
    const r = g.tokenizeLine(line, stack);
    for (const t of r.tokens) out.push([line.slice(t.startIndex, t.endIndex), t.scopes[t.scopes.length - 1], t.scopes]);
    stack = r.ruleStack;
  }
  return out;
}

/** The innermost scope of the first token that is exactly `word` (after `from` of them). */
function scopeOf(tokens, word, from = 0) {
  const found = tokens.filter(([t]) => t.trim() === word)[from];
  assert.ok(found, `no token "${word}"`);
  return found[1];
}

test("every sample tokenizes, with a scope for each line's words", async () => {
  const g = await grammar();
  const files = fs.readdirSync(samples).filter((f) => f.endsWith(".holon"));
  assert.ok(files.length >= 10, "the samples are found");
  for (const f of files) {
    const tokens = tokenize(g, fs.readFileSync(path.join(samples, f), "utf8"));
    assert.ok(tokens.some(([, s]) => s === "keyword.other.flow.holon"), `${f}: its flow header`);
  }
});

test("each kind of word gets its scope", async () => {
  const g = await grammar();
  const text = fs.readFileSync(path.join(samples, "slugs.holon"), "utf8") + fs.readFileSync(path.join(samples, "approval.holon"), "utf8") + "# a note\nflow beat\nevery 10s\nn = 1.5 + now\n";
  const t = tokenize(g, text);
  assert.equal(scopeOf(t, "flow"), "keyword.other.flow.holon");
  assert.equal(scopeOf(t, "slugs"), "entity.name.namespace.holon");
  assert.equal(scopeOf(t, "port"), "keyword.other.header.holon");
  assert.equal(scopeOf(t, "8110"), "constant.numeric.holon");
  assert.equal(scopeOf(t, "optional"), "keyword.other.header.holon");
  assert.equal(scopeOf(t, "secret"), "variable.language.noun.holon");
  assert.equal(scopeOf(t, "receipt_key"), "variable.other.member.holon");
  assert.equal(scopeOf(t, "posts"), "variable.other.constant.holon", "a collection's header");
  assert.equal(scopeOf(t, "holon"), "keyword.other.holon.holon");
  assert.equal(scopeOf(t, "slugify"), "entity.name.function.holon", "a holon's name where it is defined");
  assert.equal(scopeOf(t, "slugify", 1), "entity.name.function.call.holon", "and where it is called");
  assert.equal(scopeOf(t, "lowercase"), "entity.name.function.call.holon");
  assert.equal(scopeOf(t, "slug"), "variable.other.definition.holon", "a binding");
  assert.equal(scopeOf(t, "route"), "keyword.other.route.holon");
  assert.equal(scopeOf(t, "POST"), "support.constant.method.holon");
  assert.equal(scopeOf(t, "/slugs"), "string.unquoted.path.holon");
  assert.equal(scopeOf(t, "respond"), "keyword.control.holon");
  assert.equal(scopeOf(t, "missing"), "entity.other.attribute-name.holon");
  assert.equal(scopeOf(t, "??"), "keyword.operator.holon");
  assert.equal(scopeOf(t, "resumes"), "keyword.other.resumes.holon");
  assert.equal(scopeOf(t, "await"), "keyword.control.holon");
  assert.equal(scopeOf(t, "1d"), "constant.numeric.duration.holon");
  assert.equal(scopeOf(t, "10s"), "constant.numeric.duration.holon");
  assert.equal(scopeOf(t, "1.5"), "constant.numeric.holon");
  assert.equal(scopeOf(t, "now", 1), "variable.language.noun.holon");
  assert.equal(scopeOf(t, "# a note"), "comment.line.number-sign.holon");
  // Inside a string's {…}: the expression, as code; around it, the string.
  const interpolated = t.find(([w, , scopes]) => w === "resume_token" && scopes.includes("meta.interpolation.holon"));
  assert.ok(interpolated, "a call inside an interpolation");
  assert.equal(interpolated[1], "entity.name.function.call.holon");
  assert.equal(scopeOf(t, "/paid/"), "string.quoted.double.holon");
});
