// For tests: an answer the server holds, as on a connection it left unread (R97), from a stand-in
// `fetch`. Every stand-in that holds an answer holds it here, so that each behaves as a browser's
// fetch does when its request is given up: its promise rejects at once, inside `abort()` itself,
// with the signal's reason (an AbortError DOMException when none is given), before anything after
// the abort has run. A stand-in that only held its answer, or Node's own fetch (which settles a
// few turns later), let the editor's own timer win where a browser's fetch wins: the editor said
// a give-up right in its tests and took it for another failure in Chrome (server.ts's `once`).

/** A request whose answer is held: `answered` is what the stand-in `fetch` returns; `answer`
 *  gives the answer, `fail` fails it as a dropped connection does (a `TypeError`), and `dropped`
 *  says whether the editor gave it up. Once given up, neither lands: a browser's fetch never
 *  answers a request aborted before its answer came. */
export type Held = {
  answered: Promise<Response>;
  answer: (res: Response) => void;
  fail: (e?: unknown) => void;
  dropped: () => boolean;
};

export function held(init?: RequestInit): Held {
  const signal = init?.signal ?? undefined;
  let answer: (res: Response) => void = () => undefined;
  let fail: (e?: unknown) => void = () => undefined;
  const answered = new Promise<Response>((resolve, reject) => {
    answer = resolve;
    fail = (e = new TypeError("Failed to fetch")) => reject(e);
    if (!signal) return;
    if (signal.aborted) return reject(signal.reason);
    signal.addEventListener("abort", () => reject(signal.reason), { once: true });
  });
  return { answered, answer, fail, dropped: () => signal?.aborted === true };
}
