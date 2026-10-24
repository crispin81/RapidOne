import { useEffect, useRef, useState } from "react";
import { Coffee } from "lucide-react";
import { openExternal } from "../openExternal";
import { COFFEE_URL } from "./About";

/** The home screen's coffee line: shown from the first public release (Chris, 24 Oct). */
const SHOW_COFFEE = true;

const CHAR_MS = 21;
const LINE_PAUSE_MS = 195;

interface Props {
  /**
   * The working folder's name, once one is open (the home screen): the
   * prompt then offers to resume editing, or to open another folder.
   */
  folderName?: string | null;
  /** The first choice: open a folder, or (home screen) resume editing. */
  onPrompt: () => void;
  /** The home screen's second choice: open another folder. */
  onOpenFolder?: () => void;
  /** A folder being read (it's opened, no photo ready yet): its name and how far along. */
  loading?: { folderName: string; done: number; total: number } | null;
  /** On a fresh start (no folder open): pick up where the last session left off, next to "open working folder". */
  resume?: { name: string; run: () => void } | null;
}

const BAR_CELLS = 20;

/** "[██████░░░░] 40/120", the folder-reading progress as a terminal would show it. */
function progressText(done: number, total: number): string {
  const filled = total > 0 ? Math.round((done / total) * BAR_CELLS) : 0;
  return `[${"█".repeat(filled)}${"░".repeat(BAR_CELLS - filled)}] ${done}/${total || "?"}`;
}

/**
 * Start / home screen prompt in the style of a terminal: the lines type
 * themselves out, then a blinking block cursor waits on the chosen line.
 * ↑/↓ move between the choices, Enter or a click runs one.
 */
export default function TerminalPrompt({ folderName, onPrompt, onOpenFolder, loading, resume }: Props) {
  const home = Boolean(folderName);
  const intro = [
    "RapidOne // RAW engine online",
    // Nothing to say about the folder on a fresh start: just the choices below.
    ...(home ? [`> working folder: ${folderName}`] : loading ? [`> reading folder: ${loading.folderName}`] : []),
  ];
  const choices: { text: string; title: string; run: () => void }[] = home
    ? [
        { text: "> resume editing", title: "Back to the photo (Enter)", run: onPrompt },
        ...(onOpenFolder ? [{ text: "> open another folder", title: "Choose another working folder", run: onOpenFolder }] : []),
      ]
    : resume && !loading
      ? [
          // The default (first, so Enter runs it) when there's a last session to go back to.
          { text: "> resume last session", title: `Open ${resume.name} again, at the photo you were on (Enter)`, run: resume.run },
          { text: "> open working folder", title: "Choose a different working folder", run: onPrompt },
        ]
      : [{ text: "> open working folder", title: "Choose the working folder (Enter)", run: onPrompt }];
  const lines = [...intro, ...choices.map((c) => c.text)];

  const reduced = typeof window !== "undefined" && window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
  const total = lines.reduce((n, l) => n + l.length, 0);
  const [typed, setTyped] = useState(reduced ? total : 0);
  const [selected, setSelected] = useState(0);

  useEffect(() => {
    if (typed >= total) return;
    // Pause briefly at the end of each line, as if the machine were thinking.
    let ends = 0;
    const atLineEnd = lines.some((l) => (ends += l.length) === typed && typed > 0);
    const t = window.setTimeout(() => setTyped((n) => n + 1), atLineEnd ? LINE_PAUSE_MS : CHAR_MS);
    return () => window.clearTimeout(t);
  }, [typed, total, lines]);

  // Latest choices and selection for the key handler, registered once.
  const live = useRef({ choices, selected });
  live.current = { choices, selected };
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      const { choices, selected } = live.current;
      if (e.key === "Enter") {
        e.preventDefault();
        choices[Math.min(selected, choices.length - 1)].run();
      } else if (e.key === "ArrowUp" || e.key === "ArrowDown") {
        // Ours alone: the app's own ↑/↓ (next photo, slider nudge) mustn't also run.
        e.preventDefault();
        e.stopImmediatePropagation();
        const step = e.key === "ArrowDown" ? 1 : -1;
        setSelected((s) => (s + step + choices.length) % choices.length);
      }
    }
    // Capture phase, so this runs before the app's handler on the same window.
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);

  // Split what has been typed so far back into lines.
  let left = typed;
  const shown = lines.map((l) => {
    const part = l.slice(0, Math.max(0, Math.min(l.length, left)));
    left -= l.length;
    return part;
  });
  const done = typed >= total;

  return (
    <div className="terminal" role="group" aria-label={home ? "Home" : "Choose working folder"}>
      {intro.map(
        (_, i) =>
          shown[i].length > 0 && (
            <div key={i} className={`terminal__line${i === 0 ? " terminal__line--title" : ""}`}>
              {shown[i]}
            </div>
          ),
      )}
      {/* Live, not typed out: it changes as photos are read. */}
      {loading && shown[intro.length - 1].length === intro[intro.length - 1].length && (
        <div className="terminal__line terminal__progress">{progressText(loading.done, loading.total)}</div>
      )}
      {choices.map((c, i) => {
        const text = shown[intro.length + i];
        // The first choice shows (boxed) from the start of its typing; later ones once typed.
        if (i > 0 && text.length === 0) return null;
        const on = i === selected;
        return (
          <button
            key={c.text}
            type="button"
            className={`terminal__line ${on ? "terminal__prompt" : "terminal__alt"}`}
            onClick={c.run}
            onMouseEnter={() => setSelected(i)}
            title={c.title}
          >
            {text}
            {on && <span className={`terminal__cursor${done ? " terminal__cursor--blink" : ""}`} aria-hidden="true" />}
          </button>
        );
      })}
      {/* At the bottom, once the prompt has typed itself out. */}
      {SHOW_COFFEE && done && (
        <button type="button" className="terminal__line terminal__coffee" onClick={() => openExternal(COFFEE_URL)} title={COFFEE_URL}>
          Buy the developer, Chris a <Coffee size={16} aria-label="coffee" />
        </button>
      )}
    </div>
  );
}
