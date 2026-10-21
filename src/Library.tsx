import { useEffect, useRef, useState } from "react";
import type { CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Download, ExternalLink, LoaderCircle, X, Zap } from "lucide-react";
import {
  siAlmalinux, siArchlinux, siCachyos, siDebian, siFedora, siKalilinux, siKubuntu, siLinuxmint, siOpensuse,
  siParrotsecurity, siPopos, siProxmox, siRockylinux, siTails, siUbuntu, siXubuntu, siZorin,
} from "simple-icons";
import { errText, fmtSize, nf, t } from "./i18n";

type Release = { version: string; file: string; url: string; sha256: string; size: number };
type Entry = {
  id: string; name: string; desc: string; release: Release | null; error: string | null;
  page: string | null; local: string | null; outdated: string | null;
};
type Dl = { phase: "connect" | "download" | "verify"; done: number; total: number; sources: number; speed: number };

// Bazzite's own mark from its press kit (ublue-os/bazzite, Apache-2.0): the "B" plus its d-pad outline.
const BAZZITE = `<path fill-rule="evenodd" d="M 150,50 C 94.7715,50 50,94.7715 50,150 v 92 58 C 50,438.071 161.929,550 300,550 438.071,550 550,438.071 550,300 550,161.929 438.071,50 300,50 H 242 V 178 H 376.185 C 432.416,178 478,223.584 478,279.815 478,389.269 389.27,478 279.815,478 223.584,478 178,432.416 178,376.185 V 242 H 50 V 178 H 178 V 50 Z m 92,326.185 V 242 H 376.185 C 397.07,242 414,258.93 414,279.815 414,353.923 353.923,414 279.815,414 258.93,414 242,397.07 242,376.185 Z"/><path d="m 178,65 h -15 v 15 83 H 80 65 v 15 64 15 h 15 83 v 83 15 h 15 64 15 v -15 -83 h 83 15 V 242 178 163 H 340 257 V 80 65 h -15 z" fill="none" stroke="currentColor" stroke-width="30"/>`;

// Official marks from Simple Icons (CC0). Windows isn't in it: four panes drawn plainly.
const LOGOS: Record<string, { path?: string; svg?: string; viewBox?: string; hex: string }> = {
  zorin: siZorin, cachyos: siCachyos, parrot: siParrotsecurity,
  bazzite: { svg: BAZZITE, viewBox: "0 0 600 600", hex: "8A5CF6" },
  ubuntu: siUbuntu, "ubuntu-server": siUbuntu, kubuntu: siKubuntu, xubuntu: siXubuntu, mint: siLinuxmint,
  popos: siPopos, fedora: siFedora, debian: siDebian, arch: siArchlinux, opensuse: siOpensuse,
  rocky: siRockylinux, alma: siAlmalinux, kali: siKalilinux, tails: siTails, proxmox: siProxmox,
  windows11: { path: "M3 3h8.4v8.4H3zm9.6 0H21v8.4h-8.4zM3 12.6h8.4V21H3zm9.6 0H21V21h-8.4z", hex: "3BA7F0" },
};

/** Brand colour, lifted when it's too dark to glow on graphite (AlmaLinux is pure black). */
function glowColor(hex: string) {
  const [r, g, b] = [0, 2, 4].map((i) => parseInt(hex.slice(i, i + 2), 16));
  const lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
  return lum < 70 ? `color-mix(in srgb, #${hex} 45%, #e8eaed)` : `#${hex}`;
}

function DistroLogo({ id, name }: { id: string; name: string }) {
  const logo = LOGOS[id];
  const color = logo ? glowColor(logo.hex) : "var(--light)";
  return (
    <span className="distro-logo" style={{ "--brand": color } as CSSProperties} aria-hidden>
      {logo ? (
        logo.svg ? (
          <svg viewBox={logo.viewBox} dangerouslySetInnerHTML={{ __html: logo.svg }} />
        ) : (
          <svg viewBox="0 0 24 24">
            <path d={logo.path} />
          </svg>
        )
      ) : (
        <b>{name.slice(0, 2)}</b>
      )}
    </span>
  );
}

