// What Getting started teaches: the first steps, and the docs, one topic each, in the words the
// editor itself uses. Kept here as content, apart from how it is shown (WelcomePage.tsx).

/** A first step: what to do, a button for it, and what shows it done (`Done`: a flow saved, one
 *  deployed, one that ran). */
export type Step = { title: string; body: string; action?: "samples" | "new" | "flows"; doneWhen?: keyof Done };
export type Done = { saved: boolean; deployed: boolean; ran: boolean };

export const START_STEPS: Step[] = [
  {
    title: "Open a sample",
    body: "The Library lists samples, one per feature: Hello API is the smallest. Opening one gives you an unsaved copy in a tab of its own.",
    action: "samples",
  },
  {
    title: "Deploy it",
    doneWhen: "deployed",
    body: "Press deploy. Holon shows what the flow may do (receive requests, read a secret, write a collection…) and asks you to approve it; then it runs, on the port its Settings say (the bottom pane, b).",
  },
  {
    title: "Send it a request",
    doneWhen: "ran",
    body: "Use Try a request, in the editor's bottom pane (send local, send live; :call in the palette opens it), or curl, as each sample's \"try\" line says. Every run is kept.",
  },
  {
    title: "Look at the run",
    body: "Open the flow from Flows: its Runs tab lists every run, and a run shows what came in, each step and what went out. Step through it in the editor, and a replay checks the run reproduces exactly.",
    action: "flows",
  },
  {
    title: "Build your own",
    doneWhen: "saved",
    body: "Press new, drag nodes from the palette on the left and wire an output to an input of the same type. Save it, deploy it, and it runs the same way.",
    action: "new",
  },
];

export type Topic = { title: string; paragraphs: string[] };

export const TOPICS: Topic[] = [
  {
    title: "Flows and nodes",
    paragraphs: [
      "A flow is nodes wired together. Each node is a small, pure operation (add, concat, field, json_parse…) or a template built from them (request, respond). Its inputs are on the left, its outputs on the right, coloured by type; a wire joins an output to an input of a type that fits.",
      "Params are set on the node itself. The ⓘ on a node, and on each palette entry, says what it does.",
    ],
  },
  {
    title: "Routes and servers",
    paragraphs: [
      "A request node (a method and a path, like GET /hello/:name) wired to a respond node makes a route; a flow with routes is a server, answering on the port its Settings say. The editor's bottom pane lists its routes under Try a request, and sends it requests.",
      "\"Web pages from\" in its Settings lets pages on other sites call it.",
    ],
  },
  {
    title: "Saving, tabs and modes",
    paragraphs: [
      "Save names a flow; each open flow has a tab. A tab is in edit mode (the palette, save, deploy) or view mode (read only: the deployed version, with its runs). Undo and redo work per tab.",
    ],
  },
  {
    title: "Deploying and permissions",
    paragraphs: [
      "What a flow may do is worked out from the flow itself: its manifest. Deploying shows it and asks your approval; the server grants only that, and only what its policy allows. Some permissions can be optional: the flow runs without them.",
      "Once deployed, a flow can be paused, stopped (its state kept), updated in place, or deleted, from its tab or its page under Flows.",
    ],
  },
  {
    title: "Runs, replay and view mode",
    paragraphs: [
      "Every run is logged with what it received, what it did outside (requests, writes) and what it answered. View mode replays a run on the version it ran on, shows each node's values, and steps through it; a failed run opens where it failed.",
      "The history button, beside the runs' filter, shows every version deployed and what was done to it (grants, secrets, pauses), by whom. Pick any row there to see its details and restore its code, its state or both, or press ↺ on a row (there or in the run list) to go back to it: a dry run shows what it would change, and its diff compares each part (the flow's text, the grant, the state's values) with what is there now, and names the files the runs since wrote, which a restore leaves as they are, saying when one was changed outside a run; nothing changes until you approve it. ⧗ beside a cell or an item in the Flow state lists the runs that changed it, each with its value before and after, a restore between them included; a run's row opens it.",
    ],
  },
  {
    title: "Holons: flows inside flows",
    paragraphs: [
      "Select nodes and group them into one node: a holon. Select it to expand it into a canvas of its own (edit inside, then apply: a new version) or ungroup it in place. A saved flow without routes can be placed as a node from the palette's flows; ↻ on it takes its newest save.",
      "A holon is identified by its content: change anything inside and it is a new version, while flows using the old one keep it until they update.",
    ],
  },
  {
    title: "Flows as text",
    paragraphs: [
      "Any flow can be written as a TypeScript file, NAME.holon.ts: flow(\"hello\") and its handlers, route.get(\"/hello/:name\", (req) => …), each const a node. It's the same flow: drawn or written, it compiles to the same unit. Export one from the flows page (.holon.ts) or import one (import text); edit it in any editor and keep it in git.",
      "On any machine, a screen or not: holon new, holon check (its routes, what it may do, or each mistake by line), holon show (an outline) and holon fmt (the canonical form). The holon command is the one the installer puts on your PATH, the same one that runs the server (holon serve): holon check my-flow.holon.ts, anywhere.",
    ],
  },
  {
    title: "Secrets",
    paragraphs: [
      "A secret node reads a secret by name; set its value in the editor's Settings → secrets, per deployment. Values are sealed on the server, versioned, can be rolled back (a dry run says what it would do first), and are never shown again or logged.",
    ],
  },
  {
    title: "The marketplace",
    paragraphs: [
      "A marketplace is itself a Holon flow (the Marketplace sample). Set one in the Library's Marketplace tab to publish saved flows to it (from the flows page) and place its holons from the palette. Placing one shows what it may do, and checks every part against its hash.",
    ],
  },
  {
    title: "Accounts and roles",
    paragraphs: [
      "Viewers read; editors also save, deploy and run; admins also write secrets and manage the server from People and access: people (invited with a one-time link), the agents and services they own with their tokens, and signing in through an identity provider.",
    ],
  },
];
