import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { FolderPlus, Plus, Sparkles, X } from "lucide-react";
import type { AutoStyle } from "../api";

/** One photo, every photo, or every photo levelled too (one render each). */
export type AutoStyleScope = "photo" | "all" | "all-straighten";

interface Props {
  styles: AutoStyle[];
  /** No folder open: styles can't be applied (training still works). */
  applyDisabled: boolean;
  /** "Applying style 12/120" or "Training: reading 40%" while running, else null. */
  progress: string | null;
  onApply: (style: AutoStyle, scope: AutoStyleScope) => void;
  /** Train Style: a new style from folders of edited RAWs. */
  onTrain: (name: string, folders: string[]) => void;
  /** Increase training: more folders added to a style, retrained on all. */
  onIncrease: (style: AutoStyle, folders: string[]) => void;
}

const plural = (n: number, word: string) => `${n.toLocaleString()} ${word}${n === 1 ? "" : "s"}`;

async function pickFolders(title: string): Promise<string[]> {
  const chosen = await open({ directory: true, multiple: true, title });
  if (!chosen) return [];
  return (Array.isArray(chosen) ? chosen : [chosen]).map((f) => f.replace(/\\/g, "/"));
}

/**
 * Style: Apply Style sets the style's look, then each photo's white balance,
 * tint and exposure as the photographer would, learned from their own edits.
 * Train Style makes a style from folders of edited RAWs (named first);
 * Increase training (+ beside a style) adds more folders to one.
 * All photos + straighten also levels each photo as it goes (this photo's own Auto level and
 * keystone are in the Keystone panel).
 */
export default function AutoStyleMenu({
  styles,
  applyDisabled,
  progress,
  onApply,
  onTrain,
  onIncrease,
}: Props) {
  const [open_, setOpen] = useState(false);
  const [training, setTraining] = useState(false);
  const [name, setName] = useState("");
  const [folders, setFolders] = useState<string[]>([]);
  const ref = useRef<HTMLDivElement>(null);
  // The folder dialog is open: a click outside then isn't a reason to close.
  const picking = useRef(false);

  useEffect(() => {
    if (!open_) return;
    const onDown = (e: PointerEvent) => {
      if (!picking.current && ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("pointerdown", onDown);
    return () => window.removeEventListener("pointerdown", onDown);
  }, [open_]);

  const pick = async (title: string) => {
    picking.current = true;
    try {
      return await pickFolders(title);
    } finally {
      picking.current = false;
    }
  };

  const apply = (style: AutoStyle, scope: AutoStyleScope) => {
    setOpen(false);
    onApply(style, scope);
  };

  const addFolders = async () => {
    const more = await pick("Folders of your edited RAWs (Lightroom / Aftershoot .xmp edits or RapidOne edits)");
    setFolders((f) => [...f, ...more.filter((m) => !f.includes(m))]);
  };

  const increase = async (style: AutoStyle) => {
    const more = await pick(`More edited folders for "${style.name}"`);
    if (more.length === 0) return;
    setOpen(false);
    onIncrease(style, more);
  };

  const train = () => {
    setOpen(false);
    setTraining(false);
    onTrain(name.trim(), folders);
    setName("");
    setFolders([]);
  };

  const busy = progress;
  const nameTaken = styles.some((s) => s.name.trim().toLowerCase() === name.trim().toLowerCase());

  return (
    <div className="topbar__menu" ref={ref}>
      <button
        type="button"
        className={busy ? "is-on" : undefined}
        disabled={Boolean(busy)}
        onClick={() => setOpen((o) => !o)}
        title="Style: your look, plus white balance and exposure as you'd set them, learned from your own edits (and every photo levelled, if you like)"
        aria-label="Style"
      >
        {/* Its name shows (the other tools are icons only), as Export's does. */}
        <Sparkles size={14} /> {busy ?? <span>Teach / apply look</span>}
      </button>
      {open_ && (
        <div className="topbar__dropdown autostyle__menu" role="menu">
          <p className="autostyle__heading">Apply Style</p>
          {styles.length === 0 && <p className="autostyle__empty">No styles yet: train one from folders of your edits.</p>}
          {styles.map((s) => (
            <div key={s.path} className="autostyle__style">
              {/* Name, then what it learned from with + (more training) beside it. */}
              <div className="autostyle__info">
                <span className="autostyle__name" title={s.name}>
                  {s.name}
                </span>
                <span className="autostyle__count">
                  {s.photos > 0 && (
                    <small>
                      {plural(s.photos, "photo")}, {plural(s.shoots, "shoot")}
                    </small>
                  )}
                  <button
                    type="button"
                    className="autostyle__increase"
                    disabled={s.photos === 0}
                    onClick={() => void increase(s)}
                    title={
                      s.photos === 0
                        ? "This style was made before styles kept their training photos: train a new one to add to it"
                        : `Increase training: add more edited folders to "${s.name}" and retrain`
                    }
                    aria-label={`Increase training for ${s.name}`}
                  >
                    <Plus size={12} />
                  </button>
                </span>
              </div>
              <button type="button" disabled={applyDisabled} onClick={() => apply(s, "all")} title="Apply this style to every photo in the folder">
                All photos
              </button>
              <button
                type="button"
                disabled={applyDisabled}
                onClick={() => apply(s, "all-straighten")}
                title="Apply this style to every photo in the folder and level each one (cropping out the white the turn uncovers)"
              >
                All photos + straighten
              </button>
            </div>
          ))}
          {!training ? (
            <button type="button" className="autostyle__train" onClick={() => setTraining(true)} title="Learn a new style from folders of your edited photos">
              Train new style…
            </button>
          ) : (
            <div className="autostyle__form">
              <p className="autostyle__heading">Train new style</p>
              <input
                type="text"
                value={name}
                autoFocus
                placeholder="Style name, e.g. Weddings"
                onChange={(e) => setName(e.currentTarget.value)}
                onKeyDown={(e) => e.stopPropagation()}
              />
              {nameTaken && <p className="autostyle__warn">A style with that name exists: use + beside it to add to it.</p>}
              {folders.map((f) => (
                <div key={f} className="autostyle__folder" title={f}>
                  <span>{f.split("/").slice(-2).join("/")}</span>
                  <button type="button" onClick={() => setFolders((all) => all.filter((x) => x !== f))} aria-label={`Remove ${f}`}>
                    <X size={11} />
                  </button>
                </div>
              ))}
              <button type="button" onClick={() => void addFolders()} title="Folders of RAWs you edited in Lightroom, Aftershoot or RapidOne (only edited RAWs are used, each with the edit Edit opens it with)">
                <FolderPlus size={13} /> {folders.length ? "Add more folders" : "Choose folders of your edits"}
              </button>
              <p className="autostyle__hint">One shoot per folder is fine; more shoots make it better (10-20 is ideal). Masks are ignored.</p>
              <div className="autostyle__actions">
                <button type="button" onClick={() => setTraining(false)}>
                  Cancel
                </button>
                <button type="button" className="is-primary" disabled={!name.trim() || nameTaken || folders.length === 0} onClick={train}>
                  Train
                </button>
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
