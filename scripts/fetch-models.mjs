// Downloads the public AI models RapidOne runs on any computer into models/
// (git-ignored): the Lite remover, the subject mask, depth and the scene geometry. From Hugging
// Face; each licence allows redistribution. Files already there at the right size are skipped.
//
//   npm run fetch-models
//
// Not here: Chris's own files (trained styles, his camera profiles, our noise
// model) - those go across with scripts/pack-for-laptop.ps1 - and the big
// graphics-card removers (Large: Klein, Medium: RORem).
import { createWriteStream, existsSync, mkdirSync, statSync, renameSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";

const models = join(dirname(fileURLToPath(import.meta.url)), "..", "models");
const hf = (repo, file) => `https://huggingface.co/${repo}/resolve/main/${file}`;
const FILES = [
  // Lite remover: fill (LaMa, Apache-2.0; trained on Places2, CC BY 4.0).
  ["lama_fp32.onnx", hf("Carve/LaMa-ONNX", "lama_fp32.onnx"), "LaMa (Suvorov et al.), Apache-2.0, ONNX by Carve; trained on Places2 (CC BY 4.0)"],
  // Lite remover: outline (MobileSAM, MIT).
  ["mobile_sam_image_encoder.onnx", hf("Acly/MobileSAM", "mobile_sam_image_encoder.onnx"), "MobileSAM (Zhang et al.), MIT, ONNX by Acly"],
  ["sam_mask_decoder_multi.onnx", hf("Acly/MobileSAM", "sam_mask_decoder_multi.onnx"), "MobileSAM mask decoder, MIT"],
  // Lite remover: find by name (Grounding DINO tiny, Apache-2.0).
  ["grounding-dino-tiny-int8.onnx", hf("onnx-community/grounding-dino-tiny-ONNX", "onnx/model_int8.onnx"), "Grounding DINO tiny (IDEA Research), Apache-2.0, ONNX by onnx-community"],
  ["grounding-dino-tokenizer.json", hf("onnx-community/grounding-dino-tiny-ONNX", "tokenizer.json"), "Grounding DINO tokenizer (BERT uncased vocabulary), Apache-2.0"],
  // Subject mask (BiRefNet lite, MIT; identical to the official release's export).
  ["birefnet-lite.onnx", hf("onnx-community/BiRefNet_lite-ONNX", "onnx/model.onnx"), "BiRefNet lite (Zheng Peng et al.), MIT, ONNX by onnx-community"],
  // Depth mask (Depth Anything V2 Small, Apache-2.0). Base / Large are CC-BY-NC: never use them.
  ["depth-anything-v2-small.onnx", hf("onnx-community/depth-anything-v2-small", "onnx/model.onnx"), "Depth Anything V2 Small (Yang et al.), Apache-2.0, ONNX by onnx-community"],
  // Optional High quality depth (Depth Anything 3 Mono-Large, Apache-2.0; ONNX by Jens-Duttke,
  // Apache-2.0). Never bundled: the app downloads it on demand (Depth quality: High) into its own
  // models folder. Here for a development machine that wants it.
  ["da-v3-mono-large_fp16_opset21_optimized.onnx", hf("Jens-Duttke/Depth-Anything-3-MONO-ONNX", "da-v3-mono-large_fp16_opset21_optimized.onnx"), "Depth Anything 3 Mono-Large (ByteDance-Seed), Apache-2.0; ONNX export by Jens-Duttke (Apache-2.0)"],
  // Scene geometry (MoGe-2: metric points, normals, sky mask, FOV), MIT. ViT-S on the CPU, ViT-B on a card.
  ["moge-2-vits-normal.onnx", hf("Ruicheng/moge-2-vits-normal-onnx", "model.onnx"), "MoGe-2 (Wang et al., Microsoft), MIT; DINOv2 backbone Apache-2.0"],
  ["moge-2-vitb-normal.onnx", hf("Ruicheng/moge-2-vitb-normal-onnx", "model.onnx"), "MoGe-2 (Wang et al., Microsoft), MIT; DINOv2 backbone Apache-2.0"],
];

mkdirSync(models, { recursive: true });
const notes = [];
for (const [name, url, licence] of FILES) {
  const dest = join(models, name);
  notes.push(`${name}: ${licence}\n  ${url}`);
  const head = await fetch(url, { method: "HEAD", redirect: "follow" });
  const size = Number(head.headers.get("content-length") ?? 0);
  if (existsSync(dest) && size > 0 && statSync(dest).size === size) {
    console.log(`have ${name}`);
    continue;
  }
  process.stdout.write(`downloading ${name} (${(size / 1e6).toFixed(0)} MB)... `);
  const res = await fetch(url, { redirect: "follow" });
  if (!res.ok || !res.body) throw new Error(`${name}: ${res.status} ${res.statusText}`);
  const part = `${dest}.part`;
  await pipeline(Readable.fromWeb(res.body), createWriteStream(part));
  renameSync(part, dest);
  console.log("done");
}
writeFileSync(join(models, "MODELS-LICENCES.txt"), notes.join("\n") + "\n");
// MoGe-2's backbone is DINOv2, whose Apache-2.0 licence asks for its notice wherever the weights go:
// shipped beside the models (tauri.release.conf.json bundles it).
writeFileSync(join(models, "NOTICE-DINOv2.txt"), `MoGe-2 (Wang et al., Microsoft) uses the DINOv2 backbone.

DINOv2
Copyright (c) Meta Platforms, Inc. and affiliates.
Licensed under the Apache License, Version 2.0 (the "License"); you may not use these files except in
compliance with the License. A copy of the License is at:

  http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software distributed under the License is
distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and limitations under the License.

MoGe-2 itself: Copyright (c) Microsoft Corporation, MIT licence.
`);
console.log(`Models in ${models}`);
