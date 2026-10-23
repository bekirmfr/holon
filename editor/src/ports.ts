// How a variadic node names the input it gains (PrimitiveNode's "+", the text format's wires):
// the next letter while they are single letters (a, b → c), else `in<n>`. Pure, shared.

export function nextPortName(existing: string[]): string {
  const isSingleLetter = existing.length > 0 && existing.every((n) => /^[a-z]$/.test(n));
  if (isSingleLetter) {
    const max = Math.max(...existing.map((n) => n.charCodeAt(0)));
    if (max < "z".charCodeAt(0)) return String.fromCharCode(max + 1);
  }
  return `in${existing.length + 1}`;
}
