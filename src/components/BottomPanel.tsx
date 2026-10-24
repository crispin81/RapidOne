import type { ReactNode } from "react";
import { Building2, Ruler } from "lucide-react";
import { PANEL_ORDER, SLIDER_GROUPS, type Adjustments, type SliderGroup } from "../adjustments";
import type { Curves } from "../curves";
import type { Grade } from "../grade";
import type { Histogram } from "../histogram";
import type { Mixer } from "../mixer";
import CurvesPanel from "./CurvesPanel";
import CalibrationPanel from "./CalibrationPanel";
import type { PanelPicks } from "../panelKeys";
import ColourMixer from "./ColourMixer";
import GradeWheels from "./GradeWheels";
import VSlider from "./VSlider";
import { CircleDashed } from "lucide-react";
import type { SliderDef } from "../adjustments";


/**
 * Lightroom's name for each panel RapidOne calls something else, shown in
 * brackets on hover ("Definition (Presence)") so Lightroom users can map them.
 */
const LIGHTROOM_NAMES: Record<string, string> = {
  Definition: "Presence",
  Curves: "Tone Curve",
  Keystone: "Transform",
  Mixer: "Color Mixer",
  Grade: "Color Grading",
  "Fine tuning": "Calibration",
  Sharpen: "Detail",
};

function PanelTitle({ name }: { name: string }) {
  const lightroom = LIGHTROOM_NAMES[name];
  return (
    <h3 className="panel__title" title={`${lightroom ? `${name} (Lightroom: ${lightroom})` : name}\nClick, or Shift+${name[0]}, selects it; Esc, Shift+${name[0]} or a click on this title again leaves`}>
      {/* Gold, underlined first letter = the group's key. */}
      <span className="panel__key">{name[0]}</span>
      {name.slice(1)}
    </h3>
  );
}

const GROUPS_BY_NAME: Record<string, SliderGroup> = Object.fromEntries(SLIDER_GROUPS.map((g) => [g.name, g]));

/** The group a slider belongs to, for the lit-up group while its key is held. */
function groupOf(sliderId: string | null | undefined): string | null {
  if (!sliderId) return null;
  return SLIDER_GROUPS.find((g) => g.sliders.some((d) => d.id === sliderId))?.name ?? null;
}


interface Props {
  values: Adjustments;
  /** Per-photo neutral values (Temperature/Tint start at the as-shot white balance). */
  neutrals: Adjustments;
  activeSliderId: string | null;
  /** The panel selected by its letter key: lit up, the others dimmed. */
  selectedPanel: string | null;
  /** Select a panel by clicking it (null: deselect). */
  onSelectPanel: (name: string | null) => void;
  showNames: boolean;
  /** The tick boxes at the left of the W/B group: short slider names, and the D65 corridor lines and glow round the photo. */
  onToggleNames: () => void;
  surround: boolean;
  onToggleSurround: () => void;
  curves: Curves;
  histogram: Histogram | null;
  /** The photo's ISO and aperture ("ISO 3200 · f/2.8"), in Curves' corner. */
  shotLabel?: string | null;
  grade: Grade;
  mixer: Mixer;
  onChange: (id: string, value: number) => void;
  onCurvesChange: (curves: Curves) => void;
  onGradeChange: (grade: Grade) => void;
  onMixerChange: (mixer: Mixer) => void;
  /** Which auto transform is measuring right now, if any. */
  autoBusy: "level" | "keystone" | null;
  /** Which auto straighten this photo has (lit; clicking it again turns it off). */
  autoActive: "level" | "keystone" | null;
  onAutoLevel: () => void;
  onAutoKeystone: () => void;
  /** What's picked inside the Mixer, Grade, Fine tuning and Curves panels. */
  picks: PanelPicks;
  onPicks: (p: Partial<PanelPicks>) => void;
  /** A mask tool is open: the sliders masks have act on the picked mask (`values` holds its
   *  numbers for them), the others are greyed out; the tick boxes are Show mask and Invert. */
  /** A right-click on a group (its name): its menu (Reset its settings). */
  onGroupMenu?: (name: string, x: number, y: number) => void;
  mask?: {
    /** The slider as the mask uses it, or null (greyed out). */
    defOf: (def: SliderDef) => SliderDef | null;
    /** A mask is picked (otherwise every slider is greyed out). */
    picked: boolean;
    showMask: boolean;
    onShowMask: () => void;
    invert: boolean;
    onInvert: () => void;
  };
}

