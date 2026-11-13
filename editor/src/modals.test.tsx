// Every modal dialog in the editor keeps the focus while it is open (ui/focusTrap.ts).
//
// Guards: as it opens it takes the focus; Tab and Shift+Tab go round its own controls, the same
// ones each way, never out to the page beneath; a focus that fell out of it (a click on its text)
// comes back in on the next Tab; as it closes the focus goes back to the control that opened it;
// and of two open at once, the one opened last has the Tab until it closes.
// Why: the page beneath a modal is still there and still takes keys. A Tab that leaves the dialog
// puts the focus on a control the dialog hides, where Enter or Space acts unseen (a deploy, a
// delete); a keyboard or screen-reader user can't tell where they are. And a dialog that closes
// with the focus inside it drops the focus to the page's start.
// If it fails: the message names the modal and where the focus went; a modal added to the editor
// (`aria-modal="true"` in a source file) without a line in MODALS fails the first test, until it
// is given the trap and a line here.

import { readdirSync, readFileSync } from "node:fs";
import { join, relative } from "node:path";
import { act, useState, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { AccountDialog } from "./AuthGate";
import { CellHistoryButton } from "./CellHistory";
import { DialogProvider, useDialog } from "./Dialog";
import { useDryRun, type DryRunReport } from "./DryRun";
import { FolderPicker } from "./FolderPicker";
import type { WValue } from "./holon";
import { CommandPalette } from "./ui/CommandPalette";
import { KeysDialog } from "./ui/Keys";
import { lap, press, settle, what } from "./ui/pressKey";
import { ValueModal } from "./ViewPanes";

vi.mock("./server", async (actual) => ({
  ...(await actual<typeof import("./server")>()),
  getChanges: async () => ({
    name: "count",
    item: null,
    collection: false,
    now: { type: "Number", value: 3 },
    changes: [
      { seq: 7, at_ms: 1, kind: "request", before: { type: "Number", value: 2 }, after: { type: "Number", value: 3 } },
      { seq: 5, at_ms: 0, kind: "request", before: { type: "Number", value: 1 }, after: { type: "Number", value: 2 } },
    ],
    more: true,
  }),
  getVersions: async () => [],
  whoAmI: async () => ({ name: "ada", kind: "person", role: "admin" }),
  mayI: async () => ({}),
  browseFolders: async () => ({
    path: "/srv/data",
    parent: "/srv",
    roots: [{ name: "/", path: "/" }],
    folders: [
      { name: "in", path: "/srv/data/in" },
      { name: "out", path: "/srv/data/out" },
    ],
    more: false,
    native: false,
  }),
}));

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

let container: HTMLDivElement | null = null;
let root: Root | null = null;
afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  container = null;
});

/** A page with a button that opens `modal` (handed its close), and a control of its own after it:
 *  where a Tab that left the dialog would go. */
function Opens({ modal }: { modal: (close: () => void) => ReactNode }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button onClick={() => setOpen(true)}>open it</button>
      {open && modal(() => setOpen(false))}
      <button>the page's own button</button>
    </>
  );
}

/** A question of the dialogs', asked by the page's button. */
function Asks({ ask }: { ask: (d: ReturnType<typeof useDialog>) => Promise<unknown> }) {
  const d = useDialog();
  return (
    <>
      <button onClick={() => void ask(d)}>open it</button>
      <button>the page's own button</button>
    </>
  );
}

const REPORT: DryRunReport = {
  title: 'Restore "shop" to version 2?',
  sections: [{ heading: "what it does", lines: ["Its code goes back to version 2."] }],
  action: "Restore",
  diffs: async () => [
    {
      title: "code",
      legend: "- now, + version 2",
      lines: [
        { sign: " ", text: "flow shop" },
        { sign: "-", text: 'respond("hi")' },
        { sign: "+", text: 'respond("hello")' },
      ],
    },
  ],
};

function DryRunPage() {
  const { ask, modal } = useDryRun();
  return (
    <>
      <button onClick={() => void ask(REPORT)}>open it</button>
      {modal}
      <button>the page's own button</button>
    </>
  );
}

const RECORD: WValue = {
  type: "Record",
  value: [
    ["id", { type: "Number", value: 7 }],
    ["who", { type: "Record", value: [["name", { type: "Text", value: "Ada" }]] }],
  ],
};

