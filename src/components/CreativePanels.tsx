import { lazy, Suspense, useEffect, useMemo, useRef, useState } from "react";
import { Pipette, Sun } from "lucide-react";
import {
  BOKEH_MODES,
  BOKEH_SHAPES,
  focusBand,
  lensLine,
  LIGHT_TYPES,
  intensityForWatts,
  isCelestial,
  lightScope,
  lightWatts,
  typeOf,
  wattsText,
  type Bokeh,
  type Light,
  type LightKind,
  type Mist,
  type Relight,
} from "../creative";
import { bokehLens, sceneInfo, type BokehLens, type SceneInfo } from "../api";
import { nudge, type SliderDef } from "../adjustments";
import { adjustHint } from "../panelKeys";
import { PanelTitle } from "./BottomPanel";
import VSlider from "./VSlider";
import VRange from "./VRange";
import DepthQualityPick from "./DepthQualityPick";
import type { ScopePlanes } from "./DepthScope";

const DepthScope = lazy(() => import("./DepthScope"));
const RelightScope = lazy(() => import("./RelightScope"));

/** A Creative slider: its letter (its first) is in creative.ts, double-click resets. */
function def(id: string, label: string, min: number, max: number, neutral: number, extra: Partial<SliderDef> = {}): SliderDef {
  return { id, label, code: "", modifier: "none", min, max, step: 1, coarseStep: 5, decimals: 0, neutral, ...extra };
}

const APERTURE = def("bokehAperture", "Blur", 0.95, 16, 1.8, { step: 0.05, coarseStep: 1 / 3, decimals: 2, code: "B", tooltip: "Blur: the f-number your lens opens up to (lower is blurrier); double-click: as shot" });
const AMOUNT = def("bokehAmount", "Blur", 0, 100, 0, { step: 1, coarseStep: 10, code: "B", tooltip: "Blur: how much the background (and foreground) blurs" });
const RANGE = def("bokehRange", "Sharp", 0, 100, 0, { step: 1, coarseStep: 10, code: "S", tooltip: "Sharp: how much depth around the focus stays sharp (0: just the lens's own depth of field)" });
const BLADES = def("bokehBlades", "Sides", 5, 11, 7, { step: 1, coarseStep: 1, code: "I", tooltip: "Sides: the aperture's blades, 5 to 11" });
const ROUNDING = def("bokehRounding", "Edge", 0, 100, 50, { step: 1, coarseStep: 10, tooltip: "Edge: the blades' rounding, 0 a sharp polygon, 100 round" });
const VINTAGE = def("bokehVintage", "Vintage", 0, 100, 0, { step: 1, coarseStep: 10, code: "V", tooltip: "Vintage: an older lens's character, the bokeh squeezed to cat's eyes toward the corners and a touch of colour fringing" });
const BOOST = def("bokehBoost", "Glow", 0, 100, 0, { step: 1, coarseStep: 10, code: "G", tooltip: "Glow: bright points bloom into brighter bokeh balls (skies are left alone)" });

const DENSITY = def("mistDensity", "Amount", 0, 100, 0, { step: 1, coarseStep: 10, code: "A", tooltip: "Amount: how thick the fog is; near the top it veils what's close to the camera too" });
const END = def("mistEnd", "Near", 0, 100, 100, { code: "N" });
const START = def("mistStart", "Far", 0, 100, 0, { code: "F" });
const MIST_TEMP = def("mistTemp", "Warm", -100, 100, -20, { step: 1, coarseStep: 10, code: "W", trackGradient: "linear-gradient(to top, #4a86d8, #b9c3cf, #e0a74a)", tooltip: "Warm: the fog takes the sky's own colour; this warms (up) or cools (down) it" });
const MIST_HEIGHT = def("mistHeight", "Ground", 0, 100, 0, { step: 1, coarseStep: 10, code: "G", tooltip: "Ground: how much the fog lies low, pooling on the ground (0: even everywhere)" });
const MIST_GLOW = def("mistGlow", "Glow", 0, 100, 0, { step: 1, coarseStep: 10, code: "L", tooltip: "Glow: sunlight glowing through the fog; Sun on the left sets where it comes from" });
const TEXTURE = def("mistTexture", "Patchy", 0, 100, 0, { step: 1, coarseStep: 10, code: "P", tooltip: "Patchy: drifting banks and wisps instead of an even fog" });

