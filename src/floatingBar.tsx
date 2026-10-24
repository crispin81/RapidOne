import { useLayoutEffect, useRef, useState } from "react";
import { GripVertical } from "lucide-react";

/** Where each floating bar was dragged to, by name: in memory only, so a bar is back in its own place
 *  when the app next opens. */
const places = new Map<string, { left: number; top: number }>();

/**
 * A floating bar (Crop, Remove, Patch, Layers) moved by its grip. Put `ref` and `style` on the bar
 * and `<BarGrip onPointerDown={grip} />` first in it; the bar stays inside the area it floats over.
 */
export function useFloatingBar(name: string) {
  const ref = useRef<HTMLDivElement>(null);
  const [place, setPlace] = useState(() => places.get(name) ?? null);
  const fit = (left: number, top: number) => {
    const bar = ref.current;
    const area = bar?.offsetParent as HTMLElement | null;
    if (!bar || !area) return { left, top };
    return {
      left: Math.min(Math.max(0, left), Math.max(0, area.clientWidth - bar.offsetWidth)),
      top: Math.min(Math.max(0, top), Math.max(0, area.clientHeight - bar.offsetHeight)),
    };
  };
  // A place from a bigger window: pulled back inside.
  useLayoutEffect(() => {
    if (!place) return;
    const f = fit(place.left, place.top);
    if (f.left !== place.left || f.top !== place.top) setPlace(f);
  });
  const grip = (e: React.PointerEvent<HTMLElement>) => {
    const bar = ref.current;
    if (!bar || e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    const handle = e.currentTarget;
    handle.setPointerCapture(e.pointerId);
    // Where the bar is on screen, inside its area: not offsetLeft / offsetTop, which leave out the
    // centring shift (translateX(-50%)) a bar starts with, so the first drag jumped it right by half
    // its width.
    const area = bar.offsetParent as HTMLElement | null;
    const b = bar.getBoundingClientRect();
    const a = area?.getBoundingClientRect();
    const start = {
      x: e.clientX,
      y: e.clientY,
      left: a && area ? b.left - a.left - area.clientLeft : bar.offsetLeft,
      top: a && area ? b.top - a.top - area.clientTop : bar.offsetTop,
    };
    const move = (ev: PointerEvent) => {
      const f = fit(start.left + ev.clientX - start.x, start.top + ev.clientY - start.y);
      places.set(name, f);
      setPlace(f);
    };
    const up = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
      handle.removeEventListener("pointercancel", up);
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
    handle.addEventListener("pointercancel", up);
  };
  // Double-click the grip: back to its own place.
  const reset = () => {
    places.delete(name);
    setPlace(null);
  };
  const style: React.CSSProperties | undefined = place ? { left: place.left, top: place.top, transform: "none" } : undefined;
  return { ref, style, grip, reset };
}

export function BarGrip({ bar }: { bar: ReturnType<typeof useFloatingBar> }) {
  return (
    <span className="cropbar__grip" onPointerDown={bar.grip} onDoubleClick={bar.reset} title="Drag to move (double-click: back to its place)">
      <GripVertical size={14} />
    </span>
  );
}
