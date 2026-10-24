import { useEffect, useRef, useState } from "react";
import { RotateCcw } from "lucide-react";
import { ALL_SLIDERS, type Adjustments } from "../adjustments";
import { parametricCurvePoints } from "../api";
import { CURVE_CHANNELS, DEFAULT_CURVES, MIN_GAP, isIdentity, nudgePoint, parametricShape, sampleCurve, type CurveChannel, type CurvePoint, type Curves } from "../curves";
import type { CurvesTab } from "../panelKeys";
import type { Histogram } from "../histogram";
import VSlider from "./VSlider";
import { adjustHint, pickHint } from "../panelKeys";

interface Props {
  curves: Curves;
  /** Input histogram of the current photo, drawn behind the curve. */
  histogram: Histogram | null;
  /** The photo's ISO and aperture, top left of the graph. */
  shotLabel?: string | null;
  onChange: (curves: Curves) => void;
  /** Slider values, for the Region (parametric) tab. */
  values: Adjustments;
  neutrals: Adjustments;
  activeSliderId: string | null;
  showNames: boolean;
  onSliderChange: (id: string, value: number) => void;
  /** The picked tab (a channel, or Region), held by App so its keys can change it. */
  tab: Tab;
  onTab: (tab: Tab) => void;
}

/** A point-curve channel, or the Region tab: Lightroom's parametric curve (four regions, three splits). */
type Tab = CurvesTab;
const SLIDER = Object.fromEntries(ALL_SLIDERS.map((d) => [d.id, d]));
const REGION_SLIDERS = ["paramHighlights", "paramLights", "paramDarks", "paramShadows"];
const SPLITS = ["paramShadowSplit", "paramMidSplit", "paramHighSplit"];
/** Smallest gap between neighbouring splits, in percent. */
const SPLIT_GAP = 5;
/** Height of the split handles along the graph's bottom. */
const SPLIT_STRIP = 14;

const SIZE = 168;
const HIT_RADIUS = 0.06;

const CHANNEL_LABEL: Record<CurveChannel, string> = { luma: "L", rgb: "RGB", r: "R", g: "G", b: "B" };
const CHANNEL_TITLE: Record<CurveChannel, string> = {
  luma: "Luminosity: brightness only, colour unchanged",
  rgb: "RGB: all channels together (also shifts saturation)",
  r: "Red",
  g: "Green",
  b: "Blue",
};
const CHANNEL_COLOUR: Record<CurveChannel, string> = {
  luma: "#3ee6ff",
  rgb: "#e8e8ec",
  r: "#ff6b6b",
  g: "#5fd38d",
  b: "#5aa9ff",
};

/**
 * Tone curve editor sized to fit the short bottom panel. Click the graph to
 * add a point, drag to move it, double-click/double-tap a point to remove
 * it. Dragging the end points does what Levels would: sideways for the
 * black/white input points, up/down for output levels. Works with mouse,
 * touch and pen.
 */

/** A flat-topped hexagon's corners around (cx, cy), radius r: the curve's points (as the sliders' thumbs). */
function hexPoints(cx: number, cy: number, r: number): string {
  return [0, 60, 120, 180, 240, 300]
    .map((a) => `${(cx + r * Math.cos((a * Math.PI) / 180)).toFixed(2)},${(cy + r * Math.sin((a * Math.PI) / 180)).toFixed(2)}`)
    .join(" ");
}

/** Filled area for one histogram channel, scaled so the tallest bin reaches the top. */
function histogramPath(bins: number[]): string {
  // Square-root scaling keeps a big spike (e.g. a white backdrop) from
  // flattening every other tone into nothing.
  const scaled = bins.map(Math.sqrt);
  const peak = Math.max(...scaled, 1);
  const step = SIZE / bins.length;
  const top = scaled.map((v, i) => `L${(i * step).toFixed(1)},${(SIZE - (v / peak) * SIZE).toFixed(1)} L${((i + 1) * step).toFixed(1)},${(SIZE - (v / peak) * SIZE).toFixed(1)}`);
  return `M0,${SIZE} ${top.join(" ")} L${SIZE},${SIZE} Z`;
}

