// Builds a signed installer and publishes it as a GitHub release the in-app updater can see.
//   node scripts/release.mjs "What changed"
// Needs: gh (logged in) and the signing key at %USERPROFILE%\.tauri\usboot.key (never commit it).
import { execSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const notes = process.argv[2] ?? "";
const { version } = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8"));
// A short target dir: deep Windows paths break the MSVC linker (LNK1104).
const target = process.env.CARGO_TARGET_DIR ?? join(homedir(), "AppData", "Local", "usboot-target");
const env = {
  ...process.env,
  CARGO_TARGET_DIR: target,
  TAURI_SIGNING_PRIVATE_KEY: readFileSync(join(homedir(), ".tauri", "usboot.key"), "utf8"),
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD: process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD ?? "",
};
execSync("npm run tauri build", { stdio: "inherit", env });

const name = `USBoot_${version}_x64-setup.exe`;
const exe = join(target, "release", "bundle", "nsis", name);
const latest = {
  version,
  notes,
  pub_date: new Date().toISOString(),
  platforms: {
    "windows-x86_64": {
      signature: readFileSync(`${exe}.sig`, "utf8"),
      url: `https://github.com/bernaljeanp/usboot/releases/download/v${version}/${name}`,
    },
  },
};
writeFileSync("latest.json", JSON.stringify(latest, null, 2));
execSync(`gh release create v${version} "${exe}" latest.json --title "USBoot ${version}" --notes-file -`, {
  stdio: ["pipe", "inherit", "inherit"],
  input: notes,
});
console.log(`Published v${version}`);
