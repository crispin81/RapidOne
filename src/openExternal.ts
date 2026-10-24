import { invoke } from "@tauri-apps/api/core";

/** A web page in the default browser (a new window if the opener plugin isn't there). */
export function openExternal(url: string) {
  invoke("plugin:opener|open_url", { url }).catch(() => window.open(url, "_blank"));
}
