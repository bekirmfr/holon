// What the admin page says of the server's own sweeps (storage.rs, docs/L4s/storage-L4.md).
import type { CacheUse, Holds, HostDisk, HostView, HostSweeps, SweepReport } from "./server";

/** A span of milliseconds, said roughly: "40 s", "7 min", "3 h", "2 days". */
export function span(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 90) return `${s} s`;
  const m = Math.round(s / 60);
  if (m < 90) return `${m} min`;
  const h = Math.round(m / 60);
  if (h < 36) return `${h} h`;
  return `${Math.round(h / 24)} days`;
}

/** A size in bytes as people read it (folders.rs's `size_text`): `512 bytes`, `64 KiB`, `1.5 MiB`. */
export function sizeText(bytes: number): string {
  for (const [size, unit] of [
    [2 ** 30, "GiB"],
    [2 ** 20, "MiB"],
    [2 ** 10, "KiB"],
  ] as const) {
    if (bytes >= size) {
      const n = bytes / size;
      return Number.isInteger(n) || n >= 100 ? `${n.toFixed(0)} ${unit}` : `${n.toFixed(1)} ${unit}`;
    }
  }
  return `${bytes} bytes`;
}

/** What one sweep did, `what` naming what it removes ("values", "file texts"), as of `now`. */
export function reportLine(report: SweepReport | null, what: string, now: number): string {
  if (!report) return "not swept yet";
  const ago = `${span(now - report.at_ms)} ago`;
  if (report.error) return `${ago}, failed: ${report.error}`;
  if (report.skipped) return `${ago}: nothing new to look at`;
  if (report.removed === 0) return `${ago}: nothing to remove (${span(report.took_ms)})`;
  return `${ago}: ${report.removed} ${what}, ${sizeText(report.bytes)} freed in ${span(report.took_ms)}`;
}

/** How often the server sweeps, and when next, as of `now`. */
export function scheduleLine(sweeps: HostSweeps, now: number): string {
  if (sweeps.every === "off") return "the server doesn't sweep on its own: only an admin's sweep removes anything";
  if (sweeps.sweeping) return `every ${sweeps.every}; sweeping now`;
  const next = sweeps.next_at_ms === null ? "" : sweeps.next_at_ms <= now ? "; the next is due now" : `; the next in ${span(sweeps.next_at_ms - now)}`;
  return `every ${sweeps.every}${next}`;
}

/** Whether `text` is a setting the server takes: `90s`, `10m`, `2h`, `1d` (a minute at the least), or `off`. */
export function isEvery(text: string): boolean {
  const t = text.trim();
  if (t === "off") return true;
  const m = /^(\d+)\s*([smhd])$/.exec(t);
  if (!m) return false;
  const seconds = Number(m[1]) * { s: 1, m: 60, h: 3600, d: 86400 }[m[2] as "s" | "m" | "h" | "d"];
  return seconds >= 60;
}

/** A cache's use against its size: "12 flows, 32 MiB of 256 MiB (12%)". */
export function cacheLine(c: CacheUse, what: string): string {
  const full = c.budget > 0 ? Math.floor((c.bytes * 100) / c.budget) : 0;
  return `${c.entries} ${what}, ${sizeText(c.bytes)} of ${sizeText(c.budget)} (${full}%)`;
}

/** How full a disk is for the server, 0 to 100: what isn't free to it (a quota, or the space kept for root, counts). */
export const diskFull = (d: HostDisk) => (d.size > 0 ? Math.ceil(((d.size - d.free) * 100) / d.size) : 0);

/** A disk as one line: "60% full: 40 GiB free of 100 GiB (60 GiB in use): data, logs, store". */
export function diskLine(d: HostDisk): string {
  return `${diskFull(d)}% full: ${sizeText(d.free)} free of ${sizeText(d.size)} (${sizeText(d.used)} in use): ${d.holds.join(", ")}`;
}

/** What the host keeps, a line each (the store, the logs, the folders' texts, what crashes left), as of `now`. */
export function keptLines(k: HostView["kept"], now: number): string[] {
  const lines = [k.at_ms === null ? "not counted yet: the server counts what it keeps shortly after it starts" : `counted ${span(now - k.at_ms)} ago`];
  if (k.store) lines.push(`the store: ${k.store.values} values, ${sizeText(k.store.on_disk)} on disk (${sizeText(k.store.bytes)} of bytes); ${k.store.put_since} put since`);
  if (k.logs) lines.push(`logs: ${k.logs.files}, ${sizeText(k.logs.on_disk)} on disk`);
  if (k.file_texts) lines.push(`file texts: ${k.file_texts.texts}, ${sizeText(k.file_texts.bytes)}`);
  const { removed, waiting } = k.leftovers;
  if (removed.files > 0 || waiting.files > 0) lines.push(`left by crashes: ${removed.files} files removed (${sizeText(removed.on_disk)}), ${waiting.files} still to remove`);
  return lines;
}

/** What one flow holds, as two lines: in memory, and on disk, as of `now`. */
export function holdsLines(h: Holds, now: number): [string, string] {
  const flow = h.flow_cache ? `its flow ${sizeText(h.flow_cache.own)}${h.flow_cache.shared > 0 ? ` (and ${sizeText(h.flow_cache.shared)} shared)` : ""}` : "its flow not cached now";
  const memory = `about ${sizeText(h.memory)} in memory: its log about ${sizeText(h.log.memory)} (the newest ${h.log.entries} of ${h.log.kept} runs), its state ${sizeText(h.state_memory)}, ${flow}`;
  const states =
    h.store && h.shares_at_ms !== null
      ? `its states ${h.store.values} values, ${sizeText(h.store.bytes)} (as the store's last sweep found, ${span(now - h.shares_at_ms)} ago)`
      : "its states not counted yet (the store's first sweep counts them)";
  const texts = h.file_texts && h.file_texts.texts > 0 ? `, file texts ${h.file_texts.texts} (${sizeText(h.file_texts.bytes)})` : "";
  return [memory, `${sizeText(h.disk)} on disk: its log ${sizeText(h.log.file.on_disk)} (${h.log.lines} lines), ${states}${texts}`];
}

/** Whether `text` is a threshold the server takes: `85%` full (1% to 99%), or `5G` free (`500M`, `1.5T`). */
export function isThreshold(text: string): boolean {
  const t = text.trim();
  const pct = /^(\d+)\s*%$/.exec(t);
  if (pct) return Number(pct[1]) >= 1 && Number(pct[1]) <= 99;
  const free = /^(\d+(\.\d+)?)\s*[kKmMgGtT]$/.exec(t);
  return !!free && Number(free[1]) > 0;
}
