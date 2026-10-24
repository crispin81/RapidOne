import { useEffect, useRef, useState } from "react";
import { Download, Palette, Trash2, Upload } from "lucide-react";
import type { RapidPreset } from "../preset";

export type PresetScope = "photo" | "all";

interface Props {
  presets: RapidPreset[];
  disabled: boolean;
  onApply: (preset: RapidPreset, scope: PresetScope) => void;
  onSave: (name: string) => void;
  onDelete: (name: string) => void;
  /** Import presets: Lightroom's (.xmp, converted) or RapidOne backups (.rpp). */
  onImport: () => void;
  /** Copy every preset into a folder, as a backup. */
  onExport: () => void;
}

/**
 * Presets dropdown (after RapidRetouch's): every preset sorted by name,
 * click to apply (to this photo or all photos), delete with a second click,
 * save the current settings under a name (the same name replaces it), and
 * import Lightroom presets, which are converted into RapidOne's.
 */
export default function PresetMenu({ presets, disabled, onApply, onSave, onDelete, onImport, onExport }: Props) {
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [scope, setScope] = useState<PresetScope>("photo");
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const ref = useRef<HTMLDivElement>(null);

  // Close on any click outside.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("pointerdown", onDown);
    return () => window.removeEventListener("pointerdown", onDown);
  }, [open]);
  useEffect(() => {
    if (!open) setConfirmDelete(null);
  }, [open]);

  const sorted = [...presets].sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base", numeric: true }));
  const trimmed = name.trim();
  const replacing = presets.some((p) => p.name.toLowerCase() === trimmed.toLowerCase());
  const save = () => {
    if (!trimmed) return;
    onSave(trimmed);
    setName("");
  };

  return (
    <div className="topbar__menu presets" ref={ref}>
      <button type="button" disabled={disabled} onClick={() => setOpen((o) => !o)} title="Presets: apply, save and import" aria-label="Presets">
        <Palette size={14} /> <span className="topbar__label">Presets</span>
      </button>
      {open && (
        <div className="topbar__dropdown presets__menu" role="menu">
          <div className="presets__scope" role="group" aria-label="Apply to">
            <span>Apply to</span>
            <button type="button" className={scope === "photo" ? "is-on" : ""} onClick={() => setScope("photo")}>
              This photo
            </button>
            <button type="button" className={scope === "all" ? "is-on" : ""} onClick={() => setScope("all")}>
              All photos
            </button>
          </div>

          {sorted.length === 0 ? (
            <p className="presets__empty">No presets yet. Save the current settings below, or import Lightroom presets.</p>
          ) : (
            <ul className="presets__list">
              {sorted.map((p) => (
                <li key={p.name}>
                  <button
                    type="button"
                    role="menuitem"
                    className="presets__apply"
                    title={p.origin === "lightroom" ? `${p.name} (imported from Lightroom)` : p.name}
                    onClick={() => {
                      onApply(p, scope);
                      setOpen(false);
                    }}
                  >
                    {p.name}
                  </button>
                  <button
                    type="button"
                    className={`presets__delete${confirmDelete === p.name ? " presets__delete--confirm" : ""}`}
                    onClick={() => {
                      if (confirmDelete === p.name) {
                        onDelete(p.name);
                        setConfirmDelete(null);
                      } else setConfirmDelete(p.name);
                    }}
                    title={confirmDelete === p.name ? "Click again to delete" : `Delete ${p.name}`}
                  >
                    {confirmDelete === p.name ? "Delete?" : <Trash2 size={12} />}
                  </button>
                </li>
              ))}
            </ul>
          )}

          <form
            className="presets__save"
            onSubmit={(e) => {
              e.preventDefault();
              save();
            }}
          >
            <input
              type="text"
              placeholder="New preset name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              onKeyDown={(e) => {
                e.stopPropagation();
                if (e.key === "Escape") setOpen(false);
              }}
            />
            <button type="submit" disabled={!trimmed} title="Save this photo's settings as a preset">
              {replacing ? "Replace" : "Save"}
            </button>
          </form>

          <button
            type="button"
            className="presets__import"
            onClick={() => {
              setOpen(false);
              onImport();
            }}
            title="Choose Lightroom presets (.xmp, each converted into a RapidOne preset) or RapidOne preset backups (.rpp)"
          >
            <Download size={13} /> Import presets…
          </button>
          <button
            type="button"
            className="presets__import"
            disabled={presets.length === 0}
            onClick={() => {
              setOpen(false);
              onExport();
            }}
            title="Copy every preset into a folder, as a backup (bring them back with Import)"
          >
            <Upload size={13} /> Export presets (backup)…
          </button>
        </div>
      )}
    </div>
  );
}
