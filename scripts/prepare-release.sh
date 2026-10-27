#!/usr/bin/env bash
# Stage what a release build bundles beyond the code (src-tauri/tauri.release.conf.json):
# uv and RapidRetouch's engine source, both from the RapidRetouch module commit
# Cargo.lock pins, and a check that the AI models are all in models/.
# Usage: scripts/prepare-release.sh [rust target triple] (default: this machine's).
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
cd "$here"

if [ -f src-tauri/.cargo/config.toml ]; then
  echo "The modules are linked to local clones: run 'npm run unlink-modules' first." >&2
  exit 1
fi

target="${1:-$(rustc -vV | sed -n 's/^host: //p')}"

# Every model the installer carries (never autostyle-*.json: trained styles are personal).
missing=0
for f in birefnet-lite.onnx denoise-student.onnx grounding-dino-tiny-int8.onnx grounding-dino-tokenizer.json \
         lama_fp32.onnx mobile_sam_image_encoder.onnx sam_mask_decoder_multi.onnx MODELS-LICENCES.txt; do
  [ -f "models/$f" ] || { echo "missing models/$f (npm run fetch-models)" >&2; missing=1; }
done
[ "$missing" = 0 ] || exit 1

# The RapidRetouch module as Cargo checked it out (the pinned commit).
plugin_manifest="$(cd src-tauri && cargo metadata --format-version 1 --locked 2>/dev/null \
  | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{const p=JSON.parse(s).packages.find(p=>p.name==="tauri-plugin-rapidretouch");console.log(p.manifest_path)})')"
module="$(dirname "$(dirname "$plugin_manifest")")"
[ -f "$module/engine/pyproject.toml" ] || { echo "RapidRetouch engine not found at $module/engine" >&2; exit 1; }

rm -rf src-tauri/retouch-engine
mkdir -p src-tauri/retouch-engine
cp "$module/engine/pyproject.toml" "$module/engine/uv.lock" "$module/engine/README.md" src-tauri/retouch-engine/
cp -r "$module/engine/src" src-tauri/retouch-engine/src
find src-tauri/retouch-engine -name __pycache__ -type d -prune -exec rm -rf {} +

# The module's own uv fetcher (pinned version), run from a scratch copy so it
# writes there rather than into Cargo's checkout.
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/scripts"
cp "$module/scripts/fetch-uv.sh" "$tmp/scripts/"
bash "$tmp/scripts/fetch-uv.sh" "$target"
mkdir -p src-tauri/binaries
cp "$tmp"/src-tauri/binaries/uv-"$target"* src-tauri/binaries/

# Windows: Microsoft's C++ runtime beside RapidOne.exe (tauri.release.windows.conf.json). ONNX Runtime,
# built into the exe, needs msvcp140*.dll / vcruntime140*.dll, which aren't part of Windows: a PC without
# the Visual C++ Redistributable couldn't start RapidOne. Microsoft allows shipping them with an app
# ("app-local"); taken from Visual Studio's own redistributable folder.
if [[ "$target" == *windows-msvc ]]; then
  crt="$(ls -d "/c/Program Files (x86)/Microsoft Visual Studio/"*/*/VC/Redist/MSVC/*/x64/Microsoft.VC14*.CRT          "/c/Program Files/Microsoft Visual Studio/"*/*/VC/Redist/MSVC/*/x64/Microsoft.VC14*.CRT 2>/dev/null | sort | tail -1 || true)"
  [ -n "$crt" ] || { echo "Visual Studio's C++ redistributable folder not found (VC/Redist/MSVC/*/x64/Microsoft.VC14*.CRT)" >&2; exit 1; }
  rm -rf src-tauri/windows-lib
  mkdir -p src-tauri/windows-lib
  for f in msvcp140.dll msvcp140_1.dll vcruntime140.dll vcruntime140_1.dll; do cp "$crt/$f" src-tauri/windows-lib/; done
  echo "C++ runtime from $crt"
fi

echo "Release files staged for $target (RapidRetouch engine from $module)."
