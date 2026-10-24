import { useEffect, useRef, useState } from "react";
import { ClipboardCopy, ClipboardPaste } from "lucide-react";
import { COPY_GROUPS } from "../copySettings";
import { keyLabel } from "../macKeys";

interface Props {
  disabled: boolean;
  /** The groups ticked for the next copy. */
  groups: string[];
  onGroupsChange: (groups: string[]) => void;
  /** What's on the clipboard ("P1313741, 8 groups"), or null. */
  copied: string | null;
  onCopy: () => void;
  /** Selected: the photos picked in the filmstrip or library, or the open one. */
  onPaste: (scope: "photo" | "selected" | "all") => void;
  /** Photos picked (Ctrl / Shift-click); 0: just the open photo. */
  selectedCount?: number;
  /** Copied and not pasted yet: the Paste button glows. */
  pasteFresh?: boolean;
}

/**
 * Copy settings from this photo (choose which, Lightroom-style) in its menu;
 * the Paste button beside it pastes them to the selected photos (or the open
 * one; Ctrl+A selects all). One undo step per photo.
 * Shortcuts: Ctrl+Shift+C copy, Ctrl+Shift+V paste.
 */
export default function CopyPasteMenu({ disabled, groups, onGroupsChange, copied, onCopy, onPaste, selectedCount = 0, pasteFresh = false }: Props) {
  const many = selectedCount > 1;
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("pointerdown", onDown);
    return () => window.removeEventListener("pointerdown", onDown);
  }, [open]);

  const toggle = (id: string) => onGroupsChange(groups.includes(id) ? groups.filter((g) => g !== id) : [...groups, id]);

  return (
    <div className="topbar__menu" ref={ref}>
      <button
        type="button"
        disabled={disabled}
        onClick={() => setOpen((o) => !o)}
        title={keyLabel("Copy settings: copy this photo's settings (Ctrl+Shift+C) and paste them to others (Ctrl+Shift+V)")}
        aria-label="Copy settings"
      >
        <ClipboardCopy size={14} /> <span className="topbar__label">Copy</span>
      </button>
      {open && (
        <div className="topbar__dropdown autostyle__menu copypaste__menu" role="menu">
          <p className="autostyle__heading">Copy settings</p>
          {/* Each group with an on / off switch (Lightroom's ticks, in RapidOne's style). */}
          {COPY_GROUPS.map((g) => {
            const on = groups.includes(g.id);
            return (
              <button
                key={g.id}
                type="button"
                role="switch"
                aria-checked={on}
                className={`copypaste__group${on ? " copypaste__group--on" : ""}`}
                onClick={() => toggle(g.id)}
              >
                <span className="copypaste__name">{g.label}</span>
                <span className="toggle" aria-hidden="true">
                  <span className="toggle__knob" />
                </span>
              </button>
            );
          })}
          <div className="copypaste__quick">
            <button type="button" onClick={() => onGroupsChange(COPY_GROUPS.map((g) => g.id))}>
              All
            </button>
            <button type="button" onClick={() => onGroupsChange(COPY_GROUPS.filter((g) => g.id !== "wb" && g.id !== "exposure" && g.byDefault).map((g) => g.id))}
              title="Everything but white balance and exposure (keeps Auto Style's per-photo values)">
              Look only
            </button>
            <button type="button" onClick={() => onGroupsChange([])}>
              None
            </button>
          </div>
          <button
            type="button"
            className="autostyle__train"
            disabled={groups.length === 0}
            onClick={onCopy}
            title={keyLabel("Copy the ticked settings from this photo (Ctrl+Shift+C)")}
          >
            Copy from this photo
          </button>
          <p className="autostyle__hint">{copied ? `Copied: ${copied}` : "Nothing copied yet."}</p>
        </div>
      )}
      {/* Paste: what was copied, to the selected photos (or the open one), straight away. */}
      <button
        type="button"
        disabled={disabled || !copied}
        // Lit (an orange glow) from a copy until it's pasted; it can still be pasted again after.
        className={pasteFresh ? "paste--ready" : undefined}
        onClick={() => onPaste("selected")}
        title={
          copied
            ? keyLabel(`Paste settings (${copied}) to ${many ? `the ${selectedCount} selected photos` : "this photo"} (Ctrl+Shift+V)`)
            : "Paste settings: copy a photo's settings first"
        }
        aria-label="Paste settings"
      >
        <ClipboardPaste size={14} /> <span className="topbar__label">Paste</span>
      </button>
    </div>
  );
}
