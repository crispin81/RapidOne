import { useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import type { ExportFormat, ExportNaming, ExportPresets, ExportRow, ExportSize } from "rapidretouch/ui";
import { exportPhoto, exportWorkers, exportPlan, type PlannedExport, type RenderSettings } from "./api";
import type { PhotoInfo } from "./types";

const baseName = (path: string) => path.split(/[\\/]/).pop() ?? path;

/** A saved set of export settings (Chris, 24 Oct): everything but which photos. */
interface ExportPreset {
  name: string;
  format: ExportFormat;
  size: ExportSize;
  folder: string | null;
  naming: ExportNaming;
}

const PRESETS_KEY = "rapidone.exportPresets";

function loadPresets(): ExportPreset[] {
  try {
    const list = JSON.parse(localStorage.getItem(PRESETS_KEY) ?? "[]");
    return Array.isArray(list) ? list.filter((p) => typeof p?.name === "string" && p.format && p.size && p.naming) : [];
  } catch {
    return [];
  }
}

function storePresets(list: ExportPreset[]) {
  try {
    localStorage.setItem(PRESETS_KEY, JSON.stringify(list));
  } catch {
    // Not kept (storage off): they last until the app closes.
  }
}

/** The thumbnail as the dialog wants it: base64 without the data-URL prefix. */
const rawThumb = (photo?: PhotoInfo) => photo?.thumbBase64.replace(/^data:image\/\w+;base64,/, "");

/**
 * Export (the dialog is RapidRetouch's, so the options are the same). Photos
 * are written one after another in capture-time order, each with its own
 * saved edit (`ensureEdit` reads a photo's .rpd or Lightroom edit first if it
 * hasn't been opened this session), rendered by RapidOne's engine.
 */
export function useExport({
  photoOf,
  ensureEdit,
  settingsFor,
  onFinished,
  startFolder,
}: {
  photoOf: (path: string) => PhotoInfo | undefined;
  /** Makes sure the photo's saved edit is loaded (resolves when it is). */
  ensureEdit: (path: string) => Promise<void>;
  /** The engine settings for the photo's loaded edit. */
  settingsFor: (path: string) => RenderSettings;
  onFinished: (message: string) => void;
  /** Where the folder chooser starts. */
  startFolder: string | null;
}) {
  // The photos the dialog can export: the selected ones (or the open one), or all those shown;
  // which of the two is chosen in the dialog (and remembered for the next time it opens).
  const [choice, setChoice] = useState<{ selected: string[]; all: string[] } | null>(null);
  const [scope, setScope] = useState<"selected" | "all">("selected");
  const paths = choice ? (scope === "all" ? choice.all : choice.selected) : null;
  const [format, setFormat] = useState<ExportFormat>("jpeg");
  const [size, setSize] = useState<ExportSize>({ mode: "full", px: 2048 });
  const [folder, setFolder] = useState<string | null>(null);
  const [naming, setNaming] = useState<ExportNaming>({ mode: "original" });
  const [plan, setPlan] = useState<PlannedExport[]>([]);
  const [rows, setRows] = useState<ExportRow[]>([]);
  const [running, setRunning] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const cancel = useRef(false);
  const started = rows.some((r) => r.state !== "waiting");

  // Export presets: the settings (not the photos), kept between sessions.
  const [presets, setPresets] = useState<ExportPreset[]>(loadPresets);
  const same = (p: ExportPreset) =>
    JSON.stringify([p.format, p.size, p.folder, p.naming]) === JSON.stringify([format, size, folder, naming]);
  const presetMenu: ExportPresets = {
    names: presets.map((p) => p.name),
    current: presets.find(same)?.name ?? null,
    onPick: (name) => {
      const p = presets.find((x) => x.name === name);
      if (!p) return;
      setFormat(p.format);
      setSize(p.size);
      setFolder(p.folder);
      setNaming(p.naming);
    },
    onSave: (name) => {
      const next = [...presets.filter((p) => p.name !== name), { name, format, size, folder, naming }].sort((a, b) => a.name.localeCompare(b.name));
      setPresets(next);
      storePresets(next);
    },
    onDelete: (name) => {
      const next = presets.filter((p) => p.name !== name);
      setPresets(next);
      storePresets(next);
    },
  };

  // The plan follows the options until the export starts: names, order, and
  // which files are already there.
  useEffect(() => {
    if (!paths || started) return;
    let stale = false;
    const sources = paths.map((path) => ({ path, timestampMs: photoOf(path)?.timestampMs ?? 0 }));
    exportPlan(sources, naming, folder, format)
      .then((p) => {
        if (stale) return;
        setPlan(p);
        setRows(p.map((x) => ({ path: x.path, thumb: rawThumb(photoOf(x.path)), state: "waiting", fraction: 0 })));
      })
      .catch((e) => {
        if (!stale) onFinished(`Couldn't plan the export: ${e}`);
      });
    return () => {
      stale = true;
    };
  }, [paths, started, naming, folder, format, photoOf, onFinished]);

  const openFor = useCallback((photos: { selected: string[]; all: string[] }) => {
    if (photos.selected.length === 0 && photos.all.length === 0) return;
    setRows([]);
    setPlan([]);
    setCancelling(false);
    setChoice(photos);
    // Nothing selected to export: start on all of them.
    if (photos.selected.length === 0) setScope("all");
  }, []);

  const close = useCallback(() => {
    setChoice(null);
    setRows([]);
    setPlan([]);
  }, []);

  const chooseFolder = useCallback(async () => {
    const dir = await open({ directory: true, defaultPath: folder ?? startFolder ?? undefined });
    if (typeof dir === "string") setFolder(dir.replace(/\\/g, "/"));
  }, [folder, startFolder]);

  const run = useCallback(async () => {
    const queue = plan;
    // The rows' progress, gathered and shown once a second, not on every step of every photo: a long
    // export redrew the dialog all the time, and twice WebKitGTK's drawing locked up partway through
    // an unattended export (24 Oct). Trying fewer redraws.
    const pendingRows = new Map<string, Partial<ExportRow>>();
    const flushRows = () => {
      if (pendingRows.size === 0) return;
      const patches = new Map(pendingRows);
      pendingRows.clear();
      setRows((rs) => rs.map((r) => (patches.has(r.path) ? { ...r, ...patches.get(r.path) } : r)));
    };
    const flushTimer = window.setInterval(flushRows, 1000);
    // Each photo's stages as the engine reaches them (export.rs, "export-progress"): reading the RAW,
    // rendering, saving. Never over a photo already finished (an event can arrive after its result).
    const finished = new Set<string>();
    const unlisten = await listen<{ path: string; fraction: number; message: string }>("export-progress", (e) => {
      if (!finished.has(e.payload.path)) update(e.payload.path, { fraction: e.payload.fraction, message: e.payload.message });
    }).catch(() => null);
    const update = (path: string, patch: Partial<ExportRow>) => pendingRows.set(path, { ...pendingRows.get(path), ...patch });
    cancel.current = false;
    setCancelling(false);
    setRunning(true);
    const longEdge = format === "jpeg" && size.mode === "long" ? size.px : null;
    let done = 0;
    let failed = 0;
    // Several photos at once, as many as this computer's cores and free memory suit (export.rs).
    const workers = await exportWorkers().catch(() => 1);
    let next = 0;
    const work = async () => {
      while (next < queue.length) {
        const item = queue[next++];
        if (cancel.current) {
          update(item.path, { state: "cancelled" });
          continue;
        }
        update(item.path, { state: "exporting", fraction: 0.05, message: "Reading its edit…" });
        try {
          await ensureEdit(item.path);
          update(item.path, { fraction: 0.15, message: "Reading the RAW…" });
          const written = await exportPhoto(item.path, settingsFor(item.path), item.out, format, longEdge);
          finished.add(item.path);
          update(item.path, { state: "done", fraction: 1, message: written });
          done++;
        } catch (e) {
          finished.add(item.path);
          update(item.path, { state: "failed", message: String(e) });
          failed++;
        }
      }
    };
    await Promise.all(Array.from({ length: Math.min(workers, queue.length) }, work));
    unlisten?.();
    window.clearInterval(flushTimer);
    flushRows();
    setRunning(false);
    setCancelling(false);
    onFinished(
      cancel.current
        ? `Export stopped: ${done} exported${failed ? `, ${failed} failed` : ""}.`
        : `${done} photo${done === 1 ? "" : "s"} exported${failed ? `, ${failed} failed` : ""}.`,
    );
  }, [plan, format, size, ensureEdit, settingsFor, onFinished]);

  const first = plan[0] ? baseName(plan[0].out) : null;
  const savedAs = plan.length
    ? `Saved as ${first}${plan.length > 1 ? `, … ${baseName(plan[plan.length - 1].out)}` : ""}, in the order the photos were taken.`
    : undefined;

  return {
    openFor,
    /** The props for RapidRetouch's ExportDialog, or null while it is closed. */
    dialog:
      choice && rows.length > 0
        ? {
            presets: presetMenu,
            scope: {
              value: scope,
              selectedLabel: choice.selected.length > 1 ? `Selected (${choice.selected.length})` : "Selected",
              allLabel: `All shown (${choice.all.length})`,
              onChange: setScope,
            },
            rows,
            format,
            size,
            folder,
            running,
            cancelling,
            naming,
            savedAs,
            clashes: plan.filter((p) => p.exists).length,
            onNaming: setNaming,
            onFormat: setFormat,
            onSize: setSize,
            onChooseFolder: () => void chooseFolder(),
            onSameFolder: () => setFolder(null),
            onStart: () => void run(),
            onCancel: () => {
              cancel.current = true;
              setCancelling(true);
            },
            onClose: close,
          }
        : null,
  };
}
