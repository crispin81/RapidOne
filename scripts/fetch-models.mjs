// Downloads the public AI models RapidOne runs on any computer into models/
// (git-ignored): the Lite remover, the subject mask. From Hugging Face; each
// licence allows redistribution. Files already there at the right size are
// skipped.
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
console.log(`Models in ${models}`);
