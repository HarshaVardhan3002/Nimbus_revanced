//! Instant file search across the computer, in the spirit of Everything (voidtools).
//!
//! Every file and folder name on the fixed drives is held in memory, packed: one long
//! run of the names as written, one run of the same names in lower case (each ending in
//! a zero byte), and a small record per name pointing at its folder. A search scans the
//! lower-case run with a fast byte search (memchr's memmem, several gigabytes a second),
//! so a whole disk answers in milliseconds, on every key press.
//!
//! The first walk of the drives runs in the background on all cores. After that the
//! index is kept in a cache file and is back at once on the next start, then walked
//! again quietly. The system's change notices (ReadDirectoryChangesW on Windows,
//! FSEvents on macOS, inotify on Linux) keep it current in between; a quiet full walk
//! every few hours catches anything they missed.
//!
//! Only names are indexed, never what is in the files. Folders that are pure noise are
//! left out: .git, node_modules, build caches marked with CACHEDIR.TAG, the recycle
//! bin, and Windows' own component store.

use memchr::memmem;
use parking_lot::{Mutex, RwLock};
use rayon::prelude::*;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{MAIN_SEPARATOR, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

const NONE: u32 = u32::MAX;
const DIR: u8 = 1;
const GONE: u8 = 2;
/// Inside app data, caches, hidden or system folders: shown after everything else.
const NOISY: u8 = 4;
const ROOT: u8 = 8;

const MAGIC: &[u8; 8] = b"NIMBUSF1";
/// A full walk this often, for anything the change notices missed.
const REWALK: Duration = Duration::from_secs(6 * 3600);
/// Changes are gathered this long before the index takes them in.
const SETTLE: Duration = Duration::from_millis(1200);

/// Folder names never walked into (lower case).
const SKIP: &[&str] = &[".git", ".hg", ".svn", "node_modules", "__pycache__", "$recycle.bin", "system volume information", ".trash", ".trashes", ".spotlight-v100", ".fseventsd"];
/// Whole folders never walked into (lower case, Windows).
const SKIP_PATHS: &[&str] = &[
    "c:\\windows\\winsxs",
    "c:\\windows\\installer",
    "c:\\windows\\servicing",
    "c:\\windows\\softwaredistribution",
    "c:\\windows\\assembly",
    "c:\\windows\\system32\\driverstore",
    "c:\\$windows.~bt",
    "c:\\$windows.~ws",
];
/// Folder names whose contents rank last (lower case).
const NOISE: &[&str] = &[
    "appdata",
    "programdata",
    "windows",
    "program files",
    "program files (x86)",
    "library",
    "site-packages",
    "dist-packages",
    "cache",
    "caches",
    "temp",
    "tmp",
    "$windows.~bt",
];

#[derive(Clone, Copy)]
struct Entry {
    name: u32,
    len: u16,
    low: u32,
    llen: u16,
    parent: u32,
    flags: u8,
}

/// The names, packed. Entries are only ever appended, so their offsets into `lower` rise
/// with their index, and a folder always comes before what is in it.
#[derive(Default)]
pub struct Index {
    names: Vec<u8>,
    lower: Vec<u8>,
    entries: Vec<Entry>,
    /// Folders by a hash of their lower-case path (see `child_hash`), for change notices.
    dirs: HashMap<u64, u32>,
    gone: usize,
}

fn child_hash(parent: u64, name_lower: &str) -> u64 {
    let mut h = DefaultHasher::new();
    parent.hash(&mut h);
    name_lower.hash(&mut h);
    h.finish()
}

fn root_hash(low: &str) -> u64 {
    let mut h = DefaultHasher::new();
    low.hash(&mut h);
    h.finish()
}

fn is_noise(name_lower: &str) -> bool {
    name_lower.starts_with('.') || NOISE.contains(&name_lower)
}

impl Index {
    fn push(&mut self, name: &str, parent: u32, flags: u8) -> u32 {
        let low = name.to_lowercase();
        let i = self.entries.len() as u32;
        self.entries.push(Entry {
            name: self.names.len() as u32,
            len: name.len().min(u16::MAX as usize) as u16,
            low: self.lower.len() as u32,
            llen: low.len().min(u16::MAX as usize) as u16,
            parent,
            flags,
        });
        self.names.extend_from_slice(&name.as_bytes()[..name.len().min(u16::MAX as usize)]);
        self.lower.extend_from_slice(&low.as_bytes()[..low.len().min(u16::MAX as usize)]);
        self.lower.push(0);
        i
    }

    fn name(&self, i: u32) -> &str {
        let e = self.entries[i as usize];
        std::str::from_utf8(&self.names[e.name as usize..e.name as usize + e.len as usize]).unwrap_or("")
    }

    /// The lower-case name with its closing zero byte.
    fn low(&self, i: u32) -> &[u8] {
        let e = self.entries[i as usize];
        &self.lower[e.low as usize..=e.low as usize + e.llen as usize]
    }

    /// The full path, or `None` if it or a folder above it is gone.
    fn path(&self, i: u32) -> Option<String> {
        let mut parts = Vec::new();
        let mut at = i;
        while at != NONE {
            let e = self.entries[at as usize];
            if e.flags & GONE != 0 {
                return None;
            }
            parts.push(self.name(at));
            at = e.parent;
        }
        parts.reverse();
        Some(parts.join(&MAIN_SEPARATOR.to_string()))
    }

    pub fn count(&self) -> usize {
        self.entries.len() - self.gone
    }

    /// Rebuild the folder lookup after loading. Parents come before children, so one
    /// pass in order sees every parent's hash before its children need it.
    fn rebuild_dirs(&mut self, roots: &[Root]) {
        self.dirs.clear();
        let mut hashes = vec![0u64; self.entries.len()];
        let mut r = 0;
        for i in 0..self.entries.len() {
            let e = self.entries[i];
            if e.flags & DIR == 0 || e.flags & GONE != 0 {
                continue;
            }
            let h = if e.flags & ROOT != 0 {
                let h = roots.get(r).map(|x| root_hash(&x.low)).unwrap_or(0);
                r += 1;
                h
            } else {
                child_hash(hashes[e.parent as usize], &String::from_utf8_lossy(&self.low(i as u32)[..e.llen as usize]))
            };
            hashes[i] = h;
            self.dirs.insert(h, i as u32);
        }
    }
}

// ---- walking the drives ---------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Root {
    path: PathBuf,
    /// Lower case, without a closing separator ("c:" or "/home/sai").
    low: String,
    /// The root's own entry name: "C:", or the full folder path elsewhere.
    show: String,
}

