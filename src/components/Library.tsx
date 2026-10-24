import HSlider from "./HSlider";
import { Check, X, Star } from "lucide-react";
import ReadyBadges from "./ReadyBadges";
import type { ReactNode } from "react";
import "../library.css";
import type { PhotoInfo } from "../types";
import { StackBadge, StripFilterBar, type StripFilter } from "./Filmstrip";
import { keyLabel } from "../macKeys";
import type { PhotoLook } from "../sidecar";

interface Props {
  /** The photos shown (already filtered), in capture order. */
  photos: PhotoInfo[];
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
  /** Photos in the folder. */
  total: number;
  counts: { stars: number[]; picked: number };
  filter: StripFilter;
  onFilter: (filter: StripFilter) => void;
  /** All | Rated | Unrated, once the shoot is organised (as on the filmstrip). */
  viewSwitch?: ReactNode;
  /** The photo open in the loupe. */
  currentPath: string | null;
  /** The photos picked for export, when more than the open one. */
  selection: ReadonlySet<string>;
  /** Stacks (as on the filmstrip): count and open state on a stack's first photo shown. */
  stacks?: Record<string, { count: number; open: boolean }>;
  onToggleStack?: (path: string) => void;
  /** A click; Ctrl toggles a photo, Shift takes a range. */
  onSelect: (index: number, keys?: { ctrl: boolean; shift: boolean }) => void;
  /** Double-click: open it in the loupe. */
  onOpen: (index: number) => void;
  onSelectAll: () => void;
  onSelectNone: () => void;
  /** Thumbnail width in pixels. */
  size: number;
  onSize: (size: number) => void;
  /** Right-click on a photo (its path and where the pointer is): the photo's menu. */
  onMenu?: (path: string, x: number, y: number) => void;
  /** The docked bottom panel's height: the grid ends above it, so its last row can be scrolled to. */
  bottomInset?: number;
}

export const LIBRARY_MIN = 100;
export const LIBRARY_MAX = 320;

/**
 * The library: the photos as a grid in the main pane (instead of the loupe),
 * with the filmstrip's filter, All | Rated | Unrated switch and marks.
 * Selecting works as on the filmstrip, so Export selected can start here.
 */
export default function Library({
  photos,
  ready,
  transforms,
  denoised,
  looks,
  edited,
  total,
  counts,
  filter,
  onFilter,
  viewSwitch,
  currentPath,
  selection,
  stacks,
  onToggleStack,
  onSelect,
  onOpen,
  onSelectAll,
  onSelectNone,
  size,
  onSize,
  onMenu,
  bottomInset = 0,
}: Props) {
  const filtered = filter.stars > 0 || filter.picked;
  return (
    <section className="gridlib" style={{ ["--gridlib-size" as string]: `${size}px`, bottom: bottomInset }}>
      <div className="gridlib__bar">
        <StripFilterBar filter={filter} onFilter={onFilter} counts={counts} shown={photos.length} total={total} viewSwitch={viewSwitch} />
        <div className="gridlib__tools">
          <span className="gridlib__selected" title={keyLabel("Ctrl-click adds or removes a photo, Shift-click takes a range")}>
            {selection.size > 0 ? `${selection.size} selected` : "none selected"}
          </span>
          <button type="button" onClick={onSelectAll} disabled={photos.length === 0} title="Select every photo shown">
            Select all
          </button>
          <button type="button" onClick={onSelectNone} disabled={selection.size === 0} title="Clear the selection">
            Select none
          </button>
          <label className="gridlib__size" title="Thumbnail size">
            Size
            <HSlider min={LIBRARY_MIN} max={LIBRARY_MAX} step={10} value={size} onChange={onSize} width={100} aria-label="Thumbnail size" />
          </label>
        </div>
      </div>
      {filtered && photos.length === 0 && <p className="strip-filter__empty">No photos match.</p>}
      <div className="gridlib__grid">
        {photos.map((photo, i) => {
          const rejected = photo.rating === -1;
          const current = photo.path === currentPath;
          const picked = selection.has(photo.path);
          return (
            <button
              key={photo.path}
              type="button"
              className={`thumb gridlib__thumb${current ? " thumb--selected" : ""}${picked ? " thumb--multi" : ""}${rejected ? " thumb--rejected" : ""}`}
              onClick={(e) => onSelect(i, { ctrl: e.ctrlKey || e.metaKey, shift: e.shiftKey })}
              onDoubleClick={() => onOpen(i)}
              onContextMenu={
                onMenu
                  ? (e) => {
                      e.preventDefault();
                      onMenu(photo.path, e.clientX, e.clientY);
                    }
                  : undefined
              }
              title={keyLabel(`${photo.fileName}\nDouble-click to open it in the loupe · Ctrl-click adds or removes · Shift-click takes a range`)}
            >
              <img src={photo.thumbBase64} alt={photo.fileName} draggable={false} loading="lazy" />
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
                {!rejected && photo.rating > 0 && (
                  <span className="thumb__stars">
                    {Array.from({ length: photo.rating }).map((_, n) => (
                      <Star key={n} size={11} fill="currentColor" stroke="none" />
                    ))}
                  </span>
                )}
              </div>
            </button>
          );
        })}
      </div>
    </section>
  );
}
