// How long the editor gives the server to answer, in one place: server.ts holds its requests to
// them, and the end-to-end tests read them too (e2e/fixtures.ts's WAITS), so a test never gives up
// on what the page still waits for. Nothing here reads the page's build (import.meta.env), so the
// tests, run outside it, import it as it is.

/** How long the server is given to answer, by what is asked (ms). On a busy machine the server's
 *  HTTP library can leave a new connection unread until another one closes (tiny_http's pool of
 *  connection threads): a request written into it gets no answer at all, neither a status nor an
 *  error, and a page waited on it for a minute or more, its buttons disabled (R97). Past its time a
 *  request is given up and its connection dropped (aborted), so what is asked next goes on another:
 *  - `read`, a read of what the server holds, answered in milliseconds when it is: asked again
 *    (READ_AGAIN_MS), its first answer never taken if it comes late; meanwhile the page says it
 *    can't reach the server and is trying again (server.ts's `askingAgainNow`, can.tsx's
 *    UNREACHED);
 *  - `work`, a read that has the server work first (a version replayed on recent runs, a
 *    database's tables, a run's export, the logs): not asked again, which would redo the work;
 *  - `change`, everything else: never sent twice, since it may have been done; given up, it is
 *    said so (server.ts's `Unanswered`), and the pages show what the server holds at their next
 *    read.
 *  The host's folder picker waits on a person, as long as they take ("a person"). Changed only by
 *  tests. */
export const ANSWER_WITHIN = { read: 8_000, work: 120_000, change: 120_000 };
/** After a read ran out, how long before each time it is asked again: twice. */
export const READ_AGAIN_MS = [1_000, 3_000];
