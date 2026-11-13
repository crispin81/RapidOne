//! Closing the window, once the page has saved everything (App.tsx's close handler).
//!
//! Linux with NVIDIA's driver: WebKitGTK's web process (WebKitWebProcess, which draws the page)
//! crashed with SIGSEGV on nearly every close of 1.2.1 (core dumps on nvidia-omarchy, 1 and 3 Nov).
//! When the window goes, the web process notices and calls exit() itself; its Skia GPU thread
//! (SkiaGPUWorker) then runs its per-thread cleanup and calls into NVIDIA's EGL library
//! (libnvidia-eglcore) while the main thread is already tearing EGL down: a use after free in the
//! driver. Nothing in RapidOne's own process reaches that race. So on Linux the web process is
//! stopped first, by WebKit's own `webkit_web_view_terminate_web_process` (an immediate kill, so
//! its exit-time cleanup, where the race is, never runs), with the window hidden, and then the
//! window is destroyed as before. By then the page has nothing left to do: Retouch's edits handed
//! back, the .rpd written, the hand-off TIFFs cleared. Elsewhere this is just `destroy()`.

#[tauri::command]
pub fn close_app(window: tauri::WebviewWindow) {
    #[cfg(target_os = "linux")]
    {
        use webkit2gtk::WebViewExt;
        let _ = window.hide();
        let win = window.clone();
        let stopped = window.with_webview(move |webview| {
            // On GTK's main thread, where WebKit must be called.
            webview.inner().terminate_web_process();
            let _ = win.destroy();
        });
        if stopped.is_ok() {
            return;
        }
    }
    let _ = window.destroy();
}
