import type { CSSProperties } from "react";
import { DEFAULT_MIXER, hueTrack, isBandChanged, MIXER_COLOURS, MIXER_SLIDERS, type BandShift, type Mixer, type MixerColour } from "../mixer";
import { adjustHint, pickHint } from "../panelKeys";
import { RingDot } from "./Hex";
import VSlider from "./VSlider";

interface Props {
  mixer: Mixer;
  showNames: boolean;
  onChange: (mixer: Mixer) => void;
  /** The picked band (held by App so its keys can change it). */
  band: MixerColour;
  onBand: (band: MixerColour) => void;
  /** The held key's target (mixerHue / mixerSat / mixerLum), shown active. */
  activeSliderId: string | null;
}

/**
 * Compact HSL mixer: a 2x4 grid of colour dots picks the band, and the three
 * sliders beside it adjust that band. A ring marks bands that have been
 * changed; double-click a dot to reset its band.
 */
export default function ColourMixer({ mixer, showNames, onChange, band, onBand: setBand, activeSliderId }: Props) {
  const current = mixer[band];

  function setShift(key: keyof BandShift, value: number) {
    onChange({ ...mixer, [band]: { ...current, [key]: value } });
  }

  return (
    <div className="mixer">
      <div className="mixer__dots">
        {MIXER_COLOURS.map((c) => (
          <button
            key={c.id}
            type="button"
            className={`mixer__dot${c.id === band ? " mixer__dot--on" : ""}${isBandChanged(mixer[c.id]) ? " mixer__dot--changed" : ""}`}
            style={{ "--dot": c.swatch } as CSSProperties}
            title={`${c.label} - double-click to reset\n${pickHint("Mixer", c.label[0])}`}
            aria-label={c.label}
            aria-pressed={c.id === band}
            onClick={() => setBand(c.id)}
            onDoubleClick={() => onChange({ ...mixer, [c.id]: DEFAULT_MIXER[c.id] })}
          >
            <RingDot size={18} gap={2} on={c.id === band} spot={isBandChanged(mixer[c.id])} />
          </button>
        ))}
      </div>
      {MIXER_SLIDERS.map((def) => (
        <VSlider
          key={def.id}
          def={def.id === "hue" ? { ...def, trackGradient: hueTrack(band) } : def}
          value={current[def.id as keyof BandShift]}
          neutral={0}
          active={activeSliderId === `mixer${def.label === "Hue" ? "Hue" : def.label === "Saturation" ? "Sat" : "Lum"}`}
          showName={showNames}
          onChange={(v) => setShift(def.id as keyof BandShift, v)}
          hint={adjustHint("Mixer", def.label[0])}
        />
      ))}
    </div>
  );
}
