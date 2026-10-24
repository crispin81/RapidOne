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

export function computeHistogram(src: string): Promise<Histogram> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => {
      const scale = Math.min(1, SAMPLE_DIM / Math.max(img.naturalWidth, img.naturalHeight));
      const w = Math.max(1, Math.round(img.naturalWidth * scale));
      const h = Math.max(1, Math.round(img.naturalHeight * scale));
      const canvas = document.createElement("canvas");
      canvas.width = w;
      canvas.height = h;
      const ctx = canvas.getContext("2d");
      if (!ctx) return reject(new Error("no 2D canvas"));
      ctx.drawImage(img, 0, 0, w, h);
      const data = ctx.getImageData(0, 0, w, h).data;

      const hist: Histogram = {
        luma: new Array(HISTOGRAM_BINS).fill(0),
        r: new Array(HISTOGRAM_BINS).fill(0),
        g: new Array(HISTOGRAM_BINS).fill(0),
        b: new Array(HISTOGRAM_BINS).fill(0),
      };
      const bin = (v: number) => Math.min(HISTOGRAM_BINS - 1, Math.floor((v / 256) * HISTOGRAM_BINS));
      for (let i = 0; i < data.length; i += 4) {
        const r = data[i];
        const g = data[i + 1];
        const b = data[i + 2];
        hist.r[bin(r)]++;
        hist.g[bin(g)]++;
        hist.b[bin(b)]++;
        hist.luma[bin(0.2126 * r + 0.7152 * g + 0.0722 * b)]++;
      }
      resolve(hist);
    };
    img.onerror = () => reject(new Error("couldn't load preview for histogram"));
    img.src = src;
  });
}
