import { useEffect, useState, type ReactNode } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { Coffee, Globe, X } from "lucide-react";
import { openExternal } from "../openExternal";
import aboutPhoto from "../assets/about.jpg";

export const COFFEE_URL = "https://buymeacoffee.com/chriscorkphotography";
const WEBSITE_URL = "https://www.chriscorkphotography.co.uk";
const INSTAGRAM_URL = "https://www.instagram.com/chriscorkphotography";
const FACEBOOK_URL = "https://www.facebook.com/chriscorkphoto";
const AGPL_URL = "https://www.gnu.org/licenses/agpl-3.0.html";

/** One thing the app uses: its name, licence, what it does here, and where it's from. */
type Item = { name: string; licence: string; does: string; url: string };
/** A part of the app (Cull, Edit, Retouch, or the shell around them), by category. */
type Part = { title: string; intro: string; groups: { heading: string; items: Item[] }[] };

const PARTS: Part[] = [
  {
    title: "Cull",
    intro: "Sorting a shoot: previews, stars and picks, and photos with closed eyes flagged.",
    groups: [
      {
        heading: "AI models",
        items: [
          { name: "YuNet", licence: "MIT", does: "Finds the faces in each photo", url: "https://github.com/opencv/opencv_zoo/tree/main/models/face_detection_yunet" },
          { name: "MediaPipe Face Landmarker", licence: "Apache-2.0", does: "Places the points round each face's eyes", url: "https://ai.google.dev/edge/mediapipe/solutions/vision/face_landmarker" },
          { name: "MediaPipe face blendshapes", licence: "Apache-2.0", does: "Tells a closed eye from a half-blink", url: "https://ai.google.dev/edge/mediapipe/solutions/vision/face_landmarker" },
        ],
      },
      {
        heading: "Libraries",
        items: [
          { name: "tract", licence: "MIT / Apache-2.0", does: "Runs the face models on the CPU", url: "https://github.com/sonos/tract" },
          { name: "rawler", licence: "LGPL-2.1", does: "Reads the previews inside RAW files", url: "https://github.com/dnglab/dnglab" },
          { name: "image", licence: "MIT / Apache-2.0", does: "Decodes and resizes previews", url: "https://github.com/image-rs/image" },
          { name: "kamadak-exif", licence: "BSD-2-Clause", does: "Reads each photo's camera details", url: "https://github.com/kamadak/exif-rs" },
          { name: "sysinfo", licence: "MIT", does: "Lists the drives to browse", url: "https://github.com/GuillaumeGomez/sysinfo" },
        ],
      },
    ],
  },
  {
    title: "Edit",
    intro: "RapidOne's own RAW engine: colour, tone, masks, crop, noise reduction and object removal.",
    groups: [
      {
        heading: "AI models",
        items: [
          { name: "BiRefNet (lite)", licence: "MIT", does: "The Subject mask, the subject-aware vignette, and keeping people out of removals", url: "https://github.com/ZhengPeng7/BiRefNet" },
          { name: "SCUNet", licence: "Apache-2.0", does: "AI noise reduction (and the small model trained from it)", url: "https://github.com/cszn/SCUNet" },
          { name: "LaMa", licence: "Apache-2.0", does: "Fills in what Remove Lite takes away", url: "https://github.com/advimman/lama" },
          { name: "MobileSAM", licence: "MIT", does: "Outlines the object in a box drawn for Remove", url: "https://github.com/ChaoningZhang/MobileSAM" },
          { name: "Grounding DINO (tiny)", licence: "Apache-2.0", does: "Finds objects by name for Remove (\"sign\", \"bin\")", url: "https://github.com/IDEA-Research/GroundingDINO" },
        ],
      },
      {
        heading: "Libraries",
        items: [
          { name: "Lensfun database", licence: "CC BY-SA 3.0", does: "Lens profiles: distortion, colour fringing and vignetting for about 1,500 lenses", url: "https://lensfun.github.io" },
          { name: "rawler", licence: "LGPL-2.1", does: "Decodes the RAW files", url: "https://github.com/dnglab/dnglab" },
          { name: "ONNX Runtime", licence: "MIT", does: "Runs the AI models (DirectML on Windows graphics cards)", url: "https://onnxruntime.ai" },
          { name: "ort", licence: "MIT / Apache-2.0", does: "ONNX Runtime from Rust", url: "https://github.com/pykeio/ort" },
          { name: "image, tiff", licence: "MIT / Apache-2.0", does: "JPEG, PNG and TIFF in and out", url: "https://github.com/image-rs/image" },
          { name: "kamadak-exif", licence: "BSD-2-Clause", does: "Camera details and orientation", url: "https://github.com/kamadak/exif-rs" },
          { name: "rayon", licence: "MIT / Apache-2.0", does: "Uses every CPU core for the engine", url: "https://github.com/rayon-rs/rayon" },
          { name: "roxmltree", licence: "MIT / Apache-2.0", does: "Reads the lens database", url: "https://github.com/RazrFalcon/roxmltree" },
          { name: "flate2", licence: "MIT / Apache-2.0", does: "Reads the compressed tables in camera profiles", url: "https://github.com/rust-lang/flate2-rs" },
          { name: "serde", licence: "MIT / Apache-2.0", does: "Saves edits and settings", url: "https://serde.rs" },
        ],
      },
    ],
  },
  {
    title: "Retouch",
    intro: "Portrait retouching: skin, eyes, teeth, wrinkles, body, backdrop, glasses reflections and Patch.",
    groups: [
      {
        heading: "AI models",
        items: [
          { name: "BiRefNet HR", licence: "MIT", does: "Finds the subject, for backdrop smoothing and the Neck, Body and Clothes tools", url: "https://huggingface.co/ZhengPeng7/BiRefNet_HR" },
          { name: "MediaPipe Face Landmarker", licence: "Apache-2.0", does: "Finds faces, eyes and lips for the Skin, Eyes, Mouth and Dodge & Burn tools", url: "https://ai.google.dev/edge/mediapipe/solutions/vision/face_landmarker" },
          { name: "MediaPipe Multi-class Selfie Segmentation", licence: "Apache-2.0", does: "Tells skin from clothing, for the Neck, Body and Clothes tools", url: "https://ai.google.dev/edge/mediapipe/solutions/vision/image_segmenter" },
        ],
      },
      {
        heading: "Libraries",
        items: [
          { name: "PyTorch", licence: "BSD-3-Clause", does: "Runs the AI models (on the CPU; a Mac's own graphics)", url: "https://pytorch.org" },
          { name: "OpenCV", licence: "Apache-2.0", does: "The retouching itself: blurs, blends, warps", url: "https://opencv.org" },
          { name: "MediaPipe", licence: "Apache-2.0", does: "Runs the face and selfie models", url: "https://ai.google.dev/edge/mediapipe" },
          { name: "NumPy", licence: "BSD-3-Clause", does: "The maths on every pixel", url: "https://numpy.org" },
          { name: "LibRaw (rawpy)", licence: "LGPL-2.1 / CDDL (rawpy: MIT)", does: "Opens RAW files", url: "https://www.libraw.org" },
          { name: "kornia, timm, einops", licence: "Apache-2.0 / Apache-2.0 / MIT", does: "Parts of the subject model", url: "https://github.com/kornia/kornia" },
          { name: "Transformers, Hugging Face Hub", licence: "Apache-2.0", does: "Loads and downloads the models", url: "https://huggingface.co/docs/transformers" },
          { name: "Pillow, tifffile", licence: "MIT-CMU / BSD-3-Clause", does: "Reads and writes images, 16-bit TIFFs", url: "https://python-pillow.org" },
          { name: "uv", licence: "MIT / Apache-2.0", does: "Sets up the retouching engine on first run", url: "https://github.com/astral-sh/uv" },
        ],
      },
    ],
  },
  {
    title: "The app",
    intro: "The window, the interface and everything round the three parts.",
    groups: [
      {
        heading: "Built with",
        items: [
          { name: "Tauri", licence: "MIT / Apache-2.0", does: "The desktop app around everything", url: "https://tauri.app" },
          { name: "React", licence: "MIT", does: "The interface", url: "https://react.dev" },
          { name: "Vite", licence: "MIT", does: "Builds the interface", url: "https://vite.dev" },
          { name: "Lucide", licence: "ISC", does: "The icons", url: "https://lucide.dev" },
          { name: "Exo 2", licence: "SIL Open Font License 1.1", does: "The typeface", url: "https://fonts.google.com/specimen/Exo+2" },
        ],
      },
    ],
  },
];

