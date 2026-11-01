import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { X } from "lucide-react";
import { openExternal } from "../openExternal";

const LATEST_URL = "https://api.github.com/repos/crispin81/RapidOne/releases/latest";
/** The last answer and when it came, and the version whose chip was closed (localStorage). */
const CHECK_KEY = "rapidone.updateCheck";
const HIDDEN_KEY = "rapidone.updateHidden";
const DAY_MS = 24 * 60 * 60 * 1000;
/** After the app has started, so it costs the start nothing. */
const DELAY_MS = 5000;

type Release = { at: number; tag: string; url: string };

function stored<T>(key: string): T | null {
  try {
    return JSON.parse(localStorage.getItem(key) ?? "null") as T | null;
  } catch {
    return null;
  }
}

function store(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Not fatal: it's checked again next start.
  }
}

/** The latest release: the last answer if it's under a day old, else GitHub's; null if neither (offline, rate limit). */
async function latestRelease(): Promise<Release | null> {
  const kept = stored<Release>(CHECK_KEY);
  if (kept && Date.now() - kept.at < DAY_MS) return kept;
  try {
    const res = await fetch(LATEST_URL, { headers: { Accept: "application/vnd.github+json" } });
    if (!res.ok) return kept;
    const r = (await res.json()) as { tag_name?: string; html_url?: string };
    if (!r.tag_name || !r.html_url?.startsWith("https://github.com/")) return kept;
    const fresh = { at: Date.now(), tag: r.tag_name, url: r.html_url };
    store(CHECK_KEY, fresh);
    return fresh;
  } catch {
    return kept;
  }
}

/** "Update available" on the home screen: only when a newer release is out, and quietly nothing otherwise. */
export default function UpdateChip() {
  const [update, setUpdate] = useState<Release | null>(null);
  useEffect(() => {
    let live = true;
    const timer = window.setTimeout(async () => {
      try {
        const release = await latestRelease();
        if (!release || stored<string>(HIDDEN_KEY) === release.tag) return;
        const current = await getVersion();
        if (live && (await invoke<boolean>("version_is_newer", { latest: release.tag, current }))) setUpdate(release);
      } catch {
        // Nothing to say.
      }
    }, DELAY_MS);
    return () => {
      live = false;
      window.clearTimeout(timer);
    };
  }, []);
  if (!update) return null;
  return (
    <div className="update-chip">
      <button type="button" className="update-chip__open" onClick={() => openExternal(update.url)} title="Open the release page">
        Update available: {update.tag}
      </button>
      <button
        type="button"
        className="update-chip__close"
        aria-label="Hide for this version"
        title="Hide for this version"
        onClick={() => {
          store(HIDDEN_KEY, update.tag);
          setUpdate(null);
        }}
      >
        <X size={12} />
      </button>
    </div>
  );
}