impl Root {
    fn new(path: PathBuf) -> Root {
        let s = path.to_string_lossy().trim_end_matches(['\\', '/']).to_string();
        Root { low: s.to_lowercase(), show: s, path }
    }
}

/// What to index: every fixed drive on Windows; the home folder, apps and other
/// volumes on macOS; the home folder and the usual mount points on Linux.
pub fn roots() -> Vec<Root> {
    let mut out = Vec::new();
    #[cfg(windows)]
    {
        use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
        let mask = unsafe { GetLogicalDrives() };
        for n in 0..26u32 {
            if mask & (1 << n) == 0 {
                continue;
            }
            let letter = (b'A' + n as u8) as char;
            let wide: Vec<u16> = format!("{letter}:\\").encode_utf16().chain([0]).collect();
            // 3 is DRIVE_FIXED: no network shares, sticks or disc drives.
            if unsafe { GetDriveTypeW(windows::core::PCWSTR(wide.as_ptr())) } == 3 {
                out.push(Root::new(PathBuf::from(format!("{letter}:\\"))));
            }
        }
    }
    #[cfg(not(windows))]
    {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut add = |p: PathBuf| {
            if p.is_dir() && !out.iter().any(|r: &Root| p.starts_with(&r.path)) {
                out.push(Root::new(p));
            }
        };
        if let Some(h) = home {
            add(h);
        }
        #[cfg(target_os = "macos")]
        {
            add(PathBuf::from("/Applications"));
            if let Ok(rd) = std::fs::read_dir("/Volumes") {
                // The start-up disk shows up there as a link; real volumes are folders.
                for e in rd.flatten().filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false)) {
                    add(e.path());
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        for p in ["/mnt", "/media", "/opt", "/srv"] {
            add(PathBuf::from(p));
        }
    }
    out
}

#[derive(Default)]
struct Listing {
    /// Names not walked into, and whether each is a folder.
    leaves: Vec<(String, bool)>,
    /// Folders walked into (`None`: left out on purpose, but its name is kept).
    dirs: Vec<(String, Option<Listing>)>,
}

/// One folder's contents: the names to keep, and the folders to go into.
fn read(path: &Path) -> (Vec<(String, bool)>, Vec<String>) {
    let Ok(rd) = std::fs::read_dir(path) else { return Default::default() };
    let (mut leaves, mut dirs) = (Vec::new(), Vec::new());
    let mut cache_tag = false;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(t) = e.file_type() else { continue };
        // Links and junctions are listed but never followed, so nothing is walked twice.
        if t.is_dir() && !t.is_symlink() {
            dirs.push(name);
        } else {
            cache_tag |= name == "CACHEDIR.TAG";
            leaves.push((name, t.is_dir()));
        }
    }
    if cache_tag {
        // A build cache (Cargo's target, for one): keep this level's names, go no deeper.
        leaves.extend(dirs.drain(..).map(|d| (d, true)));
    }
    (leaves, dirs)
}

fn walk(path: &Path, low: &str) -> Listing {
    let (leaves, dirs) = read(path);
    let dirs = dirs
        .into_par_iter()
        .map(|d| {
            let dl = d.to_lowercase();
            let sub_low = format!("{low}{MAIN_SEPARATOR}{dl}");
            let skip = SKIP.contains(&dl.as_str()) || SKIP_PATHS.contains(&sub_low.as_str());
            let sub = (!skip).then(|| walk(&path.join(&d), &sub_low));
            (d, sub)
        })
        .collect();
    Listing { leaves, dirs }
}

