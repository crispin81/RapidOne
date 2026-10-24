import { useId } from "react";
import { Layers } from "lucide-react";

/**
 * The Linear gradient tool's icon: an upright frame filled with a gradient
 * that fades from the top down, drawn in the current text colour (so it
 * follows hover and the lit state like the lucide icons beside it).
 */
export function LinearGradientIcon({ size = 14 }: { size?: number }) {
  const id = useId();
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <defs>
        <linearGradient id={id} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="currentColor" stopOpacity="0.95" />
          <stop offset="1" stopColor="currentColor" stopOpacity="0" />
        </linearGradient>
      </defs>
      <rect x="6" y="2.5" width="12" height="19" rx="2" fill={`url(#${id})`} />
      <rect x="6" y="2.5" width="12" height="19" rx="2" stroke="currentColor" strokeWidth="2" />
    </svg>
  );
}

/**
 * The Radial gradient tool's icon: a circle filled with a gradient that is
 * solid in the middle and fades towards the edge, in the current text colour.
 */
export function RadialGradientIcon({ size = 14 }: { size?: number }) {
  const id = useId();
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <defs>
        <radialGradient id={id} cx="0.5" cy="0.5" r="0.5">
          <stop offset="0" stopColor="currentColor" stopOpacity="0.95" />
          <stop offset="1" stopColor="currentColor" stopOpacity="0" />
        </radialGradient>
      </defs>
      <circle cx="12" cy="12" r="9.5" fill={`url(#${id})`} />
      <circle cx="12" cy="12" r="9.5" stroke="currentColor" strokeWidth="2" />
    </svg>
  );
}

/**
 * The Masks button's icon (double width): the radial gradient icon and lucide's layers icon,
 * side by side, both in the current text colour.
 */
export function MasksIcon({ size = 14 }: { size?: number }) {
  return (
    <span style={{ display: "inline-flex", alignItems: "center", gap: 2 }} aria-hidden="true">
      <RadialGradientIcon size={size} />
      <Layers size={size} />
    </span>
  );
}
