// Points the big download button at the installer for the visitor's OS, read from the latest GitHub release.
(async () => {
  const btn = document.getElementById("download");
  if (!btn) return;
  const ua = navigator.userAgent;
  const os = /Windows/.test(ua) ? "win" : /Mac OS X|Macintosh/.test(ua) ? "mac" : /Linux|X11/.test(ua) ? "linux" : "win";
  const pick = { win: /_x64-setup\.exe$/, mac: /macos.*\.dmg$/, linux: /linux.*\.AppImage$/ };
  const label = btn.dataset[os];
  if (label) btn.querySelector("span").textContent = label;
  try {
    const r = await fetch("https://api.github.com/repos/bernaljeanp/usboot/releases/latest");
    const rel = await r.json();
    const asset = rel.assets?.find((a) => pick[os].test(a.name));
    if (asset) btn.href = asset.browser_download_url;
    const v = document.getElementById("version");
    if (v && rel.tag_name) v.textContent = rel.tag_name;
  } catch {
    /* offline or rate-limited: the button still opens the releases page */
  }
})();