fn add(ix: &mut Index, l: Listing, parent: u32, parent_hash: u64, noisy: bool) {
    for (n, dir) in l.leaves {
        let quiet = noisy || is_noise(&n.to_lowercase());
        let i = ix.push(&n, parent, if dir { DIR } else { 0 } | if quiet { NOISY } else { 0 });
        if dir {
            ix.dirs.insert(child_hash(parent_hash, &n.to_lowercase()), i);
        }
    }
    for (n, sub) in l.dirs {
        let nl = n.to_lowercase();
        let quiet = noisy || is_noise(&nl);
        let i = ix.push(&n, parent, DIR | if quiet { NOISY } else { 0 });
        let h = child_hash(parent_hash, &nl);
        ix.dirs.insert(h, i);
        if let Some(s) = sub {
            add(ix, s, i, h, quiet);
        }
    }
}

/// Walk every root, all cores at once.
pub fn build(roots: &[Root]) -> Index {
    let listings: Vec<Listing> = roots.par_iter().map(|r| walk(&r.path, &r.low)).collect();
    let mut ix = Index::default();
    for (r, l) in roots.iter().zip(listings) {
        let i = ix.push(&r.show, NONE, DIR | ROOT);
        let h = root_hash(&r.low);
        ix.dirs.insert(h, i);
        add(&mut ix, l, i, h, false);
    }
    ix
}

// ---- searching --------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub path: String,
    pub name: String,
    /// The folder it is in.
    pub folder: String,
    pub dir: bool,
    pub size: Option<u64>,
    /// Last changed, in ms since 1970.
    pub modified: Option<i64>,
}

struct Word {
    needle: Vec<u8>,
    /// Matched against the whole path (it has a folder separator in it).
    path: bool,
}

/// "report 2024" finds names with both words; "*.pdf" or "ext:pdf" finds names ending
/// in .pdf; "work\report" (or "work/report") finds names with "report" in a folder
/// whose path has "work\" in it.
fn words(query: &str) -> Vec<Word> {
    let sep = MAIN_SEPARATOR.to_string();
    let mut out = Vec::new();
    let mut folder_only = None;
    for raw in query.split_whitespace() {
        let mut w = raw.to_lowercase();
        if w.contains(['/', '\\']) {
            let p = w.replace(['/', '\\'], &sep);
            let cut = p.rfind(MAIN_SEPARATOR).unwrap() + 1;
            out.push(Word { needle: p[..cut].as_bytes().to_vec(), path: true });
            folder_only = p[..cut - 1].rsplit(MAIN_SEPARATOR).next().map(str::to_string);
            w = p[cut..].to_string();
        }
        let (w, ends) = if let Some(ext) = w.strip_prefix("ext:") {
            (format!(".{}", ext.trim_start_matches('.')), true)
        } else {
            let ends = w.starts_with("*.") && !w[2..].contains('*');
            (w, ends)
        };
        for part in w.split('*').filter(|p| !p.is_empty()) {
            let mut needle = part.as_bytes().to_vec();
            if ends {
                needle.push(0);
            }
            out.push(Word { needle, path: false });
        }
    }
    // Only a folder ("downloads/"): find the folder itself.
    if !out.iter().any(|w| !w.path) {
        if let Some(f) = folder_only.filter(|f| !f.is_empty()) {
            out.push(Word { needle: f.into_bytes(), path: false });
        }
    }
    out
}

