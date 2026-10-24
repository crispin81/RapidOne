import { useEffect, useRef } from "react";
import { Check, X, Star } from "lucide-react";
import ReadyBadges from "./ReadyBadges";
import "../strip.css";
import type { PhotoInfo } from "../types";
import { keyLabel } from "../macKeys";
import type { PhotoLook } from "../sidecar";

/** How the stars filter compares: this many and above (the default), exactly, or this many and below (still rated). */
export type StarsOp = "gte" | "eq" | "lte";

/** Which photos the filmstrip shows: by stars (0 = any), picked only. */
export interface StripFilter {
  stars: number;
  op: StarsOp;
  picked: boolean;
}

export const NO_STRIP_FILTER: StripFilter = { stars: 0, op: "gte", picked: false };

/** Whether a photo passes the filter. "And below" means 1 star up to the number: unrated and rejected photos aren't included. */
export function passesStripFilter(p: PhotoInfo, f: StripFilter): boolean {
  if (f.picked && !p.pick) return false;
  if (!f.stars) return true;
  if (f.op === "eq") return p.rating === f.stars;
  if (f.op === "lte") return p.rating >= 1 && p.rating <= f.stars;
  return p.rating >= f.stars;
}

const OPS: { op: StarsOp; sign: string; words: string }[] = [
  { op: "gte", sign: "≥", words: "and above" },
  { op: "eq", sign: "=", words: "exactly" },
  { op: "lte", sign: "≤", words: "and below" },
];

interface Props {
  /** The photos shown (already filtered). */
  photos: PhotoInfo[];
  /** Photos in the folder. */
  total: number;
  /** Photos per star (index 0 = 1 star) and picked, in the whole folder. */
  counts: { stars: number[]; picked: number };
  filter: StripFilter;
  onFilter: (filter: StripFilter) => void;
  currentIndex: number;
  /** A click on a photo; `ctrl` / `shift` are held for a multiple selection (Ctrl toggles, Shift takes a range). */
  onSelect: (index: number, keys?: { ctrl: boolean; shift: boolean }) => void;
  /** The photos picked for export (paths), when more than the open one. */
  selection?: ReadonlySet<string>;
  /** Stacks: on a stack's first photo shown, how many it holds and whether it is open. */
  stacks?: Record<string, { count: number; open: boolean }>;
  /** The stack badge was clicked: open or close that stack. */
  onToggleStack?: (path: string) => void;
  /** All | Rated | Unrated, at the top (once the shoot is organised). */
  viewSwitch?: React.ReactNode;
  /** Right-click on a photo (its path and where the pointer is): the photo's menu. */
  onMenu?: (path: string, x: number, y: number) => void;
  /** Photos whose full picture is rendered with their current edit: a sparkle in the corner. */
  ready?: ReadonlySet<string>;
  /** Photos levelled (1) or levelled and keystoned (2) by Auto: their icons under the sparkle. */
  transforms?: Readonly<Record<string, number>>;
  /** Photos with Noise on: Denoise's icon under the others, once rendered. */
  denoised?: ReadonlySet<string>;
  /** Where a photo's look came from (a style, a preset or its XMP): its icon, once rendered. */
  looks?: Readonly<Record<string, PhotoLook>>;
  /** Photos with an edit: the "edited" icon at the top. */
  edited?: ReadonlySet<string>;
}

/**
 * The filmstrip's top: All | Rated | Unrated (once organised), then the
 * stars filter (N and above / exactly / and below; click again for all),
 * picked only, and the count. Shared with Retouch's film strip.
 */
export function StripFilterBar({
  filter,
  onFilter,
  counts,
  shown,
  total,
  viewSwitch,
}: {
  filter: StripFilter;
  onFilter: (filter: StripFilter) => void;
  counts: { stars: number[]; picked: number };
  shown: number;
  total: number;
  viewSwitch?: React.ReactNode;
}) {
  const filtered = filter.stars > 0 || filter.picked;
  // Filter: N stars and above / exactly / and below (click again for all), and/or picked only.
  return (
    <div className="strip-filter">
      {viewSwitch}
      <button
        type="button"
        className="strip-filter__op"
        onClick={() => onFilter({ ...filter, op: OPS[(OPS.findIndex((o) => o.op === filter.op) + 1) % OPS.length].op })}
        title={`Stars: ${OPS.find((o) => o.op === filter.op)?.words} (click to change: and above, exactly, and below)`}
      >
        {OPS.find((o) => o.op === filter.op)?.sign}
      </button>
      {[1, 2, 3, 4, 5].map((n) => (
        <button
          key={n}
          type="button"
          className={`strip-filter__star${filter.stars >= n ? " strip-filter__star--on" : ""}`}
          onClick={() => onFilter({ ...filter, stars: filter.stars === n ? 0 : n })}
          title={`${n} star${n === 1 ? "" : "s"} ${OPS.find((o) => o.op === filter.op)?.words} (${counts.stars[n - 1]} with exactly ${n}); click again for all`}
        >
          <Star size={11} fill={filter.stars >= n ? "currentColor" : "none"} />
        </button>
      ))}
      <button
        type="button"
        className={`strip-filter__pick${filter.picked ? " strip-filter__pick--on" : ""}`}
        onClick={() => onFilter({ ...filter, picked: !filter.picked })}
        title={`Picked only (${counts.picked})`}
      >
        <Check size={12} />
      </button>
      <span className="strip-filter__count" title={filtered ? "Shown / in the folder" : "Photos in the folder"}>
        {filtered ? `${shown}/${total}` : total}
      </span>
    </div>
  );
}

