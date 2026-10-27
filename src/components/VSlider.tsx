import { useRef, useState } from "react";
import {
  clampToSlider,
  formatValue,
  fromFraction,
  nudge,
  shortName,
  toFraction,
  type SliderDef,
} from "../adjustments";
import { adjustHint, sliderPanel } from "../panelKeys";
import Hex, { hexPoints } from "./Hex";

interface Props {
  def: SliderDef;
  value: number;
  neutral: number;
  /** True while this slider's key is held down. */
  active: boolean;
  /** Global "slider names" toggle - off gives narrower, two-letter sliders. */
  showName: boolean;
  onChange: (value: number) => void;
  /** Keyboard hint for the hover text, where the slider's letter alone doesn't say (Mixer, Grade ...). */
  hint?: string;
}

/**
 * Vertical slider: value above, and a short horizontal name below ("High";
 * first letter underlined = the shortcut key), or two letters in the narrow layout.
 * The full name shows on hover.
 * Mouse/touch/pen all go through pointer events. Dragging keeps the thumb
 * under the pointer (one track length = the full range); pressing on the
 * track away from the thumb jumps there first, as in Lightroom. The wheel
 * nudges by the fine step; double-click/double-tap resets.
 *
 * The value readout works like Lightroom's: drag sideways across it to scrub
 * (one pixel = 1/200 of the track, Lightroom's feel of 1 unit per pixel on
 * a ±100 slider), or click it to type a value. Double-clicking the name
 * resets, as double-clicking Lightroom's slider label does.
 */
/** A plain slider's shortcut, from its letter and the panel it's in. */
function autoHint(def: SliderDef): string | undefined {
  const panel = sliderPanel(def.id);
  return def.code && panel && panel !== "Parametric" ? adjustHint(panel, def.code.replace(/^Key/, "")) : undefined;
}

