/**
 * Denoise's icon: a circle, grainy dots on the left half and smooth on the right (noisy to clean). The top
 * bar's Denoise all and the thumbnails' badge (one place to change it). Drawn as lucide's are (24 x 24,
 * currentColor), so it sits with the other icons.
 */
export default function DenoiseIcon({ size }: { size: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" aria-hidden="true">
      <circle cx="12" cy="12" r="9.5" />
      <path d="M12 2.5a9.5 9.5 0 0 1 0 19z" fill="currentColor" stroke="none" />
      <g fill="currentColor" stroke="none">
        <circle cx="8.6" cy="7.4" r="1.3" />
        <circle cx="5.6" cy="11" r="1.3" />
        <circle cx="9.4" cy="12.2" r="1.3" />
        <circle cx="6.6" cy="15.4" r="1.3" />
        <circle cx="9.6" cy="17.2" r="1.3" />
      </g>
    </svg>
  );
}
