import { useRef } from "react";
import Hex, { hexPoints } from "./Hex";

interface Props {
  /** The two points, 0..100, `low` never above `high`. */
  low: number;
  high: number;
  lowNeutral: number;
  highNeutral: number;
  lowLabel: string;
  highLabel: string;
  title: string;
  /** Which point's key is held, if either. */
  active: "low" | "high" | null;
  showName: boolean;
  onLow: (value: number) => void;
  onHigh: (value: number) => void;
}

/**
 * A VSlider with two points: the lower one's name then the upper one's below it, each with its key
 * letter. A press moves the nearer point (the wheel too); double-clicking the names resets both.
 */
export default function VRange({ low, high, lowNeutral, highNeutral, lowLabel, highLabel, title, active, showName, onLow, onHigh }: Props) {
  const trackRef = useRef<HTMLDivElement>(null);
  // `which` is null when the two points were pressed together: the way it moves picks one.
  const drag = useRef<{ which: "low" | "high" | null; startY: number; startValue: number } | null>(null);

  const put = (which: "low" | "high", v: number) => {
    if (which === "low") onLow(Math.round(Math.min(high, Math.max(0, v))));
    else onHigh(Math.round(Math.max(low, Math.min(100, v))));
  };
  const at = (clientY: number) => {
    const rect = trackRef.current!.getBoundingClientRect();
    return (1 - (clientY - rect.top) / rect.height) * 100;
  };
  /** The nearer point; on top of each other, the one that can move that way. */
  const nearer = (v: number): "low" | "high" => (low === high ? (v > high ? "high" : v < low ? "low" : low >= 100 ? "low" : "high") : Math.abs(v - low) < Math.abs(v - high) ? "low" : "high");

  function onPointerDown(e: React.PointerEvent) {
    e.currentTarget.setPointerCapture(e.pointerId);
    const v = at(e.clientY);
    const which = nearer(v);
    let startValue = which === "low" ? low : high;
    // Pressing on a thumb grabs it where it is; anywhere else jumps it there first (as VSlider).
    if ((Math.abs(v - startValue) / 100) * trackRef.current!.getBoundingClientRect().height > 8) {
      startValue = Math.min(100, Math.max(0, v));
      put(which, startValue);
    }
    drag.current = { which: low === high && startValue === low ? null : which, startY: e.clientY, startValue };
  }

  function onPointerMove(e: React.PointerEvent) {
    const d = drag.current;
    if (!d || !e.currentTarget.hasPointerCapture(e.pointerId)) return;
    if (!d.which && e.clientY === d.startY) return;
    d.which ??= e.clientY < d.startY ? "high" : "low";
    put(d.which, d.startValue + ((d.startY - e.clientY) / trackRef.current!.getBoundingClientRect().height) * 100);
  }

  const point = (which: "low" | "high", label: string) => (
    <>
      <span className="vslider__keyletter">{label[0]}</span>
      {showName && label.slice(1)}
      {which === "low" && "–"}
    </>
  );
  const changed = low !== lowNeutral || high !== highNeutral;
  return (
    <div className={`vslider vslider--range${active ? " vslider--active" : ""}${showName ? "" : " vslider--compact"}`} title={title}>
      <div className={`vslider__value${changed ? "" : " vslider__value--neutral"}`}>
        {low}–{high}
      </div>
      <div className="vslider__body">
        <div
          className="vslider__hit"
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={() => (drag.current = null)}
          onPointerCancel={() => (drag.current = null)}
          onWheel={(e) => {
            const which = nearer(at(e.clientY));
            put(which, (which === "low" ? low : high) + (e.deltaY < 0 ? 1 : -1));
          }}
          role="group"
          aria-label={`${lowLabel} and ${highLabel}`}
        >
          <div className="vslider__track" ref={trackRef}>
            <div className="vslider__fill" style={{ bottom: `${low}%`, height: `${high - low}%` }} />
            {([["low", low], ["high", high]] as const).map(([which, v]) => (
              <Hex key={which} w={14} h={12} className="vslider__thumb" style={{ bottom: `${v}%` }}>
                <polygon points={hexPoints(14, 12, 1)} />
              </Hex>
            ))}
          </div>
        </div>
      </div>
      <div
        className="vslider__key"
        onDoubleClick={() => {
          onLow(lowNeutral);
          onHigh(highNeutral);
        }}
      >
        {point("low", lowLabel)}
        {point("high", highLabel)}
      </div>
    </div>
  );
}
