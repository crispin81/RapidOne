import type { PhotoLook } from "./sidecar";
export interface PhotoInfo {
  path: string;
  fileName: string;
  timestampMs: number;
  thumbBase64: string;
  /** -1 = rejected, 0 = none, 1-5 = stars. */
  rating: number;
  pick: boolean;
  /** Its saved picture is rendered with its current edit (the sparkle, after a reopen). */
  rendered?: boolean;
  /** It has an edit when the folder was read (the "edited" icon until its edit is loaded). */
  edited?: boolean;
  /** Where its look came from when the folder was read (a style, a preset, its XMP): the thumbnail's icon. */
  look?: PhotoLook;
  /** Its edit had Noise on when the folder was read: the thumbnail's denoise icon. */
  denoised?: boolean;
}

export interface ScanProgress {
  scan: number;
  total: number;
  done: number;
}

export interface FullPreview {
  path: string;
  imageBase64: string;
  width: number;
  height: number;
}

/** A preview from RapidOne's own RAW engine (or the embedded JPEG as a fallback). */
export interface ViewerImage extends FullPreview {
  source: "engine" | "camera";
  /**
   * A stand-in until the engine's picture with this photo's edit arrives:
   * the camera JPEG of a RAW, or the engine's unedited first develop. Shown
   * plain and muted, without the edit's quick approximations.
   */
  pending?: boolean;
  /** Exposure (stops) already baked in by the engine; 0 for the camera JPEG. */
  exposure?: number;
  camera?: string;
  /** A half-size render shown while a slider moves (the full one follows). */
  interactive?: boolean;
  /** Rendered without the crop (Masks, Patch and Erase show the whole photo): their veil goes over it. */
  uncropped?: boolean;
  /** Full-resolution pixels per pixel of this image (what 100% zoom is); 0 or missing if unknown. */
  fullScale?: number;
  decodeMs?: number;
  developMs?: number;
}
