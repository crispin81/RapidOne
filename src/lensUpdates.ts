import { lensDbSave, lensDbVersion } from "./api";

/** When the lens database was last checked for an update (localStorage). */
const LAST_CHECK_KEY = "rapidone.lensProfilesChecked";
const WEEK_MS = 7 * 24 * 60 * 60 * 1000;
const REPO = "https://api.github.com/repos/lensfun/lensfun";

/**
 * The lens profiles' update: once a week (or now, `force`), Lensfun's latest database on
 * GitHub (its last change to data/db) replaces ours if it's newer; every file is fetched
 * and checked by the engine before it's used. Returns a line to show, or null when there's
 * nothing to say (not due, already up to date on a weekly check, or offline).
 */
export async function checkLensUpdates(force = false): Promise<string | null> {
  try {
    const last = Number(localStorage.getItem(LAST_CHECK_KEY) ?? 0);
    if (!force && Date.now() - last < WEEK_MS) return null;
  } catch {
    // No storage: check anyway.
  }
  try {
    const res = await fetch(`${REPO}/commits?path=data/db&per_page=1`);
    if (!res.ok) throw new Error(`GitHub answered ${res.status}`);
    const [latest] = (await res.json()) as { sha: string; commit: { committer: { date: string } } }[];
    try {
      localStorage.setItem(LAST_CHECK_KEY, String(Date.now()));
    } catch {
      // Not fatal: it's checked again next start.
    }
    if (!latest) return null;
    const sha = latest.sha.slice(0, 7);
    const date = latest.commit.committer.date;
    const [ours, ourDate] = await lensDbVersion();
    if (sha === ours || date <= ourDate) return force ? "Lens profiles are up to date." : null;
    const list = await fetch(`${REPO}/contents/data/db?ref=${latest.sha}`);
    if (!list.ok) throw new Error(`GitHub answered ${list.status}`);
    const items = ((await list.json()) as { name: string; type: string; download_url: string | null }[]).filter(
      (f) => f.type === "file" && f.name.endsWith(".xml") && f.download_url,
    );
    const files = await Promise.all(
      items.map(async (f): Promise<[string, string]> => {
        const r = await fetch(f.download_url!);
        if (!r.ok) throw new Error(`${f.name}: ${r.status}`);
        return [f.name, await r.text()];
      }),
    );
    const lenses = await lensDbSave(files, sha, date);
    return `Lens profiles updated from Lensfun: ${lenses.toLocaleString()} lenses.`;
  } catch (e) {
    // Offline or GitHub busy: the profiles in use stay; a weekly check says nothing.
    return force ? `Couldn't check for lens profile updates: ${e instanceof Error ? e.message : e}` : null;
  }
}