impl Index {
    /// The best `limit` matches: whole name first, then names that start with it, then
    /// a word in the name that does, then anywhere; within each, nearer the top of the
    /// drive first, then shorter names; app data, caches and hidden folders after all
    /// the rest.
    pub fn search(&self, query: &str, limit: usize) -> Vec<(u32, String)> {
        let ws = words(query);
        let Some(first) = ws.iter().filter(|w| !w.path).max_by_key(|w| w.needle.len()) else { return Vec::new() };
        // The name the person typed, without folders: "users/ada/downloads" is "downloads".
        let whole: Vec<u8> = ws.iter().filter(|w| !w.path).map(|w| w.needle.strip_suffix(&[0]).unwrap_or(&w.needle)).collect::<Vec<_>>().join(&b' ');
        let finder = memmem::Finder::new(&first.needle);
        let others: Vec<(memmem::Finder, bool)> = ws.iter().filter(|w| !std::ptr::eq(*w, first)).map(|w| (memmem::Finder::new(&w.needle), w.path)).collect();

        // One pass through the names. Offsets only rise, so the entry they fall in is
        // found by stepping forward, and after a match the search goes on from the next
        // name: a one-letter query costs one look per name, not one per letter.
        let mut found: Vec<(u8, u16, u32)> = Vec::new();
        let (lower, entries) = (&self.lower[..], &self.entries[..]);
        let mut pos = 0;
        let mut i = 0usize;
        while let Some(rel) = finder.find(&lower[pos..]) {
            let off = pos + rel;
            while i + 1 < entries.len() && entries[i + 1].low as usize <= off {
                i += 1;
            }
            let e = entries[i];
            pos = e.low as usize + e.llen as usize + 1;
            if e.flags & (GONE | ROOT) != 0 {
                continue;
            }
            let low = &lower[e.low as usize..pos];
            if !others.iter().filter(|(_, p)| !p).all(|(f, _)| f.find(low).is_some()) {
                continue;
            }
            let at = off - e.low as usize;
            let name = &low[..e.llen as usize];
            let tier = if name == whole.as_slice() {
                0
            } else if at == 0 {
                1
            } else if !name[at - 1].is_ascii_alphanumeric() {
                2
            } else {
                3
            };
            found.push((tier + if e.flags & NOISY != 0 { 4 } else { 0 }, e.llen, i as u32));
        }
        let with_path = others.iter().any(|(_, p)| *p);
        // Room for matches dropped below (gone, or not matching a folder word), and for
        // the depth to reorder.
        let keep = if with_path { found.len() } else { (limit * 4).min(found.len()) };
        if keep < found.len() {
            found.select_nth_unstable(keep);
            found.truncate(keep);
        }
        let depth = |mut at: u32| {
            let mut d = 0u16;
            while at != NONE {
                at = entries[at as usize].parent;
                d += 1;
            }
            d
        };
        let mut ranked: Vec<(u8, u16, u16, u32)> = found.into_iter().map(|(t, len, i)| (t, depth(i), len, i)).collect();
        ranked.sort_unstable();
        let mut out = Vec::new();
        for (_, _, _, i) in ranked {
            let Some(path) = self.path(i) else { continue };
            if with_path {
                let pl = path.to_lowercase();
                if !others.iter().filter(|(_, p)| *p).all(|(f, _)| f.find(pl.as_bytes()).is_some()) {
                    continue;
                }
            }
            out.push((i, path));
            if out.len() == limit {
                break;
            }
        }
        out
    }
}

// ---- the cache file -----------------------------------------------------------------

fn save(ix: &Index, roots: &[Root], file: &Path) -> std::io::Result<()> {
    let tmp = file.with_extension("tmp");
    {
        let mut w = BufWriter::with_capacity(1 << 20, std::fs::File::create(&tmp)?);
        w.write_all(MAGIC)?;
        w.write_all(&(roots.len() as u32).to_le_bytes())?;
        for r in roots {
            w.write_all(&(r.low.len() as u32).to_le_bytes())?;
            w.write_all(r.low.as_bytes())?;
        }
        for arena in [&ix.names, &ix.lower] {
            w.write_all(&(arena.len() as u64).to_le_bytes())?;
            w.write_all(arena)?;
        }
        w.write_all(&(ix.entries.len() as u64).to_le_bytes())?;
        for e in &ix.entries {
            w.write_all(&e.name.to_le_bytes())?;
            w.write_all(&e.len.to_le_bytes())?;
            w.write_all(&e.low.to_le_bytes())?;
            w.write_all(&e.llen.to_le_bytes())?;
            w.write_all(&e.parent.to_le_bytes())?;
            w.write_all(&[e.flags])?;
        }
        w.flush()?;
    }
    std::fs::rename(&tmp, file)
}

fn load(roots: &[Root], file: &Path) -> Option<Index> {
    let mut r = BufReader::with_capacity(1 << 20, std::fs::File::open(file).ok()?);
    let mut take = |n: usize| -> Option<Vec<u8>> {
        let mut b = vec![0; n];
        r.read_exact(&mut b).ok()?;
        Some(b)
    };
    if take(8)? != MAGIC {
        return None;
    }
    let u32_ = |b: &[u8]| u32::from_le_bytes(b.try_into().unwrap());
    let n = u32_(&take(4)?) as usize;
    let mut lows = Vec::new();
    for _ in 0..n {
        let len = u32_(&take(4)?) as usize;
        lows.push(String::from_utf8(take(len)?).ok()?);
    }
    // Drives came or went: walk afresh instead.
    if lows != roots.iter().map(|r| r.low.clone()).collect::<Vec<_>>() {
        return None;
    }
    let mut ix = Index::default();
    for arena in [&mut ix.names, &mut ix.lower] {
        let len = u64::from_le_bytes(take(8)?.try_into().ok()?) as usize;
        *arena = take(len)?;
    }
    let count = u64::from_le_bytes(take(8)?.try_into().ok()?) as usize;
    let raw = take(count.checked_mul(17)?)?;
    ix.entries = raw
        .chunks_exact(17)
        .map(|c| Entry {
            name: u32_(&c[0..4]),
            len: u16::from_le_bytes([c[4], c[5]]),
            low: u32_(&c[6..10]),
            llen: u16::from_le_bytes([c[10], c[11]]),
            parent: u32_(&c[12..16]),
            flags: c[16],
        })
        .collect();
    // A damaged file must not index out of bounds later.
    let ok = ix.entries.iter().enumerate().all(|(i, e)| {
        e.name as usize + e.len as usize <= ix.names.len() && (e.low as usize + e.llen as usize) < ix.lower.len() && (e.parent == NONE || (e.parent as usize) < i)
    });
    if !ok {
        return None;
    }
    ix.gone = ix.entries.iter().filter(|e| e.flags & GONE != 0).count();
    ix.rebuild_dirs(roots);
    Some(ix)
}