/** Power: the light's Intensity shown as flash power in watt-seconds, a stop per quarter of the
 *  track. It starts (and double-click resets) at the type's typical power. */
function powerDef(l: Light): SliderDef {
  const t = LIGHT_TYPES.find((x) => x.id === typeOf(l)) ?? LIGHT_TYPES[0];
  // The sun and moon have no watts: a brightness, a stop per quarter of the track (the air already
  // dims a low sun by itself).
  if (isCelestial(l))
    return def("lightIntensity", "Bright", 0, 100, t.intensity, {
      step: 1,
      coarseStep: 25,
      tooltip: `How bright the ${t.label.toLowerCase()} is: each quarter of the track is a stop. A low sun is dimmer by itself (more air to pass through).`,
    });
  return def("lightIntensity", "Power", 0, 100, t.intensity, {
    step: 1,
    coarseStep: 25,
    format: (v) => wattsText(lightWatts(l, v)),
    parse: (watts) => intensityForWatts(l, watts),
    tooltip: `Flash power in watt-seconds (Ws). A ${t.label.toLowerCase()} starts at a typical ${wattsText(lightWatts(l, t.intensity))} Ws; each quarter of the track is a stop (double the power). Double-click: back to ${wattsText(lightWatts(l, t.intensity))} Ws.`,
  });
}

const RL_KELVIN = def("lightKelvin", "Warm", 2000, 12000, 5600, { step: 100, coarseStep: 1000, decimals: 0, code: "K", scale: "mired", reverse: true, trackGradient: "linear-gradient(to top, #6f8bff, #e8e2d6, #ffb257)", tooltip: "The light's colour as a blackbody's: up the track is warmer (lower Kelvin), down is cooler, as the RAW panel's Kelvin. Neutral (5600 K) is a flash's own daylight." });
const RL_TINT = def("lightTint", "Tint", -100, 100, 0, { step: 1, coarseStep: 10, code: "T", trackGradient: "linear-gradient(to top, #5fae58, #c0c0c0, #c45cb8)", tooltip: "Green to magenta, as the RAW panel's Tint: up is magenta, down is green" });
const RL_SOFTNESS = def("lightSoftness", "Soft", 0, 100, 35, { step: 1, coarseStep: 10, tooltip: "How far the light wraps round edges (a large source), and how broad its sheen" });
const RL_SPREAD = def("lightSpread", "Spread", 5, 100, 50, { step: 1, coarseStep: 10, tooltip: "How wide the light falls: 100 lights the whole scene; lower, a patch breaking through the clouds, centred on the subject (its edge softens with the patch)." });
const RL_FEATHER = def("lightFeather", "Feather", 0, 100, 30, { step: 1, coarseStep: 10, tooltip: "Feathers the edges: of the light's patch (with Spread under 100) and of its shadows. Low is a clear sky's crisp edge; high, soft through haze or thin cloud." });
const RL_SIZE = def("lightSize", "Size", 0, 100, 30, { step: 1, coarseStep: 10, tooltip: "The source's size: how far it reaches in metres, and how soft its shadows are" });
const RL_AMBIENT = def("relightAmbient", "Ambient", -100, 100, 0, { step: 1, coarseStep: 10, tooltip: "The photo's own light, as a camera's shutter speed sets it: down a stop per 33, so your lights stand out. The first light you add sets it a stop down; after that it's yours." });

/** The sun's place: a small 2D pad over -0.5..1.5 in each direction (the photo's frame is the
 *  middle square), so the glow can come from off frame. Dragging the dot moves it. */
