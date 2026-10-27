import { Building2, Palette, Ruler, SlidersHorizontal, Sparkles } from "lucide-react";
import type { PhotoLook } from "../sidecar";
import DenoiseIcon from "./DenoiseIcon";

/**
 * The top right of a thumbnail, each icon in a black circle: "edited" (sliders, cyan) on any photo
 * with an edit, as Lightroom marks one; then (neon orange), once its full picture is rendered with its
 * current edit, the sparkle (the Teach / apply look icon) when its look came from a style or a Lightroom
 * XMP, or the palette (Presets' icon) from a preset (`look`: which), and under it Auto level's ruler and Auto
 * keystone's building when those were applied, and Denoise's icon when it has Noise on, so a batch going
 * through a folder shows where it's got.
 */
export default function ReadyBadges({
  edited,
  ready,
  level,
  denoised = false,
  look,
}: {
  edited: boolean;
  ready: boolean;
  level: number;
  denoised?: boolean;
  look?: PhotoLook;
}) {
  if (!edited && !ready) return null;
  return (
    <span className="thumb__badges">
      {edited && (
        <span className="thumb__ready thumb__ready--edited" title="Edited">
          <SlidersHorizontal size={10} />
        </span>
      )}
      {ready && look && (
        <span className="thumb__ready" title={look.from === "preset" ? `Preset: ${look.name}` : look.from === "xmp" ? "Look from its Lightroom XMP" : `Style: ${look.name}`}>
          {look.from === "preset" ? <Palette size={10} /> : <Sparkles size={10} />}
        </span>
      )}
      {ready && level >= 1 && (
        <span className="thumb__ready" title="Levelled (Auto level)">
          <Ruler size={10} />
        </span>
      )}
      {ready && level >= 2 && (
        <span className="thumb__ready" title="Keystoned (Auto keystone)">
          <Building2 size={10} />
        </span>
      )}
      {ready && denoised && (
        <span className="thumb__ready" title="Denoised">
          <DenoiseIcon size={10} />
        </span>
      )}
    </span>
  );
}
