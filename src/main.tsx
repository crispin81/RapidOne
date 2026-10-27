import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { applySavedRenderMode } from "./components/RenderMenu";
import "@fontsource/exo-2/300.css";
import "@fontsource/exo-2/400.css";
import "@fontsource/exo-2/500.css";
import "@fontsource/exo-2/600.css";
import "@fontsource/exo-2/700.css";
import "@fontsource/exo-2/400-italic.css";

// WebKitGTK (Linux) needs some parts of the window on drawing layers of their own (`will-change`, see
// App.css: .webkitgtk) or it draws them stale or in the wrong place for a frame. Windows' WebView2
// doesn't, and drew a stray curved line at the bottom with them while a slider moved (24 Oct).
if (/Linux/.test(navigator.userAgent) && !/Android/.test(navigator.userAgent)) document.documentElement.classList.add("webkitgtk");

// The default webview menu only offers Reload/Inspect, which do nothing useful here.
window.addEventListener("contextmenu", (e) => e.preventDefault());

// Development copy only (served over http; an installed one isn't): Ctrl+R / F5 reloads the
// interface, as WebView2 does on Windows (WebKitGTK has no such key). Rust changes still need a restart.
if (window.location.protocol.startsWith("http")) {
  window.addEventListener(
    "keydown",
    (e) => {
      if (e.key === "F5" || ((e.ctrlKey || e.metaKey) && !e.shiftKey && e.code === "KeyR")) {
        e.preventDefault();
        e.stopImmediatePropagation();
        window.location.reload();
      }
    },
    true,
  );
}

// The window lets the webview see touchpad pinch (`zoomHotkeysEnabled` in tauri.conf.json: without it
// WebView2 drops the gesture and the page never gets it). That setting also turns on the webview's own
// page zoom, which would scale the whole interface: a pinch arrives as Ctrl+wheel and Ctrl+= / - / 0 as
// keys, so stop the webview acting on them. The viewers still get both and zoom the picture themselves.
window.addEventListener("wheel", (e) => { if (e.ctrlKey) e.preventDefault(); }, { passive: false });
// WebKit (macOS) reports a pinch as gesture events: likewise only the viewers act on them.
for (const t of ["gesturestart", "gesturechange"]) window.addEventListener(t, (e) => e.preventDefault());
window.addEventListener(
  "keydown",
  (e) => {
    if ((e.ctrlKey || e.metaKey) && ["Equal", "Minus", "Digit0", "NumpadAdd", "NumpadSubtract", "Numpad0"].includes(e.code)) e.preventDefault();
  },
  true,
);

// The Render setting the user chose (Auto, GPU or CPU), before the first photo renders.
applySavedRenderMode();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
