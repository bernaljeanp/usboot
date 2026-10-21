// Dev-only: runs the real UI in a plain browser with the Rust side simulated (npm run dev → /preview.html).
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";

const GB = 1024 ** 3;
const drives = [
  { number: 1, name: "Samsung SSD 860 EVO 500G", size: 500107862016, serial: "S3Z9NB0K", letters: ["D"] },
  { number: 2, name: "SanDisk Ultra Fit", size: 32017047552, serial: "4C530001", letters: ["E"] },
];
const rel = (version: string, file: string, size: number) => ({ version, file, url: "", sha256: "", size });
const library = [
  ["ubuntu", "Ubuntu", "Desktop LTS", rel("26.04.1", "ubuntu-26.04.1-desktop-amd64.iso", 6.04 * GB)],
  ["mint", "Linux Mint", "Cinnamon", rel("22.3", "linuxmint-22.3-cinnamon-64bit.iso", 2.88 * GB)],
  ["zorin", "Zorin OS", "Core", rel("18.1", "z.iso", 3.64 * GB)],
  ["cachyos", "CachyOS", "Desktop", rel("260809", "c.iso", 2.97 * GB)],
  ["parrot", "Parrot OS", "Security", rel("7.4", "p.iso", 8.08 * GB)],
  ["fedora", "Fedora", "Workstation", rel("44-1.7", "f.iso", 2.66 * GB)],
  ["popos", "Pop!_OS", "Intel/AMD", rel("24.04 build 20", "p.iso", 2.75 * GB)],
  ["bazzite", "Bazzite", "Gaming · Steam", rel("latest", "b.iso", 7.36 * GB)],
  ["debian", "Debian", "Live GNOME", rel("13.7.0", "d.iso", 3.54 * GB)],
  ["arch", "Arch Linux", "Monthly", rel("2026.10.01", "a.iso", 1.53 * GB)],
  ["kali", "Kali Linux", "Installer", rel("2026.3", "k.iso", 4.7 * GB)],
  ["opensuse", "openSUSE", "Tumbleweed", rel("20261018", "o.iso", 4.35 * GB)],
  ["alma", "AlmaLinux", "DVD", rel("10.2", "al.iso", 9.41 * GB)],
  ["tails", "Tails", "Privacy · USB image", rel("7.15", "t.img", 1.76 * GB)],
  ["windows11", "Windows 11", "Microsoft", null],
].map(([id, name, desc, release]) => ({
  id, name, desc, release, error: null, outdated: id === "debian" ? "old.iso" : null,
  page: id === "windows11" ? "https://www.microsoft.com/software-download/windows11" : null,
  local: id === "mint" ? "C:/Users/you/Downloads/USBoot/linuxmint-22.3-cinnamon-64bit.iso" : null,
}));

async function simulate(event: string, phases: string[], total: number, extra: (id?: string) => object = () => ({}), tick = 60) {
  for (const phase of phases) {
    for (let i = 0; i <= 100; i += 1) {
      await emit(event, { phase, done: (total * i) / 100, total, sources: 3, ...extra() });
      await new Promise((r) => setTimeout(r, tick));
    }
  }
}

mockWindows("main");
mockIPC(
  async (cmd, args: any) => {
    switch (cmd) {
      case "os_locale": return new URLSearchParams(location.search).get("lang") ?? "es-CL";
      case "is_admin": return true;
      case "list_drives": return drives;
      case "inspect_image": return { path: args.path, name: args.path.split(/[\\/]/).pop(), size: 2.88 * GB, bootable: true, windows: args.path.includes("Win") };
      case "library": return library;
      case "flash":
        // each drive at its own pace, like real sticks
        await Promise.all(args.drives.map((d: any, i: number) => simulate("progress", ["write", "verify"], 2.88 * GB, () => ({ drive: d.number }), 60 + i * 25)));
        return args.drives.map(() => null);
      case "flash_windows": await simulate("progress", ["prepare", "copy", "split"], 5.4 * GB, () => ({ drive: args.drive.number })); return null;
      case "download_iso": await simulate("download", ["connect", "download", "verify"], 1.53 * GB, () => ({ id: args.id })); return null;
      case "restore_drive": await new Promise((r) => setTimeout(r, 1500)); return null;
      default: return null;
    }
  },
  { shouldMockEvents: true },
);
import("./main");
