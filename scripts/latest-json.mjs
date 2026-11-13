// CI: writes the latest.json the in-app updater reads, from the signed bundles collect.mjs gathered.
//   node scripts/latest-json.mjs <dir with the bundles and .sig files> [release notes file]
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const [dir, notesFile] = process.argv.slice(2);
const { version } = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8"));
const notes = notesFile && existsSync(notesFile) ? readFileSync(notesFile, "utf8").trim() : "";

// What each platform's updater downloads. The updater asks for "os-arch-installer" first and falls
// back to "os-arch": on Linux that default is the AppImage.
const targets = {
  [`USBoot_${version}_x64-setup.exe`]: ["windows-x86_64", "windows-x86_64-nsis"],
  [`USBoot_${version}_linux-beta_amd64.AppImage`]: ["linux-x86_64", "linux-x86_64-appimage"],
  [`USBoot_${version}_linux-beta_amd64.deb`]: ["linux-x86_64-deb"],
  [`USBoot_${version}_macos-beta_apple-silicon.app.tar.gz`]: ["darwin-aarch64", "darwin-aarch64-app"],
};

const platforms = {};
for (const [file, keys] of Object.entries(targets)) {
  const sig = join(dir, `${file}.sig`);
  if (!existsSync(sig)) continue;
  const entry = {
    signature: readFileSync(sig, "utf8"),
    url: `https://github.com/bernaljeanp/usboot/releases/download/v${version}/${file}`,
  };
  for (const key of keys) platforms[key] = entry;
}
// Windows is where the updater has always worked: never publish a manifest that drops it.
for (const must of ["windows-x86_64", "linux-x86_64", "darwin-aarch64"]) {
  if (!platforms[must]) throw new Error(`No signed bundle for ${must}: is TAURI_SIGNING_PRIVATE_KEY set?`);
}
writeFileSync(join(dir, "latest.json"), JSON.stringify({ version, notes, pub_date: new Date().toISOString(), platforms }, null, 2));
console.log(`latest.json: ${Object.keys(platforms).join(", ")}`);