/** Every modal dialog in the editor: its source file, and a page whose first button opens it. */
const MODALS: { modal: string; file: string; page: () => ReactNode }[] = [
  { modal: "confirm", file: "Dialog.tsx", page: () => <DialogProvider><Asks ask={(d) => d.confirm('Delete the saved flow "shop"?', "Delete")} /></DialogProvider> },
  { modal: "prompt", file: "Dialog.tsx", page: () => <DialogProvider><Asks ask={(d) => d.prompt("Name this flow:", "shop", "Name it")} /></DialogProvider> },
  { modal: "choice", file: "Dialog.tsx", page: () => <DialogProvider><Asks ask={(d) => d.choose("Close it?", ["Keep it open", "Discard", "Save and close"])} /></DialogProvider> },
  { modal: "notice", file: "Dialog.tsx", page: () => <DialogProvider><Asks ask={(d) => d.notice("Your password is changed.")} /></DialogProvider> },
  {
    modal: "account",
    file: "AuthGate.tsx",
    page: () => <DialogProvider><Opens modal={(close) => <AccountDialog me={{ name: "ada", kind: "person", role: "admin" }} onStart={() => {}} onClose={close} />} /></DialogProvider>,
  },
  { modal: "cell history", file: "CellHistory.tsx", page: () => <CellHistoryButton slug="shop" name="count" onOpenRun={() => {}} /> },
  { modal: "dry run", file: "DryRun.tsx", page: () => <DryRunPage /> },
  { modal: "folder picker", file: "FolderPicker.tsx", page: () => <Opens modal={(close) => <FolderPicker start="" onChoose={close} onClose={close} />} /> },
  { modal: "command palette", file: "ui/CommandPalette.tsx", page: () => <Opens modal={(close) => <CommandPalette commands={[{ label: "Flows", kind: "page", run: () => {} }]} onColon={() => null} onClose={close} />} /> },
  { modal: "keys", file: "ui/Keys.tsx", page: () => <Opens modal={(close) => <KeysDialog onClose={close} />} /> },
  { modal: "a value", file: "ViewPanes.tsx", page: () => <Opens modal={(close) => <ValueModal value={RECORD} portName="out" onClose={close} />} /> },
];

/** The modal open now, the one opened last if there are two. */
const modal = () => [...document.querySelectorAll('[role="dialog"][aria-modal="true"]')].at(-1) ?? null;

/** Show `page`, focus its first button and press it: the modal open, its data in. */
async function open(page: ReactNode): Promise<HTMLElement> {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(page));
  const opener = container.querySelector("button")!;
  await act(async () => {
    opener.focus();
    opener.click();
    await settle();
  });
  expect(modal(), "the page's first button opens it").not.toBeNull();
  return opener;
}

describe("modal dialogs keep the focus", () => {
  it("are all in the table", () => {
    const src = join(process.cwd(), "src");
    const files = (readdirSync(src, { recursive: true }) as string[])
      .filter((f) => /\.tsx$/.test(f) && !/\.test\.tsx$/.test(f))
      .filter((f) => /\saria-modal="true"/.test(readFileSync(join(src, f), "utf8")))
      .map((f) => relative(src, join(src, f)).replaceAll("\\", "/"));
    expect(files.sort()).toEqual([...new Set(MODALS.map((m) => m.file))].sort());
  });

  it.each(MODALS)("$modal: takes the focus, keeps Tab in it both ways, and gives the focus back", async ({ page }) => {
    const opener = await open(page());
    expect(modal()!.contains(document.activeElement), `focus on ${what(document.activeElement)}`).toBe(true);
    const forth = await lap(modal);
    const back = await lap(modal, true);
    expect(back.map(what)).toEqual([forth[0], ...forth.slice(1).reverse()].map(what));
    for (const shiftKey of [false, true]) {
      act(() => (document.activeElement as HTMLElement).blur());
      await press("Tab", { shiftKey });
      expect(modal()!.contains(document.activeElement), `${shiftKey ? "Shift+" : ""}Tab from outside, to ${what(document.activeElement)}`).toBe(true);
    }
    await press("Escape");
    expect(modal(), "Escape closes it").toBeNull();
    expect(document.activeElement, `focus on ${what(document.activeElement)} after it closed`).toBe(opener);
  });

  it("gives Tab to the one opened last, and back to the one beneath when it closes", async () => {
    let dialogs: ReturnType<typeof useDialog> | null = null;
    function Grab() {
      dialogs = useDialog();
      return null;
    }
    await open(
      <DialogProvider>
        <Grab />
        <Opens modal={(close) => <FolderPicker start="" onChoose={close} onClose={close} />} />
      </DialogProvider>,
    );
    const picker = modal()!;
    const inPicker = await lap(() => picker);
    const at = document.activeElement;
    let answer: unknown;
    await act(async () => {
      dialogs!.confirm("Bind it again?", "Bind again").then((a) => (answer = a));
      await settle();
    });
    const confirm = modal()!;
    expect(confirm).not.toBe(picker);
    expect(await lap(() => confirm), "its Cancel and its Bind again").toHaveLength(2);
    await press("Escape");
    expect(answer).toBe(false);
    expect(modal()).toBe(picker);
    expect(document.activeElement, "the focus is back in the picker where it was").toBe(at);
    expect((await lap(() => picker)).map(what)).toEqual(inPicker.map(what));
  });
});
