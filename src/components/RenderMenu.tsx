import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { renderStatus, setRenderMode, type RenderMode, type RenderStatus } from "../api";

/** The saved Render setting (Auto unless the user chose otherwise). */
const RENDER_MODE_KEY = "rapidone.renderMode";

const MODES: { mode: RenderMode; label: string; help: string }[] = [
  { mode: "auto", label: "Auto", help: "The graphics card when it can, else the processor" },
  { mode: "gpu", label: "GPU", help: "The graphics card; when it can't, the processor (and it says why here)" },
  { mode: "cpu", label: "CPU", help: "The processor only" },
];

/** The saved setting, applied as the app starts (the engine starts on Auto). */
export function applySavedRenderMode() {
  let saved: string | null = null;
  try {
    saved = localStorage.getItem(RENDER_MODE_KEY);
  } catch {
    saved = null;
  }
  if (saved === "gpu" || saved === "cpu") void setRenderMode(saved).catch(() => undefined);
}

/**
 * The Render setting, in the title bar: Auto (the GPU if usable, else the CPU), GPU, or CPU. It
 * switches without a restart; the menu shows the graphics card found, and why the last render
 * stayed on the processor when it did.
 */
/** Where Retouch's own engine runs (its `rapidretouch:device` event): an NVIDIA or Apple GPU, or the CPU. */
interface RetouchDevice {
  device: string;
  gpu: boolean;
}

/** `retouch`: the Retouch tab is open, so the light shows Retouch's engine (Edit's renders are idle there). */
export default function RenderMenu({ retouch = false }: { retouch?: boolean }) {
  const [open, setOpen] = useState(false);
  const [status, setStatus] = useState<RenderStatus | null>(null);
  const box = useRef<HTMLDivElement>(null);
  const refresh = useCallback(() => {
    renderStatus().then(setStatus, () => setStatus(null));
  }, []);
  useEffect(refresh, [refresh]);
  // Renders moving between the GPU and the CPU (sent only on a change, so no cost while dragging).
  useEffect(() => {
    const off = listen<RenderStatus>("render-status", (e) => setStatus(e.payload));
    // Read again now it's listening: a change before then (the GPU's warm-up at start) sent nothing here.
    void off.then(refresh);
    return () => void off.then((f) => f());
  }, [refresh]);
  // Retouch's engine, once it's up (it starts the first time its tab opens).
  const [retouchDevice, setRetouchDevice] = useState<RetouchDevice | null>(
    () => (window as unknown as { rapidretouchDevice?: RetouchDevice }).rapidretouchDevice ?? null,
  );
  useEffect(() => {
    const on = (e: Event) => setRetouchDevice((e as CustomEvent<RetouchDevice>).detail);
    window.addEventListener("rapidretouch:device", on);
    return () => window.removeEventListener("rapidretouch:device", on);
  }, []);
  useEffect(() => {
    if (!open) return;
    refresh();
    const away = (e: PointerEvent) => {
      if (box.current && !box.current.contains(e.target as Node)) setOpen(false);
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("pointerdown", away);
    window.addEventListener("keydown", key);
    return () => {
      window.removeEventListener("pointerdown", away);
      window.removeEventListener("keydown", key);
    };
  }, [open, refresh]);
  const choose = (mode: RenderMode) => {
    try {
      localStorage.setItem(RENDER_MODE_KEY, mode);
    } catch {
      /* not kept: the setting still applies until the app closes */
    }
    setRenderMode(mode).then(setStatus, () => undefined);
  };
  const mode = status?.mode ?? "auto";
  // Where the last render ran: orange in a cyan rim on the graphics card (its fire), plain cyan on the processor.
  // In Retouch, its engine's device instead (No until the engine is up).
  const onGpu = retouch ? Boolean(retouchDevice?.gpu) : mode !== "cpu" && Boolean(status?.onGpu);
  const tip = retouch
    ? "Where Retouch runs: " +
      (retouchDevice ? (retouchDevice.gpu ? `the graphics card (${retouchDevice.device})` : "the processor") : "its engine is starting")
    : "Where photos are rendered: the graphics card (GPU) or the processor (CPU). Now: " +
      (onGpu ? `the graphics card${status?.adapter ? ` (${status.adapter})` : ""}` : "the processor");
  return (
    <div className="render-menu" ref={box}>
      <button
        type="button"
        className="titlebar__about"
        onClick={() => setOpen((o) => !o)}
        title={tip}
        aria-haspopup="menu"
        aria-expanded={open}
      >
        <span className={"render-menu__light" + (onGpu ? " render-menu__light--gpu" : "")} aria-hidden="true" />
        {/* Whether the last render ran on the graphics card; the menu shows the setting (Auto / GPU / CPU). */}
        GPU: {onGpu ? "Yes" : "No"}
      </button>
      {open && (
        <div className="render-menu__pop" role="menu">
          {MODES.map((m) => (
            <button
              key={m.mode}
              type="button"
              role="menuitemradio"
              aria-checked={mode === m.mode}
              className={"render-menu__item" + (mode === m.mode ? " render-menu__item--on" : "")}
              onClick={() => choose(m.mode)}
              title={m.help}
            >
              {m.label}
              <span className="render-menu__help">{m.help}</span>
            </button>
          ))}
          {retouch && (
            <div className="render-menu__status">
              {/* The setting above is Edit's; Retouch picks its own (an NVIDIA or Apple GPU, else the processor). */}
              Retouch: {retouchDevice ? (retouchDevice.gpu ? retouchDevice.device : "the processor (no NVIDIA or Apple GPU)") : "starting"}
            </div>
          )}
          <div className="render-menu__status">
            {status?.adapter ? <>Graphics card: {status.adapter}</> : mode === "cpu" ? <>Rendering on the processor</> : <>No graphics card found yet</>}
            {mode !== "cpu" && status?.note && <div className="render-menu__note">{status.note}</div>}
          </div>
        </div>
      )}
    </div>
  );
}
