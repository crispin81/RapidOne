import { defineConfig, searchForWorkspaceRoot } from "vite";
import react from "@vitejs/plugin-react";
import scopeAwayFromModules from "./scopeCss";
// @ts-expect-error type error without @types/node package
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({
  // RapidOne's styles stop at the embedded Cull / Retouch screens (scopeCss.ts).
  plugins: [scopeAwayFromModules(), react()],

  // `npm run link-modules` (scripts/link-modules.mjs) links node_modules/rapidculling
  // and rapidretouch to local clones next to RapidOne. Vite then serves them as
  // source from there: their peer dependencies come from RapidOne's own
  // node_modules (one React), and the two clone folders may be read. Without
  // the links neither setting changes anything.
  resolve: {
    dedupe: ["react", "react-dom", "lucide-react", "@tauri-apps/api", "@tauri-apps/plugin-dialog"],
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
    fs: {
      allow: [searchForWorkspaceRoot(process.cwd()), "../RapidCulling-module", "../RapidRetouch-module"],
    },
  },
}));
