import { useRef } from "react";
import type { SliderDef } from "../adjustments";
import { DEFAULT_GRADE, type Grade, type GradeRange, type WheelValue } from "../grade";
import { adjustHint, pickHint } from "../panelKeys";
import Hex, { RingDot, hexPoints } from "./Hex";
import VSlider from "./VSlider";

interface Props {
  grade: Grade;
  showNames: boolean;
  onChange: (grade: Grade) => void;
  /** The picked range (held by App so its keys can change it). */
  range: GradeRange;
  onRange: (range: GradeRange) => void;
  /** The held key's target (gradeHue, gradeLum ...), shown active. */
  activeSliderId: string | null;
}

/** Top to bottom: Global apart, then bright to dark like the vertical sliders. */
const BUTTON_ORDER: GradeRange[] = ["global", "highs", "mids", "shadows"];

const LABEL: Record<GradeRange, string> = {
  shadows: "Shadows",
  mids: "Midtones",
  highs: "Highlights",
  global: "Global",
};

/** Mouse/touch-only sliders (no shortcut letter), Lightroom's ranges. */
function gradeSlider(id: string, label: string, min: number, neutral: number): SliderDef {
  return {
    id,
    label,
    code: "",
    modifier: "none",
    min,
    max: 100,
    step: 1,
    coarseStep: 5,
    decimals: 0,
    neutral,
  };
}
const LUMINANCE = gradeSlider("gradeLum", "Luminance", -100, 0);
const BLENDING = gradeSlider("gradeBlending", "Blending", 0, DEFAULT_GRADE.blending);
// Lightroom's Balance, named Pivot (it moves the shadows/highlights crossover)
// so its key (P) doesn't clash with Blending (B).
const BALANCE = {
  ...gradeSlider("gradeBalance", "Pivot", -100, DEFAULT_GRADE.balance),
  tooltip: "Pivot (Balance)",
};

/**
 * Lightroom-style colour grading: round buttons pick Shadows, Midtones,
 * Highlights or Global, and the one wheel edits that range. Beside it, the
 * picked range's Luminance, then Blending and Pivot (shared by all ranges).
 * Double-click a button or the wheel to reset that range.
 */
export default function GradeWheels({ grade, showNames, onChange, range, onRange: setRange, activeSliderId }: Props) {
  const setRangeValue = (patch: Partial<Grade[GradeRange]>) =>
    onChange({ ...grade, [range]: { ...grade[range], ...patch } });

  return (
    <div className="grade">
      <div className="grade__left">
        <div className="grade__pick">
          <div className="grade__ranges" role="tablist" aria-label="Grading range" aria-orientation="vertical">
            {BUTTON_ORDER.map((r) => {
              const changed = grade[r].x !== 0 || grade[r].y !== 0 || grade[r].lum !== 0;
              return (
                <button
                  key={r}
                  type="button"
                  role="tab"
                  aria-selected={range === r}
                  aria-label={LABEL[r]}
                  title={`${LABEL[r]} - double-click to reset\n${pickHint("Grade", LABEL[r][0])}`}
                  className={`grade__range grade__range--${r}${range === r ? " grade__range--on" : ""}${changed ? " grade__range--changed" : ""}`}
                  onClick={() => setRange(r)}
                  onDoubleClick={() => onChange({ ...grade, [r]: DEFAULT_GRADE[r] })}
                >
                  <RingDot size={14} gap={1} on={range === r} half={r === "global"} />
                </button>
              );
            })}
          </div>
          <Wheel
            label={LABEL[range]}
            value={grade[range]}
            onChange={(v) => setRangeValue(v)}
            onReset={() => setRangeValue({ x: 0, y: 0 })}
          />
        </div>
        <span className="wheel__label">{LABEL[range]}</span>
      </div>
      <div className="panel__sliders">
        <VSlider
          key={range}
          def={LUMINANCE}
          hint={adjustHint("Grade", "L")}
          value={grade[range].lum}
          neutral={0}
          active={activeSliderId === LUMINANCE.id}
          showName={showNames}
          onChange={(lum) => setRangeValue({ lum })}
        />
        <VSlider
          def={BLENDING}
          hint={adjustHint("Grade", "B")}
          value={grade.blending}
          neutral={BLENDING.neutral}
          active={activeSliderId === BLENDING.id}
          showName={showNames}
          onChange={(blending) => onChange({ ...grade, blending })}
        />
        <VSlider
          def={BALANCE}
          hint={adjustHint("Grade", "P")}
          value={grade.balance}
          neutral={BALANCE.neutral}
          active={activeSliderId === BALANCE.id}
          showName={showNames}
          onChange={(balance) => onChange({ ...grade, balance })}
        />
      </div>
    </div>
  );
}

/*
 * The wheel sits in a flat-topped hexagon, but its values are a disc
 * (Lightroom's hue and saturation), as before: full saturation is the outer
 * ring, a circle just inside the hexagon's sides. The colour carries on to
 * the hexagon's edge beyond it (still full strength there). Box units: -1..1 across, the
 * hexagon's corners at left and right.
 */
const APOTHEM = Math.sqrt(3) / 2;
/** The outer ring (full saturation, the limit), a little inside the hexagon's sides. */
const RING = APOTHEM * 0.92;

/** Distance from the centre to the hexagon's edge in direction `angle` (radians). */
function hexEdge(angle: number): number {
  const sixty = Math.PI / 3;
  const a = ((angle % sixty) + sixty) % sixty;
  return APOTHEM / Math.cos(a - sixty / 2);
}

