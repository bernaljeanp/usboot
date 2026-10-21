import { useEffect, useState } from "react";
import type { CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { ArrowDownToLine, Check, Disc3, Library as LibraryIcon, LoaderCircle, RotateCcw, ShieldAlert, ShieldCheck, TriangleAlert, Usb, Zap } from "lucide-react";
import Library from "./Library";
import SectorMap from "./SectorMap";
import LightBar, { Backdrop, type Rgb } from "./Shader";
import logo from "./assets/logo.png";
import { errText, fmtSize, nf, t } from "./i18n";
import "./App.css";

type Drive = { number: number; name: string; size: number; serial: string; letters: string[] };
type Image = { path: string; name: string; size: number; bootable: boolean; windows: boolean };
type Phase = "write" | "verify" | "prepare" | "copy" | "split";
type Progress = { drive: number; phase: Phase; done: number; total: number };
/** One entry per drive being flashed; several can run at once. */
type Running = { kind: "running"; per: Record<number, Progress>; since: number; windows: boolean; count: number };
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

// Windows mode has its own steps; a raw image is write then verify.
const phasesOf = (windows: boolean): Phase[] => (windows ? ["prepare", "copy", "split"] : ["write", "verify"]);

/** All drives folded into one: overall fraction, the slowest drive's step, and bytes processed. */
function overall(r: Running) {
  const phases = phasesOf(r.windows);
  const ps = Object.values(r.per);
  const step = (p: Progress) => Math.max(0, phases.indexOf(p.phase));
  const frac = ps.reduce((a, p) => a + (step(p) + (p.total ? p.done / p.total : 0)) / phases.length, 0) / r.count;
  const at = ps.length < r.count ? 0 : Math.min(...ps.map(step));
  const bytes = ps.reduce((a, p) => a + step(p) * p.total + p.done, 0);
  return { phases, frac, at, bytes, ps };
}

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
  // Several drives can be picked to flash the same image onto all of them at once.
  const [picked, setPicked] = useState<Drive[]>([]);
  const drive = picked[0] ?? null;
  const [run, setRun] = useState<Run | null>(null);
  const [dragging, setDragging] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Windows mode extras (written as autounattend.xml), like Rufus's dialog.
  const [win, setWin] = useState({ bypass: false, local: false, user: "", privacy: false });
  const running = run?.kind === "running" || run?.kind === "restoring";
  const [update, setUpdate] = useState<Update | null>(null);
  const [updating, setUpdating] = useState(false);

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
    // Quietly ask GitHub for a newer signed release; offline or dev builds just stay as they are.
    check().then(setUpdate).catch(() => {});
    const off = getCurrentWebview().onDragDropEvent((e) => {
      const p = e.payload;
      setDragging(p.type === "enter" || p.type === "over");
      if (p.type === "drop" && p.paths[0]) pick(p.paths[0]);
    });
    const offProgress = listen<Progress>("progress", (e) =>
      setRun((r) => (r?.kind !== "running" ? r : { ...r, per: { ...r.per, [e.payload.drive]: e.payload } })),
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
          setPicked((ps) => ps.map((d) => list.find((x) => sameDrive(x, d))).filter((d): d is Drive => !!d));
        })
        .catch((e) => setError(errText(e)));
    load();
    const id = setInterval(load, 3000);
    return () => clearInterval(id);
  }, [running]);

  // Windows mode prepares one drive at a time.
  useEffect(() => {
    if (image?.windows) setPicked((ps) => ps.slice(0, 1));
  }, [image]);
  const toggle = (d: Drive) =>
    setPicked((ps) => (ps.some((x) => sameDrive(x, d)) ? ps.filter((x) => !sameDrive(x, d)) : image?.windows ? [d] : [...ps, d]));

  const chooseFile = async () => {
    const path = await open({ multiple: false, filters: [{ name: t.diskImage, extensions: ["iso", "img", "bin", "raw"] }] });
    if (typeof path === "string") pick(path);
  };

  const flash = async () => {
    if (!image || !picked.length) return;
    const targets = image.windows ? picked.slice(0, 1) : picked;
    setRun({ kind: "running", per: {}, since: Date.now(), windows: image.windows, count: targets.length });
    try {
      if (image.windows) {
        const options = { bypassRequirements: win.bypass, localAccount: win.local ? win.user : null, skipPrivacy: win.privacy };
        await invoke("flash_windows", { path: image.path, drive: targets[0], options });
        setRun({ kind: "done", message: t.doneBody, flashed: true });
        return;
      }
      const results = await invoke<(string | null)[]>("flash", { path: image.path, drives: targets });
      const failed = targets.map((d, i) => [d, results[i]] as const).filter(([, r]) => r);
      if (!failed.length) setRun({ kind: "done", message: targets.length > 1 ? t.doneMany(targets.length) : t.doneBody, flashed: true });
      else if (failed.every(([, r]) => r === "cancelled")) setRun({ kind: "error", message: t.cancelled });
      else
        setRun({
          kind: "error",
          message: failed.map(([d, r]) => `${d.name}: ${r === "cancelled" ? t.cancelled : errText(r)}`).join("\n"),
        });
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

  const ov = run?.kind === "running" ? overall(run) : null;
  const writing = run?.kind === "restoring" || (!!ov && !["verify", "split"].includes(ov.phases[ov.at]));
  const lightRgb = writing ? DANGER : rgb(LIGHTS[light]);
  const fits = (d: Drive) => !image || image.size <= d.size;
  const ready = image && picked.length && picked.every(fits) && admin && !(image.windows && win.local && !win.user.trim());
  // Share of the drive the image will take: the bay map previews it, then fills it in.
  const reserve = image && drive ? Math.min(1, image.size / drive.size) : 0;
  const frac = ov ? ov.frac : 0;
  const flashed = run?.kind === "done" && run.flashed;
  const raw = run?.kind === "running" && !run.windows;
  // Idle, the map is the whole drive with the image's share outlined; once writing starts it
  // zooms to the image itself so every cell lights up. A raw flash is half writing, half verifying.
  const zoomed = run?.kind === "running" || flashed;
  const mapReserve = zoomed ? 1 : reserve;
  const written = !zoomed ? 0 : flashed ? 1 : raw ? Math.min(1, frac * 2) : frac;
  const verified = flashed ? 1 : raw ? Math.max(0, frac * 2 - 1) : 0;

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
          {update && (
            <button
              className="key primary small"
              disabled={updating || running}
              onClick={async () => {
                setUpdating(true);
                try {
                  await update.downloadAndInstall();
                  await relaunch();
                } catch (e) {
                  setError(errText(e));
                  setUpdating(false);
                }
              }}
            >
              {updating ? <LoaderCircle className="spin" size={14} /> : <ArrowDownToLine size={14} />}
              {updating ? t.updating : t.updateTo(update.version)}
            </button>
          )}
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
                      className={`drive ${picked.some((x) => sameDrive(x, d)) ? "lit" : ""}`}
                      aria-pressed={picked.some((x) => sameDrive(x, d))}
                      disabled={!fits(d)}
                      onClick={() => toggle(d)}
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
                <span className="ellipsis">{picked.length > 1 ? t.drivesMany(picked.length) : drive ? driveLabel(drive) : t.bayEmpty}</span>
                {picked.length === 1 && <b className="num">{fmtSize(drive!.size)}</b>}
              </div>
              <SectorMap color={rgb(LIGHTS[light])} written={written} verified={verified} reserve={mapReserve} />
              {run?.kind === "running" ? (
                <RunningPanel run={run} drives={picked} />
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
                    <button className="key danger-ghost" disabled={picked.length !== 1 || !admin} onClick={() => setRun({ kind: "restore", fs: "exfat", label: "USB" })}>
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
                  {picked.length > 1 ? t.confirmTitleMany(picked.length) : t.confirmTitle(drive.name)}
                </h2>
                <p className="note warn">
                  {t.confirmBody(picked.map(driveLabel).join(", "), fmtSize(picked.reduce((a, d) => a + d.size, 0)))}
                </p>
                {picked.some(isExternalDisk) && <p className="errors">{t.externalWarn}</p>}
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

function RunningPanel({ run, drives }: { run: Running; drives: Drive[] }) {
  const { phases, frac, at, bytes, ps } = overall(run);
  const pct = frac * 100;
  const secs = (Date.now() - run.since) / 1000;
  const speed = secs > 1 ? bytes / secs : 0;
  const left = frac > 0.005 ? (secs * (1 - frac)) / frac : 0;
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
        <span>{speed > 0 && left > 0 ? `${fmtSize(speed)}/s · ${t.left(fmtTime(left))}` : t.phases[phases[at]]}</span>
      </div>
      <span className="meter big" role="progressbar" aria-label={t.phases[phases[at]]} aria-valuenow={Math.round(pct)} aria-valuemin={0} aria-valuemax={100}>
        <i style={{ width: `${pct}%` }} />
      </span>
      {run.count > 1 && (
        <ul className="per-drive">
          {drives.slice(0, run.count).map((d) => {
            const p = ps.find((x) => x.drive === d.number);
            const f = p ? (Math.max(0, phases.indexOf(p.phase)) + (p.total ? p.done / p.total : 0)) / phases.length : 0;
            return (
              <li key={d.number}>
                <span className="ellipsis">{d.name}</span>
                <span className="meter">
                  <i style={{ width: `${f * 100}%` }} />
                </span>
                <b className="num">{nf.format(Math.floor(f * 100))}%</b>
              </li>
            );
          })}
        </ul>
      )}
      <button className="key danger-ghost" onClick={() => invoke("cancel_flash")}>
        {t.stop}
      </button>
    </div>
  );
}
