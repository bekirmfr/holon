// What the welcome tab teaches: the first steps, and the docs, one topic each, in the words the
// editor itself uses. Kept here as content, apart from how it is shown (WelcomePage.tsx).

export type Step = { title: string; body: string; action?: "samples" | "new" | "deployed" | "flows" };

export const START_STEPS: Step[] = [
  {
    title: "Open a sample",
    body: "The flows page lists samples, one per feature: Hello API is the smallest. Opening one gives you an unsaved copy in a tab of its own.",
    action: "samples",
  },
  {
    title: "Deploy it",
    body: "Press deploy. Holon shows what the flow may do (receive requests, read a secret, write a collection…) and asks you to approve it; then it runs, on the port set under ⚙.",
  },
  {
    title: "Send it a request",
    body: "Use the server panel at the bottom left (send local, send live), or curl, as each sample's \"try\" line says. Every run is kept.",
  },
  {
    title: "Look at the run",
    body: "Switch the tab to view. Pick a run in the log: the canvas shows what each node received and produced, and you can step through it. A replay checks the run reproduces exactly.",
    action: "deployed",
  },
  {
    title: "Build your own",
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
      "A request node (a method and a path, like GET /hello/:name) wired to a respond node makes a route; a flow with routes is a server, answering on the port set under ⚙. The server panel lists its routes and lets you send it requests.",
      "\"Web pages from\" under ⚙ lets pages on other sites call it.",
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
      "Once deployed, a flow can be paused, stopped (its state kept), updated in place, or deleted, from its tab or the deployed page.",
    ],
  },
  {
    title: "Runs, replay and view mode",
    paragraphs: [
      "Every run is logged with what it received, what it did outside (requests, writes) and what it answered. View mode replays a run on the version it ran on, shows each node's values, and steps through it; a failed run opens where it failed.",
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
    title: "Secrets",
    paragraphs: [
      "A secret node reads a secret by name; set its value under ⚙ → secrets, per deployment. Values are sealed on the server, versioned, can be rolled back, and are never shown again or logged.",
    ],
  },
  {
    title: "The marketplace",
    paragraphs: [
      "A marketplace is itself a Holon flow (the Marketplace sample). Set one on the flows page to publish saved flows to it and place its holons from the palette. Placing one shows what it may do, and checks every part against its hash.",
    ],
  },
  {
    title: "Accounts and roles",
    paragraphs: [
      "Viewers read; editors also save, deploy and run; admins also write secrets and manage the server from the admin panel: people (invited with a one-time link), API tokens for scripts, and signing in through an identity provider.",
    ],
  },
];