export default function CurvesPanel({ curves, histogram, shotLabel, onChange, values, neutrals, activeSliderId, showNames, onSliderChange, tab, onTab: setTab }: Props) {
  const region = tab === "region";
  // The point-curve channel (the last one picked, while on Region).
  const [lastChannel, setLastChannel] = useState<CurveChannel>("luma");
  useEffect(() => {
    if (tab !== "region") setLastChannel(tab);
  }, [tab]);
  const channel: CurveChannel = region ? lastChannel : tab;
  const dragSplit = useRef<number | null>(null);
  const svgRef = useRef<SVGSVGElement>(null);
  const dragIndex = useRef<number | null>(null);
  const lastTap = useRef({ time: 0, index: -1 });

  const points = curves[channel];

  // Keyboard: hold number-pad 1, 5 or 9 (or Shift + top-row 1, 5, 9, which
  // otherwise set star ratings) and press the arrows to move that square's
  // point; Ctrl for bigger steps. Listens before the app's own keys so the
  // arrows don't also change photo while a point is held.
  const [heldSquare, setHeldSquare] = useState<0 | 1 | 2 | null>(null);
  const live = useRef({ curves, channel, onChange, region });
  live.current = { curves, channel, onChange, region };
  useEffect(() => {
    const SQUARE: Record<string, 0 | 1 | 2> = { Numpad1: 0, Numpad5: 1, Numpad9: 2, Digit1: 0, Digit5: 1, Digit9: 2 };
    let held: 0 | 1 | 2 | null = null;
    function onKeyDown(e: KeyboardEvent) {
      // Typing in a box (Export's size, a name): its digits and arrows are the box's.
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable)) return;
      const square = SQUARE[e.code];
      if (square !== undefined && (e.code.startsWith("Numpad") || e.shiftKey) && !e.ctrlKey && !e.metaKey && !e.altKey) {
        e.preventDefault();
        e.stopImmediatePropagation();
        held = square;
        setHeldSquare(square);
        return;
      }
      if (held !== null && ["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"].includes(e.key)) {
        e.preventDefault();
        e.stopImmediatePropagation();
        const { curves, channel, onChange, region } = live.current;
        if (region) return;
        onChange({ ...curves, [channel]: nudgePoint(curves[channel], held, e.key, e.ctrlKey || e.metaKey ? 0.05 : 0.01) });
      }
    }
    function release() {
      held = null;
      setHeldSquare(null);
    }
    function onKeyUp(e: KeyboardEvent) {
      if (held !== null && SQUARE[e.code] === held) release();
    }
    // A window losing focus mid-hold never sees the key go up.
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("keyup", onKeyUp, true);
    window.addEventListener("blur", release);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("keyup", onKeyUp, true);
      window.removeEventListener("blur", release);
    };
  }, []);

  function setPoints(next: CurvePoint[]) {
    onChange({ ...curves, [channel]: next });
  }

  function toCurveSpace(e: React.PointerEvent): CurvePoint {
    const rect = svgRef.current!.getBoundingClientRect();
    return {
      x: Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width)),
      y: Math.min(1, Math.max(0, 1 - (e.clientY - rect.top) / rect.height)),
    };
  }

  function nearestPoint(p: CurvePoint): number {
    let best = -1;
    let bestDist = HIT_RADIUS;
    points.forEach((q, i) => {
      const d = Math.hypot(q.x - p.x, q.y - p.y);
      if (d < bestDist) {
        bestDist = d;
        best = i;
      }
    });
    return best;
  }

  // Region tab: the split handles along the bottom.
  const splitValues = SPLITS.map((id) => values[id]);
  function moveSplit(i: number, x: number) {
    const lo = i === 0 ? SLIDER[SPLITS[0]].min : splitValues[i - 1] + SPLIT_GAP;
    const hi = i === 2 ? SLIDER[SPLITS[2]].max : splitValues[i + 1] - SPLIT_GAP;
    onSliderChange(SPLITS[i], Math.round(Math.min(hi, Math.max(lo, x * 100))));
  }

  function onPointerDown(e: React.PointerEvent) {
    const p = toCurveSpace(e);
    if (region) {
      // The nearest split; a double click or tap puts it back.
      let i = 0;
      splitValues.forEach((v, n) => {
        if (Math.abs(v / 100 - p.x) < Math.abs(splitValues[i] / 100 - p.x)) i = n;
      });
      const now = performance.now();
      if (lastTap.current.index === i && now - lastTap.current.time < 300) {
        onSliderChange(SPLITS[i], neutrals[SPLITS[i]]);
        lastTap.current = { time: 0, index: -1 };
        return;
      }
      lastTap.current = { time: now, index: i };
      dragSplit.current = i;
      e.currentTarget.setPointerCapture(e.pointerId);
      return;
    }
    let index = nearestPoint(p);

    // Double click / double tap on an interior point removes it.
    const now = performance.now();
    if (index > 0 && index < points.length - 1 && lastTap.current.index === index && now - lastTap.current.time < 300) {
      setPoints(points.filter((_, i) => i !== index));
      lastTap.current = { time: 0, index: -1 };
      return;
    }
    lastTap.current = { time: now, index };

    if (index === -1) {
      // New point, slotted in by x so the list stays sorted.
      const next = [...points, p].sort((a, b) => a.x - b.x);
      index = next.indexOf(p);
      setPoints(next);
    }
    dragIndex.current = index;
    e.currentTarget.setPointerCapture(e.pointerId);
  }

  function onPointerMove(e: React.PointerEvent) {
    if (region) {
      if (dragSplit.current !== null && e.currentTarget.hasPointerCapture(e.pointerId)) moveSplit(dragSplit.current, toCurveSpace(e).x);
      return;
    }
    const i = dragIndex.current;
    if (i === null || !e.currentTarget.hasPointerCapture(e.pointerId)) return;
    const p = toCurveSpace(e);
    // Every point, end points included, stays between its neighbours so the
    // curve can never fold back on itself.
    const lo = i === 0 ? 0 : points[i - 1].x + MIN_GAP;
    const hi = i === points.length - 1 ? 1 : points[i + 1].x - MIN_GAP;
    const x = Math.min(hi, Math.max(lo, p.x));
    setPoints(points.map((q, n) => (n === i ? { x, y: p.y } : q)));
  }

  function onPointerUp() {
    dragIndex.current = null;
    dragSplit.current = null;
  }

  const amounts = ["paramShadows", "paramDarks", "paramLights", "paramHighlights"].map((id) => values[id]) as [number, number, number, number];
  const regionEdited = [...REGION_SLIDERS, ...SPLITS].some((id) => values[id] !== neutrals[id]);
  // The region curve as the engine applies it (Lightroom's, measured); the
  // rough local shape stands in until the engine's answer arrives.
  const regionKey = JSON.stringify([amounts, splitValues]);
  const [engineCurve, setEngineCurve] = useState<{ key: string; points: number[] } | null>(null);
  useEffect(() => {
    if (!region) return;
    let cancelled = false;
    parametricCurvePoints(amounts, splitValues as [number, number, number])
      .then((points) => !cancelled && setEngineCurve({ key: regionKey, points }))
      .catch(() => {
        // Not in the app (e.g. a browser preview): the local shape stays.
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [region, regionKey]);
  const shape = parametricShape(amounts, splitValues as [number, number, number]);
  const samples = !region
    ? sampleCurve(points, 96)
    : engineCurve?.key === regionKey
      ? engineCurve.points
      : Array.from({ length: 96 }, (_, i) => shape(i / 95));
  const path = samples
    .map((y, i) => `${i === 0 ? "M" : "L"}${((i / (samples.length - 1)) * SIZE).toFixed(1)},${((1 - y) * SIZE).toFixed(1)}`)
    .join(" ");
  const colour = region ? "#3ee6ff" : CHANNEL_COLOUR[channel];

  return (
    <div className="curves">
      <div className="curves__channels">
        {CURVE_CHANNELS.map((c) => (
          <button
            key={c}
            type="button"
            className={`curves__channel${c === tab ? " curves__channel--on" : ""}${isIdentity(curves[c]) ? "" : " curves__channel--edited"}`}
            style={{ color: CHANNEL_COLOUR[c] }}
            title={`${CHANNEL_TITLE[c]}\n${pickHint("Curves", { luma: "L", rgb: "A", r: "R", g: "G", b: "B" }[c])}`}
            onClick={() => {
              setTab(c);
            }}
          >
            {CHANNEL_LABEL[c]}
          </button>
        ))}
        <button
          type="button"
          className={`curves__channel${region ? " curves__channel--on" : ""}${regionEdited ? " curves__channel--edited" : ""}`}
          style={{ color: "#3ee6ff" }}
          title="Region: Lightroom's parametric curve (Highlights, Lights, Darks, Shadows). Drag the three handles along the bottom to move where the regions meet.\nKeys: Shift+C (Curves), then tap P"
          onClick={() => setTab("region")}
        >
          Reg
        </button>
        <button
          type="button"
          className="curves__channel"
          title="Reset this curve"
          onClick={() => {
            if (region) [...REGION_SLIDERS, ...SPLITS].forEach((id) => onSliderChange(id, neutrals[id]));
            else setPoints(DEFAULT_CURVES[channel]);
          }}
        >
          <RotateCcw size={11} />
        </button>
      </div>
      <div className="curves__body">
      <svg
        ref={svgRef}
        className="curves__graph"
        width={SIZE}
        height={SIZE}
        viewBox={`0 0 ${SIZE} ${SIZE}`}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
      >
        {/* Always there (empty until the histogram arrives): WebKitGTK didn't paint one added to
            the graph later until something else in it changed, e.g. a point moved. */}
        <path
          className="curves__histogram"
          d={histogram ? histogramPath(channel === "luma" || channel === "rgb" ? histogram.luma : histogram[channel]) : ""}
          fill={channel === "luma" || channel === "rgb" ? "#8a8b96" : colour}
        />
        {shotLabel && (
          <text className="curves__shot" x={4} y={11}>
            {shotLabel}
          </text>
        )}
        {/* 3x3 grid: shadows / mids / highlights on each axis. */}
        {[1 / 3, 2 / 3].map((f) => (
          <g key={f} className="curves__grid">
            <line x1={f * SIZE} y1={0} x2={f * SIZE} y2={SIZE} />
            <line x1={0} y1={f * SIZE} x2={SIZE} y2={f * SIZE} />
          </g>
        ))}
        {/* Square numbers along the diagonal, laid out like the number pad
            for the keyboard shortcuts: 9 highlights (top right), 5 mids,
            1 shadows (bottom left), sat just above each square's default
            point so the curve doesn't cover them. */}
        {[
          { n: 9, col: 2, row: 0 },
          { n: 5, col: 1, row: 1 },
          { n: 1, col: 0, row: 2 },
        ].map(({ n, col, row }, square) => (
          <text
            key={n}
            className={`curves__cellnum${heldSquare === 2 - square ? " curves__cellnum--held" : ""}`}
            x={((col + 0.5) * SIZE) / 3}
            y={((row + 0.5) * SIZE) / 3 - 14}
            textAnchor="middle"
            dominantBaseline="central"
          >
            {n}
          </text>
        ))}
        <line className="curves__diagonal" x1={0} y1={SIZE} x2={SIZE} y2={0} />
        <path d={path} fill="none" stroke={colour} strokeWidth={1.5} />
        {!region &&
          points.map((p, i) => <polygon key={i} className="curves__point" points={hexPoints(p.x * SIZE, (1 - p.y) * SIZE, 5.5)} />)}
        {region &&
          splitValues.map((v, i) => (
            <g key={SPLITS[i]}>
              {/* Where two regions meet, with its handle on the bottom edge. */}
              <line className="curves__split-line" x1={(v / 100) * SIZE} y1={0} x2={(v / 100) * SIZE} y2={SIZE} />
              <path className="curves__split-handle" d={`M${(v / 100) * SIZE},${SIZE - SPLIT_STRIP} l6,${SPLIT_STRIP - 2} l-12,0 Z`}>
                <title>{`${SLIDER[SPLITS[i]].label}: ${v}`}</title>
              </path>
            </g>
          ))}
      </svg>
      {region && (
        <div className="curves__regions">
          {REGION_SLIDERS.map((id) => (
            <VSlider
              key={id}
              def={SLIDER[id]}
              value={values[id]}
              neutral={neutrals[id]}
              active={activeSliderId === id}
              showName={showNames}
              onChange={(v) => onSliderChange(id, v)}
              hint={adjustHint("Curves", SLIDER[id].label[0], "P for Region")}
            />
          ))}
        </div>
      )}
      </div>
    </div>
  );
}
