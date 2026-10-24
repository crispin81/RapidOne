#!/usr/bin/env bash
# A quick check of the Linux release on a stock Ubuntu (default 24.04, the oldest it supports), in a
# throwaway podman container that shows its window on this desktop (Wayland).
# Usage: scripts/test-linux-release.sh [appimage|deb] [ubuntu version]
#   appimage: runs release/linux/*.AppImage (extract-and-run, as FUSE isn't
#             available in a container) on Ubuntu's own libraries.
#   deb:      installs release/linux/*.deb with apt (pulls its dependencies, as
#             a user's install would), then starts RapidOne.
# Close the window to end the test; the container is deleted afterwards.
# Not tested here: the desktop's look (GNOME/KDE) and other graphics drivers.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
mode="${1:-appimage}"
version="${2:-24.04}"
command -v podman >/dev/null || { echo "Needs podman (sudo pacman -S podman)." >&2; exit 1; }
[ -n "${WAYLAND_DISPLAY:-}" ] || { echo "Run from the Wayland desktop." >&2; exit 1; }

case "$mode" in
  appimage) run='
      apt-get install -y --no-install-recommends libgl1 libegl1 >/dev/null
      echo "== what a user without libfuse2 would see:"; (/rel/*.AppImage --version 2>&1 | head -3) || true
      echo "== starting the AppImage"; APPIMAGE_EXTRACT_AND_RUN=1 /rel/*.AppImage' ;;
  deb) run='
      echo "== installing the .deb"; apt-get install -y /rel/*.deb
      echo "== starting RapidOne"; RapidOne' ;;
  *) echo "usage: $0 [appimage|deb] [ubuntu version]" >&2; exit 1 ;;
esac

podman run --rm -it \
  -v "$here/release/linux":/rel:ro,Z \
  -v "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY":/tmp/runtime/"$WAYLAND_DISPLAY" \
  -e XDG_RUNTIME_DIR=/tmp/runtime -e WAYLAND_DISPLAY="$WAYLAND_DISPLAY" \
  --device /dev/dri --group-add keep-groups \
  docker.io/library/ubuntu:"$version" bash -euc "
    export DEBIAN_FRONTEND=noninteractive
    apt-get update >/dev/null
    $run
  "