/** The stack badge on a thumbnail: the number of photos in the stack; click to open or close it. */
export function StackBadge({ count, open, onToggle }: { count: number; open: boolean; onToggle: () => void }) {
  return (
    <span
      role="button"
      tabIndex={-1}
      className={`stack-badge${open ? " stack-badge--open" : ""}`}
      title={open ? "Close the stack" : `Stack of ${count}: the newest on top. Click to open`}
      onClick={(e) => {
        e.stopPropagation();
        onToggle();
      }}
      onDoubleClick={(e) => e.stopPropagation()}
    >
      {count}
    </span>
  );
}

export default function Filmstrip({ photos, total, counts, filter, onFilter, currentIndex, onSelect, selection, stacks, onToggleStack, viewSwitch, onMenu, ready, transforms, denoised, looks, edited }: Props) {
  const filtered = filter.stars > 0 || filter.picked;
  const currentRef = useRef<HTMLButtonElement>(null);

  // Keep the current photo in view as Shift + arrows move through the shoot.
  useEffect(() => {
    currentRef.current?.scrollIntoView({ block: "nearest" });
  }, [currentIndex]);

  return (
    <aside className="filmstrip">
      <StripFilterBar filter={filter} onFilter={onFilter} counts={counts} shown={photos.length} total={total} viewSwitch={viewSwitch} />
      {filtered && photos.length === 0 && <p className="strip-filter__empty">No photos match.</p>}
      {photos.map((photo, i) => {
        const rejected = photo.rating === -1;
        const current = i === currentIndex;
        return (
          <button
            key={photo.path}
            ref={current ? currentRef : undefined}
            type="button"
            className={`thumb${current ? " thumb--selected" : ""}${selection?.has(photo.path) ? " thumb--multi" : ""}${rejected ? " thumb--rejected" : ""}`}
            onClick={(e) => onSelect(i, { ctrl: e.ctrlKey || e.metaKey, shift: e.shiftKey })}
            onContextMenu={
              onMenu
                ? (e) => {
                    e.preventDefault();
                    onMenu(photo.path, e.clientX, e.clientY);
                  }
                : undefined
            }
            title={keyLabel(`${photo.fileName}\nKeys: ←/→ or Shift+↑/↓ for the previous / next photo · 0-5 stars · P pick · X reject\nCtrl-click adds or removes a photo, Shift-click takes a range, Ctrl+A selects all (for Export)`)}
          >
            <img src={photo.thumbBase64} alt={photo.fileName} draggable={false} />
            <ReadyBadges edited={edited?.has(photo.path) ?? false} ready={ready?.has(photo.path) ?? false} level={transforms?.[photo.path] ?? 0} denoised={denoised?.has(photo.path) ?? false} look={looks?.[photo.path]} />
            {stacks?.[photo.path] && onToggleStack && (
              <StackBadge count={stacks[photo.path].count} open={stacks[photo.path].open} onToggle={() => onToggleStack(photo.path)} />
            )}
            <div className="thumb__overlay">
              {photo.pick && (
                // A cyan tick in a black circle (as Cull's, and the other thumbnail icons).
                <span className="thumb__pick">
                  <Check size={11} strokeWidth={3} />
                </span>
              )}
              {rejected && <X className="thumb__reject" size={14} />}
              {/* The stars, as in Cull. */}
              {!rejected && photo.rating > 0 && (
                <span className="thumb__stars">
                  {Array.from({ length: photo.rating }).map((_, i) => (
                    <Star key={i} size={11} fill="currentColor" stroke="none" />
                  ))}
                </span>
              )}
            </div>
          </button>
        );
      })}
    </aside>
  );
}
