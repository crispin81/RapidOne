// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// WebKitGTK draws the window on the graphics card, natively on Wayland.
///
/// With NVIDIA's driver, its Wayland EGL libraries (both `egl-wayland` and
/// `egl-wayland2`) sign the window up for the compositor's explicit sync and
/// then commit a frame without its acquire point; Hyprland drops the app with
/// `Error 71 (Protocol error)` ("Missing acquire timeline"). So on NVIDIA,
/// WebKit hands its frames over in shared memory instead
/// (`WEBKIT_DMABUF_RENDERER_FORCE_SHM`): still drawn on the graphics card.
/// (Turning NVIDIA's explicit sync off also avoids the crash, but lets
/// half-drawn frames reach the screen.) AMD and Intel (Mesa) need nothing.
///
/// Older WebKitGTK also needs its DMA-BUF renderer forced on there (it disables it for NVIDIA).
///
/// `RAPIDONE_SAFE_DRAWING=1` brings back the old fallback for a machine whose
/// window comes up blank or crashes: XWayland and drawing on the processor,
/// which is slow (text draws in slow motion) but works everywhere.
///
/// Set *before* Tauri/GTK initializes; harmless on platforms where
/// GTK/WebKitGTK aren't in play (macOS, Windows), and never overrides a value
/// the user already set themselves.
#[cfg(target_os = "linux")]
fn apply_linux_webkit_workarounds() {
    let safe = std::env::var("RAPIDONE_SAFE_DRAWING").is_ok_and(|v| v == "1");
    let nvidia = std::path::Path::new("/proc/driver/nvidia/version").exists();
    let mut settings = vec![];
    if safe {
        settings.extend([
            ("GDK_BACKEND", "x11,wayland"),
            ("WEBKIT_DISABLE_DMABUF_RENDERER", "1"),
            ("WEBKIT_DISABLE_COMPOSITING_MODE", "1"),
        ]);
    } else if nvidia {
        settings.push(("WEBKIT_DMABUF_RENDERER_FORCE_SHM", "1"));
        // Older WebKitGTK (2.52, Ubuntu 24.04's, and the AppImage's) turns its DMA-BUF renderer off
        // for NVIDIA's driver and falls back to painting everything on the web process's main thread
        // on the processor: the interface crawled (typing, the corridor), 24 Oct. Forced back on (frames
        // still handed over in shared memory, above), it composites on its own thread and paints on
        // the graphics card. Newer WebKit already does this; there it changes nothing.
        settings.push(("WEBKIT_FORCE_DMABUF_RENDERER", "1"));
    }
    for (key, value) in settings {
        if std::env::var(key).is_err() {
            std::env::set_var(key, value);
        }
    }
    // RapidOne is dark on every distribution and desktop theme: GTK draws a
    // dropdown's open list (older WebKitGTK) and the file pickers, so ask for
    // dark Adwaita, GTK's own theme, unless the user set a theme themselves.
    // (Newer WebKitGTK follows the page's `color-scheme: dark` instead.)
    if std::env::var("GTK_THEME").is_err() {
        std::env::set_var("GTK_THEME", "Adwaita:dark");
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    apply_linux_webkit_workarounds();

    rapidone_lib::run()
}
