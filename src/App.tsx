import { useEffect, useState } from "react";
import type { CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { Check, Disc3, Library as LibraryIcon, LoaderCircle, RotateCcw, ShieldAlert, ShieldCheck, TriangleAlert, Usb, Zap } from "lucide-react";
import Library from "./Library";
import SectorMap from "./SectorMap";
import LightBar, { Backdrop, type Rgb } from "./Shader";
import logo from "./assets/logo.png";
import { errText, fmtSize, nf, t } from "./i18n";
import "./App.css";

type Drive = { number: number; name: string; size: number; serial: string; letters: string[] };
type Image = { path: string; name: string; size: number; bootable: boolean; windows: boolean };
type Progress = { phase: "write" | "verify" | "prepare" | "copy" | "split"; done: number; total: number };
type Running = { kind: "running"; p: Progress | null; since: number; base: number; windows: boolean };
type Run =
  | { kind: "confirm" }
  | Running
  | { kind: "restore"; fs: string; label: string }
  | { kind: "restoring" }
  | { kind: "done"; message: string; flashed: boolean }
  | { kind: "error"; message: string };

// Ember first: it's the brand (the logo is a flame).
const LIGHTS: Record<string, string> = { ember: "#ff9a3c", lunar: "#4fc8ff", plasma: "#a685ff", arctic: "#e4ecf6" };
const FS_NAMES: Record<string, string> = { exfat: "exFAT", ntfs: "NTFS", fat32: "FAT32" };
const DANGER: Rgb = [1, 0.27, 0.23];
const rgb = (hex: string): Rgb => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255) as Rgb;

function fmtTime(s: number) {
  return s >= 60 ? `${Math.floor(s / 60)} min ${Math.round(s % 60)} s` : `${Math.max(1, Math.round(s))} s`;
}
const driveLabel = (d: Drive) => (d.letters.length ? `${d.name} (${d.letters.map((l) => `${l}:`).join(" ")})` : d.name);
// Bigger than any common pendrive: probably an external SSD/HDD with someone's data on it.
const isExternalDisk = (d: Drive) => d.size > 128 * 1024 ** 3;
const sameDrive = (a: Drive, b: Drive) => a.number === b.number && a.serial === b.serial && a.size === b.size;

function readPref(key: string, fallback: string) {
  try {
    return localStorage.getItem(key) ?? fallback;
  } catch {
    return fallback;
  }
}
function writePref(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* preference only */
  }
}

