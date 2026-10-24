import type { Adjustments } from "./adjustments";
import type { ProfileId } from "./api";
import type { Curves } from "./curves";
import type { Grade } from "./grade";
import type { Mixer } from "./mixer";
import type { Crop } from "./crop";
import type { Spot } from "./api";
import type { Gradient } from "./gradients";

/**
 * Undo / redo, per photo (as Lightroom keeps a history per photo): each step
 * is a snapshot of the photo's whole edit before a change.
 */

/** Steps kept per photo. Snapshots are small, so this is generous. */
export const HISTORY_STEPS = 100;

/**
 * Changes to the same control closer together than this are one step, so a
 * drag, a held arrow key or a scroll counts once, not per tick.
 */
const MERGE_MS = 800;

/** A photo's edit; undefined fields are the photo's defaults. */
export interface Snapshot {
  edits?: Partial<Adjustments>;
  curves?: Curves;
  grade?: Grade;
  mixer?: Mixer;
  profile?: ProfileId;
  crop?: Crop;
  removals?: Spot[];
  gradients?: Gradient[];
}

export interface History {
  past: Snapshot[];
  future: Snapshot[];
  lastKey: string | null;
  lastAt: number;
}

export function newHistory(): History {
  return { past: [], future: [], lastKey: null, lastAt: 0 };
}

/**
 * Notes that `key` (a control, e.g. "slider:exposure") is about to change the
 * photo now in `before`. Starts a new step unless it continues the last one.
 */
export function recordChange(h: History, key: string | null, before: Snapshot, now: number) {
  const continuing = key !== null && key === h.lastKey && now - h.lastAt < MERGE_MS;
  h.lastAt = now;
  if (continuing) return;
  h.past.push(before);
  if (h.past.length > HISTORY_STEPS) h.past.shift();
  h.future = [];
  h.lastKey = key;
}

/** Steps back: returns the snapshot to restore, or null at the start. */
export function undo(h: History, current: Snapshot): Snapshot | null {
  const prev = h.past.pop();
  if (!prev) return null;
  h.future.push(current);
  h.lastKey = null;
  return prev;
}

/** Steps forward again: returns the snapshot to restore, or null at the end. */
export function redo(h: History, current: Snapshot): Snapshot | null {
  const next = h.future.pop();
  if (!next) return null;
  h.past.push(current);
  h.lastKey = null;
  return next;
}

/** `map` with `path` set to `value`, or removed when `value` is undefined. */
export function withEntry<T>(map: Record<string, T>, path: string, value: T | undefined): Record<string, T> {
  const next = { ...map };
  if (value === undefined) delete next[path];
  else next[path] = value;
  return next;
}
