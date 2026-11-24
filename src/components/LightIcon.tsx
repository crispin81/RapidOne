import type { ReactElement } from "react";

/**
 * A studio light's modifier as a small side-on drawing, for the Relight chips and the Type row:
 * the same shapes the 3D space draws (`scope/rig.ts`). The light faces right in every icon.
 */
export default function LightIcon({ type, size = 16, glow }: { type: string; size?: number; glow?: string }) {
  const lit = glow ?? "#ffffff";
  const body = "currentColor";
  let shape: ReactElement;
  switch (type) {
    case "softbox":
      shape = (
        <>
          <path d="M3 7.5 L10 2 L10 16 L3 10.5 Z" fill={body} opacity={0.55} />
          <rect x="9.6" y="2" width="1.8" height="14" fill={lit} />
          <rect x="1" y="7.6" width="2.4" height="2.8" fill={body} />
        </>
      );
      break;
    case "rim":
      shape = (
        <>
          <path d="M5 7.8 L10 1 L10 17 L5 10.2 Z" fill={body} opacity={0.55} />
          <rect x="9.6" y="1" width="1.8" height="16" fill={lit} />
          <rect x="2.6" y="7.8" width="2.4" height="2.4" fill={body} />
        </>
      );
      break;
    case "fill":
      shape = (
        <>
          <path d="M4 7.5 L10 2.5 L10 15.5 L4 10.5 Z" fill={body} opacity={0.55} />
          <path d="M9.6 2.5 L11.4 4 L11.4 14 L9.6 15.5 Z" fill={lit} />
          <rect x="1.6" y="7.6" width="2.4" height="2.8" fill={body} />
        </>
      );
      break;
    case "umbrella":
      shape = (
        <>
          <path d="M8 1.5 Q15 9 8 16.5 Q10.5 9 8 1.5 Z" fill={lit} opacity={0.9} />
          <path d="M8 1.5 Q10.5 9 8 16.5" stroke={body} strokeWidth={0.8} fill="none" />
          <line x1="1" y1="9" x2="14" y2="9" stroke={body} strokeWidth={1} />
          <rect x="2" y="7.4" width="3.6" height="3.2" rx="0.6" fill={body} />
        </>
      );
      break;
    case "dish":
      shape = (
        <>
          <path d="M5.5 7 Q9 4 11 2 L11 16 Q9 14 5.5 11 Z" fill={body} opacity={0.75} />
          <rect x="11" y="2" width="1" height="14" fill={lit} opacity={0.7} />
          <rect x="11.6" y="7.6" width="1.4" height="2.8" fill={body} />
          <rect x="2" y="7.4" width="3.6" height="3.2" fill={body} />
        </>
      );
      break;
    case "grid":
      shape = (
        <>
          <path d="M4 6 L13 7.4 L13 10.6 L4 12 Z" fill={body} opacity={0.75} />
          <rect x="12.6" y="7.4" width="1.6" height="3.2" fill={lit} />
          <line x1="13.4" y1="7.4" x2="13.4" y2="10.6" stroke={body} strokeWidth={0.4} />
          <rect x="1" y="6.6" width="3.2" height="4.8" fill={body} />
        </>
      );
      break;
    case "bounce":
      shape = (
        <>
          <rect x="4" y="9" width="5" height="6" fill={body} opacity={0.75} />
          <rect x="3.6" y="5" width="5.8" height="4" fill={body} />
          <rect x="4.2" y="4.2" width="4.6" height="1" fill={lit} />
          <path d="M8.8 4.2 L13 0.8" stroke={lit} strokeWidth={1.4} />
        </>
      );
      break;
    case "sun":
      shape = (
        <>
          <circle cx="9" cy="9" r="3.6" fill={glow ?? "#ffcf5a"} />
          {[0, 45, 90, 135, 180, 225, 270, 315].map((a) => (
            <line key={a} x1={9 + 5.2 * Math.cos((a * Math.PI) / 180)} y1={9 + 5.2 * Math.sin((a * Math.PI) / 180)} x2={9 + 7.6 * Math.cos((a * Math.PI) / 180)} y2={9 + 7.6 * Math.sin((a * Math.PI) / 180)} stroke={glow ?? "#ffcf5a"} strokeWidth={1.3} strokeLinecap="round" />
          ))}
        </>
      );
      break;
    case "moon":
      shape = <path d="M11.5 2.5 A6.5 6.5 0 1 0 15.5 13 A5.2 5.2 0 1 1 11.5 2.5 Z" fill={glow ?? "#dfe7f2"} />;
      break;
    default:
      shape = (
        <>
          <path d="M8 6 L12 3.5 L12 14.5 L8 12 Z" fill={body} opacity={0.6} />
          <circle cx="10.4" cy="9" r="1.6" fill={lit} />
          <rect x="2" y="6.8" width="6.2" height="4.4" rx="0.8" fill={body} />
        </>
      );
  }
  return (
    <svg className="light-icon" width={size} height={size} viewBox="0 0 18 18" aria-hidden="true">
      {shape}
    </svg>
  );
}