/**
 * The short Resolve-style panel: every group side by side, no scrolling,
 * in PANEL_ORDER. Curves also replaces a separate Levels tool.
 */
export default function BottomPanel({
  values,
  neutrals,
  activeSliderId,
  selectedPanel,
  onSelectPanel,
  showNames,
  onToggleNames,
  surround,
  onToggleSurround,
  curves,
  histogram,
  shotLabel,
  grade,
  mixer,
  onChange,
  onCurvesChange,
  onGradeChange,
  onMixerChange,
  autoBusy,
  autoActive,
  onAutoLevel,
  onAutoKeystone,
  picks,
  onPicks,
  mask,
  onGroupMenu,
}: Props) {


  const special: Record<string, ReactNode> = {
    Curves: (
      <CurvesPanel
        curves={curves}
        histogram={histogram}
        shotLabel={shotLabel}
        onChange={onCurvesChange}
        values={values}
        neutrals={neutrals}
        activeSliderId={activeSliderId}
        showNames={showNames}
        onSliderChange={onChange}
        tab={picks.curvesTab}
        onTab={(curvesTab) => onPicks({ curvesTab })}
      />
    ),
    Mixer: (
      <ColourMixer
        mixer={mixer}
        showNames={showNames}
        onChange={onMixerChange}
        band={picks.mixerBand}
        onBand={(mixerBand) => onPicks({ mixerBand })}
        activeSliderId={activeSliderId}
      />
    ),
    Grade: (
      <GradeWheels
        grade={grade}
        showNames={showNames}
        onChange={onGradeChange}
        range={picks.gradeRange}
        onRange={(gradeRange) => onPicks({ gradeRange })}
        activeSliderId={activeSliderId}
      />
    ),
  };

  const lit = selectedPanel ?? groupOf(activeSliderId);
  // A click anywhere in a panel selects it, as Shift + its letter does; a
  // click on the title of the selected panel deselects it.
  const pick = (name: string, e: React.PointerEvent) => {
    const onTitle = (e.target as HTMLElement).closest(".panel__title");
    onSelectPanel(onTitle && selectedPanel === name ? null : name);
  };
  // With a mask tool open, a group with none of the masks' sliders is greyed out whole.
  const maskHas = (name: string) =>
    Boolean(mask?.picked) && Boolean(GROUPS_BY_NAME[name]?.sliders.some((d) => mask?.defOf(d)));
  const groupClass = (name: string) =>
    `panel__group${lit === name ? " panel__group--lit" : ""}${mask && !maskHas(name) ? " panel__group--off" : ""}`;

  return (
    <>
      {/* The tick boxes, in a slim row on the loupe's bottom edge just above the panel, so the panel has its width for sliders.
          With a mask tool open, hidden (Show mask and each layer's Invert are in the mask layers panel): hidden, not
          removed, as a fresh element flickers in WebKitGTK's drawing. */}
      {(
        <div className="panel__options panel__options--float" style={mask ? { visibility: "hidden" } : undefined} aria-hidden={mask ? true : undefined}>
          <label className="check" title="Show short names under the sliders (off: two letters, narrower)">
            <input type="checkbox" checked={showNames} onChange={onToggleNames} aria-label="Names" />
            {<span>Names</span>}
          </label>
          <label className="check" title="Neutral D65 corridor lines and halo round the photo (off: plain dark surround)">
            <input type="checkbox" checked={surround} onChange={onToggleSurround} aria-label="D65 Glow" />
            {<span>D65 Glow</span>}
          </label>
        </div>
      )}
    <div className={`panel${lit ? " panel--focus" : ""}`}>
      {PANEL_ORDER.map((name) => {
        if (special[name]) {
          return (
            <section key={name} className={groupClass(name)} data-group={name}
              onPointerDown={(e) => pick(name, e)}
              onContextMenu={(e) => {
                e.preventDefault();
                onGroupMenu?.(name, e.clientX, e.clientY);
              }}
            >
              <PanelTitle name={name} />
              {special[name]}
            </section>
          );
        }
        const group = GROUPS_BY_NAME[name];
        return (
          <section key={name} className={groupClass(name)} data-group={name}
              onPointerDown={(e) => pick(name, e)}
              onContextMenu={(e) => {
                e.preventDefault();
                onGroupMenu?.(name, e.clientX, e.clientY);
              }}
            >
            {group.name === "Keystone" ? (
              // Small icon-only auto level / auto keystone buttons share the heading row so the sliders
              // below keep the same height as every other group.
              <div className="panel__title-row">
                <PanelTitle name={group.name} />
                <button
                  type="button"
                  className={`mini-btn${autoBusy === "level" ? " mini-btn--busy" : ""}${autoActive === "level" ? " mini-btn--on" : ""}`}
                  onClick={onAutoLevel}
                  disabled={autoBusy !== null}
                  title={autoActive === "level" ? "Auto level is on: click to turn it off" : "Auto level: straighten the horizon or verticals"}
                  aria-label="Auto level"
                  aria-pressed={autoActive === "level"}
                >
                  <Ruler size={12} />
                </button>
                <button
                  type="button"
                  className={`mini-btn${autoBusy === "keystone" ? " mini-btn--busy" : ""}${autoActive === "keystone" ? " mini-btn--on" : ""}`}
                  onClick={onAutoKeystone}
                  disabled={autoBusy !== null}
                  title={autoActive === "keystone" ? "Auto keystone is on: click to turn it off" : "Auto keystone: straighten converging verticals (also levels)"}
                  aria-label="Auto keystone"
                  aria-pressed={autoActive === "keystone"}
                >
                  <Building2 size={12} />
                </button>
              </div>
            ) : group.name === "Effects" ? (
              // The legacy (Lightroom) vignette: its switch beside the title puts its sliders in the group
              // in place of the adaptive vignette's (lit while the photo has a legacy vignette).
              <div className="panel__title-row">
                <PanelTitle name={group.name} />
                <button
                  type="button"
                  className={`mini-btn${picks.legacyVignette ? " mini-btn--on" : ""}${(values.vignette ?? 0) !== 0 ? " mini-btn--used" : ""}`}
                  onClick={(e) => {
                    e.stopPropagation();
                    onPicks({ legacyVignette: !picks.legacyVignette });
                  }}
                  title={picks.legacyVignette ? "Back to the adaptive vignette's slider" : "Legacy vignette (Lightroom's): its Amount, Midpoint, Roundness, Feather and Highlights here"}
                  aria-label="Legacy vignette"
                  aria-pressed={Boolean(picks.legacyVignette)}
                >
                  <CircleDashed size={12} />
                </button>
              </div>
            ) : (
              <PanelTitle name={group.name} />
            )}
            <div className="panel__sliders">
              {group.name === "Fine tuning" ? (
                <CalibrationPanel
                  values={values}
                  activeSliderId={activeSliderId}
                  showNames={showNames}
                  onChange={onChange}
                  primary={picks.calPrimary}
                  onPrimary={(calPrimary) => onPicks({ calPrimary })}
                />
              ) : (
                (group.name === "Effects" && picks.legacyVignette
                  ? [...GROUPS_BY_NAME["Legacy vignette"].sliders, ...group.sliders.filter((d) => d.id !== "adaptiveVignette")]
                  : group.sliders
                ).map((def) => {
                  // A mask tool open: the mask's version of the slider, or greyed out.
                  const md = mask ? (mask.picked ? mask.defOf(def) : null) : def;
                  return md ? (
                    <VSlider
                      key={def.id}
                      def={md}
                      value={values[def.id]}
                      neutral={mask ? 0 : neutrals[def.id]}
                      active={activeSliderId === def.id}
                      showName={showNames}
                      onChange={(v) => onChange(def.id, v)}
                    />
                  ) : (
                    <div key={def.id} className="vslider-off" aria-disabled="true">
                      <VSlider def={def} value={neutrals[def.id]} neutral={neutrals[def.id]} active={false} showName={showNames} onChange={() => {}} />
                    </div>
                  );
                })
              )}
            </div>
          </section>
        );
      })}
    </div>
    </>
  );
}
