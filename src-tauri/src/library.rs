//! ISO library: finds each distro's newest official ISO from its own checksum file (or API),
//! downloads it from the fastest mirrors in parallel to Downloads\USBoot and only keeps it
//! if the SHA-256 matches.
use regex::Regex;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Write};
use std::os::windows::fs::FileExt;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

struct Distro {
    id: &'static str,
    name: &'static str,
    desc: &'static str,
    source: Source,
    /// (official prefix, mirror prefix): the same file is served under each mirror prefix.
    mirrors: &'static [(&'static str, &'static str)],
}

enum Source {
    Sums {
        /// Listing page + pattern whose highest capture becomes `{v}` in `dir`.
        index: Option<(&'static str, &'static str)>,
        dir: &'static str,
        sums: Sums,
        /// Full file name; capture 1 (if any) is the version used to pick the newest.
        iso: &'static str,
        /// Download this name instead (a "Current" alias) while keeping the real file name.
        fetch: Option<&'static str>,
    },
    /// The project publishes a JSON manifest instead of a checksum file.
    Api(fn() -> Result<Release, String>),
    /// No direct download allowed (Windows): open the vendor's own page.
    Page(&'static str),
}

enum Sums {
    File(&'static str),
    /// The checksum file name changes per build: find it in the directory listing.
    Find(&'static str),
}

const UBUNTU_MIRRORS: &[(&str, &str)] = &[
    ("https://releases.ubuntu.com/", "https://mirrors.kernel.org/ubuntu-releases/"),
    ("https://releases.ubuntu.com/", "https://mirror.us.leaseweb.net/ubuntu-releases/"),
    ("https://releases.ubuntu.com/", "https://ftp.halifax.rwth-aachen.de/ubuntu-releases/"),
];

const CATALOG: &[Distro] = &[
    Distro {
        id: "ubuntu",
        name: "Ubuntu",
        desc: "Desktop LTS",
        source: Source::Sums {
            index: Some(("https://releases.ubuntu.com/", r#"href="(\d\d\.04)/""#)),
            dir: "https://releases.ubuntu.com/{v}/",
            sums: Sums::File("SHA256SUMS"),
            iso: r"ubuntu-([\d.]+)-desktop-amd64\.iso",
            fetch: None,
        },
        mirrors: UBUNTU_MIRRORS,
    },
    Distro {
        id: "ubuntu-server",
        name: "Ubuntu Server",
        desc: "LTS",
        source: Source::Sums {
            index: Some(("https://releases.ubuntu.com/", r#"href="(\d\d\.04)/""#)),
            dir: "https://releases.ubuntu.com/{v}/",
            sums: Sums::File("SHA256SUMS"),
            iso: r"ubuntu-([\d.]+)-live-server-amd64\.iso",
            fetch: None,
        },
        mirrors: UBUNTU_MIRRORS,
    },
    Distro {
        id: "kubuntu",
        name: "Kubuntu",
        desc: "KDE Plasma LTS",
        source: Source::Sums {
            index: Some(("https://cdimage.ubuntu.com/kubuntu/releases/", r#"href="(\d\d\.04)/""#)),
            dir: "https://cdimage.ubuntu.com/kubuntu/releases/{v}/release/",
            sums: Sums::File("SHA256SUMS"),
            iso: r"kubuntu-([\d.]+)-desktop-amd64\.iso",
            fetch: None,
        },
        mirrors: &[],
    },
    Distro {
        id: "xubuntu",
        name: "Xubuntu",
        desc: "Xfce LTS",
        source: Source::Sums {
            index: Some(("https://cdimage.ubuntu.com/xubuntu/releases/", r#"href="(\d\d\.04)/""#)),
            dir: "https://cdimage.ubuntu.com/xubuntu/releases/{v}/release/",
            sums: Sums::File("SHA256SUMS"),
            iso: r"xubuntu-([\d.]+)-desktop-amd64\.iso",
            fetch: None,
        },
        mirrors: &[],
    },
    Distro {
        id: "mint",
        name: "Linux Mint",
        desc: "Cinnamon",
        source: Source::Sums {
            index: Some(("https://mirrors.kernel.org/linuxmint/stable/", r#"href="(\d+\.\d+)/""#)),
            dir: "https://mirrors.kernel.org/linuxmint/stable/{v}/",
            sums: Sums::File("sha256sum.txt"),
            iso: r"linuxmint-([\d.]+)-cinnamon-64bit\.iso",
            fetch: None,
        },
        mirrors: &[
            ("https://mirrors.kernel.org/linuxmint/", "https://mirrors.edge.kernel.org/linuxmint/"),
            ("https://mirrors.kernel.org/linuxmint/", "https://mirror.csclub.uwaterloo.ca/linuxmint/"),
        ],
    },
    Distro {
        id: "zorin",
        name: "Zorin OS",
        desc: "Core",
        source: Source::Sums {
            index: Some(("https://mirrors.edge.kernel.org/zorinos-isos/", r#"href="(\d+)/""#)),
            dir: "https://mirrors.edge.kernel.org/zorinos-isos/{v}/",
            sums: Sums::File("SHA256SUMS.txt"),
            // Re-spins add "-r3": 18 r3 is older than 18.1, so the revision only breaks ties.
            iso: r"Zorin-OS-([\d.]+)-Core-64-bit(?:-r(\d+))?\.iso",
            fetch: None,
        },
        mirrors: &[("https://mirrors.edge.kernel.org/", "https://mirrors.kernel.org/")],
    },
    Distro {
        id: "popos",
        name: "Pop!_OS",
        desc: "Intel/AMD",
        source: Source::Api(popos),
        mirrors: &[],
    },
    Distro {
        id: "fedora",
        name: "Fedora",
        desc: "Workstation",
        source: Source::Sums {
            index: Some(("https://dl.fedoraproject.org/pub/fedora/linux/releases/", r#"href="(\d+)/""#)),
            dir: "https://dl.fedoraproject.org/pub/fedora/linux/releases/{v}/Workstation/x86_64/iso/",
            sums: Sums::Find(r"Fedora-Workstation-[\d.-]+-x86_64-CHECKSUM"),
            iso: r"Fedora-Workstation-Live-([\d.-]+)\.x86_64\.iso",
            fetch: None,
        },
        mirrors: &[
            ("https://dl.fedoraproject.org/pub/fedora/linux/", "https://mirrors.kernel.org/fedora/"),
            ("https://dl.fedoraproject.org/pub/fedora/linux/", "https://mirror.csclub.uwaterloo.ca/fedora/linux/"),
        ],
    },
    Distro {
        id: "cachyos",
        name: "CachyOS",
        desc: "Desktop",
        source: Source::Sums {
            index: Some(("https://mirror.cachyos.org/ISO/desktop/", r#"href="(\d{6})/""#)),
            dir: "https://mirror.cachyos.org/ISO/desktop/{v}/",
            sums: Sums::Find(r"cachyos-desktop-linux-\d+\.iso\.sha256"),
            iso: r"cachyos-desktop-linux-(\d+)\.iso",
            fetch: None,
        },
        mirrors: &[],
    },
    Distro {
        id: "bazzite",
        name: "Bazzite",
        desc: "Gaming · Steam",
        source: Source::Sums {
            index: None,
            dir: "https://download.bazzite.gg/",
            sums: Sums::File("bazzite-stable-amd64.iso-CHECKSUM"),
            iso: r"bazzite-stable-amd64\.iso",
            fetch: None,
        },
        mirrors: &[],
    },
    Distro {
        id: "debian",
        name: "Debian",
        desc: "Live GNOME",
        source: Source::Sums {
            index: None,
            dir: "https://cdimage.debian.org/debian-cd/current-live/amd64/iso-hybrid/",
            sums: Sums::File("SHA256SUMS"),
            iso: r"debian-live-([\d.]+)-amd64-gnome\.iso",
            fetch: None,
        },
        mirrors: &[
            ("https://cdimage.debian.org/debian-cd/", "https://mirrors.kernel.org/debian-cd/"),
            ("https://cdimage.debian.org/debian-cd/", "https://ftp.acc.umu.se/debian-cd/"),
        ],
    },
    Distro {
        id: "arch",
        name: "Arch Linux",
        desc: "Monthly",
        source: Source::Sums {
            index: None,
            dir: "https://geo.mirror.pkgbuild.com/iso/latest/",
            sums: Sums::File("sha256sums.txt"),
            iso: r"archlinux-(\d{4}\.\d{2}\.\d{2})-x86_64\.iso",
            fetch: None,
        },
        mirrors: &[
            ("https://geo.mirror.pkgbuild.com/", "https://fastly.mirror.pkgbuild.com/"),
            ("https://geo.mirror.pkgbuild.com/", "https://mirrors.kernel.org/archlinux/"),
            ("https://geo.mirror.pkgbuild.com/", "https://mirror.rackspace.com/archlinux/"),
        ],
    },
    Distro {
        id: "opensuse",
        name: "openSUSE",
        desc: "Tumbleweed",
        source: Source::Sums {
            index: None,
            dir: "https://download.opensuse.org/tumbleweed/iso/",
            sums: Sums::File("openSUSE-Tumbleweed-DVD-x86_64-Current.iso.sha256"),
            iso: r"openSUSE-Tumbleweed-DVD-x86_64-Snapshot(\d+)-Media\.iso",
            fetch: Some("openSUSE-Tumbleweed-DVD-x86_64-Current.iso"),
        },
        mirrors: &[],
    },
    Distro {
        id: "rocky",
        name: "Rocky Linux",
        desc: "DVD",
        source: Source::Sums {
            index: Some(("https://download.rockylinux.org/pub/rocky/", r#"href="(\d+)/""#)),
            dir: "https://download.rockylinux.org/pub/rocky/{v}/isos/x86_64/",
            sums: Sums::File("CHECKSUM"),
            iso: r"Rocky-([\d.]+)-x86_64-dvd1\.iso",
            fetch: None,
        },
        mirrors: &[],
    },
    Distro {
        id: "alma",
        name: "AlmaLinux",
        desc: "DVD",
        source: Source::Sums {
            index: Some(("https://repo.almalinux.org/almalinux/", r#"href="(\d+)/""#)),
            dir: "https://repo.almalinux.org/almalinux/{v}/isos/x86_64/",
            sums: Sums::File("CHECKSUM"),
            iso: r"AlmaLinux-([\d.]+)-x86_64-dvd\.iso",
            fetch: None,
        },
        mirrors: &[],
    },
    Distro {
        id: "kali",
        name: "Kali Linux",
        desc: "Installer",
        source: Source::Sums {
            index: None,
            dir: "https://cdimage.kali.org/current/",
            sums: Sums::File("SHA256SUMS"),
            iso: r"kali-linux-([\d.]+)-installer-amd64\.iso",
            fetch: None,
        },
        mirrors: &[
            ("https://cdimage.kali.org/", "https://kali.download/base-images/"),
            ("https://cdimage.kali.org/", "https://mirrors.ocf.berkeley.edu/kali-images/"),
        ],
    },
    Distro {
        id: "parrot",
        name: "Parrot OS",
        desc: "Security",
        source: Source::Sums {
            index: Some(("https://deb.parrot.sh/parrot/iso/", r#"href="(\d+\.\d+)/""#)),
            dir: "https://deb.parrot.sh/parrot/iso/{v}/",
            // md5, sha256 and sha512 lines all in one file; only the 64-hex ones parse as SHA-256.
            sums: Sums::File("signed-hashes.txt"),
            iso: r"Parrot-security-([\d.]+)_amd64\.iso",
            fetch: None,
        },
        mirrors: &[],
    },
    Distro {
        id: "tails",
        name: "Tails",
        desc: "Privacy · USB image",
        source: Source::Api(tails),
        mirrors: &[],
    },
    Distro {
        id: "proxmox",
        name: "Proxmox VE",
        desc: "Hypervisor",
        source: Source::Sums {
            index: None,
            dir: "https://enterprise.proxmox.com/iso/",
            sums: Sums::File("SHA256SUMS"),
            iso: r"proxmox-ve_([\d.]+-\d+)\.iso",
            fetch: None,
        },
        mirrors: &[],
    },
    Distro {
        id: "windows11",
        name: "Windows 11",
        desc: "Microsoft",
        source: Source::Page("https://www.microsoft.com/software-download/windows11"),
        mirrors: &[],
    },
];

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub version: String,
    pub file: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: &'static str,
    pub name: &'static str,
    pub desc: &'static str,
    pub release: Option<Release>,
    pub error: Option<String>,
    /// Vendor page to open instead of downloading (Windows).
    pub page: Option<&'static str>,
    /// Path of the newest ISO if it's already downloaded and verified.
    pub local: Option<String>,
    /// An older ISO of this distro sits in the folder.
    pub outdated: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub id: String,
    pub phase: &'static str,
    pub done: u64,
    pub total: u64,
    /// How many mirrors are feeding the download right now.
    pub sources: usize,
}

static CANCELLED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .user_agent("USBoot (https://github.com/bernaljeanp/usboot)")
        .build()
}

fn get_text(url: &str) -> Result<String, String> {
    agent().get(url).call().map_err(|e| e.to_string())?.into_string().map_err(|e| e.to_string())
}

/// "26.04.1" -> [26, 4, 1], so versions compare as numbers, not text.
fn version_key(v: &str) -> Vec<u64> {
    v.split(|c: char| !c.is_ascii_digit()).filter_map(|n| n.parse().ok()).collect()
}

fn full(pattern: &str) -> Regex {
    Regex::new(&format!("^{pattern}$")).expect("catalog pattern")
}

/// Both checksum formats: GNU `hash *file` and BSD `SHA256 (file) = hash`.
fn parse_sums(text: &str) -> Vec<(String, String)> {
    let gnu = Regex::new(r"^([0-9a-fA-F]{64}) [ *]?(\S+)$").unwrap();
    let bsd = Regex::new(r"^SHA256 \((.+)\) = ([0-9a-fA-F]{64})$").unwrap();
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            gnu.captures(l)
                .map(|c| (c[2].to_string(), c[1].to_lowercase()))
                .or_else(|| bsd.captures(l).map(|c| (c[1].to_string(), c[2].to_lowercase())))
        })
        .collect()
}

/// Newest file in `sums` whose name fully matches `iso`. Capture 1 is the version; an optional
/// capture 2 is a re-spin number that only breaks ties ("18 r3" < "18.1").
fn newest(sums: &[(String, String)], iso: &Regex) -> Option<(String, String, String)> {
    sums.iter()
        .filter_map(|(file, hash)| {
            let c = iso.captures(file)?;
            let version = c.get(1).map_or("latest", |m| m.as_str());
            let respin = c.get(2).map(|m| m.as_str());
            let mut key = version_key(version);
            key.resize(4, 0);
            key.extend(respin.map(version_key).unwrap_or_default());
            let shown = respin.map_or(version.to_string(), |r| format!("{version} r{r}"));
            Some((key, shown, file.clone(), hash.clone()))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, v, f, h)| (v, f, h))
}

fn json(url: &str) -> Result<serde_json::Value, String> {
    serde_json::from_str(&get_text(url)?).map_err(|e| e.to_string())
}

fn popos() -> Result<Release, String> {
    let j = json("https://api.pop-os.org/builds/24.04/intel")?;
    let url = j["url"].as_str().ok_or("no url")?.to_string();
    Ok(Release {
        version: format!("{} build {}", j["version"].as_str().unwrap_or("?"), j["build"].as_str().unwrap_or("?")),
        file: url.rsplit('/').next().unwrap_or_default().to_string(),
        sha256: j["sha_sum"].as_str().ok_or("no sha")?.to_lowercase(),
        size: j["size"].as_u64().unwrap_or(0),
        url,
    })
}

fn tails() -> Result<Release, String> {
    let j = json("https://tails.net/install/v2/Tails/amd64/stable/latest.json")?;
    let img = j["installations"][0]["installation-paths"]
        .as_array()
        .and_then(|paths| paths.iter().find(|p| p["type"] == "img"))
        .map(|p| &p["target-files"][0])
        .ok_or("no img")?;
    let url = img["url"].as_str().ok_or("no url")?.to_string();
    let file = url.rsplit('/').next().unwrap_or_default().to_string();
    Ok(Release {
        version: Regex::new(r"(\d+\.\d+(\.\d+)?)").unwrap().find(&file).map_or("?", |m| m.as_str()).to_string(),
        sha256: img["sha256"].as_str().ok_or("no sha")?.to_lowercase(),
        size: img["size"].as_u64().unwrap_or(0),
        file,
        url,
    })
}

fn resolve(d: &Distro) -> Result<Release, String> {
    let (index, dir, sums, iso, fetch) = match &d.source {
        Source::Sums { index, dir, sums, iso, fetch } => (index, dir, sums, iso, fetch),
        Source::Api(f) => return f(),
        Source::Page(_) => return Err("page".into()),
    };
    let dir = match index {
        Some((url, pattern)) => {
            let re = Regex::new(pattern).unwrap();
            let page = get_text(url)?;
            let v = re.captures_iter(&page).map(|c| c[1].to_string()).max_by_key(|v| version_key(v)).ok_or("No releases listed")?;
            dir.replace("{v}", &v)
        }
        None => dir.to_string(),
    };
    let sums_name = match sums {
        Sums::File(name) => name.to_string(),
        Sums::Find(pattern) => Regex::new(pattern).unwrap().find(&get_text(&dir)?).ok_or("No checksum file")?.as_str().to_string(),
    };
    let (version, file, sha256) = newest(&parse_sums(&get_text(&format!("{dir}{sums_name}"))?), &full(iso)).ok_or("No ISO in the checksum file")?;
    let url = format!("{dir}{}", fetch.unwrap_or(&file));
    // Checksum files sometimes list ISOs that aren't uploaded yet: make sure it's really there.
    let head = agent().head(&url).call().map_err(|e| format!("notUploaded|{file} ({e})"))?;
    let size = head.header("content-length").and_then(|s| s.parse().ok()).unwrap_or(0);
    Ok(Release { version, file, url, sha256, size })
}

pub fn folder() -> PathBuf {
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    home.join("Downloads").join("USBoot")
}

/// The hash we verified when the file finished downloading, kept next to it.
fn verified_hash(file: &str) -> Option<String> {
    std::fs::read_to_string(folder().join(format!("{file}.sha256"))).ok().map(|s| s.trim().to_string())
}

pub fn list() -> Vec<Entry> {
    let files: Vec<String> = std::fs::read_dir(folder())
        .map(|it| it.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    std::thread::scope(|s| {
        let jobs: Vec<_> = CATALOG.iter().map(|d| (d, s.spawn(|| resolve(d)))).collect();
        jobs.into_iter()
            .map(|(d, job)| {
                let page = match d.source {
                    Source::Page(url) => Some(url),
                    _ => None,
                };
                let result = if page.is_some() { Err(String::new()) } else { job.join().unwrap_or_else(|_| Err("crashed".into())) };
                let (mut local, mut outdated) = (None, None);
                if let Ok(r) = &result {
                    if files.contains(&r.file) && verified_hash(&r.file).as_deref() == Some(r.sha256.as_str()) {
                        local = Some(folder().join(&r.file).to_string_lossy().into_owned());
                    } else if files.contains(&r.file) {
                        // Same name, different content (e.g. Bazzite's "stable" ISO): it's an older build.
                        outdated = Some(r.file.clone());
                    } else if let Source::Sums { iso, .. } = &d.source {
                        let iso = full(iso);
                        outdated = files.iter().find(|f| iso.is_match(f)).cloned();
                    }
                }
                let (release, error) = match result {
                    Ok(r) => (Some(r), None),
                    Err(e) if page.is_some() => (None, Some(e).filter(|e| !e.is_empty())),
                    Err(e) => (None, Some(e)),
                };
                Entry { id: d.id, name: d.name, desc: d.desc, release, error, page, local, outdated }
            })
            .collect()
    })
}

pub fn cancel(id: &str) {
    CANCELLED.lock().unwrap().get_or_insert_with(HashSet::new).insert(id.to_string());
}

fn take_cancel(id: &str) -> bool {
    CANCELLED.lock().unwrap().as_mut().is_some_and(|s| s.remove(id))
}

fn is_cancelled(id: &str) -> bool {
    CANCELLED.lock().unwrap().as_ref().is_some_and(|s| s.contains(id))
}

const SEGMENT: u64 = 8 << 20;
const PROBE: u64 = 1 << 20;
const PER_MIRROR: usize = 3;

/// Bytes per second a mirror delivers for the first 1 MB (3 s max), or None if it can't serve ranges.
fn probe(url: &str) -> Option<f64> {
    let start = Instant::now();
    let resp = agent().get(url).set("Range", &format!("bytes=0-{}", PROBE - 1)).timeout(Duration::from_secs(3)).call().ok()?;
    if resp.status() != 206 {
        return None;
    }
    let mut buf = Vec::with_capacity(PROBE as usize);
    resp.into_reader().take(PROBE).read_to_end(&mut buf).ok()?;
    Some(buf.len() as f64 / start.elapsed().as_secs_f64().max(0.001))
}

/// Every mirror URL for this release that answered the probe, fastest first, slow ones dropped.
fn fast_mirrors(d: &Distro, r: &Release) -> Vec<String> {
    let mut urls = vec![r.url.clone()];
    for (from, to) in d.mirrors {
        if let Some(rest) = r.url.strip_prefix(from) {
            urls.push(format!("{to}{rest}"));
        }
    }
    let mut speeds: Vec<(String, f64)> = std::thread::scope(|s| {
        let jobs: Vec<_> = urls.iter().map(|u| (u, s.spawn(|| probe(u)))).collect();
        jobs.into_iter().filter_map(|(u, j)| j.join().ok().flatten().map(|v| (u.clone(), v))).collect()
    });
    speeds.sort_by(|a, b| b.1.total_cmp(&a.1));
    let best = speeds.first().map_or(0.0, |s| s.1);
    // ponytail: one probe decides; a mirror that slows down later keeps its share (re-probing would fix it).
    speeds.into_iter().filter(|(_, v)| *v >= best / 4.0).map(|(u, _)| u).collect()
}

/// Downloads to `<file>.part` from several mirrors at once in 8 MB ranges, then checks the
/// SHA-256; renamed into place only if it matches.
pub fn download(id: &str, mut emit: impl FnMut(DownloadProgress)) -> Result<String, String> {
    take_cancel(id);
    let d = CATALOG.iter().find(|d| d.id == id).ok_or("Unknown distro")?;
    let r = resolve(d)?;
    std::fs::create_dir_all(folder()).map_err(|e| e.to_string())?;
    let dest = folder().join(&r.file);
    let part = folder().join(format!("{}.part", r.file));
    emit(DownloadProgress { id: id.into(), phase: "connect", done: 0, total: r.size, sources: 0 });

    let mirrors = if r.size > 0 { fast_mirrors(d, &r) } else { vec![] };
    let result = if mirrors.is_empty() {
        stream(id, &r.url, &part, r.size, &mut emit)
    } else {
        parallel(id, &mirrors, &part, r.size, &mut emit)
    }
    .and_then(|_| verify(id, &part, &r.sha256, &mut emit));

    match result {
        Ok(()) => {
            std::fs::rename(&part, &dest).map_err(|e| e.to_string())?;
            let _ = std::fs::write(folder().join(format!("{}.sha256", r.file)), &r.sha256);
            Ok(dest.to_string_lossy().into_owned())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

fn parallel(id: &str, mirrors: &[String], part: &PathBuf, total: u64, emit: &mut impl FnMut(DownloadProgress)) -> Result<(), String> {
    let file = File::create(part).map_err(|e| e.to_string())?;
    file.set_len(total).map_err(|e| e.to_string())?;
    let segments = total.div_ceil(SEGMENT);
    let next = AtomicU64::new(0);
    let retry = Mutex::new(Vec::<u64>::new());
    let done = AtomicU64::new(0);
    let failed = AtomicBool::new(false);
    let live = AtomicU64::new(0);

    let take = || retry.lock().unwrap().pop().or_else(|| {
        let i = next.fetch_add(1, Ordering::SeqCst);
        (i < segments).then_some(i)
    });

    std::thread::scope(|s| {
        for url in mirrors {
            for _ in 0..PER_MIRROR {
                let (file, take, retry, done, failed, live) = (&file, &take, &retry, &done, &failed, &live);
                s.spawn(move || {
                    live.fetch_add(1, Ordering::SeqCst);
                    let mut strikes = 0;
                    let mut buf = vec![0u8; 1 << 20];
                    while let Some(seg) = take() {
                        if is_cancelled(id) || failed.load(Ordering::SeqCst) {
                            break;
                        }
                        let start = seg * SEGMENT;
                        let end = (start + SEGMENT).min(total) - 1;
                        if let Err(wrote) = fetch_range(url, start, end, file, &mut buf, done) {
                            // Give the range back and uncount what this attempt wrote; another connection redoes it.
                            done.fetch_sub(wrote, Ordering::Relaxed);
                            retry.lock().unwrap().push(seg);
                            strikes += 1;
                            if strikes >= 3 {
                                break; // this mirror is misbehaving; the others carry on
                            }
                        }
                    }
                    live.fetch_sub(1, Ordering::SeqCst);
                });
            }
        }
        // Progress from this thread until every connection has finished.
        std::thread::sleep(Duration::from_millis(300));
        while live.load(Ordering::SeqCst) > 0 {
            if is_cancelled(id) {
                failed.store(true, Ordering::SeqCst);
            }
            let sources = live.load(Ordering::SeqCst).div_ceil(PER_MIRROR as u64) as usize;
            emit(DownloadProgress { id: id.into(), phase: "download", done: done.load(Ordering::Relaxed).min(total), total, sources });
            std::thread::sleep(Duration::from_millis(250));
        }
    });

    if take_cancel(id) {
        return Err("cancelled".into());
    }
    if next.load(Ordering::SeqCst) < segments || !retry.lock().unwrap().is_empty() {
        return Err("mirrorsFailed".into());
    }
    Ok(())
}

/// One HTTP range into its place in the file. On failure, returns how many bytes it had counted.
fn fetch_range(url: &str, start: u64, end: u64, file: &File, buf: &mut [u8], done: &AtomicU64) -> Result<(), u64> {
    let mut at = start;
    let resp = agent().get(url).set("Range", &format!("bytes={start}-{end}")).call().map_err(|_| 0u64)?;
    if resp.status() != 206 {
        return Err(0);
    }
    let mut body = resp.into_reader();
    while at <= end {
        let n = match body.read(buf) {
            Ok(0) | Err(_) => return Err(at - start),
            Ok(n) => n.min((end + 1 - at) as usize),
        };
        file.seek_write(&buf[..n], at).map_err(|_| at - start)?;
        at += n as u64;
        done.fetch_add(n as u64, Ordering::Relaxed);
    }
    Ok(())
}

/// Plain single-connection download, for servers that don't do ranges.
fn stream(id: &str, url: &str, part: &PathBuf, size: u64, emit: &mut impl FnMut(DownloadProgress)) -> Result<(), String> {
    let resp = agent().get(url).call().map_err(|e| e.to_string())?;
    let total = resp.header("content-length").and_then(|s| s.parse().ok()).unwrap_or(size);
    let mut body = resp.into_reader();
    let mut out = std::io::BufWriter::with_capacity(1 << 20, File::create(part).map_err(|e| e.to_string())?);
    let mut buf = vec![0u8; 1 << 20];
    let (mut done, mut last) = (0u64, Instant::now());
    loop {
        if take_cancel(id) {
            return Err("cancelled".into());
        }
        let n = body.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        if last.elapsed() > Duration::from_millis(250) {
            emit(DownloadProgress { id: id.into(), phase: "download", done, total, sources: 1 });
            last = Instant::now();
        }
    }
    out.flush().map_err(|e| e.to_string())
}

fn verify(id: &str, part: &PathBuf, expected: &str, emit: &mut impl FnMut(DownloadProgress)) -> Result<(), String> {
    let mut f = File::open(part).map_err(|e| e.to_string())?;
    let total = f.metadata().map_err(|e| e.to_string())?.len();
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 4 << 20];
    let (mut done, mut last) = (0u64, Instant::now());
    loop {
        if take_cancel(id) {
            return Err("cancelled".into());
        }
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        done += n as u64;
        if last.elapsed() > Duration::from_millis(250) {
            emit(DownloadProgress { id: id.into(), phase: "verify", done, total, sources: 0 });
            last = Instant::now();
        }
    }
    let got = format!("{:x}", hasher.finalize());
    if got == expected { Ok(()) } else { Err(format!("checksum|{got}")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iso_of(id: &str) -> Regex {
        match &CATALOG.iter().find(|d| d.id == id).unwrap().source {
            Source::Sums { iso, .. } => full(iso),
            _ => unreachable!(),
        }
    }

    #[test]
    fn picks_newest_iso_from_both_checksum_formats() {
        let ubuntu = "487f87faaf547ea30e0aba4d5b53346292571256b25333a978db1692bcee9dd2 *ubuntu-26.04-desktop-amd64.iso\n\
                      dec49008a71f6098d0bcfc822021f4d042d5f2db279e4d75bdd981304f1ca5d9 *ubuntu-26.04-live-server-amd64.iso\n\
                      601e30fbf5d97759367c632e2c33630665039b7e2158fd068403da3ccf1bda1f *ubuntu-26.04.1-desktop-amd64.iso";
        let (v, file, hash) = newest(&parse_sums(ubuntu), &iso_of("ubuntu")).unwrap();
        assert_eq!((v.as_str(), file.as_str()), ("26.04.1", "ubuntu-26.04.1-desktop-amd64.iso"));
        assert!(hash.starts_with("601e30"));

        let fedora = "-----BEGIN PGP SIGNED MESSAGE-----\nSHA256 (Fedora-Workstation-Live-44-1.7.x86_64.iso) = 1620295f6a00c27c3208f0c00b8ece4eab1ec69b9002152d97488bf26a426ddf";
        let (v, ..) = newest(&parse_sums(fedora), &iso_of("fedora")).unwrap();
        assert_eq!(v, "44-1.7");

        // A sidecar like ".iso.contents" must not count as the ISO.
        let debian = "e94859a83b305cae5125dc9c6080ced040c59d099ea7381e7f300fd1dba87a17  debian-live-13.7.0-amd64-gnome.iso\n\
                      d672d2ab941b8ffe064668dd5000b51c6d157d2b07bce71e17bcf038284cca08  debian-live-13.7.0-amd64-gnome.iso.contents";
        let (_, file, _) = newest(&parse_sums(debian), &iso_of("debian")).unwrap();
        assert_eq!(file, "debian-live-13.7.0-amd64-gnome.iso");

        // Rocky also lists a "latest" alias with the same hash; the numbered one wins.
        let rocky = "SHA256 (Rocky-10.2-x86_64-dvd1.iso) = 16ca9c96cdb221ba6e1f68579f21bd69fd8da81c6a921d9068949796f91c8feb\n\
                     SHA256 (Rocky-10-latest-x86_64-dvd.iso) = 16ca9c96cdb221ba6e1f68579f21bd69fd8da81c6a921d9068949796f91c8feb";
        let (v, ..) = newest(&parse_sums(rocky), &iso_of("rocky")).unwrap();
        assert_eq!(v, "10.2");

        // No version in the name (Bazzite) still resolves.
        let bazzite = "9c8d06cd8e57f2274678edeb14b4b13a79b8117c70571a65199919a66305b5c7  bazzite-stable-amd64.iso";
        let (v, ..) = newest(&parse_sums(bazzite), &iso_of("bazzite")).unwrap();
        assert_eq!(v, "latest");

        // Zorin re-spins: 18.1 beats 18 r3; r3 beats plain 18.
        let zorin = "09959ce793b8a065b70207e46cd646b1de805fc7e0bad925ce28df6e04b7f807  Zorin-OS-18-Core-64-bit-r3.iso
                     98666287ca5afae215def4804aca479bf3b439e028bc8f4e4d8ecddb6dd27339  Zorin-OS-18-Core-64-bit.iso
                     1111111111111111111111111111111111111111111111111111111111111111  Zorin-OS-18.1-Core-64-bit.iso
                     2222222222222222222222222222222222222222222222222222222222222222  Zorin-OS-18.1-Education-64-bit.iso";
        let (v, file, _) = newest(&parse_sums(zorin), &iso_of("zorin")).unwrap();
        assert_eq!((v.as_str(), file.as_str()), ("18.1", "Zorin-OS-18.1-Core-64-bit.iso"));
        let (v, ..) = newest(&parse_sums(&zorin.replace("Zorin-OS-18.1-Core", "x")), &iso_of("zorin")).unwrap();
        assert_eq!(v, "18 r3");

        // Parrot mixes md5/sha256/sha512 for the same file: only the SHA-256 one may win.
        let parrot = "a3ddddeb89af1768ff28d45dce5c6285  Parrot-security-7.4_amd64.iso
                      ca241282068701f481d96487dcd1af788bd897cd586ceb38ee16f61e387aa291  Parrot-security-7.4_amd64.iso
                      cc85c85061aa5539afc865895c9a2354a709ca53e246264a71dc93eeb2eedaac17b9d548f25235655fefbc7c7e080398af061c041f8ba91d4ae43ddd163e931b  Parrot-security-7.4_amd64.iso";
        let (_, _, hash) = newest(&parse_sums(parrot), &iso_of("parrot")).unwrap();
        assert!(hash.starts_with("ca2412"));

        assert!(version_key("26.10") > version_key("26.04.1"));
        assert!(version_key("9") < version_key("44"));
    }

    /// Hits the real servers: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn resolves_every_distro_online() {
        for e in list() {
            if e.page.is_some() {
                continue;
            }
            let r = e.release.unwrap_or_else(|| panic!("{}: {:?}", e.id, e.error));
            println!("{:14} {:18} {} {} MB", e.id, r.version, r.file, r.size >> 20);
            assert!(r.size > 100 << 20, "{} size", e.id);
        }
    }

    /// Real parallel download of the first 64 MB-ish, then cancel: `cargo test mirrors -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn mirrors_beat_a_single_stream() {
        let d = CATALOG.iter().find(|d| d.id == "arch").unwrap();
        let r = resolve(d).unwrap();
        let t = Instant::now();
        let fast = fast_mirrors(d, &r);
        println!("probe {:?}: {fast:?}", t.elapsed());
        let part = std::env::temp_dir().join("usboot-mirror-test.part");
        let start = Instant::now();
        let res = parallel("arch-test", &fast, &part, r.size, &mut |p: DownloadProgress| {
            if p.done > 64 << 20 {
                cancel("arch-test");
            }
        });
        let secs = start.elapsed().as_secs_f64();
        let _ = std::fs::remove_file(&part);
        println!("{res:?} after {secs:.1}s");
        assert_eq!(res, Err("cancelled".into()));
    }
}

#[cfg(test)]
mod full_download {
    /// Real end-to-end download + SHA-256 into Downloads\USBoot: `cargo test full_download -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn arch_downloads_and_verifies() {
        let t = std::time::Instant::now();
        let mut last = String::new();
        let r = super::download("arch", |p| {
            let line = format!("{} {}%", p.phase, if p.total > 0 { p.done * 100 / p.total } else { 0 });
            if line != last && p.done % 10 == 0 || p.phase != "download" {
                last = line;
            }
        });
        println!("RESULT {r:?} in {:.0}s", t.elapsed().as_secs_f64());
        assert!(r.is_ok());
    }
}
