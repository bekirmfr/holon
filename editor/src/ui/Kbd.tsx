// A key, shown as a small chip beside what it does. Hidden from assistive technology: the
// button's name stays its label alone ("Cancel", not "n Cancel"), and the key is said by
// `aria-keyshortcuts` instead.

export function Kbd({ k }: { k: string }) {
  return (
    <kbd className="hl-kbd" aria-hidden="true">
      {k}
    </kbd>
  );
}
