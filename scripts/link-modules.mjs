// Develop RapidCulling / RapidRetouch inside RapidOne from local clones, live.
//
//   npm run link-modules     use ../RapidCulling-module and ../RapidRetouch-module
//   npm run unlink-modules   back to the versions pinned from GitHub
//
// Linked: Cargo builds the plugins from the clones (a [patch] in
// src-tauri/.cargo/config.toml, kept out of git) and node_modules/rapidculling
// and rapidretouch are links to the clones, so Vite serves them as source
// (edits show at once). Cargo rewrites Cargo.lock to the local paths while
// linked, so a git hook refuses RapidOne commits until unlinked: nothing local
// can be pushed. Fixes go into the module repos; push those, then unlink here
// and run `npm run update-modules` to pin the new versions.

import { execSync } from "node:child_process";
import { existsSync, lstatSync, mkdirSync, readFileSync, rmSync, writeFileSync, appendFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const work = resolve(root, "..");
const MODULES = [
  { npm: "rapidculling", crate: "tauri-plugin-rapidculling", repo: "RapidCulling-module" },
  { npm: "rapidretouch", crate: "tauri-plugin-rapidretouch", repo: "RapidRetouch-module" },
];
const cargoConfig = join(root, "src-tauri", ".cargo", "config.toml");
const hook = join(root, ".git", "hooks", "pre-commit");
const HOOK_MARK = "# rapidone: modules linked";

const run = (cmd, cwd = root) => execSync(cmd, { cwd, stdio: "inherit" });
const isLink = (p) => {
  try {
    return lstatSync(p).isSymbolicLink();
  } catch {
    return false;
  }
};

/** Keeps a path out of git on this machine only (.git/info/exclude). */
function exclude(path) {
  const file = join(root, ".git", "info", "exclude");
  const text = existsSync(file) ? readFileSync(file, "utf8") : "";
  if (!text.split(/\r?\n/).includes(path)) appendFileSync(file, `${text.endsWith("\n") || !text ? "" : "\n"}${path}\n`);
}

/** The commit guard: no RapidOne commits while modules are linked. */
function installHook() {
  const body = `#!/bin/sh
${HOOK_MARK}
if [ -f src-tauri/.cargo/config.toml ] || [ -L node_modules/rapidculling ] || [ -L node_modules/rapidretouch ]; then
  echo "RapidOne: the modules are linked to local clones (Cargo.lock points at them)."
  echo "Run 'npm run unlink-modules' before committing."
  exit 1
fi
`;
  if (existsSync(hook) && !readFileSync(hook, "utf8").includes(HOOK_MARK)) {
    console.log(`A pre-commit hook already exists (${hook}); not replaced. Add the linked-modules check by hand.`);
    return;
  }
  writeFileSync(hook, body, { mode: 0o755 });
}

function link() {
  for (const m of MODULES) {
    if (!existsSync(join(work, m.repo, "package.json"))) {
      console.error(`Missing ${join(work, m.repo)}: clone github.com/crispin81/${m.repo} (branch module) next to RapidOne.`);
      process.exit(1);
    }
  }
  mkdirSync(dirname(cargoConfig), { recursive: true });
  writeFileSync(
    cargoConfig,
    `# Written by \`npm run link-modules\` (local only, kept out of git): build the
# module plugins from the local clones. \`npm run unlink-modules\` removes it.
${MODULES.map((m) => `[patch."https://github.com/crispin81/${m.repo}"]\n${m.crate} = { path = "../../${m.repo}/plugin" }\n`).join("\n")}`,
  );
  exclude("src-tauri/.cargo/");
  installHook();
  // Links in node_modules, without touching package.json or package-lock.json.
  run(`npm install --no-save ${MODULES.map((m) => `"../${m.repo}"`).join(" ")}`);
  console.log("\nLinked. Restart `npm run tauri dev`. Unlink before committing RapidOne.");
}

function unlink() {
  rmSync(cargoConfig, { force: true });
  // Cargo.lock back to the GitHub sources (only the plugins re-resolve).
  run("cargo fetch", join(root, "src-tauri"));
  // node_modules back to the pinned GitHub versions.
  for (const m of MODULES) if (isLink(join(root, "node_modules", m.npm))) rmSync(join(root, "node_modules", m.npm), { force: true });
  run("npm install");
  console.log("\nUnlinked: back to the module versions pinned from GitHub.");
}

function update() {
  if (existsSync(cargoConfig)) {
    console.error("Modules are linked: run `npm run unlink-modules` first.");
    process.exit(1);
  }
  // Cargo.lock: only each plugin's pinned commit changes (`cargo update -p`
  // would also reshuffle unrelated crates' versions).
  const lockFile = join(root, "src-tauri", "Cargo.lock");
  let lock = readFileSync(lockFile, "utf8");
  for (const m of MODULES) {
    const url = `https://github.com/crispin81/${m.repo}`;
    const rev = execSync(`git ls-remote ${url} refs/heads/module`, { encoding: "utf8" }).split(/\s/)[0];
    if (!/^[0-9a-f]{40}$/.test(rev)) throw new Error(`No module branch found at ${url}`);
    lock = lock.replace(new RegExp(`(git\\+${url.replace(/[.]/g, "\\.")}\\?branch=module#)[0-9a-f]{40}`, "g"), `$1${rev}`);
  }
  writeFileSync(lockFile, lock);
  run("cargo fetch", join(root, "src-tauri"));
  run(`npm install ${MODULES.map((m) => `github:crispin81/${m.repo}#module`).join(" ")}`);
  console.log("\nPinned the latest pushed module versions (commit Cargo.lock and package-lock.json).");
}

const what = process.argv[2];
if (what === "link") link();
else if (what === "unlink") unlink();
else if (what === "update") update();
else console.log("Usage: node scripts/link-modules.mjs link | unlink | update");
