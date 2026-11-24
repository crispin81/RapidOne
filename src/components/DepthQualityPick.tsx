import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { depthQuality, downloadHqDepth, setDepthQuality, type DepthQualityInfo } from "../api";

/** Whether High is offered: hidden for now (Chris, 22 Nov), so everything runs at Standard and a
 *  saved High is ignored (kept for if it comes back). */
const HIGH_OFFERED: boolean = false;

/** The saved Depth quality setting (Standard unless the user chose High). */
const KEY = "rapidone.depthQuality";

/** The setting as saved, without asking the engine yet. */
export function savedDepthQuality(): "standard" | "high" {
  if (!HIGH_OFFERED) return "standard";
  try {
    return localStorage.getItem(KEY) === "high" ? "high" : "standard";
  } catch {
    return "standard";
  }
}

/**
 * The saved setting, applied as the app starts: the engine makes each photo's depth at whichever
 * quality is set when it's first asked for it.
 */
export function applySavedDepthQuality() {
  const q = savedDepthQuality();
  void setDepthQuality(q).catch(() => undefined);
}

/** The download's progress as it arrives (`depth-download`). */
interface Progress {
  done: number;
  total: number;
  finished?: boolean;
}

/** A size in bytes as "412 MB". */
function mb(bytes: number): string {
  return `${Math.round(bytes / 1e6)} MB`;
}

/**
 * The Depth quality setting: **Standard** (MoGe-2's own metric depth, the default) or **High**
 * (with Depth Anything 3 Mono-Large's detail fitted to it). The big model is never bundled, so
 * choosing High without it on the computer offers its download (its size shown) and stays Standard
 * until it has finished; the app keeps the setting like its other settings.
 */
export default function DepthQualityPick() {
  return HIGH_OFFERED ? <QualityPick /> : null;
}

function QualityPick() {
  const [info, setInfo] = useState<DepthQualityInfo | null>(null);
  const [quality, setQuality] = useState<"standard" | "high">(savedDepthQuality);
  // Chosen High but the model isn't here: the download is offered until it is.
  const [offer, setOffer] = useState(false);
  const [progress, setProgress] = useState<Progress | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    depthQuality().then(setInfo, () => setInfo(null));
  }, []);
  useEffect(refresh, [refresh]);

  // The download's own progress (sent by the engine as it goes).
  useEffect(() => {
    const off = listen<Progress>("depth-download", (e) => setProgress(e.payload));
    return () => void off.then((f) => f());
  }, []);

  const choose = (next: "standard" | "high") => {
    if (next === "high" && info && !info.hqInstalled) {
      // Offered, not set: Standard stays until the model is here.
      setOffer(true);
      setError(null);
      return;
    }
    setOffer(false);
    setQuality(next);
    try {
      localStorage.setItem(KEY, next);
    } catch {
      /* not kept: the setting still applies until the app closes */
    }
    void setDepthQuality(next).catch(() => undefined);
  };

  const start = () => {
    setError(null);
    setProgress({ done: 0, total: 1 });
    downloadHqDepth()
      .then((ok) => {
        refresh();
        if (ok) {
          setOffer(false);
          choose("high");
        } else {
          setError("the download didn't finish");
        }
      })
      .catch((e) => setError(String(e)))
      .finally(() => setProgress(null));
  };

  const pct = progress && progress.total > 0 ? Math.min(100, Math.round((progress.done / progress.total) * 100)) : 0;
  return (
    <div className="depthquality" title={"Depth quality: Standard uses the scene's own model. High adds " + (info?.hqName ?? "the High quality model") + "'s detail, fitted to its distances (sharper edges, slower)"}>
      <span>Quality</span>
      <span className="depthquality__modes" role="radiogroup" aria-label="Depth quality">
        <button
          type="button"
          role="radio"
          aria-checked={quality === "standard"}
          className={quality === "standard" ? "is-on" : undefined}
          onClick={() => choose("standard")}
        >
          Standard
        </button>
        <button
          type="button"
          role="radio"
          aria-checked={quality === "high"}
          className={quality === "high" ? "is-on" : undefined}
          disabled={info ? !info.hqAvailable : false}
          title={info && !info.hqAvailable ? "This computer can't run the High quality model (ONNX Runtime isn't there)" : undefined}
          onClick={() => choose("high")}
        >
          High
        </button>
      </span>
      {offer && !progress && (
        <button type="button" className="depthquality__offer" onClick={start} title="Download it once; it stays on this computer">
          Download {info?.hqSize ?? ""}
        </button>
      )}
      {progress && (
        <span className="depthquality__progress" title="Downloading the High quality depth model">
          {pct}%{progress.total > 0 ? ` of ${mb(progress.total)}` : ""}
        </span>
      )}
      {error && <span className="depthquality__error">{error}</span>}
    </div>
  );
}
