import { useEffect, useRef, useState } from "react";
import { Aperture, Search } from "lucide-react";
import { lensDbVersion, lensInfo, lensSearch, type LensInfo } from "../api";
import { checkLensUpdates } from "../lensUpdates";

interface Props {
  /** The open photo (none: the button is off). */
  path: string | null;
  /** Every photo corrected by its own lens (unless switched off itself). */
  auto: boolean;
  /** The open photo is corrected. */
  on: boolean;
  /** Bumped when a lens is chosen or profiles change: the panel reads the photo's lens again. */
  rev: number;
  onAuto: (on: boolean) => void;
  onPhoto: (on: boolean) => void;
  /** A lens chosen for every photo whose EXIF names `exif` (0: back to the one found). */
  onChoose: (exif: string, lensId: number) => void;
  onLoadProfiles: () => void;
}

const Toggle = ({ on }: { on: boolean }) => (
  <span className={`toggle${on ? " toggle--on" : ""}`} aria-hidden="true">
    <span className="toggle__knob" />
  </span>
);

/**
 * Lens corrections (Lensfun's profiles: distortion, colour fringing, vignetting). One switch
 * corrects every photo by its own lens, so mixed shoots just work; the open photo can be
 * switched off by itself. A lens the database doesn't recognise (or gets wrong) is chosen by
 * typing its name: it goes on every photo with the same lens name. Own profiles load as
 * Lensfun .xml files.
 */
export default function LensMenu({ path, auto, on, rev, onAuto, onPhoto, onChoose, onLoadProfiles }: Props) {
  const [open, setOpen] = useState(false);
  const [info, setInfo] = useState<LensInfo | null>(null);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<[number, string][]>([]);
  const [db, setDb] = useState<{ date: string; lenses: number } | null>(null);
  const [checking, setChecking] = useState<string | null>(null);
  const ref = useRef<HTMLDivElement>(null);

  // The photo's lens, read for each photo (not only with the panel open): the button is lit
  // only when the photo is corrected and its lens has a profile.
  useEffect(() => {
    if (!path) return;
    let stale = false;
    setInfo(null);
    lensInfo(path)
      .then(([i]) => !stale && setInfo(i))
      .catch(() => !stale && setInfo(null));
    return () => {
      stale = true;
    };
  }, [path, rev]);
  useEffect(() => {
    if (!open) return;
    lensDbVersion()
      .then(([, date, lenses]) => setDb({ date, lenses }))
      .catch(() => setDb(null));
  }, [open, rev, checking]);
  useEffect(() => {
    if (open) setQuery("");
  }, [open]);
  useEffect(() => {
    const q = query.trim();
    if (!q) {
      setHits([]);
      return;
    }
    let stale = false;
    const t = window.setTimeout(() => {
      lensSearch(q)
        .then((h) => !stale && setHits(h.slice(0, 8)))
        .catch(() => undefined);
    }, 120);
    return () => {
      stale = true;
      window.clearTimeout(t);
    };
  }, [query]);
  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("pointerdown", onDown);
    return () => window.removeEventListener("pointerdown", onDown);
  }, [open]);

  const exif = info?.exif ?? "";
  const choose = (id: number) => {
    if (!exif) return;
    onChoose(exif, id);
    setQuery("");
  };
  // This photo's lens, in words.
  const lensLine = !info
    ? "Reading the photo's lens…"
    : info.chosen
      ? info.chosen[1]
      : info.detected
        ? info.detected[1]
        : exif
          ? exif
          : "No lens recorded in this photo";
  // A lens the database has no profile for.
  const missing = Boolean(info && !info.chosen && !info.detected && exif);
  // Corrected for real: switched on, and a profile for its lens.
  const available = Boolean(info?.chosen || info?.detected);
  const corrected = on && available;
  const lensNote = info?.chosen ? `chosen for every "${exif}" photo` : info?.detected ? "found from the photo" : exif ? "type its name below to choose it" : "";

  return (
    <div className="topbar__menu lensmenu" ref={ref}>
      <button
        type="button"
        className={corrected ? "is-on" : undefined}
        disabled={!path}
        onClick={() => setOpen((o) => !o)}
        title="Lens corrections: distortion, colour fringing and vignetting, from each photo's lens"
        aria-label="Lens corrections"
      >
        <Aperture size={14} /> <span className="topbar__label">Lens</span>
      </button>
      {open && path && (
        <div className="lensmenu__pop" onKeyDown={(e) => e.stopPropagation()}>
          <label className="lensmenu__on" title="Every photo corrected by its own lens (each can still be switched off below)">
            <Toggle on={auto} />
            <input
              type="checkbox"
              checked={auto}
              onChange={() => onAuto(!auto)}
            />
            Correct lenses automatically
          </label>
          <div className="lensmenu__photo">
            <div className="lensmenu__lens">
              <span>
                {lensLine}
                {missing && <span className="lensmenu__missing">: not available</span>}
              </span>
              {lensNote && <span className="lensmenu__note">{lensNote}</span>}
            </div>
            <label
              className={`lensmenu__on lensmenu__on--small${available ? "" : " is-disabled"}`}
              title={available ? "This photo corrected, or not" : "No profile for this lens: choose one by its name below"}
            >
              <Toggle on={corrected} />
              <input type="checkbox" checked={corrected} disabled={!available} onChange={() => onPhoto(!on)} />
              This photo
            </label>
          </div>
          {exif && (
            <>
              <label className="lensmenu__search">
                <Search size={13} />
                <input
                  value={query}
                  placeholder={info?.detected || info?.chosen ? "Not right? Type the lens's name" : "Type the lens's name"}
                  onChange={(e) => setQuery(e.currentTarget.value)}
                  onKeyDown={(e) => {
                    e.stopPropagation();
                    if (e.key === "Enter" && hits[0]) choose(hits[0][0]);
                    else if (e.key === "Escape") {
                      if (query) setQuery("");
                      else setOpen(false);
                    }
                  }}
                />
                {info?.chosen && (
                  <button type="button" onClick={() => choose(0)} title="Forget the choice: back to the lens found from the photo">
                    Auto
                  </button>
                )}
              </label>
              {hits.length > 0 && (
                <div className="lensmenu__hits">
                  <p className="lensmenu__note">Goes on every photo taken with "{exif}"</p>
                  {hits.map(([id, name]) => (
                    <button key={id} type="button" className={id === info?.chosen?.[0] ? "is-current" : undefined} onClick={() => choose(id)}>
                      {name}
                    </button>
                  ))}
                </div>
              )}
            </>
          )}
          <div className="lensmenu__db">
            <span>{db ? `Lensfun profiles, ${new Date(db.date).toLocaleDateString()} (${db.lenses.toLocaleString()} lenses)` : "Lensfun profiles"}</span>
            <button
              type="button"
              disabled={checking === "…"}
              onClick={() => {
                setChecking("…");
                void checkLensUpdates(true).then((m) => setChecking(m ?? ""));
              }}
            >
              {checking === "…" ? "Checking…" : "Check now"}
            </button>
          </div>
          {checking && checking !== "…" && <p className="lensmenu__note">{checking}</p>}
          <button type="button" className="lensmenu__load" onClick={onLoadProfiles} title="Add your own lens profiles, in Lensfun's format (.xml)">
            Load lens profile (.xml)…
          </button>
        </div>
      )}
    </div>
  );
}
