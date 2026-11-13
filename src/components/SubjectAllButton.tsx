/**
 * "People mask: all photos" (Chris, 13 Nov): one click gives every photo in the folder a subject
 * mask (photos that have one keep theirs) and finds each subject in the background; its Exposure
 * and Contrast then show in the main panel's People Mask group (Shift+P). Beside Denoise all.
 */

/** Head and shoulders, our own drawing, as lucide's icons are drawn (24 x 24, currentColor), so it
 *  sits with the top bar's other icons. */
export function SubjectIcon({ size }: { size: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <circle cx="12" cy="8" r="4" fill="currentColor" stroke="none" />
      <path d="M4 21v-1.5A5.5 5.5 0 0 1 9.5 14h5a5.5 5.5 0 0 1 5.5 5.5V21z" fill="currentColor" stroke="none" />
      <rect x="2" y="2" width="20" height="20" rx="3" strokeDasharray="3 2.5" />
    </svg>
  );
}

interface Props {
  /** No folder open, or no photos: nothing to do. */
  disabled: boolean;
  /** "Finding people 42/330" while running, else null. */
  progress: string | null;
  /** How far it is (0-1): the button fills as the Style button does. */
  fraction?: number | null;
  onApply: () => void;
}

export default function SubjectAllButton({ disabled, progress, fraction = null, onApply }: Props) {
  return (
    <div className="topbar__menu">
      <button
        type="button"
        className={progress ? "is-on autostyle__button--busy" : undefined}
        disabled={disabled || Boolean(progress)}
        onClick={onApply}
        title="People mask: all photos. Finds the people in every photo in this folder and adds People Mask Exposure and Contrast to the main panel."
        aria-label="People mask: all photos"
      >
        {/* While it runs, it fills left to right with the progress bars' orange-to-cyan (the Style button's). */}
        {progress && (
          <span
            className={`autostyle__fill${fraction == null ? " autostyle__fill--unknown" : ""}`}
            style={fraction == null ? undefined : { width: `${Math.round(Math.min(1, Math.max(0, fraction)) * 100)}%` }}
            aria-hidden="true"
          />
        )}
        <span className="autostyle__label">
          <SubjectIcon size={14} />
        </span>
        {progress && <span className="autostyle__label">{progress}</span>}
      </button>
    </div>
  );
}
