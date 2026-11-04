// What the admin page says of the server's own sweeps (storage.rs, docs/L4s/storage-L4.md).
import type { HostSweeps, SweepReport } from "./server";

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