// The icon set has no brand logos: simple outline glyphs in its style.
function Instagram({ size = 14 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2}>
      <rect x="3" y="3" width="18" height="18" rx="5" />
      <circle cx="12" cy="12" r="4" />
      <circle cx="17.5" cy="6.5" r="0.8" fill="currentColor" stroke="none" />
    </svg>
  );
}

function Facebook({ size = 14 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2}>
      <path d="M15 3h-2.5A3.5 3.5 0 0 0 9 6.5V21M6 11h8" strokeLinecap="round" />
    </svg>
  );
}

function Link({ href, children }: { href: string; children: ReactNode }) {
  return (
    <a
      href={href}
      onClick={(e) => {
        e.preventDefault();
        openExternal(href);
      }}
    >
      {children}
    </a>
  );
}

/**
 * About RapidOne: Chris's photo on the left; on the right who made it, links, the licence,
 * and everything each part (Cull, Edit, Retouch, the app) is built on, by category, with
 * each one's licence and what it does here. Esc or a click outside closes it.
 */
export default function About({ onClose }: { onClose: () => void }) {
  const [version, setVersion] = useState<string | null>(null);
  useEffect(() => {
    getVersion().then(setVersion, () => undefined);
  }, []);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopImmediatePropagation();
      onClose();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  return (
    <div className="about-modal" onClick={onClose}>
      <div className="about-box" onClick={(e) => e.stopPropagation()}>
        <button type="button" className="about-box__close" onClick={onClose} title="Close (Esc)">
          <X size={15} />
        </button>
        <div className="about-box__photo">
          <img src={aboutPhoto} alt="A portrait by Chris Cork Photography" draggable={false} />
          <div className="about-box__watermark">Chris Cork Photography</div>
        </div>
        <div className="about-box__text">
          <h2>
            RapidOne {version && <span className="about-box__version">v{version}</span>}
          </h2>
          <p>
            A RAW editor for photographers, with culling and retouching built in, made by Chris Cork, a wedding and
            portrait photographer who also captures astro for fun. Everything runs on your own computer: your photos never leave it.
          </p>
          <div className="about-box__links">
            <Link href={WEBSITE_URL}>
              <Globe size={14} /> Website
            </Link>
            <Link href={INSTAGRAM_URL}>
              <Instagram size={14} /> Instagram
            </Link>
            <Link href={FACEBOOK_URL}>
              <Facebook size={14} /> Facebook
            </Link>
          </div>

          <h3>Free</h3>
          <p>
            RapidOne and its parts are free software under the{" "}
            <Link href={AGPL_URL}>GNU Affero General Public License v3</Link>.
          </p>

          <div className="about-box__coffee">
            <p>
              RapidOne's development is powered almost entirely by coffee: I consume a worrying amount of it while
              building this app. Every donation goes straight into the developer, and performance
              improves accordingly. ☕😄
            </p>
            <button type="button" onClick={() => openExternal(COFFEE_URL)}>
              <Coffee size={15} /> Buy me a coffee
            </button>
          </div>

          {PARTS.map((part) => (
            <section key={part.title} className="about-box__part">
              <h3>{part.title}</h3>
              <p className="about-box__small">{part.intro}</p>
              {part.groups.map((g) => (
                <div key={g.heading} className="about-box__group">
                  <h4>{g.heading}</h4>
                  <ul>
                    {g.items.map((it) => (
                      <li key={it.name}>
                        <Link href={it.url}>{it.name}</Link> <span className="about-box__licence">{it.licence}</span>
                        <br />
                        <span className="about-box__small">{it.does}</span>
                      </li>
                    ))}
                  </ul>
                </div>
              ))}
            </section>
          ))}
          <p className="about-box__small">
            These are the main parts. RapidOne is also built on many smaller open-source libraries, too many to list
            here: thank you to everyone who makes them.
          </p>

        </div>
      </div>
    </div>
  );
}
