// The Inspector's "it may here" (gaps-web.md G21): what the selected node lets the flow do, as
// the manifest says it (its unit's capabilities, by consent name), and which of its fields that
// comes from: a field whose value a manifest item names is a permission's, and one a
// `SecretRead` names is a secret's.

export type MayHere = { item: string; said: string; isNew: boolean };

/** What a field's value is to the manifest: named by an item (a permission's), and by a secret
 *  read (a secret's). A value is named as the item quotes it, or as it is when it is quoted
 *  already. */
export function fieldMarks(value: string, items: string[]): { permission: boolean; secret: boolean } {
  if (value === "") return { permission: false, secret: false };
  const quoted = value.startsWith('"') ? value : JSON.stringify(value);
  const naming = items.filter((i) => i.includes(`(${quoted})`) || i.includes(`(${quoted},`) || i.includes(`, ${quoted})`));
  return { permission: naming.length > 0, secret: naming.some((i) => i.startsWith("SecretRead(")) };
}
