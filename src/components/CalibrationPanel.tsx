import type { CSSProperties } from "react";
import { ALL_SLIDERS, DEFAULTS, type Adjustments, type SliderDef } from "../adjustments";
import { adjustHint, pickHint } from "../panelKeys";
import { RingDot } from "./Hex";
import VSlider from "./VSlider";

interface Props {
  values: Adjustments;
  activeSliderId: string | null;
  showNames: boolean;
  onChange: (id: string, value: number) => void;
  /** The picked primary (held by App so its keys can change it). */
  primary: Primary;
  onPrimary: (p: Primary) => void;
}

import type { Primary } from "../panelKeys";

const PRIMARIES: { name: Primary; swatch: string; hueTrack: string }[] = [
  // Hue tracks as Lightroom's Calibration shows them: lowered at the bottom, raised at the top.
  { name: "Red", swatch: "#e0474a", hueTrack: "linear-gradient(to top, #d94bb3, #e0474a, #ec8a3a)" },
  { name: "Green", swatch: "#4fbf5a", hueTrack: "linear-gradient(to top, #e6cf3e, #4fbf5a, #3fc4c4)" },
  { name: "Blue", swatch: "#3d6fe0", hueTrack: "linear-gradient(to top, #3fc4c4, #3d6fe0, #8a52d6)" },
];

const byId = (id: string) => ALL_SLIDERS.find((d) => d.id === id)!;

/**
 * Calibration in the same compact style as the colour mixer: Shadow tint,
 * then a dot per primary (red / green / blue) and Hue / Saturation sliders
 * for whichever primary is selected. Double-click a dot to reset it.
 */
export default function CalibrationPanel({ values, activeSliderId, showNames, onChange, primary, onPrimary: setPrimary }: Props) {
  const shadowTint = byId("calShadowTint");
  // Same underlying sliders (cal<Primary>Hue / Sat), shown with generic names.
  const pair: SliderDef[] = [
    { ...byId(`cal${primary}Hue`), label: "Hue", trackGradient: PRIMARIES.find((p) => p.name === primary)?.hueTrack },
    { ...byId(`cal${primary}Sat`), label: "Saturation" },
  ];
  const changed = (p: Primary) =>
    values[`cal${p}Hue`] !== DEFAULTS[`cal${p}Hue`] || values[`cal${p}Sat`] !== DEFAULTS[`cal${p}Sat`];

  const slider = (def: SliderDef) => (
    <VSlider
      key={def.id}
      def={def}
      value={values[def.id]}
      neutral={DEFAULTS[def.id]}
      active={activeSliderId === def.id}
      showName={showNames}
      onChange={(v) => onChange(def.id, v)}
      hint={def.id === "calShadowTint" ? undefined : adjustHint("Fine tuning", def.label[0])}
    />
  );

  return (
    <div className="mixer">
      {slider(shadowTint)}
      <div className="mixer__dots mixer__dots--column">
        {PRIMARIES.map((p) => (
          <button
            key={p.name}
            type="button"
            className={`mixer__dot${p.name === primary ? " mixer__dot--on" : ""}${changed(p.name) ? " mixer__dot--changed" : ""}`}
            style={{ "--dot": p.swatch } as CSSProperties}
            title={`${p.name} primary - double-click to reset\n${pickHint("Fine tuning", p.name[0])}`}
            aria-label={`${p.name} primary`}
            aria-pressed={p.name === primary}
            onClick={() => setPrimary(p.name)}
            onDoubleClick={() => {
              onChange(`cal${p.name}Hue`, DEFAULTS[`cal${p.name}Hue`]);
              onChange(`cal${p.name}Sat`, DEFAULTS[`cal${p.name}Sat`]);
            }}
          >
            <RingDot size={18} gap={2} on={p.name === primary} spot={changed(p.name)} />
          </button>
        ))}
      </div>
      {pair.map(slider)}
    </div>
  );
}
