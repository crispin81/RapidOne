import { useEffect, useRef, useState } from "react";
import DenoiseIcon from "./DenoiseIcon";

/** The ISOs the menu offers as the threshold: the cameras' third-stop steps. */
const ISOS = [
  100, 125, 160, 200, 250, 320, 400, 500, 640, 800, 1000, 1250, 1600, 2000, 2500, 3200, 4000, 5000, 6400, 8000, 10000, 12800,
  16000, 20000, 25600, 51200,
];
const SETTINGS_KEY = "rapidone.denoiseAll";

interface Props {
  /** No folder open: nothing to apply to. */
  disabled: boolean;
  /** "Denoise: reading ISO 40/120" while running, else null. */
  progress: string | null;
  /** Noise `amount` on every photo in the folder shot at `minIso` or above. */
  onApply: (minIso: number, amount: number) => void;
}

function loadSettings(): { iso: number; amount: number } {
  try {
    const s = JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? "{}");
    return { iso: ISOS.includes(s.iso) ? s.iso : 800, amount: typeof s.amount === "number" ? s.amount : 75 };
  } catch {
    return { iso: 800, amount: 75 };
  }
}

/**
 * Denoise all: the Noise slider set on every photo in the folder shot at the chosen ISO or above
 * (photos below it keep theirs). The last ISO and amount are remembered.
 */
export default function DenoiseMenu({ disabled, progress, onApply }: Props) {
  const [open, setOpen] = useState(false);
  const [{ iso, amount }, setSettings] = useState(loadSettings);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("pointerdown", onDown);
    return () => window.removeEventListener("pointerdown", onDown);
  }, [open]);

  const change = (next: { iso: number; amount: number }) => {
    setSettings(next);
    try {
      localStorage.setItem(SETTINGS_KEY, JSON.stringify(next));
    } catch {
      // Not fatal: the menu just starts from the defaults next time.
    }
  };

  const apply = () => {
    setOpen(false);
    onApply(iso, amount);
  };

  return (
    <div className="topbar__menu" ref={ref}>
      <button
        type="button"
        className={progress ? "is-on" : undefined}
        disabled={disabled || Boolean(progress)}
        onClick={() => setOpen((o) => !o)}
        title="Denoise all: noise reduction on every photo shot at or above an ISO"
        aria-label="Denoise all"
      >
        <DenoiseIcon size={14} />
        {progress && <span>{progress}</span>}
      </button>
      {open && (
        <div className="topbar__dropdown autostyle__menu denoise__menu" role="menu" onKeyDown={(e) => e.stopPropagation()}>
          <p className="autostyle__heading">Denoise all photos</p>
          <label className="denoise__row" title={`If ISO is equal to or greater than ${iso.toLocaleString()}: photos shot at this ISO or above get the denoise below`}>
            <span>If ISO ≥</span>
            <select value={iso} onChange={(e) => change({ iso: Number(e.currentTarget.value), amount })}>
              {ISOS.map((v) => (
                <option key={v} value={v}>
                  {v.toLocaleString()}
                </option>
              ))}
            </select>
            <span>apply</span>
          </label>
          <label className="denoise__row" title="How much noise reduction: 100 is the full automatic denoise, 50 halfway">
            <span>Denoise</span>
            <input
              type="range"
              min={0}
              max={100}
              value={amount}
              onChange={(e) => change({ iso, amount: Number(e.currentTarget.value) })}
            />
            <span className="denoise__value">{amount}</span>
          </label>
          <div className="autostyle__actions">
            <button type="button" onClick={() => setOpen(false)}>
              Cancel
            </button>
            <button type="button" className="is-primary" onClick={apply} title={`Noise ${amount} on every photo at ISO ${iso.toLocaleString()} or above`}>
              Apply
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
