// Long ids in a line of text (a run's or a unit's hash, 64 hex characters) said by their first
// eight, as the editor says a unit elsewhere: the whole id is for a title, not the line.

const LONG_ID = /\b[0-9a-f]{16,}\b/g;

/** `text` with each long hex id cut to its first eight characters and "…". */
export const shortIds = (text: string): string => text.replace(LONG_ID, (id) => `${id.slice(0, 8)}…`);
