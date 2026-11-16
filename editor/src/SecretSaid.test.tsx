// A line of what a flow may do with a secret is marked wherever it is listed: the key, and the
// secret's colour.

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { initSync } from "../wasm/holon_wasm.js";
import { MayStrip } from "./MayStrip";
import { isSecretItem, Said, saidAsText, SECRET_MARK, SecretLines } from "./SecretSaid";

// Core's words, through the wasm package as the editor has them.
initSync({ module: readFileSync(join(process.cwd(), "wasm", "holon_wasm_bg.wasm")) });

describe("a secret's line", () => {
  it("is every item of a secret, and no other", () => {
    for (const item of ['SecretRead("k")', 'SecretAnswer("k")', 'SecretRead("a\\"b")', "SecretAccess"]) expect(isSecretItem(item), item).toBe(true);
    for (const item of ['StateRead("secret")', 'HttpRequest("secret.example.com")', "Clock", "Limit(max_requests=80)", 'FileRead("secrets")', 'Await("secret")']) {
      expect(isSecretItem(item), item).toBe(false);
    }
  });

  it("has the key before its words, in the secret's colour; another line is as it was", () => {
    const secret = renderToStaticMarkup(<Said item={'SecretRead("k")'} said={'read the secret "k"; a hash or signature made with it goes to its callers'} />);
    expect(secret).toContain('class="hl-secret-said"');
    expect(secret).toContain(`aria-hidden="true">${SECRET_MARK}</span>read the secret`);
    expect(renderToStaticMarkup(<Said item="Clock" said="read the clock" />)).toBe("read the clock");
    expect(saidAsText('SecretAnswer("k")', "answer its callers with the secret")).toBe(`${SECRET_MARK} answer its callers with the secret`);
    expect(saidAsText("Clock", "read the clock")).toBe("read the clock");
  });

  it("is in the secret's colour in a question's text, which reads as it did", () => {
    const text = `Deploy "x"?\n\nIt needs your approval to:\n• receive requests\n• ${saidAsText('SecretRead("k")', 'read the secret "k"')}\n\nIts state is kept.`;
    const shown = renderToStaticMarkup(<SecretLines text={text} />);
    expect(shown.match(/hl-secret-said/g)?.length).toBe(1);
    expect(shown).toContain(`<span class="hl-secret-said">\n• ${SECRET_MARK} read the secret &quot;k&quot;</span>`);
    expect(shown.replace(/<[^>]+>/g, "").replace(/&quot;/g, '"')).toBe(text);
    expect(renderToStaticMarkup(<SecretLines text={"Remove it?\nIt can't be undone."} />)).toBe("Remove it?\nIt can&#x27;t be undone.");
  });

  it("says, in the It may strip, where the draft sends it and what is made with it", () => {
    const strip = renderToStaticMarkup(
      <MayStrip
        deployed
        sent={{ k: ["made:@callers"], t: ["auth.example.com", "answer:@callers"] }}
        items={[
          { item: 'SecretRead("k")', change: "same" },
          { item: 'SecretRead("t")', change: "new" },
          { item: 'SecretRead("old")', change: "gone" },
          { item: "Clock", change: "same" },
        ]}
      />,
    );
    expect(strip).toContain("read the secret &quot;k&quot;; a hash or signature made with it goes to its callers");
    expect(strip).toContain("read the secret &quot;t&quot;, and send it to https://auth.example.com; what is answered to it goes to its callers");
    expect(strip.match(/hl-may__chip--secret/g)?.length).toBe(3);
    expect(strip).toContain('class="hl-may__chip hl-may__chip--same" title="Clock">read the clock');
  });
});

/** Guards: no page lists what a flow may do in words of its own making: each goes through `Said`
 *  (or `saidAsText`, where only text goes), so a secret's line is marked there too.
 *  Why: the lists were written one by one (the strip, a flow's status, its settings, the
 *  Inspector, the library, the host's holdings, the deploy question), each calling
 *  `describeItem`, and none marked a secret.
 *  If it fails: a file calls `describeItem` or `describeItemSending` and isn't named below. Show
 *  the item through `Said`, or name the file here with why its words aren't a listed line. */
describe("every list of what a flow may do", () => {
  const WORDS_ONLY: Record<string, string> = {
    "SecretSaid.tsx": "Said itself",
    "MayStrip.tsx": "its words go into Said",
    "App.tsx": "builds the Inspector's rows, which MayRows shows through Said",
    "canvasLevel.ts": "builds a level's rows for the same Inspector",
    "FlowCells.tsx": "a title's text: through saidAsText",
    "OverviewPage.tsx": "a secret chip of its own colour: through saidAsText",
    "useDeployFlow.ts": "a question's lines: through saidAsText",
    "LibraryPage.tsx": "matches a search against the words; the list itself goes through Said",
    "HostPages.tsx": "the policy's one-line summary of what deploys without asking",
    "PolicyViews.tsx": "its words go into Said",
    "timeline.ts": "the history's sentences of what a version or a restore changed: words inside a sentence, not a listed line",
    "useMarketplace.ts": "a question's lines: through saidAsText",
  };

  it("goes through Said", () => {
    const src = join(__dirname);
    const files = (dir: string): string[] =>
      readdirSync(dir, { withFileTypes: true }).flatMap((f) => (f.isDirectory() ? files(join(dir, f.name)) : /\.(ts|tsx)$/.test(f.name) && !/\.test\./.test(f.name) ? [join(dir, f.name)] : []));
    const calling = files(src)
      .filter((path) =>
        readFileSync(path, "utf8")
          .split("\n")
          // Used, called or handed on (`.map(describeItem)`): any mention that isn't its import.
          .some((line) => /\bdescribeItem(Sending)?\b/.test(line) && !/^\s*(import|export)\b/.test(line) && !/^\s*(\/\/|\*|\/\*)/.test(line)),
      )
      .map((path) => path.slice(src.length + 1).replace(/\\/g, "/"));
    expect(calling.filter((f) => !(f in WORDS_ONLY))).toEqual([]);
    // And none is named that no longer calls them.
    expect(Object.keys(WORDS_ONLY).filter((f) => !calling.includes(f))).toEqual([]);
    // Those named for saidAsText use it.
    for (const f of ["FlowCells.tsx", "OverviewPage.tsx", "useDeployFlow.ts", "useMarketplace.ts"]) expect(readFileSync(join(src, f), "utf8"), f).toContain("saidAsText(");
  });
});