export default function Library({ hidden, onUse }: { hidden: boolean; onUse: (path: string) => void }) {
  const [entries, setEntries] = useState<Entry[] | null>(null);
  const [dls, setDls] = useState<Record<string, Dl>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const samples = useRef<Record<string, { at: number; done: number }>>({});

  const load = () => invoke<Entry[]>("library").then(setEntries);
  useEffect(() => {
    load();
    const off = listen<Omit<Dl, "speed"> & { id: string }>("download", ({ payload: p }) => {
      // Smoothed speed from the byte counter, per download.
      const now = performance.now();
      const prev = samples.current[p.id];
      setDls((d) => {
        const old = d[p.id];
        let speed = old?.speed ?? 0;
        if (prev && p.phase === old?.phase && now > prev.at) {
          const inst = ((p.done - prev.done) * 1000) / (now - prev.at);
          speed = speed ? speed * 0.7 + inst * 0.3 : inst;
        } else if (p.phase !== old?.phase) speed = 0;
        return { ...d, [p.id]: { ...p, speed } };
      });
      samples.current[p.id] = { at: now, done: p.done };
    });
    return () => {
      off.then((f) => f());
    };
  }, []);

  const download = async (id: string) => {
    setErrors(({ [id]: _, ...rest }) => rest);
    setDls((d) => ({ ...d, [id]: { phase: "connect", done: 0, total: 0, sources: 0, speed: 0 } }));
    delete samples.current[id];
    try {
      await invoke<string>("download_iso", { id });
    } catch (e) {
      if (e !== "cancelled") setErrors((x) => ({ ...x, [id]: errText(e) }));
    }
    setDls(({ [id]: _, ...rest }) => rest);
    load();
  };

  return (
    <section className="view scroll" hidden={hidden}>
      <div className="hero">
        <h1>{t.libraryTitle}</h1>
        <p className="lede">{t.libraryLede}</p>
      </div>
      {entries === null ? (
        <div className="busy">
          <LoaderCircle className="spin" size={18} />
          {t.searching}
        </div>
      ) : (
        <div className="distros">
          {entries.map((e) => {
            const dl = dls[e.id];
            const pct = dl?.total ? (dl.done / dl.total) * 100 : 0;
            return (
              <article key={e.id} className={`distro ${dl ? "busy-card" : ""} ${e.local ? "ready" : ""}`}>
                <header>
                  <DistroLogo id={e.id} name={e.name} />
                  <span className="distro-name">
                    <strong>
                      {e.name}
                      {e.outdated && !e.local && <em className="flag">{t.newVersion}</em>}
                    </strong>
                    <small>
                      {t.descs[e.desc] ?? e.desc}
                      {e.release && ` · ${e.release.version === "latest" ? t.latest : e.release.version}`}
                    </small>
                  </span>
                </header>

                {dl ? (
                  <div className="dl">
                    <span className="meter" role="progressbar" aria-label={e.name} aria-valuenow={Math.round(pct)} aria-valuemin={0} aria-valuemax={100}>
                      <i style={{ width: `${dl.phase === "connect" ? 0 : pct}%` }} />
                    </span>
                    <span className="dl-line">
                      <span className="dim">
                        {dl.phase === "connect"
                          ? t.findingMirrors
                          : dl.phase === "verify"
                            ? t.checkingSha
                            : `${fmtSize(dl.speed)}/s · ${t.mirrors(dl.sources)}`}
                      </span>
                      {dl.phase !== "connect" && <b className="num">{nf.format(Math.floor(pct))}%</b>}
                      <button className="icon-btn danger" aria-label={t.stop} title={t.stop} onClick={() => invoke("cancel_download", { id: e.id })}>
                        <X size={15} />
                      </button>
                    </span>
                  </div>
                ) : (
                  <footer>
                    <span className="dim num">{e.page ? t.officialSite : e.release?.size ? fmtSize(e.release.size) : "—"}</span>
                    {e.page ? (
                      <button className="key small" onClick={() => openUrl(e.page!)}>
                        <ExternalLink size={14} />
                        {t.openMicrosoft}
                      </button>
                    ) : e.local ? (
                      <button className="key primary small" onClick={() => onUse(e.local!)}>
                        <Zap size={14} />
                        {t.use}
                      </button>
                    ) : e.release ? (
                      <button className="key small" onClick={() => download(e.id)}>
                        <Download size={14} />
                        {t.download}
                      </button>
                    ) : (
                      <small className="dim" title={e.error ? errText(e.error) : ""}>
                        {t.unavailable}
                      </small>
                    )}
                  </footer>
                )}
                {e.page && <p className="hint">{t.windowsNote}</p>}
                {errors[e.id] && <p className="errors">{errors[e.id]}</p>}
              </article>
            );
          })}
        </div>
      )}
      <p className="hint">{t.libraryHint}</p>
    </section>
  );
}
