#!/usr/bin/env bash
# Puts our launch settings in the AppImage's GTK hook and packs it again. The Tauri CLI writes its
# own copy of linuxdeploy-plugin-gtk.sh over ~/.cache/tauri's on every bundle, so a patched copy put
# there beforehand is lost (1.1.0 shipped without it); the hook has to be changed after bundling.
# Run inside build-linux-release.sh's container. Stops the build if the hook can't be confirmed.
set -euo pipefail
bundle=release/.cache/target/release/bundle/appimage
hook="$bundle/RapidOne.AppDir/apprun-hooks/linuxdeploy-plugin-gtk.sh"
image="$(ls "$bundle"/*.AppImage)"
wayland='[ "$RAPIDONE_SAFE_DRAWING" = 1 ] && export GDK_BACKEND=x11,wayland || export GDK_BACKEND=wayland,x11'

# Safe drawing wants XWayland first; otherwise native Wayland, X11 where there's none.
sed -i "s~^# export GDK_BACKEND=x11.*~${wayland//&/\\&}~" "$hook"
# Dark on a light desktop too: this runs before main.rs, whose dark default only applies when GTK_THEME is unset.
sed -i 's|Adwaita:\$GTK_THEME_VARIANT|Adwaita:dark|' "$hook"

rm "$image"
(cd "$bundle" && ARCH=x86_64 LDAI_OUTPUT="$(basename "$image")" \
  /root/.cache/tauri/linuxdeploy-plugin-appimage.AppImage --appimage-extract-and-run --appdir RapidOne.AppDir)

# What matters is what's in the packed file, so read it back from there.
check="$(mktemp -d)"
(cd "$check" && "$OLDPWD/$image" --appimage-extract 'apprun-hooks/*' >/dev/null)
packed="$check/squashfs-root/apprun-hooks/linuxdeploy-plugin-gtk.sh"
grep -qxF "$wayland" "$packed" || { echo "AppImage hook has no Wayland line" >&2; exit 1; }
grep -q 'Adwaita:dark' "$packed" || { echo "AppImage hook has no dark theme" >&2; exit 1; }
rm -rf "$check"
echo "AppImage hook confirmed: Wayland, dark, safe drawing."
