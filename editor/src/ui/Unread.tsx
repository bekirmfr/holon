// A list that couldn't be read says so, and why, where it would have said it was empty: "nothing
// waits" from a list never read tells whoever decides that nothing does (R44). `polled`: it is read
// again by itself, every few seconds, so it says so; else reopening the page reads it again.

export function Unread({ what, why, polled = false }: { what: string; why: string; polled?: boolean }) {
  return (
    <p role="alert" className="hl-flow__bad hl-unread">
      Couldn&apos;t read {what}: {why}. {polled ? "It's read again every few seconds." : "Open the page again to read it again."}
    </p>
  );
}
