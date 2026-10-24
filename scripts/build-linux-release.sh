#!/usr/bin/env bash
# The Linux release (AppImage + .deb), built inside Ubuntu 24.04 so it runs on
# that and every newer distribution (an AppImage uses the host's glibc, which
# must be at least as new as the build machine's: an Arch build needs Arch's).
# Not older: ONNX Runtime's prebuilt library (with WebGPU, for AI denoise on the
# graphics card) itself needs glibc 2.38 / GLIBCXX_3.4.32, so 22.04 can't link it.
# Needs podman (rootless) or docker. Output: release/linux/.
# First run downloads the image, Rust and the build tools (~15-20 min); later
# runs reuse them (cached in release/.cache).
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
cd "$here"
command -v podman >/dev/null || { echo "Needs podman (sudo pacman -S podman); rootless, no daemon." >&2; exit 1; }

scripts/prepare-release.sh x86_64-unknown-linux-gnu
# Everything private is fetched here, with this machine's GitHub login: the module
# crates (Cargo) and packages (npm). The container then builds offline from them.
(cd src-tauri && cargo fetch --locked)
[ -d node_modules/rapidretouch ] || npm install --no-audit --no-fund
mkdir -p release/.cache/rustup release/.cache/cargo-bin release/.cache/tauri release/.cache/ort release/linux

image=rapidone-build:24.04
ctx="$(mktemp -d)"
podman image exists "$image" || podman build -t "$image" -f - "$ctx" <<'DOCKERFILE'
FROM docker.io/library/ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      curl ca-certificates git build-essential pkg-config file patchelf xdg-utils \
      libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libssl-dev libgtk-3-dev libfuse2t64 \
 && curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
 && apt-get install -y nodejs && rm -rf /var/lib/apt/lists/*
DOCKERFILE
rmdir "$ctx"

# Rootless podman: root inside is this user outside, so everything written
# (target dir, caches) stays owned by you. Cargo's registry and git checkouts
# are this machine's own (already fetched above); its build output goes to
# release/.cache/target, apart from the dev build's.
podman run --rm \
  -v "$here":/src:Z -w /src \
  -v "$HOME/.cargo/registry":/root/.cargo/registry \
  -v "$HOME/.cargo/git":/root/.cargo/git \
  -v "$here/release/.cache/cargo-bin":/root/.cargo/bin \
  -v "$here/release/.cache/rustup":/root/.rustup \
  -v "$here/release/.cache/tauri":/root/.cache/tauri \
  -v "$here/release/.cache/ort":/root/.cache/ort.pyke.io \
  -e CARGO_TARGET_DIR=/src/release/.cache/target \
  -e APPIMAGE_EXTRACT_AND_RUN=1 \
  "$image" bash -euc '
    export PATH=/root/.cargo/bin:$PATH
    # Cargo offline (its crates were fetched above), set in its own config rather than
    # CARGO_NET_OFFLINE: that would also stop ONNX Runtime'"'"'s build script fetching its
    # prebuilt library (ort-sys skips the download then, and the link fails).
    printf "[net]\noffline = true\n" > /root/.cargo/config.toml
    [ -x /root/.cargo/bin/cargo ] || curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path
    # The patched AppImage GTK hook: native Wayland, dark dialogs, safe drawing.
    cp .github/appimage/linuxdeploy-plugin-gtk.sh /root/.cache/tauri/linuxdeploy-plugin-gtk.sh
    chmod +x /root/.cache/tauri/linuxdeploy-plugin-gtk.sh
    # ONNX Runtime'"'"'s WebGPU library (libwebgpu_dawn.so, AI denoise on the graphics card) is a
    # file of its own the app loads from beside itself ($ORIGIN) or, installed from the .deb,
    # from /usr/lib/RapidOne. Built first, then the library put where packaging finds it.
    export RUSTFLAGS="-C link-arg=-Wl,-rpath,\$ORIGIN/../lib/RapidOne"
    npx tauri build --verbose --no-bundle --config src-tauri/tauri.release.conf.json
    mkdir -p src-tauri/linux-lib
    cp -L release/.cache/target/release/libwebgpu_dawn.so src-tauri/linux-lib/
    LD_LIBRARY_PATH=/src/release/.cache/target/release npx tauri bundle --verbose --bundles appimage,deb --config src-tauri/tauri.release.conf.json
    cp release/.cache/target/release/bundle/appimage/*.AppImage release/.cache/target/release/bundle/deb/*.deb release/linux/
  '
ls -lh release/linux
