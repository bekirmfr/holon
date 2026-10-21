// Snapshot undo/redo for the canvas, kept free of React so it can be tested directly. The canvas
// commits a snapshot 400 ms after edits settle (so a drag is one entry, not sixty); `undo` and
// `redo` take the canvas as it is *now*, so an edit still inside that window is committed first
// instead of being lost.

export class History<S> {
  past: S[] = [];
  future: S[] = [];

  constructor(
    public committed: S,
    private readonly same: (a: S, b: S) => boolean,
    private readonly limit = 50,
  ) {}

  /** Start over from `s`, e.g. after New or Open. */
  reset(s: S): void {
    this.past = [];
    this.future = [];
    this.committed = s;
  }

  /** Record `s` as a new edit. A no-op if nothing changed; otherwise clears redo. */
  commit(s: S): boolean {
    if (this.same(s, this.committed)) return false;
    this.past.push(this.committed);
    if (this.past.length > this.limit) this.past.shift();
    this.future = [];
    this.committed = s;
    return true;
  }

  /** The snapshot to restore, or null if there is nothing to undo. `current` is the canvas now. */
  undo(current: S): S | null {
    this.commit(current);
    const prev = this.past.pop();
    if (prev === undefined) return null;
    this.future.push(this.committed);
    this.committed = prev;
    return prev;
  }

  /** The snapshot to restore, or null. An uncommitted edit counts as a new edit, which ends redo. */
  redo(current: S): S | null {
    this.commit(current);
    const next = this.future.pop();
    if (next === undefined) return null;
    this.past.push(this.committed);
    this.committed = next;
    return next;
  }
}
