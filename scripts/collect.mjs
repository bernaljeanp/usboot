// CI: copies the bundles Tauri just built (and their updater signatures) into one folder under the
// names the releases have always used, so the Homebrew cask and winget manifest URLs keep working.
//   node scripts/collect.mjs <bundle dir> <out dir>
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

const [bundle, out] = process.argv.slice(2);
const { version } = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8"));
mkdirSync(out, { recursive: true });

// [folder, ending of the file Tauri writes, name on the release]
const wanted = [
  ["nsis", "-setup.exe", `USBoot_${version}_x64-setup.exe`],
  ["appimage", ".AppImage", `USBoot_${version}_linux-beta_amd64.AppImage`],
  ["deb", ".deb", `USBoot_${version}_linux-beta_amd64.deb`],
  ["dmg", ".dmg", `USBoot_${version}_macos-beta_apple-silicon.dmg`],
  ["macos", ".app.tar.gz", `USBoot_${version}_macos-beta_apple-silicon.app.tar.gz`],
];

let copied = 0;
for (const [dir, ending, name] of wanted) {
  const folder = join(bundle, dir);
  if (!existsSync(folder)) continue;
  const file = readdirSync(folder).find((f) => f.endsWith(ending));
  if (!file) continue;
  copyFileSync(join(folder, file), join(out, name));
  copied++;
  // The signature covers the file's bytes, not its name.
  if (existsSync(join(folder, `${file}.sig`))) copyFileSync(join(folder, `${file}.sig`), join(out, `${name}.sig`));
  console.log(`${file} -> ${name}`);
}
if (!copied) throw new Error(`No bundles found in ${bundle}`);