// ---- keeping it current -------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize)]
pub struct Status {
    pub on: bool,
    pub files: usize,
    pub scanning: bool,
    /// When the last full walk finished, in ms since 1970.
    pub walked: Option<i64>,
    /// How long it took, in ms.
    pub took: Option<u64>,
    pub trouble: Option<String>,
}

#[derive(Default)]
pub struct Files {
    index: RwLock<Index>,
    roots: Mutex<Vec<Root>>,
    /// Bumped whenever a new index replaces the old, so changes read from the old one
    /// are not applied to the new.
    generation: AtomicU64,
    /// Bumped to stop the running worker (file search switched off, or restarted).
    run: AtomicU64,
    scanning: AtomicBool,
    dirty: AtomicBool,
    status: Mutex<Status>,
    file: Mutex<Option<PathBuf>>,
    /// Lower-case folders whose changes are ignored (Nimbus's own data).
    ignore: Mutex<Vec<String>>,
}

fn ms(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl Files {
    pub fn status(&self) -> Status {
        let mut s = self.status.lock().clone();
        s.files = self.index.read().count();
        s.scanning = self.scanning.load(Ordering::Relaxed);
        s
    }

    /// Search by name; also returns each hit's size and when it last changed.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Hit> {
        let found = self.index.read().search(query, limit.clamp(1, 200));
        found
            .into_iter()
            .filter_map(|(_, path)| {
                let p = Path::new(&path);
                // Gone since the last change notice: leave it out.
                let meta = std::fs::symlink_metadata(p).ok()?;
                Some(Hit {
                    name: p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone()),
                    folder: p.parent().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default(),
                    dir: meta.is_dir(),
                    size: (!meta.is_dir()).then(|| meta.len()),
                    modified: meta.modified().ok().map(ms),
                    path,
                })
            })
            .collect()
    }

    fn rewalk(&self, app: &AppHandle) {
        let roots = self.roots.lock().clone();
        self.scanning.store(true, Ordering::Relaxed);
        let _ = app.emit("files", self.status());
        let began = Instant::now();
        let fresh = build(&roots);
        let took = began.elapsed();
        {
            let mut ix = self.index.write();
            *ix = fresh;
            self.generation.fetch_add(1, Ordering::SeqCst);
        }
        self.scanning.store(false, Ordering::Relaxed);
        {
            let mut s = self.status.lock();
            s.walked = Some(ms(SystemTime::now()));
            s.took = Some(took.as_millis() as u64);
        }
        self.dirty.store(true, Ordering::Relaxed);
        self.save();
        eprintln!("[nimbus] file index: {} names in {:.1} s", self.index.read().count(), took.as_secs_f32());
        let _ = app.emit("files", self.status());
    }

    fn save(&self) {
        if !self.dirty.swap(false, Ordering::Relaxed) {
            return;
        }
        let Some(file) = self.file.lock().clone() else { return };
        let roots = self.roots.lock().clone();
        if let Err(e) = save(&self.index.read(), &roots, &file) {
            eprintln!("[nimbus] the file index was not saved: {e}");
        }
    }

    /// Folders whose contents changed: read them again and bring the index in line.
    /// New folders are walked whole; removed ones drop out with all that was in them.
    fn changed(&self, folders: HashSet<PathBuf>) {
        let roots = self.roots.lock().clone();
        let ignore = self.ignore.lock().clone();
        let generation = self.generation.load(Ordering::SeqCst);

        // Find each folder in the index, or the nearest folder above it that is there.
        let mut targets: HashMap<u32, PathBuf> = HashMap::new();
        {
            let ix = self.index.read();
            for f in folders {
                let fl = f.to_string_lossy().to_lowercase();
                if ignore.iter().any(|i| fl.starts_with(i.as_str())) {
                    continue;
                }
                let Some(root) = roots.iter().find(|r| fl.starts_with(&r.low)) else { continue };
                let rest = fl[root.low.len()..].trim_start_matches(['\\', '/']).to_string();
                let parts: Vec<&str> = rest.split(['\\', '/']).filter(|p| !p.is_empty()).collect();
                if parts.iter().any(|p| SKIP.contains(p)) {
                    continue;
                }
                // Hashes of each level, root first, to step back up when a level is new.
                let mut hashes = vec![root_hash(&root.low)];
                for p in &parts {
                    hashes.push(child_hash(*hashes.last().unwrap(), p));
                }
                let mut path = f.clone();
                for h in hashes.iter().rev() {
                    if let Some(&i) = ix.dirs.get(h) {
                        if ix.entries[i as usize].flags & GONE == 0 {
                            targets.insert(i, path);
                            break;
                        }
                    }
                    if !path.pop() {
                        break;
                    }
                }
            }
        }
        if targets.is_empty() {
            return;
        }

        // What the index holds for those folders now, in one pass over all names.
        let mut held: HashMap<u32, HashMap<String, (u32, bool)>> = targets.keys().map(|&k| (k, HashMap::new())).collect();
        {
            let ix = self.index.read();
            for (i, e) in ix.entries.iter().enumerate() {
                if e.flags & GONE == 0 {
                    if let Some(h) = held.get_mut(&e.parent) {
                        h.insert(String::from_utf8_lossy(&ix.low(i as u32)[..e.llen as usize]).into_owned(), (i as u32, e.flags & DIR != 0));
                    }
                }
            }
        }

        // Read the folders again; walk any new folders in them, off the lock.
        struct Change {
            parent: u32,
            gone: Vec<u32>,
            new: Listing,
        }
        let changes: Vec<Change> = targets
            .into_par_iter()
            .map(|(parent, path)| {
                let (leaves, dirs) = read(&path);
                let was = &held[&parent];
                let mut now: HashSet<String> = HashSet::new();
                let mut new = Listing::default();
                for (n, dir) in leaves {
                    let nl = n.to_lowercase();
                    if !was.contains_key(&nl) {
                        new.leaves.push((n, dir));
                    }
                    now.insert(nl);
                }
                for d in dirs {
                    let dl = d.to_lowercase();
                    if !was.contains_key(&dl) {
                        let low = format!("{}{MAIN_SEPARATOR}{dl}", path.to_string_lossy().to_lowercase().trim_end_matches(['\\', '/']));
                        let skip = SKIP.contains(&dl.as_str()) || SKIP_PATHS.contains(&low.as_str());
                        let sub = (!skip).then(|| walk(&path.join(&d), &low));
                        new.dirs.push((d, sub));
                    }
                    now.insert(dl);
                }
                let gone = was.iter().filter(|(n, _)| !now.contains(*n)).map(|(_, (i, _))| *i).collect();
                Change { parent, gone, new }
            })
            .collect();

        let mut ix = self.index.write();
        if self.generation.load(Ordering::SeqCst) != generation {
            // A full walk replaced the index meanwhile; it has all this already.
            return;
        }
        let mut touched = false;
        for c in changes {
            for i in c.gone {
                let e = &mut ix.entries[i as usize];
                if e.flags & GONE == 0 {
                    e.flags |= GONE;
                    ix.gone += 1;
                    touched = true;
                }
            }
            if !c.new.leaves.is_empty() || !c.new.dirs.is_empty() {
                // The parent's hash, found again from the lookup.
                let Some(h) = ix.dirs.iter().find(|(_, v)| **v == c.parent).map(|(k, _)| *k) else { continue };
                let noisy = ix.entries[c.parent as usize].flags & NOISY != 0;
                add(&mut ix, c.new, c.parent, h, noisy);
                touched = true;
            }
        }
        if touched {
            self.dirty.store(true, Ordering::Relaxed);
        }
    }
}

