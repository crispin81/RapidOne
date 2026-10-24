/**
 * A tiny Vite plugin: keeps RapidOne's own stylesheet (src/App.css) out of the
 * embedded Cull and Retouch screens. Those modules (RapidCulling,
 * RapidRetouch) share many class names with RapidOne (.thumb, .titlebar,
 * .filmstrip, .viewer ...), so every RapidOne rule is made to stop at a
 * module's root element: each selector gets
 * `:where(:not(.rapidculling *, .rapidretouch *))` on its subject. `:where`
 * adds no specificity, so RapidOne's own cascade is unchanged. The source CSS
 * stays as written.
 */
import type { Plugin } from "vite";

const GUARD = ":where(:not(.rapidculling *, .rapidretouch *))";
/** Selectors left global: the document itself. */
const GLOBAL = /^(:root|html|body|#root)\b/;

/** Adds the guard to one selector, before any trailing ::pseudo-element. */
function guardSelector(sel: string): string {
  const s = sel.trim();
  if (!s || GLOBAL.test(s) || /^(from|to|\d+%)$/.test(s)) return s;
  const m = s.match(/^(.*?)(::[a-zA-Z-]+(\([^)]*\))?)$/);
  return m ? `${m[1]}${GUARD}${m[2]}` : `${s}${GUARD}`;
}

/** Splits a selector list on top-level commas (not inside brackets). */
function splitList(list: string): string[] {
  const out: string[] = [];
  let depth = 0;
  let cur = "";
  for (const ch of list) {
    if (ch === "(" || ch === "[") depth++;
    if (ch === ")" || ch === "]") depth--;
    if (ch === "," && depth === 0) {
      out.push(cur);
      cur = "";
    } else cur += ch;
  }
  out.push(cur);
  return out;
}

/** Walks the stylesheet, guarding style-rule selectors (inside @media too, not @keyframes / @font-face). */
export function scopeCss(css: string): string {
  let out = "";
  let i = 0;
  const walk = (inKeyframes: boolean): void => {
    while (i < css.length) {
      // Copy comments as they are.
      if (css.startsWith("/*", i)) {
        const end = css.indexOf("*/", i + 2);
        const stop = end < 0 ? css.length : end + 2;
        out += css.slice(i, stop);
        i = stop;
        continue;
      }
      if (css[i] === "}") {
        out += "}";
        i++;
        return;
      }
      // A prelude: up to the next "{" or ";" (an at-statement).
      let j = i;
      while (j < css.length && css[j] !== "{" && css[j] !== ";" && css[j] !== "}" && !css.startsWith("/*", j)) j++;
      if (j >= css.length || css[j] === "}" || css.startsWith("/*", j)) {
        out += css.slice(i, j);
        i = j;
        continue;
      }
      const prelude = css.slice(i, j);
      if (css[j] === ";") {
        out += prelude + ";";
        i = j + 1;
        continue;
      }
      const head = prelude.trim();
      if (head.startsWith("@")) {
        out += prelude + "{";
        i = j + 1;
        if (/^@(media|supports|layer|container)/.test(head)) walk(false);
        else walk(/^@(-webkit-)?keyframes/.test(head) || head.startsWith("@font-face"));
        continue;
      }
      // A style rule: guard its selectors, copy its block.
      const lead = prelude.match(/^\s*/)![0];
      out += inKeyframes ? prelude : lead + splitList(prelude).map(guardSelector).join(", ") + " ";
      const close = css.indexOf("}", j);
      out += css.slice(j, close + 1);
      i = close + 1;
    }
  };
  walk(false);
  return out;
}

export default function scopeAwayFromModules(): Plugin {
  return {
    name: "rapidone-scope-css",
    enforce: "pre",
    transform(code, id) {
      // RapidOne's own sheets that only the Edit screen uses. strip.css is
      // left global on purpose: Retouch's library header uses the filmstrip's filter bar.
      if (/[\\/]src[\\/](App|library)\.css(\?|$)/.test(id)) return { code: scopeCss(code), map: null };
      return null;
    },
  };
}