export default function App() {
  const [view, setView] = useState<"flash" | "library">("flash");
  const [light, setLight] = useState(() => readPref("usboot.light", "ember"));
  const [admin, setAdmin] = useState(true);
  const [image, setImage] = useState<Image | null>(null);
  const [drives, setDrives] = useState<Drive[] | null>(null);
  const [drive, setDrive] = useState<Drive | null>(null);
  const [run, setRun] = useState<Run | null>(null);
  const [dragging, setDragging] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Windows mode extras (written as autounattend.xml), like Rufus's dialog.
  const [win, setWin] = useState({ bypass: false, local: false, user: "", privacy: false });
  const running = run?.kind === "running" || run?.kind === "restoring";

  const pick = async (path: string) => {
    try {
      setImage(await invoke<Image>("inspect_image", { path }));
      setError(null);
    } catch (e) {
      setError(errText(e));
    }
  };

  useEffect(() => writePref("usboot.light", light), [light]);
  useEffect(() => {
    invoke<boolean>("is_admin").then(setAdmin);
    const off = getCurrentWebview().onDragDropEvent((e) => {
      const p = e.payload;
      setDragging(p.type === "enter" || p.type === "over");
      if (p.type === "drop" && p.paths[0]) pick(p.paths[0]);
    });
    const offProgress = listen<Progress>("progress", (e) =>
      setRun((r) => (r?.kind !== "running" ? r : r.p?.phase === e.payload.phase ? { ...r, p: e.payload } : { ...r, p: e.payload, since: Date.now(), base: e.payload.done })),
    );
    return () => {
      off.then((f) => f());
      offProgress.then((f) => f());
    };
  }, []);

  // Hot-plug: re-list drives every few seconds, but never while writing.
  useEffect(() => {
    if (running) return;
    const load = () =>
      invoke<Drive[]>("list_drives")
        .then((list) => {
          setDrives(list);
          setDrive((d) => (d && list.find((x) => sameDrive(x, d))) || null);
        })
        .catch((e) => setError(errText(e)));
    load();
    const id = setInterval(load, 3000);
    return () => clearInterval(id);
  }, [running]);

  const chooseFile = async () => {
    const path = await open({ multiple: false, filters: [{ name: t.diskImage, extensions: ["iso", "img", "bin", "raw"] }] });
    if (typeof path === "string") pick(path);
  };

  const flash = async () => {
    if (!image || !drive) return;
    setRun({ kind: "running", p: null, since: Date.now(), base: 0, windows: image.windows });
    try {
      const options = { bypassRequirements: win.bypass, localAccount: win.local ? win.user : null, skipPrivacy: win.privacy };
      await invoke(image.windows ? "flash_windows" : "flash", { path: image.path, drive, ...(image.windows && { options }) });
      setRun({ kind: "done", message: t.doneBody, flashed: true });
    } catch (e) {
      setRun({ kind: "error", message: e === "cancelled" ? t.cancelled : errText(e) });
    }
  };

  const restore = async (fs: string, label: string) => {
    if (!drive) return;
    setRun({ kind: "restoring" });
    try {
      await invoke("restore_drive", { drive, fs, label });
      setRun({ kind: "done", message: t.restoredBody(FS_NAMES[fs]), flashed: false });
    } catch (e) {
      setRun({ kind: "error", message: errText(e) });
    }
  };

  const writing = run?.kind === "restoring" || (run?.kind === "running" && run.p?.phase !== "verify" && run.p?.phase !== "split");
  const lightRgb = writing ? DANGER : rgb(LIGHTS[light]);
  const fits = (d: Drive) => !image || image.size <= d.size;
  const ready = image && drive && fits(drive) && admin && !(image.windows && win.local && !win.user.trim());
  // Share of the drive the image will take: the bay map previews it, then fills it in.
  const reserve = image && drive ? Math.min(1, image.size / drive.size) : 0;
  const p = run?.kind === "running" ? run.p : null;
  const frac = p ? p.done / p.total : 0;
  const flashed = run?.kind === "done" && run.flashed;
  const verifyPhase = p?.phase === "verify" || p?.phase === "split";
  // Idle, the map is the whole drive with the image's share outlined; once writing starts it
  // zooms to the image itself so every cell lights up.
  const zoomed = run?.kind === "running" || flashed;
  const mapReserve = zoomed ? 1 : reserve;
  const written = !zoomed ? 0 : flashed || verifyPhase ? 1 : frac;
  const verified = flashed ? 1 : verifyPhase ? frac : 0;

  return (
    <div className="shell" style={{ "--light": writing ? "#ff453a" : LIGHTS[light] } as CSSProperties}>
      <Backdrop color={lightRgb} />
      <header className="top">
        <div className="brand">
          <img src={logo} alt="" />
          <span>USBoot</span>
        </div>
        <nav aria-label="Sections">
          <button className={view === "flash" ? "on" : ""} aria-current={view === "flash" ? "page" : undefined} onClick={() => setView("flash")}>
            <Zap size={16} />
            {t.navFlash}
          </button>
          <button className={view === "library" ? "on" : ""} aria-current={view === "library" ? "page" : undefined} onClick={() => setView("library")}>
            <LibraryIcon size={16} />
            {t.navLibrary}
          </button>
        </nav>
        <div className="top-right">
          <div className="swatches" role="radiogroup" aria-label={t.lightColor}>
            {Object.entries(LIGHTS).map(([id, hex]) => (
              <button
                key={id}
                role="radio"
                aria-checked={light === id}
                aria-label={t.lights[id]}
                title={t.lights[id]}
                className={light === id ? "on" : ""}
                style={{ "--sw": hex } as CSSProperties}
                onClick={() => setLight(id)}
              />
            ))}
          </div>
          <span className={`admin ${admin ? "" : "warn"}`}>
            {admin ? <ShieldCheck size={16} /> : <ShieldAlert size={16} />}
            {admin ? t.admin : t.limited}
          </span>
        </div>
      </header>
      <LightBar color={lightRgb} busy={running} />

      <main>
        <Library
          hidden={view !== "library"}
          onUse={(path) => {
            pick(path);
            setView("flash");
          }}
        />
        <section className="view scroll" hidden={view !== "flash"}>
          <div className="flash-grid">
            <div className="flash-steps" inert={running || undefined}>
              <div className="hero">
                <h1>{t.hero}</h1>
                <p className="lede">{t.lede}</p>
              </div>

              <h2 className={`section-title step-title ${image ? "done" : ""}`}>
                <span>{image ? <Check size={13} /> : 1}</span>
                {t.stepImage}
              </h2>
              {image ? (
                <>
                  <button className="drive lit wide" onClick={chooseFile} title={t.change}>
                    <Disc3 size={28} />
                    <span className="drive-body">
                      <b className="ellipsis">{image.name}</b>
                      <span className="tags">
                        <small>{fmtSize(image.size)}</small>
                        <span className={`tag ${image.bootable || image.windows ? "file" : ""}`}>
                          {image.windows ? "Windows" : image.bootable ? t.bootable : t.notBootable}
                        </span>
                      </span>
                    </span>
                  </button>
                  {(image.windows || !image.bootable) && (
                    <div className="notes">
                      <p className={`note ${image.windows ? "" : "warn"}`}>{image.windows ? t.windowsIso : t.notHybrid}</p>
                    </div>
                  )}
                  {image.windows && (
                    <fieldset className="win-opts">
                      <legend>{t.winOptions}</legend>
                      <label>
                        <input type="checkbox" className="check" checked={win.bypass} onChange={(e) => setWin({ ...win, bypass: e.target.checked })} />
                        <span>
                          {t.winBypass}
                          <small>{t.winBypassHint}</small>
                        </span>
                      </label>
                      <label>
                        <input type="checkbox" className="check" checked={win.local} onChange={(e) => setWin({ ...win, local: e.target.checked })} />
                        <span>
                          {t.winLocal}
                          <small>{t.winLocalHint}</small>
                        </span>
                      </label>
                      {win.local && (
                        <input
                          className="field"
                          aria-label={t.winUserName}
                          placeholder={t.winUserName}
                          maxLength={20}
                          value={win.user}
                          onChange={(e) => setWin({ ...win, user: e.target.value })}
                        />
                      )}
                      <label>
                        <input type="checkbox" className="check" checked={win.privacy} onChange={(e) => setWin({ ...win, privacy: e.target.checked })} />
                        <span>
                          {t.winPrivacy}
                          <small>{t.winPrivacyHint}</small>
                        </span>
                      </label>
                    </fieldset>
                  )}
                </>
              ) : (
                <button className={`drive pick wide ${dragging ? "over" : ""}`} onClick={chooseFile}>
                  <Disc3 size={28} />
                  <span className="drive-body">
                    <b>{t.chooseImage}</b>
                    <small>{t.chooseHint}</small>
                  </span>
                </button>
              )}

              <h2 className={`section-title step-title ${drive ? "done" : ""}`}>
                <span>{drive ? <Check size={13} /> : 2}</span>
                {t.stepDrive}
              </h2>
              {drives === null ? (
                <div className="busy">
                  <LoaderCircle className="spin" size={18} />
                </div>
              ) : drives.length === 0 ? (
                <div className="empty">
                  <Usb size={28} />
                  <p>{t.noDrives}</p>
                  <p className="hint">{t.noDrivesHint}</p>
                </div>
              ) : (
                <div className="drive-list">
                  {drives.map((d) => (
                    <button
                      key={`${d.number}-${d.serial}`}
                      className={`drive ${drive && sameDrive(drive, d) ? "lit" : ""}`}
                      aria-pressed={!!drive && sameDrive(drive, d)}
                      disabled={!fits(d)}
                      onClick={() => setDrive(d)}
                    >
                      <Usb size={26} />
                      <span className="drive-body">
                        <b className="ellipsis">{driveLabel(d)}</b>
                        <span className="tags">
                          <small>
                            {fmtSize(d.size)}
                            {!fits(d) && ` · ${t.tooSmall}`}
                          </small>
                          {isExternalDisk(d) && <span className="flag">{t.externalDisk}</span>}
                        </span>
                      </span>
                    </button>
                  ))}
                </div>
              )}
              {error && <p className="errors">{error}</p>}
            </div>

            {/* The drive bay: the selected drive as a live sector map. */}
            <aside className={`bay ${running ? "on" : ""}`} aria-live="polite">
              <div className="bay-head">
                <span className="bay-led" />
                <span className="ellipsis">{drive ? driveLabel(drive) : t.bayEmpty}</span>
                {drive && <b className="num">{fmtSize(drive.size)}</b>}
              </div>
              <SectorMap color={rgb(LIGHTS[light])} written={written} verified={verified} reserve={mapReserve} />
              {run?.kind === "running" ? (
                <RunningPanel run={run} />
              ) : (
                <>
                  <p className="bay-caption">
                    {flashed
                      ? t.bayDone
                      : image && drive
                        ? t.bayFits(fmtSize(image.size), fmtSize(drive.size), nf.format(Math.max(1, Math.round(reserve * 100))))
                        : t.bayHint}
                  </p>
                  <div className="bay-actions">
                    <button className="key danger big" disabled={!ready} onClick={() => setRun({ kind: "confirm" })}>
                      <Zap size={18} />
                      {t.flash}
                    </button>
                    <button className="key danger-ghost" disabled={!drive || !admin} onClick={() => setRun({ kind: "restore", fs: "exfat", label: "USB" })}>
                      <RotateCcw size={16} />
                      {t.restore}
                    </button>
                  </div>
                  <p className="hint">{t.eraseWarn}</p>
                </>
              )}
            </aside>
          </div>
        </section>
      </main>

      {run && drive && run.kind !== "running" && (
        <div className="overlay">
          <div className="sheet" role="dialog" aria-modal aria-labelledby="sheet-title">
            {run.kind === "confirm" && (
              <>
                <h2 id="sheet-title">
                  <TriangleAlert size={22} color="var(--danger)" />
                  {t.confirmTitle(drive.name)}
                </h2>
                <p className="note warn">{t.confirmBody(driveLabel(drive), fmtSize(drive.size))}</p>
                {isExternalDisk(drive) && <p className="errors">{t.externalWarn}</p>}
                <div className="footer-actions">
                  <button className="key" autoFocus onClick={() => setRun(null)}>
                    {t.cancel}
                  </button>
                  <button className="key danger" disabled={!image} onClick={flash}>
                    <Zap size={16} />
                    {t.eraseAndFlash}
                  </button>
                </div>
              </>
            )}
            {run.kind === "restore" && (
              <>
                <h2 id="sheet-title">
                  <TriangleAlert size={22} color="var(--danger)" />
                  {t.restoreTitle(drive.name)}
                </h2>
                <p className="note warn">{t.restoreBody(driveLabel(drive), fmtSize(drive.size))}</p>
                {isExternalDisk(drive) && <p className="errors">{t.externalWarn}</p>}
                <div className="toolbar">
                  <div className="segmented" role="radiogroup" aria-label={t.fileSystem}>
                    {Object.keys(FS_NAMES)
                      // FAT32 tops out at 32 GB in Windows' own formatter
                      .filter((fs) => fs !== "fat32" || drive.size <= 32 * 1024 ** 3)
                      .map((fs) => (
                        <button key={fs} role="radio" aria-checked={run.fs === fs} className={run.fs === fs ? "on" : ""} onClick={() => setRun({ ...run, fs })}>
                          {FS_NAMES[fs]}
                        </button>
                      ))}
                  </div>
                  <input
                    className="field"
                    aria-label={t.label}
                    placeholder={t.label}
                    maxLength={11}
                    value={run.label}
                    onChange={(e) => setRun({ ...run, label: e.target.value })}
                  />
                </div>
                <p className="hint">{t.fsHint[run.fs]}</p>
                <div className="footer-actions">
                  <button className="key" autoFocus onClick={() => setRun(null)}>
                    {t.cancel}
                  </button>
                  <button className="key danger" onClick={() => restore(run.fs, run.label)}>
                    <RotateCcw size={16} />
                    {t.eraseAndRestore}
                  </button>
                </div>
              </>
            )}
            {run.kind === "restoring" && (
              <div className="busy">
                <LoaderCircle className="spin" size={22} />
                <span id="sheet-title">{t.restoring}</span>
              </div>
            )}
            {(run.kind === "done" || run.kind === "error") && (
              <>
                <h2 id="sheet-title">{run.kind === "done" ? t.done : drive.name}</h2>
                {run.kind === "done" ? (
                  <p className="clean">
                    <Check size={18} />
                    {run.message}
                  </p>
                ) : (
                  <p className="errors">{run.message}</p>
                )}
                <div className="footer-actions">
                  <button className="key primary" autoFocus onClick={() => setRun(null)}>
                    {t.close}
                  </button>
                </div>
              </>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

function RunningPanel({ run }: { run: Running }) {
  const p = run.p;
  const pct = p ? (p.done / p.total) * 100 : 0;
  const secs = (Date.now() - run.since) / 1000;
  const speed = p && secs > 1 ? (p.done - run.base) / secs : 0;
  // Windows mode has its own steps; a raw image is write then verify.
  const phases: Progress["phase"][] = run.windows ? ["prepare", "copy", "split"] : ["write", "verify"];
  const at = Math.max(0, phases.indexOf(p?.phase ?? phases[0]));
  return (
    <div className="bay-run">
      <ol className="steps">
        {phases.map((ph, i) => (
          <li key={ph} className={i < at ? "done" : i === at ? "now" : ""}>
            <span>{i + 1}</span>
            {t.phases[ph]}
          </li>
        ))}
      </ol>
      <div className="stats">
        <b>{nf.format(Math.floor(pct))}%</b>
        <span>{speed > 0 && p ? `${fmtSize(speed)}/s · ${t.left(fmtTime((p.total - p.done) / speed))}` : t.phases[phases[at]]}</span>
      </div>
      <span className="meter big" role="progressbar" aria-label={t.phases[phases[at]]} aria-valuenow={Math.round(pct)} aria-valuemin={0} aria-valuemax={100}>
        <i style={{ width: `${pct}%` }} />
      </span>
      <button className="key danger-ghost" onClick={() => invoke("cancel_flash")}>
        {t.stop}
      </button>
    </div>
  );
}
