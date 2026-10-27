//! Touchpad pinch on Linux. WebKitGTK acts on a pinch itself and zooms the whole interface (the page),
//! and the page never hears of it: no Ctrl+wheel as WebView2 sends on Windows, no gesture events as
//! WebKit sends on macOS (27 Oct, found on the Zenbook). So the pinch is taken off the webview here and
//! handed to the page as macOS's gesture events (`gesturestart` / `gesturechange` / `gestureend`, with
//! the scale since the pinch began), which the viewers already zoom the picture by. Elsewhere this does
//! nothing.

/// Take touchpad pinches off the main window's webview.
pub fn start(app: &tauri::App) {
    #[cfg(target_os = "linux")]
    {
        use gtk::gdk;
        use gtk::glib::translate::ToGlibPtr;
        use gtk::prelude::*;
        use tauri::Manager;
        let Some(window) = app.get_webview_window("main") else { return };
        let page = window.clone();
        let _ = window.with_webview(move |webview| {
            webview.inner().connect_event(move |_, event| {
                let Some(pinch) = event.downcast_ref::<gdk::EventTouchpadPinch>() else {
                    return gtk::glib::Propagation::Proceed;
                };
                // (gdk's `is_phase` reads the phase as a bool, so read it from the event itself.)
                let raw: *const gdk::ffi::GdkEventTouchpadPinch = pinch.to_glib_none().0;
                // SAFETY: the pointer is the event GTK is delivering, alive for this call.
                let kind = match unsafe { (*raw).phase } as i32 {
                    gdk::ffi::GDK_TOUCHPAD_GESTURE_PHASE_BEGIN => "gesturestart",
                    gdk::ffi::GDK_TOUCHPAD_GESTURE_PHASE_UPDATE => "gesturechange",
                    _ => "gestureend",
                };
                let (x, y) = pinch.position();
                // Every event of one pinch goes to what was under the pointer as it began.
                let _ = page.eval(format!(
                    "(() => {{
                        const e = new UIEvent('{kind}', {{ bubbles: true, cancelable: true }});
                        Object.assign(e, {{ scale: {scale}, clientX: {x}, clientY: {y} }});
                        if ('{kind}' === 'gesturestart') window.__pinchTarget = document.elementFromPoint({x}, {y});
                        (window.__pinchTarget || document).dispatchEvent(e);
                    }})()",
                    scale = pinch.scale(),
                ));
                gtk::glib::Propagation::Stop
            });
        });
    }
}