/// Show a file in the system's file manager, selected where it can be.
pub fn reveal(p: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Explorer wants "/select," and the path as one argument, quoted as it is.
        std::process::Command::new("explorer.exe")
            .raw_arg(format!("/select,\"{}\"", p.display()))
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Could not open Explorer: {e}"))
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg("-R").arg(p).spawn().map(|_| ()).map_err(|e| format!("Could not open Finder: {e}"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // The freedesktop file manager call selects the file; plain xdg-open shows its folder.
        let uri = format!("file://{}", p.display());
        let ok = std::process::Command::new("dbus-send")
            .args(["--session", "--dest=org.freedesktop.FileManager1", "--type=method_call", "/org/freedesktop/FileManager1", "org.freedesktop.FileManager1.ShowItems"])
            .arg(format!("array:string:{uri}"))
            .arg("string:")
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            return Ok(());
        }
        crate::open_outside(p.parent().unwrap_or(p)).map_err(|e| format!("Could not open the folder: {e}"))
    }
}

/// Start or stop file search, as the setting says.
pub fn set(app: &AppHandle, on: bool) {
    let state = app.state::<crate::AppState>();
    let files = state.files.clone();
    let mine = files.run.fetch_add(1, Ordering::SeqCst) + 1;
    files.status.lock().on = on;
    if !on {
        *files.index.write() = Index::default();
        files.generation.fetch_add(1, Ordering::SeqCst);
        if let Some(f) = files.file.lock().clone() {
            let _ = std::fs::remove_file(f);
        }
        let _ = app.emit("files", files.status());
        return;
    }
    let app = app.clone();
    std::thread::Builder::new()
        .name("file-index".into())
        .spawn(move || work(app, files, mine))
        .ok();
}

