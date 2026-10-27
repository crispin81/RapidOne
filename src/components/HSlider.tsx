import { useRef } from "react";
import Hex, { hexPoints } from "./Hex";

interface Props {
  min: number;
  max: number;
  step: number;
  value: number;
  onChange: (value: number) => void;
  /** Where the fill starts: above it fills neon orange, below cyan (default: `min`, so it fills orange). */
  neutral?: number;
  /** Double-click (or double-tap) goes back to this (default: `neutral`). */
  reset?: number;
  /** Track width in px (default 110). */
  width?: number;
  title?: string;
  "aria-label"?: string;
  /** A drag let go, a wheel step or a reset: for a setting sent once it settles (Patch's Feather). */
  onCommit?: (value: number) => void;
}

/**
 * A horizontal slider in the panel sliders' look: a thin track, the fill from its neutral
 * (orange up, cyan down) and the white hexagon thumb (orange on hover). Pressing on the
 * track away from the thumb jumps there first, then the drag keeps the thumb under the
 * pointer; the wheel nudges by a step; double-click resets.
 */
export default function HSlider({ min, max, step, value, onChange, neutral = min, reset, width = 110, title, onCommit, ...rest }: Props) {
  const trackRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<{ startX: number; startValue: number } | null>(null);
  const lastTapRef = useRef(0);
  const span = max - min || 1;
  const frac = (v: number) => Math.min(1, Math.max(0, (v - min) / span));
  const snap = (v: number) => {
    const s = Math.round((Math.min(max, Math.max(min, v)) - min) / step) * step + min;
    return Number(s.toFixed(6));
  };
  const f = frac(value);
  const nf = frac(neutral);
  const resetTo = reset ?? neutral;

  const latest = useRef(value);
  latest.current = value;
  const set = (v: number) => {
    latest.current = v;
    onChange(v);
  };
  const onPointerDown = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    e.stopPropagation();
    const now = performance.now();
    if (now - lastTapRef.current < 300) {
      lastTapRef.current = 0;
      dragRef.current = null;
      set(resetTo);
      onCommit?.(resetTo);
      return;
    }
    lastTapRef.current = now;
    e.currentTarget.setPointerCapture(e.pointerId);
    const r = trackRef.current!.getBoundingClientRect();
    let startValue = value;
    if (Math.abs(e.clientX - (r.left + f * r.width)) > 8) {
      startValue = snap(min + ((e.clientX - r.left) / r.width) * span);
      set(startValue);
    }
    dragRef.current = { startX: e.clientX, startValue };
  };
  const onPointerMove = (e: React.PointerEvent) => {
    const d = dragRef.current;
    if (!d || !e.currentTarget.hasPointerCapture(e.pointerId)) return;
    const w = trackRef.current!.getBoundingClientRect().width || 1;
    set(snap(d.startValue + ((e.clientX - d.startX) / w) * span));
  };
  const end = () => {
    if (dragRef.current) onCommit?.(latest.current);
    dragRef.current = null;
  };

  return (
    <div
      className="hslider"
      style={{ width }}
      title={title}
      role="slider"
      aria-label={rest["aria-label"]}
      aria-valuemin={min}
      aria-valuemax={max}
      aria-valuenow={value}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={end}
      onPointerCancel={end}
      onWheel={(e) => {
        const v = snap(value + (e.deltaY < 0 ? step : -step));
        set(v);
        onCommit?.(v);
      }}
    >
      <div className="hslider__track" ref={trackRef}>
        <div
          className={`hslider__fill${value > neutral ? " hslider__fill--plus" : ""}`}
          style={{ left: `${Math.min(f, nf) * 100}%`, width: `${Math.abs(f - nf) * 100}%` }}
        />
        <Hex w={14} h={12} className="hslider__thumb" style={{ left: `${f * 100}%` }}>
          <polygon points={hexPoints(14, 12, 1)} />
        </Hex>
      </div>
    </div>
  );
}
