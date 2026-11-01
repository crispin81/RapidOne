import { useRef } from "react";
import { Pipette } from "lucide-react";
import HSlider from "./HSlider";
import type { Adjustments } from "../adjustments";
import "../picker.css";

/** Each colour's hue band on screen (HSV / HSL hue, degrees): 0..100 on its range spans these (`defringe.rs`). */
const BANDS = {
  purple: { start: 220, label: "Purple" },
  green: { start: 60, label: "Green" },
} as const;
const SPAN = 120;

const bandGradient = (start: number) =>
  `linear-gradient(to right, ${[0, 0.25, 0.5, 0.75, 1].map((t) => `hsl(${(start + t * SPAN) % 360} 85% 55%)`).join(", ")})`;

/** A hue range on a colour band: two handles; the nearer one follows a drag; double-click resets. */
function HueRange({ start, lo, hi, onChange, onReset, label }: { start: number; lo: number; hi: number; onChange: (lo: number, hi: number) => void; onReset: () => void; label: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const which = useRef<"lo" | "hi" | null>(null);
  const valueAt = (clientX: number) => {
    const r = ref.current!.getBoundingClientRect();
    return Math.round(Math.min(100, Math.max(0, ((clientX - r.left) / Math.max(1, r.width)) * 100)));
  };
  const move = (v: number) => {
    if (which.current === "lo") onChange(Math.min(v, hi), hi);
    else if (which.current === "hi") onChange(lo, Math.max(v, lo));
  };
  return (
    <div
      ref={ref}
      className="huerange"
      role="group"
      aria-label={label}
      title={`${label}: drag either end (double-click resets)`}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        e.stopPropagation();
        const v = valueAt(e.clientX);
        which.current = Math.abs(v - lo) <= Math.abs(v - hi) && !(v > hi) ? "lo" : "hi";
        e.currentTarget.setPointerCapture(e.pointerId);
        move(v);
      }}
      onPointerMove={(e) => {
        if (which.current && e.currentTarget.hasPointerCapture(e.pointerId)) move(valueAt(e.clientX));
      }}
      onPointerUp={() => (which.current = null)}
      onPointerCancel={() => (which.current = null)}
      onDoubleClick={onReset}
    >
      <div className="huerange__band" style={{ background: bandGradient(start) }} />
      <div className="huerange__shade" style={{ left: 0, width: `${lo}%` }} />
      <div className="huerange__shade" style={{ left: `${hi}%`, right: 0 }} />
      <div className="huerange__handle" style={{ left: `${lo}%` }} />
      <div className="huerange__handle" style={{ left: `${hi}%` }} />
    </div>
  );
}

interface Props {
  values: Adjustments;
  onSet: (changes: Record<string, number>) => void;
  /** The fringe picker is waiting for a click on the photo. */
  picking: boolean;
  onPick: () => void;
}

/**
 * Lightroom's Defringe (Lens Corrections): an Amount and a hue range for purple and for green
 * fringes, taken out along high-contrast edges only. The picker reads a fringe's colour off
 * the photo and sets its range.
 */
export default function Defringe({ values, onSet, picking, onPick }: Props) {
  const colour = (kind: "purple" | "green", defaults: [number, number]) => {
    const cap = kind === "purple" ? "Purple" : "Green";
    const amountId = `defringe${cap}`;
    const loId = `defringe${cap}HueLo`;
    const hiId = `defringe${cap}HueHi`;
    const amount = values[amountId] ?? 0;
    const lo = values[loId] ?? defaults[0];
    const hi = values[hiId] ?? defaults[1];
    return (
      <>
        <div className="defringe__row">
          <span>{BANDS[kind].label} amount</span>
          <HSlider min={0} max={20} step={1} value={amount} onChange={(v) => onSet({ [amountId]: v })} width={120} aria-label={`${BANDS[kind].label} amount`} title={`${BANDS[kind].label} fringes taken out along edges (0 = off)`} />
          <span className="defringe__value">{amount}</span>
        </div>
        <div className="defringe__row">
          <span>{BANDS[kind].label} hue</span>
          <HueRange
            label={`${BANDS[kind].label} hue`}
            start={BANDS[kind].start}
            lo={lo}
            hi={hi}
            onChange={(l, h) => onSet({ [loId]: l, [hiId]: h })}
            onReset={() => onSet({ [loId]: defaults[0], [hiId]: defaults[1] })}
          />
          <span className="defringe__value">
            {lo}–{hi}
          </span>
        </div>
      </>
    );
  };
  return (
    <div className="defringe">
      <div className="defringe__head">
        <span title="Defringe: purple and green colour fringes along high-contrast edges">Defringe</span>
        <button
          type="button"
          className={`defringe__pick${picking ? " pick-btn--on" : ""}`}
          onClick={onPick}
          title={picking ? "Click a fringe on the photo (Esc to cancel)" : "Pick a fringe's colour on the photo: sets its hue range"}
          aria-pressed={picking}
        >
          <Pipette size={12} /> Pick fringe
        </button>
      </div>
      {colour("purple", [30, 70])}
      {colour("green", [40, 60])}
    </div>
  );
}
