// Exposure for the rough preview, done the way the real engine will: in
// linear light, with a soft highlight shoulder instead of a hard clip.
//
// The preview image is gamma-encoded sRGB. Multiplying those values directly
// (what CSS brightness() does) is roughly 2.2x too strong - "+1" behaved like
// +2 stops - and everything past white clipped instantly. So: decode to
// linear, multiply by 2^stops, ease highlights into white, re-encode.

function srgbToLinear(v: number): number {
  return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
}

function linearToSrgb(v: number): number {
  return v <= 0.0031308 ? 12.92 * v : 1.055 * v ** (1 / 2.4) - 0.055;
}

/** Where the shoulder starts (linear). Below this, exposure is an exact gain. */
const KNEE = 0.75;

/**
 * Builds the highlight shoulder for a given brightest input (`peak`, the old
 * white after gain). It's the identity below the knee, then an exponential
 * ease that meets the knee with slope 1 (no visible kink) and lands exactly
 * on white at `peak` - so the brightest part of the photo just reaches white
 * instead of clipping, and nothing gets dimmed.
 */
function makeShoulder(peak: number): (x: number) => number {
  const room = 1 - KNEE;
  const span = peak - KNEE;
  if (span <= room) return (x) => Math.min(x, 1); // nothing goes past white
  // Find the ease's rate `a` so that g(t) = room * (1 - e^(-t/a)) / (1 - e^(-span/a))
  // starts with slope 1: room / (a * (1 - e^(-span/a))) = 1. Bisection is plenty.
  let lo = 1e-4;
  let hi = 1e4;
  for (let i = 0; i < 60; i++) {
    const a = Math.sqrt(lo * hi);
    const slope = room / (a * (1 - Math.exp(-span / a)));
    if (slope > 1) lo = a;
    else hi = a;
  }
  const a = Math.sqrt(lo * hi);
  const norm = 1 - Math.exp(-span / a);
  return (x) => (x <= KNEE ? x : KNEE + (room * (1 - Math.exp(-(Math.min(x, peak) - KNEE) / a))) / norm);
}

/** Per-channel exposure as a function of a gamma-encoded value (0..1). */
export function exposureFunction(stops: number): (v: number) => number {
  if (stops === 0) return (v) => v;
  const gain = 2 ** stops;
  const shoulder = makeShoulder(gain);
  return (v) => linearToSrgb(shoulder(srgbToLinear(v) * gain));
}
