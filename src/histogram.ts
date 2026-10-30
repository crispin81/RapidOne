// Histogram of the photo shown behind the curve. It's the *input* histogram
// (before edits), as in Photoshop's Curves: points are placed against the
// tones the image actually has.

export interface Histogram {
  luma: number[];
  r: number[];
  g: number[];
  b: number[];
}

export const HISTOGRAM_BINS = 64;

/** Longest edge the preview is shrunk to before counting - plenty for a 64-bin histogram. */
const SAMPLE_DIM = 256;

export async function computeHistogram(src: string): Promise<Histogram> {
  const img = new Image();
  img.src = src;
  // Fully decoded first: drawn straight from onload while a slider was moving, WebKitGTK sometimes
  // drew nothing yet, and the histogram came out all black.
  await img.decode();
  const scale = Math.min(1, SAMPLE_DIM / Math.max(img.naturalWidth, img.naturalHeight));
  const w = Math.max(1, Math.round(img.naturalWidth * scale));
  const h = Math.max(1, Math.round(img.naturalHeight * scale));
  const canvas = document.createElement("canvas");
  canvas.width = w;
  canvas.height = h;
  // Kept in ordinary memory (it's read straight back), not on the graphics card.
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  if (!ctx) throw new Error("no 2D canvas");
  ctx.drawImage(img, 0, 0, w, h);
  const data = ctx.getImageData(0, 0, w, h).data;

  const hist: Histogram = {
    luma: new Array(HISTOGRAM_BINS).fill(0),
    r: new Array(HISTOGRAM_BINS).fill(0),
    g: new Array(HISTOGRAM_BINS).fill(0),
    b: new Array(HISTOGRAM_BINS).fill(0),
  };
  const bin = (v: number) => Math.min(HISTOGRAM_BINS - 1, Math.floor((v / 256) * HISTOGRAM_BINS));
  let counted = 0;
  for (let i = 0; i < data.length; i += 4) {
    // A JPEG has no transparent pixels: one is a part the canvas didn't get drawn.
    if (data[i + 3] === 0) continue;
    const r = data[i];
    const g = data[i + 1];
    const b = data[i + 2];
    hist.r[bin(r)]++;
    hist.g[bin(g)]++;
    hist.b[bin(b)]++;
    hist.luma[bin(0.2126 * r + 0.7152 * g + 0.0722 * b)]++;
    counted++;
  }
  // Nothing drawn: no histogram rather than an all-black one (the last one stays up).
  if (counted === 0) throw new Error("preview didn't draw for the histogram");
  return hist;
}
