import type { CSSProperties, ReactNode } from "react";

/**
 * The interface's small hexagons (flat-topped) as SVG, so outlines are drawn
 * as one even line rather than two clipped shapes, and edges stay crisp.
 * Sizes are whole pixels.
 */

/** A flat-topped hexagon in a w x h box, inset by `d` px all round (outlines sit on half pixels). */
export function hexPoints(w: number, h: number, d = 0): string {
  const [l, r, t, b] = [d, w - d, d, h - d];
  const q = (r - l) / 4;
  return [
    [l + q, t],
    [r - q, t],
    [r, h / 2],
    [r - q, b],
    [l + q, b],
    [l, h / 2],
  ]
    .map(([x, y]) => `${x},${y}`)
    .join(" ");
}

/** The left half of the hexagon (Grade's Global dot is half black, half white). */
export function hexLeftHalf(w: number, h: number, d = 0): string {
  const [l, t, b] = [d, d, h - d];
  const q = (w - 2 * d) / 4;
  return [
    [l + q, t],
    [w / 2, t],
    [w / 2, b],
    [l + q, b],
    [l, h / 2],
  ]
    .map(([x, y]) => `${x},${y}`)
    .join(" ");
}

export default function Hex({
  w,
  h,
  className,
  style,
  children,
}: {
  w: number;
  h: number;
  className?: string;
  style?: CSSProperties;
  /** Polygons inside the w x h viewBox (use hexPoints). */
  children: ReactNode;
}) {
  return (
    <svg className={className} style={style} width={w} height={h} viewBox={`0 0 ${w} ${h}`} aria-hidden="true">
      {children}
    </svg>
  );
}

/**
 * A dot control (Mixer band, Fine tuning primary, Grade range): a hexagonal
 * ring in the button's `--ring` colour (thicker when `on`), a gap, then the
 * colour (`--dot`), or half black / half white for `half`; `spot` adds a
 * white centre (changed).
 */
export function RingDot({ size, gap, on, half, spot }: { size: number; gap: number; on: boolean; half?: boolean; spot?: boolean }) {
  const ring = on ? 2 : 1;
  const inner = ring + gap;
  const c = size / 2;
  return (
    <Hex w={size} h={size}>
      <polygon points={hexPoints(size, size, ring / 2)} style={{ fill: "none", stroke: "var(--ring)", strokeWidth: ring }} />
      {half ? (
        <>
          <polygon points={hexPoints(size, size, inner)} style={{ fill: "#ffffff" }} />
          <polygon points={hexLeftHalf(size, size, inner)} style={{ fill: "#000000" }} />
        </>
      ) : (
        <polygon points={hexPoints(size, size, inner)} style={{ fill: "var(--dot)" }} />
      )}
      {spot && <polygon points={hexPoints(4, 4)} transform={`translate(${c - 2} ${c - 2})`} style={{ fill: "#ffffff" }} />}
    </Hex>
  );
}