function SunPad({ u, v, onChange }: { u: number; v: number; onChange: (u: number, v: number) => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const drag = useRef(false);
  const put = (e: { clientX: number; clientY: number }) => {
    const r = ref.current?.getBoundingClientRect();
    if (!r || r.width < 1 || r.height < 1) return;
    const x = Math.min(1, Math.max(0, (e.clientX - r.left) / r.width));
    const y = Math.min(1, Math.max(0, (e.clientY - r.top) / r.height));
    onChange(-0.5 + x * 2, -0.5 + y * 2);
  };
  const dotX = (Math.min(1.5, Math.max(-0.5, u)) + 0.5) / 2;
  const dotY = (Math.min(1.5, Math.max(-0.5, v)) + 0.5) / 2;
  return (
    <div
      ref={ref}
      className="creative-sunpad"
      title="The sun's place: the inner square is the photo, the outer margin off frame. Dragging moves the glow."
      onPointerDown={(e) => {
        e.stopPropagation();
        drag.current = true;
        (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
        put(e);
      }}
      onPointerMove={(e) => {
        if (drag.current) put(e);
      }}
      onPointerUp={(e) => {
        drag.current = false;
        (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
      }}
      onPointerCancel={() => (drag.current = false)}
      onDoubleClick={(e) => {
        e.stopPropagation();
        onChange(0.5, 0.2);
      }}
    >
      <span className="creative-sunpad__frame" />
      <span className="creative-sunpad__dot" style={{ left: `${dotX * 100}%`, top: `${dotY * 100}%` }} />
    </div>
  );
}

interface Props {
  path: string | null;
  bokeh: Bokeh;
  mist: Mist;
  relight: Relight;
  onChange: (patch: Partial<Bokeh>, key?: string | null) => void;
  onMist: (patch: Partial<Mist>, key?: string | null) => void;
  onRelight: (patch: Partial<Relight>, key?: string | null) => void;
  onLight: (id: number, patch: Partial<Light>, key?: string | null) => void;
  onAddLight: (kind: LightKind, typeId?: string, place?: Partial<Light>) => void;
  onRemoveLight: (id: number) => void;
  /** The light selected: its sliders show, and Place moves it. */
  selectedLight: number | null;
  onSelectLight: (id: number | null) => void;
  /** The Depth panel's status: the depth map is being found. */
  findingDepth: boolean;
  showDepth: boolean;
  onShowDepth: () => void;
  pickFocus: boolean;
  onPickFocus: () => void;
  pickMistStart: boolean;
  onPickMistStart: () => void;
  pickSun: boolean;
  onPickSun: () => void;
  /** The scopes: docked in the panel, or floating over the photo. */
  scopeDocked: boolean;
  onScopeDocked: (docked: boolean) => void;
  /** Relight's own 3D space: docked in its panel, or floating over the photo. */
  relightScopeDocked: boolean;
  onRelightScopeDocked: (docked: boolean) => void;
  /** Creative is showing: only then is the scope (a heavy 3D view, always on there) mounted. */
  scopeMounted: boolean;
  /** The Bokeh focus in the depth map (control 0..1 and metres), from App. */
  focusControl: number | null;
  focusMetres: number | null;
  /** The f-number's own sharp zone (control depth), from App. */
  zone: { near: number; far: number } | null;
  /** The scope's planes moved: near/far resize the range, focus moves the focus point. */
  onPlanes: (planes: ScopePlanes, key: string | null) => void;
  /** A point clicked in the scope: put the focus there. */
  onScopePick: (control: number) => void;
  /** The panel selected (lit, the others dimmed) by a click in it; its title again deselects. */
  litPanel: string | null;
  onLitPanel: (name: string | null) => void;
  /** Right-click on a panel: its menu (Reset), as the RAW panels. */
  onGroupMenu: (name: string, x: number, y: number) => void;
  /** The slider whose letter is held ("creative:aperture"), and where its keys and wheel go. */
  heldId: string | null;
  adjustRef: React.RefObject<((id: string, steps: number, coarse: boolean, reset: boolean) => void) | null>;
  /** A fresh scope (the Depth panel reset). */
  scopeKey: number;
  /** The Names tick box, as the RAW panels. */
  showNames: boolean;
  /** The photo's white balance as set (Kelvin), for a new light's colour. */
  photoKelvin?: number;
}

/** The Creative tab's panels: Depth (status, quality, the docked Depth Scope, Show depth) and
 *  Bokeh (lens-aware blur, the lens line, focus and lens character). */
export default function CreativePanels({
  path,
  bokeh,
  mist,
  relight,
  onChange,
  onMist,
  onRelight,
  onLight,
  onAddLight,
  onRemoveLight,
  selectedLight,
  onSelectLight,
  findingDepth,
  showDepth,
  onShowDepth,
  pickFocus,
  onPickFocus,
  pickMistStart,
  onPickMistStart,
  pickSun,
  onPickSun,
  scopeDocked,
  onScopeDocked,
  relightScopeDocked,
  onRelightScopeDocked,
  scopeMounted,
  focusControl,
  focusMetres,
  zone,
  onPlanes,
  onScopePick,
  litPanel,
  onLitPanel,
  onGroupMenu,
  heldId,
  adjustRef,
  scopeKey,
  showNames,
  photoKelvin,
}: Props) {
  const [lens, setLens] = useState<BokehLens | null>(null);
  const [info, setInfo] = useState<SceneInfo | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    setLens(null);
    setInfo(null);
    setProblem(null);
    if (!path) return;
    bokehLens(path)
      .then((l) => alive && setLens(l))
      .catch(() => alive && setLens(null));
    sceneInfo(path)
      .then((i) => alive && setInfo(i))
      .catch(() => alive && setInfo(null));
    return () => {
      alive = false;
    };
  }, [path]);

  // Where the focus sits in the depth map, for the scope's focus plane (and its metres): the
  // subject's median when Auto is on, else the picked point (App works it out).
  const focusLabel = useMemo(() => {
    if (bokeh.focusAuto) return focusMetres !== null ? `Auto (subject) · ${focusMetres < 10 ? focusMetres.toFixed(1) : Math.round(focusMetres)} m` : "Auto (subject)";
    return focusMetres !== null ? `${focusMetres < 10 ? focusMetres.toFixed(1) : Math.round(focusMetres)} m` : "from your click";
  }, [bokeh.focusAuto, focusMetres]);

  const planes: ScopePlanes = {
    ...focusBand(focusControl ?? 0.5, zone, bokeh.focusRange),
    focus: focusControl ?? undefined,
    fog: mist.enabled ? mist.start / 100 : undefined,
    fogFar: mist.enabled && mist.far > 0 ? mist.far / 100 : undefined,
    fogHeight: Math.max(0, Math.min(1, mist.height / 100)),
    fadeNear: 0,
    fadeFar: 0,
  };
  const lensText = lensLine(lens);
  // The shot's own lens (the engine's stand-ins without EXIF): where the sliders sit, adding nothing, until moved.
  const shotN = lens?.fNumber ?? 4;
  // As the RAW panels: captured, as the scope keeps its own clicks.
  const group = (name: string) => ({
    className: `panel__group panel__group--creative${litPanel === name ? " panel__group--lit" : ""}`,
    "data-group": name,
    onPointerDownCapture: (e: React.PointerEvent) => onLitPanel((e.target as HTMLElement).closest(".panel__title") && litPanel === name ? null : name),
    onContextMenu: (e: React.MouseEvent) => {
      e.preventDefault();
      onGroupMenu(name, e.clientX, e.clientY);
    },
  });

  const set = (patch: Partial<Bokeh>, key: string | null = null) => onChange(patch, key);

  // Every slider in the two panels, for drawing and for its held letter. `id` is creative.ts's held id.
  interface Row {
    id: string;
    def: SliderDef;
    value: number;
    put: (v: number) => void;
  }
  const row = (key: string, def: SliderDef, value: number, put: (v: number) => void): Row => ({ id: `creative:${key}`, def, value, put });
  const bokehRow = (key: keyof Bokeh & string, def: SliderDef) => row(key, def, bokeh[key] as number, (v) => set({ [key]: v }, `creative:${key}`));
  const mistRow = (key: keyof Mist & string, def: SliderDef) => row(key, def, mist[key] as number, (v) => onMist({ [key]: v }, `mist:${key}`));
  // Relight: the selected light's sliders (their ids carry the light, so a drag stays one step).
  const light = relight.lights.find((l) => l.id === selectedLight) ?? null;
  const lightRow = (l: Light, key: keyof Light & string, def: SliderDef) => ({
    id: `creative:light:${l.id}:${key}`,
    def,
    value: l[key] as number,
    put: (v: number) => onLight(l.id, { [key]: v }, `relight:${l.id}:${key}`),
  });
  // One row: the flash's power, then the light's character and the tool's Ambient. Where it
  // stands is set in the 3D space.
  // The sun and moon swap Soft and Size for their shadows' Edge and the Spread of their light.
  const relightRows: Row[] = light
    ? isCelestial(light)
      ? [
          // The sky's own: how wide it falls and its shadows' edge first, then its strength and colour.
          lightRow(light, "cone", RL_SPREAD),
          // Feather: the patch's edge and the shadows' edge as one (the shadows a little crisper).
          {
            id: `creative:light:${light.id}:feather`,
            def: RL_FEATHER,
            value: light.feather,
            put: (v: number) => onLight(light.id, { feather: v, shadowSoft: Math.round(v * 0.6) }, `relight:${light.id}:feather`),
          },
          lightRow(light, "intensity", powerDef(light)),
          lightRow(light, "kelvin", RL_KELVIN),
          lightRow(light, "tint", RL_TINT),
        ]
      : [
          lightRow(light, "intensity", powerDef(light)),
          lightRow(light, "kelvin", RL_KELVIN),
          lightRow(light, "tint", RL_TINT),
          lightRow(light, "softness", RL_SOFTNESS),
          lightRow(light, "size", RL_SIZE),
        ]
    : [];
  const ambientRow: Row = { id: "creative:relight:ambient", def: RL_AMBIENT, value: relight.ambient, put: (v) => onRelight({ ambient: v }, "relight:ambient") };
  const bokehRows: Row[] = [
    bokeh.mode !== "manual"
      ? row("aperture", { ...APERTURE, neutral: shotN }, bokeh.aperture || shotN, (v) => set({ aperture: v === shotN ? 0 : Math.min(16, Math.max(0.95, Math.round(v * 100) / 100)) }, "creative:aperture"))
      : bokehRow("amount", AMOUNT),
    bokehRow("focusRange", RANGE),
    ...(bokeh.shape === "blades" ? [bokehRow("blades", BLADES), bokehRow("bladeRounding", ROUNDING)] : []),
    bokehRow("boost", BOOST),
    // Vintage: cat's eye and fringing together, the way an older lens has both.
    row("catsEye", VINTAGE, bokeh.catsEye, (v) => set({ catsEye: v, fringe: Math.round(v * 0.6) }, "creative:catsEye")),
  ];
  const densityRow = mistRow("density", DENSITY);
  const mistRows: Row[] = [
    // Fewer, plainer sliders (24 Nov): Soft, Tint, the ground fog's fall-off and the ground's level
    // stay as saved but are set for you (a gentle fade, the sky's colour, a natural fall-off, the
    // ground as found). Soft barely showed once Near reaches the camera.
    mistRow("temperature", MIST_TEMP),
    mistRow("height", MIST_HEIGHT),
    mistRow("glow", MIST_GLOW),
    mistRow("texture", TEXTURE),
  ];
  // The fog's two points never pass each other.
  const startRow = row("far", START, mist.far, (v) => onMist({ far: Math.min(v, mist.start) }, "mist:far"));
  const endRow = row("start", END, mist.start, (v) => onMist({ start: Math.max(v, mist.far) }, "mist:start"));

  // A held letter's arrows, wheel and Backspace (App's adjustHeld). Carries on from its own last
  // value while they come quickly, as re-renders lag behind.
  const going = useRef({ id: "", value: 0, at: 0 });
  useEffect(() => {
    adjustRef.current = (id, steps, coarse, reset) => {
      const r = [...bokehRows, densityRow, startRow, endRow, ...mistRows, ...relightRows, ambientRow].find((x) => x.id === id);
      if (!r) return;
      const now = performance.now();
      let v = going.current.id === id && now - going.current.at < 300 ? going.current.value : r.value;
      if (reset) v = r.def.neutral;
      else for (let i = 0; i < Math.abs(steps); i++) v = nudge(r.def, v, steps > 0 ? 1 : -1, coarse);
      going.current = { id, value: v, at: now };
      r.put(v);
    };
  });

  const slider = (panel: string) => (r: Row) => (
    <VSlider key={r.id} def={r.def} value={r.value} neutral={r.def.neutral} active={heldId === r.id} showName={showNames} hint={adjustHint(panel, r.def.code || r.def.label[0])} onChange={r.put} />
  );

  return (
    <>
      <section {...group("Depth")}>
        <div className="panel__title-row">
          <PanelTitle name="Depth" />
        </div>
        <div className="creative-body">
          {(() => {
            // The panel's own buttons and its status, at the top of the left column.
            const showBtn = (
              <button type="button" className={showDepth ? "is-on" : undefined} onClick={onShowDepth} aria-pressed={showDepth} title="Show the depth picture over the photo (warm near, cool far, the focus band yellow)">
                Show
              </button>
            );
            const status = (
              <span className="creative-side__note creative-side__wide" title={info ? `${info.model} on ${info.ep}` : undefined}>
                {findingDepth ? (
                  <span className="creative-status__busy">Finding depth…</span>
                ) : lens?.dNear && lens.dFar ? (
                  `${lens.dNear < 10 ? lens.dNear.toFixed(1) : Math.round(lens.dNear)}–${lens.dFar < 10 ? lens.dFar.toFixed(1) : Math.round(lens.dFar)} m`
                ) : (
                  (problem ?? "No depth yet")
                )}
              </span>
            );
            if (scopeMounted && path && scopeDocked)
              return (
                <div className="creative-scope">
                  <Suspense fallback={<div className="creative-scope__note">Loading the 3D view…</div>}>
                    <DepthScope
                      key={scopeKey}
                      path={path}
                      planes={planes}
                      lights={lightScope(relight, selectedLight)}
                      onPlanesChange={(p, key) => {
                        // A fog-plane drag sets Mist's End; a focus-plane drag moves the focus point;
                        // the near/far planes resize Bokeh's focus range.
                        if (p.fog !== undefined && Math.abs(p.fog * 100 - mist.start) > 0.01) onMist({ start: Math.max(mist.far, p.fog * 100) }, key);
                        else if (p.focus !== undefined && focusControl !== null && Math.abs(p.focus - focusControl) > 1e-4) onScopePick(p.focus);
                        else onPlanes(p, key);
                      }}
                      onPick={(control) => onScopePick(control)}
                      onUndock={() => onScopeDocked(false)}
                      side={
                        <>
                          {showBtn}
                          {status}
                        </>
                      }
                      docked
                    />
                  </Suspense>
                </div>
              );
            return (
              <>
                <div className="creative-side">
                  {showBtn}
                  {!scopeDocked && (
                    <button type="button" className="btn-redock" onClick={() => onScopeDocked(true)} title="Put the Depth Scope back in this panel">
                      Redock
                    </button>
                  )}
                  {status}
                </div>
                <div className="creative-scope creative-scope--empty">
                  <span className="creative-scope__note">{!scopeDocked ? "The Depth Scope is floating over the photo" : path ? "Loading the 3D view…" : "Open a photo to see its depth"}</span>
                </div>
              </>
            );
          })()}
          <DepthQualityPick />
        </div>
      </section>

      <section {...group("Bokeh")}>
        <div className="panel__title-row">
          <PanelTitle name="Bokeh" />
        </div>
        <div className="creative-body">
          <div className="creative-side" role="radiogroup" aria-label="Bokeh mode">
            {BOKEH_MODES.map((m) => (
              <button
                key={m.id}
                type="button"
                role="radio"
                aria-checked={bokeh.mode === m.id}
                className={bokeh.mode === m.id ? "is-on" : undefined}
                title={`${m.title}\n${lensText}`}
                onClick={() => set({ mode: m.id }, null)}
              >
                {m.label}
              </button>
            ))}
            <button type="button" className={pickFocus ? "is-on" : undefined} onClick={onPickFocus} title={`Click-to-focus: click what should be in focus on the photo (focus: ${focusLabel})`} aria-pressed={pickFocus}>
              <Pipette size={11} /> Focus
            </button>
            <button type="button" className={bokeh.focusAuto ? "is-on" : undefined} onClick={() => set({ focusAuto: !bokeh.focusAuto }, null)} title="Focus on the subject (its matte's median distance) instead of a point" aria-pressed={bokeh.focusAuto}>
              Auto focus
            </button>
            <span className="creative-side__note" title={lensText}>
              {focusLabel}
            </span>
          </div>
          <div className="creative-side" role="radiogroup" aria-label="Aperture shape">
            {BOKEH_SHAPES.map((sh) => (
              <button
                key={sh.id}
                type="button"
                role="radio"
                aria-checked={bokeh.shape === sh.id}
                className={bokeh.shape === sh.id ? "is-on" : undefined}
                title={sh.title}
                onClick={() => set({ shape: sh.id }, null)}
              >
                {sh.label}
              </button>
            ))}
          </div>
          <div className="panel__sliders">{bokehRows.map(slider("Bokeh"))}</div>
        </div>
      </section>

      <section {...group("Mist")}>
        <div className="panel__title-row">
          <PanelTitle name="Mist" />
        </div>
        <div className="creative-body">
          <div className="creative-side" aria-label="Mist pickers">
            <button type="button" className={pickMistStart ? "is-on" : undefined} onClick={onPickMistStart} title="Pick how near the fog comes: click a point on the photo, and Near moves to its distance" aria-pressed={pickMistStart}>
              <Pipette size={11} /> Near
            </button>
            <button type="button" className={pickSun ? "is-on" : undefined} onClick={onPickSun} title="Place the sun: click (or just outside the photo) where the glow comes from" aria-pressed={pickSun}>
              <Sun size={11} /> Sun
            </button>
            <SunPad u={mist.glowU} v={mist.glowV} onChange={(u, v) => onMist({ glowU: u, glowV: v }, "mist:sun")} />
          </div>
        <div className="panel__sliders">
          {slider("Mist")(densityRow)}
          <VRange
            low={mist.far}
            high={mist.start}
            lowNeutral={START.neutral}
            highNeutral={END.neutral}
            lowLabel="Far"
            highLabel="Near"
            title={`Far (bottom): how far back the fog goes, 0 without a limit. Near (top): how close it comes; near the top it reaches the camera, lower it to leave the subject clear\n${adjustHint("Mist", "F (Far) or N (Near)")}`}
            active={heldId === startRow.id ? "low" : heldId === endRow.id ? "high" : null}
            showName={showNames}
            onLow={(v) => onMist({ far: v }, "mist:range")}
            onHigh={(v) => onMist({ start: v }, "mist:range")}
          />
          {mistRows.map(slider("Mist"))}
          </div>
        </div>
      </section>

      <section {...group("Relight")}>
        <div className="panel__title-row">
          <PanelTitle name="Relight" />
        </div>
        <div className="creative-relight">
          {relightScopeDocked ? (
            <div className="creative-scope creative-scope--relight">
              {path ? (
                <Suspense fallback={<div className="creative-scope__note">Loading the 3D space…</div>}>
                  <RelightScope
                    path={path}
                    lights={relight.lights}
                    selected={selectedLight}
                    onSelect={onSelectLight}
                    onMove={(id, patch) => onLight(id, patch, `relight:${id}:place`)}
                    onEdit={onLight}
                    onAdd={(typeId, place) => onAddLight("point", typeId, place)}
                    onRemove={onRemoveLight}
                    ambient={relight.ambient}
                    photoKelvin={photoKelvin}
                    onAmbient={(v, key) => onRelight({ ambient: v }, key)}
                    onUndock={() => onRelightScopeDocked(false)}
                  />
                </Suspense>
              ) : (
                <div className="creative-scope__note">Open a photo to see its lighting space</div>
              )}
            </div>
          ) : (
            <div className="creative-scope creative-scope--relight creative-scope--redock">
              <div className="creative-side">
                <button type="button" className="btn-redock" onClick={() => onRelightScopeDocked(true)} title="Put the lighting space back in this panel">
                  Redock
                </button>
              </div>
              <span className="creative-scope__note">The lighting space is floating over the photo</span>
            </div>
          )}
          <div className="creative-relight__controls">
            <div className="panel__sliders">{[ambientRow, ...relightRows].map(slider("Relight"))}</div>
          </div>
        </div>
      </section>
    </>
  );
}

