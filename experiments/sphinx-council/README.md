# The Sphinx's council (an experiment)

A trial of the chess sample's possible "each piece is an agent" mode, played in Claude Code rather
than through the Holon server. White is the session's own Claude. Black is the Sphinx (the
`sphinx` agent, `.claude/agents/sphinx.md`), advised each round by a council: one `chess-piece`
agent (`.claude/agents/chess-piece.md`, Haiku) per Black piece with a legal move, each given only
its own legal moves. Subagents can't spawn agents here, so the session convenes the council for
the Sphinx and hands it what each piece said, verbatim; the Sphinx decides.

Every move goes through the chess sample's own `rules` holon, which `game.js` reads from
`crates/holon-author/samples/chess.holon` at startup, so the game is held to the same rules as the
flow, and the Sphinx to the same contract: a move that isn't one of the legal moves is "broken",
and the rules' fallback is played instead.

- `game.js`: `node game.js show | player <move> | sphinx <SAN> | new`
- `state.json`: the game (the rules' board, with the Sphinx's turns)
- `game-log.md`: each round's council, the Sphinx's choice and deliberation, and token counts

This is not a sample flow: nothing here is deployed or tested. Agent definitions in
`.claude/agents/` are read when a session starts, not mid-session.