export default function VSlider({ def, value, neutral, active, showName, onChange, hint }: Props) {
  const trackRef = useRef<HTMLDivElement>(null);
  const lastTapRef = useRef(0);
  const dragRef = useRef<{ startY: number; startValue: number } | null>(null);
  const scrubRef = useRef<{ startX: number; startValue: number; moved: boolean } | null>(null);
  /** Text being typed into the readout, or null when not editing. */
  const [draft, setDraft] = useState<string | null>(null);

  const fraction = toFraction(def, value);
  const neutralFraction = toFraction(def, neutral);
  const fillLow = Math.min(fraction, neutralFraction);
  const fillHigh = Math.max(fraction, neutralFraction);

  /** Rounds to what the readout shows (0.01 EV for exposure, whole numbers otherwise). */
  function toDisplayPrecision(v: number): number {
    const unit = 10 ** -def.decimals;
    return clampToSlider(def, Math.round(v / unit) * unit);
  }

  function onPointerDown(e: React.PointerEvent) {
    // Touch has no dblclick event, so detect a double tap here instead.
    const now = performance.now();
    if (now - lastTapRef.current < 300) {
      lastTapRef.current = 0;
      dragRef.current = null;
      onChange(neutral);
      return;
    }
    lastTapRef.current = now;
    e.currentTarget.setPointerCapture(e.pointerId);

    // Pressing on the thumb grabs it where it is; anywhere else on the track
    // jumps the value there first.
    const rect = trackRef.current!.getBoundingClientRect();
    const thumbY = rect.bottom - fraction * rect.height;
    let startValue = value;
    if (Math.abs(e.clientY - thumbY) > 8) {
      const f = Math.min(1, Math.max(0, (rect.bottom - e.clientY) / rect.height));
      startValue = toDisplayPrecision(fromFraction(def, f));
      onChange(startValue);
    }
    dragRef.current = { startY: e.clientY, startValue };
  }

  function onPointerMove(e: React.PointerEvent) {
    const drag = dragRef.current;
    if (!drag || !e.currentTarget.hasPointerCapture(e.pointerId)) return;
    // Up is positive; screen y grows downward. Moving relative to where the
    // drag started (rather than snapping to the pointer) means grabbing the
    // thumb slightly off-centre doesn't make it jump.
    // Move in track position (not value) so non-linear tracks (Kelvin) still
    // keep the dot under the pointer.
    const trackPx = trackRef.current!.getBoundingClientRect().height;
    const f = toFraction(def, drag.startValue) + (drag.startY - e.clientY) / trackPx;
    onChange(toDisplayPrecision(fromFraction(def, f)));
  }

  function onPointerUp() {
    dragRef.current = null;
  }

  function onWheel(e: React.WheelEvent) {
    onChange(nudge(def, value, e.deltaY < 0 ? 1 : -1, false));
  }

  function onValuePointerDown(e: React.PointerEvent) {
    if (draft !== null) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    scrubRef.current = { startX: e.clientX, startValue: value, moved: false };
  }

  function onValuePointerMove(e: React.PointerEvent) {
    const scrub = scrubRef.current;
    if (!scrub || !e.currentTarget.hasPointerCapture(e.pointerId)) return;
    const dx = e.clientX - scrub.startX;
    if (!scrub.moved && Math.abs(dx) < 3) return; // a click, not a scrub (yet)
    scrub.moved = true;
    onChange(toDisplayPrecision(fromFraction(def, toFraction(def, scrub.startValue) + dx / 200)));
  }

  function onValuePointerUp() {
    const scrub = scrubRef.current;
    scrubRef.current = null;
    if (scrub && !scrub.moved) setDraft(formatValue(def, value));
  }

  function commitDraft() {
    if (draft === null) return;
    const typed = Number(draft.replace(/[^\d.+-]/g, ""));
    setDraft(null);
    if (draft.trim() !== "" && Number.isFinite(typed)) onChange(toDisplayPrecision(typed));
  }

  const changed = value !== neutral;
  // Short name ("High"), or just its first two letters in the narrow layout.
  const name = showName ? shortName(def.label) : shortName(def.label).slice(0, 2);

  return (
    <div
      className={`vslider${active ? " vslider--active" : ""}${showName ? "" : " vslider--compact"}`}
      title={[def.tooltip ?? def.label, hint ?? autoHint(def)].filter(Boolean).join("\n")}
    >
      {draft !== null ? (
        <input
          className="vslider__value vslider__input"
          value={draft}
          autoFocus
          onFocus={(e) => e.currentTarget.select()}
          onChange={(e) => setDraft(e.currentTarget.value)}
          onBlur={commitDraft}
          onKeyDown={(e) => {
            // Keep typing out of the app's global shortcuts.
            e.stopPropagation();
            if (e.key === "Enter") commitDraft();
            if (e.key === "Escape") setDraft(null);
          }}
        />
      ) : (
        <div
          className={`vslider__value${changed ? "" : " vslider__value--neutral"}`}
          onPointerDown={onValuePointerDown}
          onPointerMove={onValuePointerMove}
          onPointerUp={onValuePointerUp}
          onPointerCancel={() => (scrubRef.current = null)}
        >
          {formatValue(def, value)}
        </div>
      )}
      <div className="vslider__body">
        <div
          className="vslider__hit"
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerCancel={onPointerUp}
          onWheel={onWheel}
          role="slider"
          aria-label={def.label}
          aria-valuemin={def.min}
          aria-valuemax={def.max}
          aria-valuenow={value}
          aria-orientation="vertical"
        >
          <div
            className={`vslider__track${def.trackGradient ? " vslider__track--scale" : ""}`}
            ref={trackRef}
            style={def.trackGradient ? { background: def.trackGradient } : undefined}
          >
            {/* Colour-scale tracks (Temperature, Tint) show the scale instead of a gold fill, as in RapidRetouch. */}
            {!def.trackGradient && (
              // Raised above its neutral: neon orange; lowered: cyan.
              <div
                className={`vslider__fill${fraction > neutralFraction ? " vslider__fill--plus" : ""}`}
                style={{ bottom: `${fillLow * 100}%`, height: `${(fillHigh - fillLow) * 100}%` }}
              />
            )}
            <Hex w={14} h={12} className="vslider__thumb" style={{ bottom: `${fraction * 100}%` }}>
              <polygon points={hexPoints(14, 12, 1)} />
            </Hex>
          </div>
        </div>
      </div>
      <div className="vslider__key" onDoubleClick={() => onChange(neutral)}>
        {/* In the group-then-slider key scheme every slider's key is its first letter. */}
        <span className="vslider__keyletter">{name[0]}</span>
        {name.slice(1)}
      </div>
    </div>
  );
}
