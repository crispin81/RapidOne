/** A Mac (or iPad): shortcuts use ⌘ where other systems use Ctrl. */
export const IS_MAC = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

/**
 * A shortcut's label as this computer names it: every "Ctrl" is ⌘ on a Mac ("Ctrl+Z" -> "⌘Z",
 * "Ctrl+Shift+C" -> "⇧⌘C", "Ctrl-click" -> "⌘-click"); elsewhere unchanged. The keys themselves
 * accept either (e.ctrlKey || e.metaKey).
 */
export function keyLabel(text: string): string {
  if (!IS_MAC) return text;
  return text
    .replace(/Ctrl\+Shift\+/g, "⇧⌘")
    .replace(/Ctrl\+/g, "⌘")
    .replace(/\bCtrl\b/g, "⌘");
}