/** A point in the box (x, y) to the wheel's value (within the unit disc). */
function boxToValue(x: number, y: number): WheelValue {
  const angle = Math.atan2(y, x);
  const s = Math.min(1, Math.hypot(x, y) / RING);
  return { x: Math.round(s * Math.cos(angle) * 1000) / 1000, y: Math.round(s * Math.sin(angle) * 1000) / 1000 };
}

/** The wheel's value to its point in the box. */
function valueToBox(v: WheelValue): { x: number; y: number } {
  const angle = Math.atan2(v.y, v.x);
  const r = Math.hypot(v.x, v.y) * RING;
  return { x: r * Math.cos(angle), y: r * Math.sin(angle) };
}

/** Hue stops clockwise from the right (as grade.ts: red right, magenta below). */
const HUES = ["#c44e4e", "#c04eb4", "#5064c8", "#46b6c0", "#53b852", "#c4b446"].map((h) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16)));

/** The hexagonal wheel's picture (drawn once): hues round, fading to dark at the centre. */
let wheelImage: string | null = null;
function hexWheelImage(): string {
  if (wheelImage) return wheelImage;
  const n = 256;
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = n;
  const g = canvas.getContext("2d")!;
  const img = g.createImageData(n, n);
  for (let py = 0; py < n; py++) {
    for (let px = 0; px < n; px++) {
      const x = ((px + 0.5) / n) * 2 - 1;
      const y = ((py + 0.5) / n) * 2 - 1;
      const angle = Math.atan2(y, x);
      if (Math.hypot(x, y) > hexEdge(angle)) continue;
      // Saturation within the outer ring; the corners beyond carry its full colour out to the edge.
      const s = Math.min(1, Math.hypot(x, y) / RING);
      const t = ((((angle * 180) / Math.PI + 360) % 360) / 60) % 6;
      const i = Math.floor(t);
      const f = t - i;
      const [a, b] = [HUES[i], HUES[(i + 1) % 6]];
      const hue = a.map((c, k) => c + (b[k] - c) * f);
      // A black centre (no tint) fading out to full colour at the ring.
      const fade = Math.pow(1 - s, 1.6);
      const dark = [7, 9, 13];
      const o = (py * n + px) * 4;
      for (let k = 0; k < 3; k++) img.data[o + k] = hue[k] * (1 - fade) + dark[k] * fade;
      img.data[o + 3] = 255;
    }
  }
  g.putImageData(img, 0, 0);
  // A target: the outer ring (full strength, the limit) and a faint ring at
  // half strength (the crosshair is drawn on the handle, so it moves with it) through the centre, broken at the middle.
  const c = n / 2;
  const ring = (r: number, colour: string, width: number) => {
    g.strokeStyle = colour;
    g.lineWidth = width;
    g.beginPath();
    g.arc(c, c, c * RING * r, 0, Math.PI * 2);
    g.stroke();
  };
  ring(0.5, "rgba(190, 246, 255, 0.3)", 1.5);
  // The limit: a bright cyan ring on a dark line, so it shows against any colour.
  ring(1, "rgba(8, 8, 8, 0.85)", 5);
  ring(1, "#3ee6ff", 2.5);
  // The outline.
  g.strokeStyle = "#1a4552";
  g.lineWidth = 2;
  g.beginPath();
  for (let k = 0; k < 6; k++) {
    const a = (k * Math.PI) / 3;
    const [x, y] = [n / 2 + (n / 2 - 1) * Math.cos(a), n / 2 + (n / 2 - 1) * Math.sin(a)];
    if (k === 0) g.moveTo(x, y);
    else g.lineTo(x, y);
  }
  g.closePath();
  g.stroke();
  wheelImage = canvas.toDataURL();
  return wheelImage;
}

function Wheel({
  label,
  value,
  onChange,
  onReset,
}: {
  label: string;
  value: WheelValue;
  onChange: (v: WheelValue) => void;
  onReset: () => void;
}) {
  const ringRef = useRef<HTMLDivElement>(null);
  const lastTap = useRef(0);

  function fromPointer(e: React.PointerEvent): WheelValue {
    const rect = ringRef.current!.getBoundingClientRect();
    const x = ((e.clientX - rect.left) / rect.width) * 2 - 1;
    const y = ((e.clientY - rect.top) / rect.height) * 2 - 1;
    return boxToValue(x, y);
  }

  function onPointerDown(e: React.PointerEvent) {
    const now = performance.now();
    if (now - lastTap.current < 300) {
      lastTap.current = 0;
      onReset();
      return;
    }
    lastTap.current = now;
    e.currentTarget.setPointerCapture(e.pointerId);
    onChange(fromPointer(e));
  }

  function onPointerMove(e: React.PointerEvent) {
    if (e.currentTarget.hasPointerCapture(e.pointerId)) onChange(fromPointer(e));
  }

  const changed = value.x !== 0 || value.y !== 0;
  const at = valueToBox(value);
  return (
    <div
      ref={ringRef}
      className="wheel__ring"
      style={{ backgroundImage: `url(${hexWheelImage()})` }}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      title={`${label}: drag to tint, double-click to reset\n${adjustHint("Grade", "H (hue) or S (saturation)")}`}
    >
      <div className="wheel__centre" />
      <div className="wheel__cross" style={{ left: `${(at.x + 1) * 50}%`, top: `${(at.y + 1) * 50}%` }} />
      <Hex
        w={10}
        h={10}
        className={`wheel__puck${changed ? " wheel__puck--moved" : ""}`}
        style={{
          left: `${(at.x + 1) * 50}%`,
          top: `${(at.y + 1) * 50}%`,
        }}
      >
        <polygon points={hexPoints(10, 10, 1)} style={{ fill: "#ffffff", stroke: "var(--ring)", strokeWidth: 2 }} />
      </Hex>
    </div>
  );
}
