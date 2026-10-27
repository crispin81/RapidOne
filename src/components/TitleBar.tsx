import { getVersion } from "@tauri-apps/api/app";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useState } from "react";
import { Minus, Square, X } from "lucide-react";
import About from "./About";
import RenderMenu from "./RenderMenu";
import { openExternal } from "../openExternal";

/** Chris's RapidOne tutorials on YouTube (the Learn button). */
const LEARN_URL = "https://www.youtube.com/playlist?list=PLC5lWfy1MHI8";

const appWindow = getCurrentWindow();

interface Props {
  /** A folder being read (Edit's scan): its loading bar, right of About. */
  progress?: { done: number; total: number } | null;
  /** Retouch's status line (engine, progress, errors) goes here, in Retouch only: in the right half,
   *  between the title and the window buttons, clear of the top bar's tools. */
  statusSlot?: (el: HTMLDivElement | null) => void;
  /** A retouched photo coming back into Edit (saving, adding, opening): shown first. */
  handBack?: { label: string; fraction: number } | null;
  /** The tab showing: a module's loading shows only on its own tab (Cull's in Cull, Retouch's in Retouch). */
  mode?: "cull" | "edit" | "retouch";
}

/** Something loading: a label, and how far (done of total, or a fraction; neither: just working). */
interface Loading {
  label: string;
  done?: number;
  total?: number;
  fraction?: number | null;
}

/**
 * The window's own title bar: About, the loading bar (whatever Edit, Cull or Retouch is loading:
 * Edit's folder read from `progress`, the modules' from their `rapidone:loading` events), the title,
 * and the window buttons.
 */
export default function TitleBar({ progress, statusSlot, handBack, mode }: Props) {
  const [about, setAbout] = useState(false);
  // The app's version (tauri.conf.json), as "V1.0" (the last digit only when it isn't 0).
  const [version, setVersion] = useState("");
  useEffect(() => {
    getVersion().then((v) => setVersion(v.replace(/^(\d+\.\d+)\.0$/, "$1")), () => undefined);
  }, []);
  const [modules, setModules] = useState<Record<string, Loading | null>>({});
  useEffect(() => {
    const on = (e: Event) => {
      const { source, loading } = (e as CustomEvent<{ source: string; loading: Loading | null }>).detail;
      setModules((m) => ({ ...m, [source]: loading }));
    };
    window.addEventListener("rapidone:loading", on);
    return () => window.removeEventListener("rapidone:loading", on);
  }, []);
  const loading: Loading | null = handBack
    ? handBack
    : progress && progress.done < progress.total
      ? { label: "Reading folder", done: progress.done, total: progress.total }
      : mode === "cull"
        ? (modules.cull ?? null)
        : mode === "retouch"
          ? (modules.retouch ?? null)
          : null;
  const fraction =
    loading?.fraction ?? (loading?.total ? (loading.done ?? 0) / loading.total : null);
  return (
    <div className="titlebar" data-tauri-drag-region>
      {/* About, top left: who made it, and everything each part is built on; Learn: the video tutorials. */}
      <div className="titlebar__links">
        <button type="button" className="titlebar__about" onClick={() => setAbout(true)} title="About RapidOne">
          About
        </button>
        <button type="button" className="titlebar__about titlebar__learn" onClick={() => openExternal(LEARN_URL)} title="Video tutorials on YouTube">
          Learn
          <svg className="titlebar__play" width="15" height="11" viewBox="0 0 15 11" aria-hidden="true">
            <rect x="0.75" y="0.75" width="13.5" height="9.5" rx="2.5" fill="#000" stroke="#ff0033" strokeWidth="1.5" />
            <path d="M6 3.2v4.6L9.9 5.5z" fill="#fff" />
          </svg>
        </button>
        <RenderMenu retouch={mode === "retouch"} />
        {loading && (
          // The orange-to-cyan loading bar (with its sweep), after Render in the same row, so it never runs
          // over or under the buttons; up here nothing covers it, on every tab.
          <span className="titlebar__scan" role="status" title={loading.label + (loading.total ? `: ${loading.done} of ${loading.total}` : "")}>
            <span className="titlebar__scan-label">{loading.label}</span>
            <span className="handback-progress__bar">
              <span className="handback-progress__fill" style={{ display: "block", width: `${Math.max(3, (fraction ?? 0.35) * 100)}%` }} />
            </span>
            {loading.total ? (
              <span className="titlebar__scan-count">
                {loading.done}/{loading.total}
              </span>
            ) : fraction != null ? (
              <span className="titlebar__scan-count">{Math.round(fraction * 100)}%</span>
            ) : null}
          </span>
        )}
      </div>
      {about && <About onClose={() => setAbout(false)} />}
      {statusSlot && <div className="titlebar__status" ref={statusSlot} />}
      <span className="titlebar__title" data-tauri-drag-region>
        RapidOne{version && ` // V${version}`}
      </span>
      <div className="titlebar__controls">
        <button type="button" onClick={() => appWindow.minimize()} title="Minimize">
          <Minus size={13} />
        </button>
        <button type="button" onClick={() => appWindow.toggleMaximize()} title="Maximize">
          <Square size={11} />
        </button>
        <button type="button" className="titlebar__close" onClick={() => appWindow.close()} title="Close">
          <X size={13} />
        </button>
      </div>
    </div>
  );
}
