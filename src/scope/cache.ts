/**
 * The Depth Scope's cloud, loaded once per photo and kept for the session: opening the scope
 * again on the same photo is instant. `scene_points` is only asked again after `forgetScope`
 * (the photo's scene was made again, or the first attempt failed).
 */

import { scenePoints } from "../api";
import { decodeCloud, type ScopeCloud } from "./cloud";

const clouds = new Map<string, Promise<ScopeCloud | null>>();

/** How many photos' clouds are kept: about 7 MB each at 300k points, so three is plenty. */
const MAX_CLOUDS = 3;

/** Drops the oldest photo's cloud once there are more than MAX_CLOUDS (the map is in order). */
function trim() {
  while (clouds.size > MAX_CLOUDS) {
    const oldest = clouds.keys().next();
    if (oldest.done) break;
    clouds.delete(oldest.value);
  }
}

/** The photo's point cloud (null when it has no scene geometry); loaded and decoded once. */
export function scopeCloud(path: string, maxPoints?: number): Promise<ScopeCloud | null> {
  const key = `${path}\u0001${maxPoints ?? ""}`;
  const cached = clouds.get(key);
  if (cached) return cached;
  const load = scenePoints(path, maxPoints).then((b64) => (b64 ? decodeCloud(b64) : null));
  // A failed load isn't kept, nor a photo with no geometry yet (its depth model may come later):
  // the next open tries again.
  load.then((c) => c === null && clouds.get(key) === load && clouds.delete(key)).catch(() => clouds.delete(key));
  clouds.set(key, load);
  trim();
  return load;
}

/** Drops a photo's cloud (or every photo's), so the next open loads it again. */
export function forgetScope(path?: string) {
  if (path === undefined) {
    clouds.clear();
    return;
  }
  for (const key of [...clouds.keys()]) if (key.startsWith(`${path}\u0001`)) clouds.delete(key);
}
