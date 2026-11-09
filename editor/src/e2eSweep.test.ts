// @vitest-environment node
// The flows sweep in e2e/samples.e2e.ts deploys every sample except those in DEPLOYED_BY_OWN_TEST,
// which a test of their own deploys. This reads the file and checks each listed title still has
// such a test, so a rename or a removed test fails here, not in the e2e run. A test deploys through
// the deploy() helper, or by clicking the deploy button and approving in the dialog.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const SRC = readFileSync(join(process.cwd(), "e2e", "samples.e2e.ts"), "utf8");

/** The titles in DEPLOYED_BY_OWN_TEST, read from the file's literal. */
function listed(): string[] {
  const block = /const DEPLOYED_BY_OWN_TEST = new Set\(\[\n([\s\S]*?)\]\);/.exec(SRC);
  if (!block) throw new Error("DEPLOYED_BY_OWN_TEST not found in e2e/samples.e2e.ts");
  return [...block[1].matchAll(/^\s*"(.*)",$/gm)].map((m) => m[1]);
}

/** The test bodies: each from its `test(` line to the next one. */
const tests = SRC.split(/\n(?=test\()/).slice(1);

describe("the flows sweep's own-test list", () => {
  const titles = listed();

  it("lists samples", () => {
    expect(titles.length).toBeGreaterThan(0);
  });

  it.each(titles)("%s is opened and deployed by a test of its own", (title) => {
    const opens = `openSample(page, "${title}"`;
    const covered = tests.some((body) => {
      const at = body.indexOf(opens);
      return at !== -1 && /\bdeploy\(|deployBtn|Approve and deploy|button", \{ name: \/\^\(deploy/.test(body.slice(at));
    });
    expect(covered).toBe(true);
  });
});