fn work(app: AppHandle, files: Arc<Files>, mine: u64) {
    let alive = || files.run.load(Ordering::SeqCst) == mine;
    let roots = roots();
    *files.roots.lock() = roots.clone();
    let cache = app.path().app_cache_dir().ok();
    if let Some(dir) = &cache {
        let _ = std::fs::create_dir_all(dir);
        *files.file.lock() = Some(dir.join("files.idx"));
    }
    // Changes inside Nimbus's own folders (the cache file among them) are not news.
    *files.ignore.lock() = [app.path().app_cache_dir(), app.path().app_data_dir(), app.path().app_local_data_dir(), app.path().app_config_dir()]
        .into_iter()
        .flatten()
        .map(|p| p.to_string_lossy().to_lowercase())
        .collect();

    // The saved index first, so search works at once; then a fresh walk.
    if let Some(ix) = files.file.lock().clone().and_then(|f| load(&roots, &f)) {
        *files.index.write() = ix;
        files.generation.fetch_add(1, Ordering::SeqCst);
        let _ = app.emit("files", files.status());
    }

    // Change notices, gathered on a channel; a watcher that fails just means more walks.
    let (tx, rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(tx).ok();
    if let Some(w) = watcher.as_mut() {
        use notify::Watcher;
        for r in &roots {
            if let Err(e) = w.watch(&r.path, notify::RecursiveMode::Recursive) {
                eprintln!("[nimbus] no change notices for {}: {e}", r.path.display());
            }
        }
    }

    if !alive() {
        return;
    }
    files.rewalk(&app);
    let mut walked = Instant::now();
    let mut saved = Instant::now();
    let mut pending: HashSet<PathBuf> = HashSet::new();
    let mut first_pending: Option<Instant> = None;
    // The system dropped notices (too many at once): walk again, but at most every ten minutes.
    let mut missed = false;
    while alive() {
        match rx.recv_timeout(Duration::from_millis(400)) {
            Ok(Ok(ev)) => {
                use notify::EventKind::*;
                use notify::event::ModifyKind;
                if ev.need_rescan() {
                    missed = true;
                    continue;
                }
                // Only names coming, going or changing matter; writes to a file do not.
                if matches!(ev.kind, Create(_) | Remove(_) | Modify(ModifyKind::Name(_)) | Any) {
                    for p in ev.paths {
                        if let Some(parent) = p.parent() {
                            pending.insert(parent.to_path_buf());
                        }
                    }
                    first_pending.get_or_insert_with(Instant::now);
                }
            }
            Ok(Err(_)) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // No watcher: nothing will ever arrive, so wait here instead of spinning.
                watcher = None;
                std::thread::sleep(Duration::from_millis(400));
            }
        }
        if first_pending.is_some_and(|t| t.elapsed() > SETTLE) {
            files.changed(std::mem::take(&mut pending));
            first_pending = None;
            let _ = app.emit("files", files.status());
        }
        let (count, gone) = {
            let ix = files.index.read();
            (ix.entries.len(), ix.gone)
        };
        // Many names gone since the last walk: walk again to pack the index tight.
        let soon = walked.elapsed() > REWALK / 36;
        if walked.elapsed() > REWALK || ((watcher.is_none() || missed) && soon) || gone > count / 4 + 50_000 {
            missed = false;
            files.rewalk(&app);
            walked = Instant::now();
            saved = Instant::now();
        }
        if saved.elapsed() > Duration::from_secs(600) {
            files.save();
            saved = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> (tempdir::Dir, Vec<Root>) {
        let d = tempdir::Dir::new();
        for f in [
            "Documents/Tax Report 2024.pdf",
            "Documents/report-notes.txt",
            "Documents/Work/Quarterly REPORT.xlsx",
            "Pictures/Ünïcode Bild.png",
            "AppData/Local/report.log",
            "project/node_modules/report/index.js",
            "project/target/CACHEDIR.TAG",
            "project/target/debug/report.exe",
            "project/src/main.rs",
        ] {
            let p = d.0.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        let roots = vec![Root::new(d.0.clone())];
        (d, roots)
    }

    mod tempdir {
        pub struct Dir(pub std::path::PathBuf);
        impl Dir {
            pub fn new() -> Dir {
                let p = std::env::temp_dir().join(format!("nimbus-files-{}-{}", std::process::id(), rand()));
                std::fs::create_dir_all(&p).unwrap();
                Dir(p)
            }
        }
        fn rand() -> u64 {
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    fn names(ix: &Index, q: &str) -> Vec<String> {
        ix.search(q, 20).into_iter().map(|(i, _)| ix.name(i).to_string()).collect()
    }

    #[test]
    fn finds_ranks_and_skips() {
        let (_d, roots) = tree();
        let ix = build(&roots);
        let got = names(&ix, "report");
        // Name starts with it, then a word does, then app data last; never node_modules or
        // inside a build cache.
        assert_eq!(got, vec!["report-notes.txt", "Tax Report 2024.pdf", "Quarterly REPORT.xlsx", "report.log"]);
        assert_eq!(names(&ix, "tax 2024"), vec!["Tax Report 2024.pdf"]);
        assert_eq!(names(&ix, "*.pdf"), vec!["Tax Report 2024.pdf"]);
        assert_eq!(names(&ix, "ext:xlsx"), vec!["Quarterly REPORT.xlsx"]);
        assert_eq!(names(&ix, "work/report"), vec!["Quarterly REPORT.xlsx"]);
        assert_eq!(names(&ix, "ünï"), vec!["Ünïcode Bild.png"]);
        // The build cache's own folder names stay findable; what is inside them does not.
        assert_eq!(names(&ix, "debug"), vec!["debug"]);
        assert!(names(&ix, "main.rs") == vec!["main.rs"]);
        let (_, path) = &ix.search("tax", 1)[0];
        assert!(path.ends_with(&format!("Documents{MAIN_SEPARATOR}Tax Report 2024.pdf")), "{path}");
    }

    #[test]
    fn saved_and_loaded() {
        let (d, roots) = tree();
        let ix = build(&roots);
        let file = d.0.join("files.idx");
        save(&ix, &roots, &file).unwrap();
        let back = load(&roots, &file).unwrap();
        assert_eq!(back.count(), ix.count());
        assert_eq!(names(&back, "report"), names(&ix, "report"));
        assert_eq!(back.dirs.len(), ix.dirs.len());
        // Other drives: the old file is not used.
        assert!(load(&[Root::new(PathBuf::from("Z:\\nowhere"))], &file).is_none());
        // A cut-off file is turned down, not trusted.
        let bytes = std::fs::read(&file).unwrap();
        std::fs::write(&file, &bytes[..bytes.len() - 9]).unwrap();
        assert!(load(&roots, &file).is_none());
    }

    #[test]
    fn follows_changes() {
        let (d, roots) = tree();
        let files = Files::default();
        *files.roots.lock() = roots.clone();
        *files.index.write() = build(&roots);
        let docs = d.0.join("Documents");
        std::fs::remove_file(docs.join("report-notes.txt")).unwrap();
        std::fs::create_dir_all(docs.join("New Folder/Deep")).unwrap();
        std::fs::write(docs.join("New Folder/Deep/report final.docx"), b"x").unwrap();
        std::fs::remove_dir_all(docs.join("Work")).unwrap();
        // Notices name the changed item's folder; a brand-new folder's own notice may
        // arrive before its parent is known.
        files.changed([docs.clone(), docs.join("New Folder/Deep")].into_iter().collect());
        let got: Vec<String> = files.search("report", 20).into_iter().map(|h| h.name).collect();
        assert_eq!(got, vec!["report final.docx", "Tax Report 2024.pdf", "report.log"]);
        // Its folder comes back after removal, and is found again.
        std::fs::create_dir_all(docs.join("Work")).unwrap();
        std::fs::write(docs.join("Work/report again.md"), b"x").unwrap();
        files.changed([docs.join("Work")].into_iter().collect());
        assert!(files.search("again", 5).iter().any(|h| h.name == "report again.md"));
    }

    /// Walks this computer's real drives and times a search. Run by hand:
    /// cargo test -p nimbus --lib files::tests::real_drives -- --ignored --nocapture
    #[test]
    #[ignore = "walks the whole disk"]
    fn real_drives() {
        let roots = roots();
        let t = Instant::now();
        let ix = build(&roots);
        println!("roots {:?}: {} names in {:.1} s", roots.iter().map(|r| &r.show).collect::<Vec<_>>(), ix.count(), t.elapsed().as_secs_f32());
        println!("arenas: names {} MB, lower {} MB, entries {} MB", ix.names.len() >> 20, ix.lower.len() >> 20, (ix.entries.len() * std::mem::size_of::<Entry>()) >> 20);
        for q in ["e", "report", "nimbus", "*.pdf", "tauri conf", "users/ada/downloads"] {
            let t = Instant::now();
            let hits = ix.search(q, 50);
            println!("{q:>22}: {:>3} hits in {:>6.2} ms, first {:?}", hits.len(), t.elapsed().as_secs_f64() * 1000.0, hits.first().map(|h| &h.1));
        }
        let file = std::env::temp_dir().join("nimbus-files-real.idx");
        let t = Instant::now();
        save(&ix, &roots, &file).unwrap();
        let s = t.elapsed();
        let t = Instant::now();
        let back = load(&roots, &file).unwrap();
        println!("cache {} MB: saved in {:.2} s, loaded in {:.2} s ({} names)", std::fs::metadata(&file).unwrap().len() >> 20, s.as_secs_f32(), t.elapsed().as_secs_f32(), back.count());
        let _ = std::fs::remove_file(file);
    }
}
