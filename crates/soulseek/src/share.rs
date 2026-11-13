//! What we share: an index of the library folders, built read-only (files
//! are only listed, stat'ed, and their headers read for the audio
//! attributes; nothing under a shared folder is ever written, moved or
//! renamed), and the answers to searches, browse and folder requests made
//! from it.
//!
//! Files are known to peers by a virtual path, `<folder name>\<path inside
//! it>` with backslashes, and only paths in the index are ever opened, so a
//! request cannot reach outside the shared folders. Hidden files and folders
//! (a leading dot, like `.musiclib`) and symbolic links are left out.
//!
//! Searches go through a word index (lowercased words of the virtual path,
//! split at anything that is not a letter or digit), so an incoming search
//! costs a few hash lookups and a list intersection, not a pass over the
//! library. The audio attributes of each file are cached in the state
//! folder by path, size and modification time, so a rescan only reads the
//! headers of new or changed files.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::UNIX_EPOCH;

use crate::codec::{Reader, Writer};
use crate::msg::{DirEntry, FileEntry, PeerMsg};

/// Extensions shared: audio, and what goes with an album (cue sheets, rip
/// logs, covers).
const SHARED_EXTENSIONS: &[&str] = &[
    "flac", "mp3", "ogg", "oga", "opus", "m4a", "aac", "wav", "aif", "aiff", "ape", "wv", "wma", "dsf", "dff", "alac",
    "cue", "log", "jpg", "jpeg", "png",
];

/// Files of those kinds that are the player's own, not part of an album:
/// the blurred backdrop it draws behind an album.
const SKIPPED_NAMES: &[&str] = &["ambient.png"];

pub(crate) const LOSSLESS_EXTENSIONS: &[&str] = &["flac", "wav", "aif", "aiff", "ape", "wv", "alac", "dsf", "dff"];

/// Audio attributes of a file; zero = unknown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Attrs {
    pub bitrate: u32,
    pub duration: u32,
    pub sample_rate: u32,
    pub bit_depth: u32,
}

impl Attrs {
    /// The attribute list as the protocol sends it: duration, sample rate
    /// and bit depth for lossless files, bitrate and duration for lossy ones.
    fn wire(&self, lossless: bool) -> Vec<(u32, u32)> {
        let mut v = Vec::with_capacity(3);
        if !lossless && self.bitrate > 0 {
            v.push((0, self.bitrate));
        }
        if self.duration > 0 {
            v.push((1, self.duration));
        }
        if lossless {
            if self.sample_rate > 0 {
                v.push((4, self.sample_rate));
            }
            if self.bit_depth > 0 {
                v.push((5, self.bit_depth));
            }
        }
        v
    }
}

struct SharedFile {
    /// Virtual path, `Root\dir\file.flac`.
    vpath: Box<str>,
    root: u16,
    size: u64,
    attrs: Attrs,
}

impl SharedFile {
    fn ext(&self) -> &str {
        ext_of(&self.vpath)
    }

    fn name(&self) -> &str {
        self.vpath.rsplit('\\').next().unwrap_or(&self.vpath)
    }

    fn dir(&self) -> &str {
        self.vpath.rsplit_once('\\').map_or("", |(d, _)| d)
    }

    fn entry(&self, full: bool) -> FileEntry {
        let ext = self.ext().to_ascii_lowercase();
        let lossless = LOSSLESS_EXTENSIONS.contains(&ext.as_str());
        FileEntry {
            name: if full { self.vpath.to_string() } else { self.name().to_owned() },
            size: self.size,
            attrs: self.attrs.wire(lossless),
            ext,
        }
    }
}

fn ext_of(name: &str) -> &str {
    let file = name.rsplit(['\\', '/']).next().unwrap_or(name);
    file.rsplit_once('.').map_or("", |(_, e)| e)
}

/// A shared folder and the name peers see it under.
#[derive(Clone, Debug)]
struct Root {
    name: String,
    path: PathBuf,
}

/// The index of everything shared.
#[derive(Default)]
pub struct ShareIndex {
    roots: Vec<Root>,
    /// Sorted by virtual path.
    files: Vec<SharedFile>,
    dirs: u32,
    words: HashMap<Box<str>, Vec<u32>>,
    browse: OnceLock<Arc<[u8]>>,
}

/// The words of a path or query term, lowercased.
fn words(s: &str) -> impl Iterator<Item = String> + '_ {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase)
}

/// A parsed search query.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Query {
    include: Vec<String>,
    /// `*term`: a word that ends with it.
    partial: Vec<String>,
    exclude: Vec<String>,
}

impl Query {
    pub(crate) fn parse(q: &str) -> Self {
        let mut out = Self::default();
        for term in q.split_whitespace() {
            if let Some(t) = term.strip_prefix('-') {
                out.exclude.extend(words(t));
            } else if let Some(t) = term.strip_prefix('*') {
                out.partial.extend(words(t));
            } else {
                out.include.extend(words(term));
            }
        }
        out.include.sort();
        out.include.dedup();
        out
    }

    /// Too vague to answer: nothing to look for, or only one or two
    /// letters (a search for "a" would list the whole library).
    fn too_vague(&self) -> bool {
        let longest = self.include.iter().chain(&self.partial).map(|w| w.chars().count()).max().unwrap_or(0);
        longest < 2 || (self.include.len() + self.partial.len() == 1 && longest < 3)
    }
}

/// How a scan reads files.
#[derive(Clone, Copy, Debug)]
pub struct ScanOptions {
    /// Read audio headers for bitrate, duration, sample rate and bit depth.
    pub read_attrs: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self { read_attrs: true }
    }
}

type Cache = HashMap<String, (u64, u64, Attrs)>;

const CACHE_VERSION: u32 = 1;

fn load_cache(path: &Path) -> Cache {
    let Ok(data) = std::fs::read(path) else {
        return Cache::new();
    };
    let mut r = Reader::new(&data);
    let parse = |r: &mut Reader<'_>| -> Result<Cache, crate::codec::Malformed> {
        if r.u32()? != CACHE_VERSION {
            return Ok(Cache::new());
        }
        let n = r.count(28)?;
        let mut c = Cache::with_capacity(n);
        for _ in 0..n {
            let p = r.str()?;
            let size = r.u64()?;
            let mtime = r.u64()?;
            let a = Attrs { bitrate: r.u32()?, duration: r.u32()?, sample_rate: r.u32()?, bit_depth: r.u32()? };
            c.insert(p, (size, mtime, a));
        }
        Ok(c)
    };
    parse(&mut r).unwrap_or_default()
}

fn save_cache(path: &Path, cache: &Cache) {
    let mut w = Writer::new();
    w.u32(CACHE_VERSION).u32(cache.len() as u32);
    for (p, (size, mtime, a)) in cache {
        w.str(p).u64(*size).u64(*mtime).u32(a.bitrate).u32(a.duration).u32(a.sample_rate).u32(a.bit_depth);
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, w.into_inner()).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

fn read_attrs(path: &Path) -> Attrs {
    use lofty::file::AudioFile as _;
    let mut opts = lofty::config::ParseOptions::new();
    opts = opts.read_tags(false).read_cover_art(false);
    let Ok(probe) = lofty::probe::Probe::open(path).and_then(|p| Ok(p.options(opts).guess_file_type()?)) else {
        return Attrs::default();
    };
    let Ok(file) = probe.read() else {
        return Attrs::default();
    };
    let p = file.properties();
    Attrs {
        bitrate: p.audio_bitrate().or(p.overall_bitrate()).unwrap_or(0),
        duration: p.duration().as_secs() as u32,
        sample_rate: p.sample_rate().unwrap_or(0),
        bit_depth: p.bit_depth().map_or(0, u32::from),
    }
}

/// Virtual names for the shared folders: each folder's own name, made
/// unique.
fn root_names(folders: &[PathBuf]) -> Vec<Root> {
    let mut roots: Vec<Root> = Vec::new();
    for f in folders {
        let base = f
            .file_name()
            .map(|n| n.to_string_lossy().replace('\\', "_"))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "Music".into());
        let mut name = base.clone();
        let mut i = 2;
        while roots.iter().any(|r| r.name == name) {
            name = format!("{base} {i}");
            i += 1;
        }
        roots.push(Root { name, path: f.clone() });
    }
    roots
}

impl ShareIndex {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Lists `folders` (read-only). `cache` is where attributes are kept
    /// between scans (outside the shared folders).
    pub fn scan(folders: &[PathBuf], cache: Option<&Path>, opts: ScanOptions) -> Self {
        let roots = root_names(folders);
        let old = cache.map(load_cache).unwrap_or_default();
        let mut fresh = Cache::with_capacity(old.len());
        let mut files = Vec::new();
        let mut dirs = 0u32;
        for (ri, root) in roots.iter().enumerate() {
            let mut stack = vec![(root.path.clone(), root.name.clone())];
            while let Some((dir, vdir)) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&dir) else { continue };
                let mut has_files = false;
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if name.starts_with('.') {
                        continue;
                    }
                    let Ok(ft) = e.file_type() else { continue };
                    if ft.is_dir() {
                        stack.push((e.path(), format!("{vdir}\\{name}")));
                        continue;
                    }
                    if !ft.is_file() {
                        continue;
                    }
                    let ext = ext_of(&name).to_ascii_lowercase();
                    if !SHARED_EXTENSIONS.contains(&ext.as_str()) {
                        continue;
                    }
                    if SKIPPED_NAMES.iter().any(|n| name.eq_ignore_ascii_case(n)) {
                        continue;
                    }
                    let Ok(md) = e.metadata() else { continue };
                    let size = md.len();
                    let mtime = md
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map_or(0, |d| d.as_secs());
                    let path = e.path();
                    let key = path.to_string_lossy().into_owned();
                    let attrs = match old.get(&key) {
                        Some((s, m, a)) if *s == size && *m == mtime => *a,
                        _ if opts.read_attrs && !matches!(ext.as_str(), "cue" | "log" | "jpg" | "jpeg" | "png") => {
                            read_attrs(&path)
                        }
                        _ => Attrs::default(),
                    };
                    fresh.insert(key, (size, mtime, attrs));
                    files.push(SharedFile { vpath: format!("{vdir}\\{name}").into(), root: ri as u16, size, attrs });
                    has_files = true;
                }
                if has_files {
                    dirs += 1;
                }
            }
        }
        if let Some(c) = cache
            && fresh != old
        {
            save_cache(c, &fresh);
        }
        Self::build(roots, files, dirs)
    }

    fn build(roots: Vec<Root>, mut files: Vec<SharedFile>, dirs: u32) -> Self {
        files.sort_by(|a, b| a.vpath.cmp(&b.vpath));
        let mut index: HashMap<Box<str>, Vec<u32>> = HashMap::new();
        for (i, f) in files.iter().enumerate() {
            let mut seen: Vec<String> = words(&f.vpath).collect();
            seen.sort();
            seen.dedup();
            for w in seen {
                index.entry(w.into_boxed_str()).or_default().push(i as u32);
            }
        }
        for v in index.values_mut() {
            v.shrink_to_fit();
        }
        index.shrink_to_fit();
        Self { roots, files, dirs, words: index, browse: OnceLock::new() }
    }

    pub fn file_count(&self) -> u32 {
        self.files.len() as u32
    }

    pub fn dir_count(&self) -> u32 {
        self.dirs
    }

    /// The real file behind a virtual path, with its size.
    pub fn resolve(&self, vpath: &str) -> Option<(PathBuf, u64)> {
        let i = self.files.binary_search_by(|f| (*f.vpath).cmp(vpath)).ok()?;
        let f = &self.files[i];
        let root = self.roots.get(f.root as usize)?;
        let rel = f.vpath.strip_prefix(root.name.as_str())?.trim_start_matches('\\');
        let mut p = root.path.clone();
        for part in rel.split('\\') {
            p.push(part);
        }
        Some((p, f.size))
    }

    /// Files matching `query`, at most `limit`; none when the query hits a
    /// phrase the server excludes.
    pub fn search(&self, query: &str, excluded: &[String], limit: usize) -> Vec<FileEntry> {
        let q = Query::parse(query);
        if q.too_vague() {
            return Vec::new();
        }
        let lower = query.to_lowercase();
        if excluded.iter().any(|p| !p.is_empty() && lower.contains(p.as_str())) {
            return Vec::new();
        }
        let mut lists: Vec<&[u32]> = Vec::with_capacity(q.include.len() + q.partial.len());
        for w in &q.include {
            match self.words.get(w.as_str()) {
                Some(l) => lists.push(l),
                None => return Vec::new(),
            }
        }
        let mut partial_sets: Vec<Vec<u32>> = Vec::new();
        for p in &q.partial {
            let mut set: Vec<u32> = self
                .words
                .iter()
                .filter(|(w, _)| w.ends_with(p.as_str()))
                .flat_map(|(_, l)| l.iter().copied())
                .collect();
            if set.is_empty() {
                return Vec::new();
            }
            set.sort_unstable();
            set.dedup();
            partial_sets.push(set);
        }
        lists.extend(partial_sets.iter().map(Vec::as_slice));
        lists.sort_by_key(|l| l.len());
        let Some((first, rest)) = lists.split_first() else {
            return Vec::new();
        };
        let exclude: Vec<&[u32]> = q.exclude.iter().filter_map(|w| self.words.get(w.as_str()).map(Vec::as_slice)).collect();
        let mut out = Vec::new();
        for &i in first.iter() {
            if rest.iter().all(|l| l.binary_search(&i).is_ok()) && !exclude.iter().any(|l| l.binary_search(&i).is_ok()) {
                let f = &self.files[i as usize];
                let lp = f.vpath.to_lowercase();
                if excluded.iter().any(|p| !p.is_empty() && lp.contains(p.as_str())) {
                    continue;
                }
                out.push(f.entry(true));
                if out.len() >= limit {
                    break;
                }
            }
        }
        out
    }

    fn dirs_where(&self, keep: impl Fn(&str) -> bool) -> Vec<DirEntry> {
        // In full-path order a folder's files can sit on both sides of a
        // subfolder's (`A\0.flac`, `A\B\x.flac`, `A\a.flac`), so they are
        // gathered per folder.
        let mut by_dir: BTreeMap<&str, Vec<FileEntry>> = BTreeMap::new();
        for f in &self.files {
            let dir = f.dir();
            if keep(dir) {
                by_dir.entry(dir).or_default().push(f.entry(false));
            }
        }
        by_dir.into_iter().map(|(name, files)| DirEntry { name: name.to_owned(), files }).collect()
    }

    /// Everything, grouped by directory (a browse).
    pub fn browse(&self) -> Vec<DirEntry> {
        self.dirs_where(|_| true)
    }

    /// The encoded browse response, built once per index.
    /// Shared by every answer: each browse request queues the same bytes.
    pub fn browse_message(&self) -> Arc<[u8]> {
        self.browse.get_or_init(|| PeerMsg::SharedFileListResponse(self.browse()).encode().into()).clone()
    }

    /// A folder with its subfolders.
    pub fn folder(&self, folder: &str) -> Vec<DirEntry> {
        let folder = folder.trim_end_matches('\\');
        let prefix = format!("{folder}\\");
        self.dirs_where(|d| d == folder || d.starts_with(&prefix))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(paths: &[&str]) -> ShareIndex {
        let files = paths
            .iter()
            .map(|p| SharedFile { vpath: (*p).into(), root: 0, size: 10, attrs: Attrs { duration: 100, ..Attrs::default() } })
            .collect();
        ShareIndex::build(vec![Root { name: "Music".into(), path: PathBuf::from("/m") }], files, 2)
    }

    #[test]
    fn query_terms() {
        let q = Query::parse("Aimer -live *ight AC/DC");
        assert_eq!(q.include, vec!["ac", "aimer", "dc"]);
        assert_eq!(q.exclude, vec!["live"]);
        assert_eq!(q.partial, vec!["ight"]);
        assert!(Query::parse("a").too_vague());
        assert!(Query::parse("ab").too_vague());
        assert!(!Query::parse("abc").too_vague());
        assert!(!Query::parse("ab cd").too_vague());
    }

    #[test]
    fn search_matches_all_words_and_skips_excluded() {
        let i = index(&[
            "Music\\Aimer\\Daydream\\01 Insane Dream.flac",
            "Music\\Aimer\\Live\\01 Insane Dream (live).flac",
            "Music\\ZUTOMAYO\\勘冴えて悔しいわ.flac",
        ]);
        let r = i.search("aimer insane", &[], 10);
        assert_eq!(r.len(), 2);
        assert_eq!(i.search("aimer insane -live", &[], 10).len(), 1);
        assert_eq!(i.search("勘冴えて悔しいわ", &[], 10).len(), 1);
        assert_eq!(i.search("*ydream", &[], 10).len(), 1);
        assert!(i.search("aimer nothing", &[], 10).is_empty());
        assert_eq!(i.search("aimer insane", &[], 1).len(), 1);
        assert!(i.search("aimer insane", &["insane dream".into()], 10).is_empty());
        assert_eq!(r[0].attrs, vec![(1, 100)]);
        assert_eq!(r[0].ext, "flac");
    }

    #[test]
    fn browse_and_folder_group_by_directory() {
        let i = index(&["Music\\A\\x.mp3", "Music\\A\\0.mp3", "Music\\A\\y.mp3", "Music\\A\\B\\z.mp3", "Music\\C\\w.mp3"]);
        let b = i.browse();
        assert_eq!(b.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["Music\\A", "Music\\A\\B", "Music\\C"]);
        assert_eq!(b[0].files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["0.mp3", "x.mp3", "y.mp3"]);
        assert_eq!(i.folder("Music\\A").len(), 2);
        assert_eq!(i.folder("Music\\C").len(), 1);
    }

    #[test]
    fn resolve_only_known_paths() {
        let i = index(&["Music\\A\\x.mp3"]);
        let (p, size) = i.resolve("Music\\A\\x.mp3").unwrap();
        assert_eq!(p, Path::new("/m").join("A").join("x.mp3"));
        assert_eq!(size, 10);
        assert!(i.resolve("Music\\A\\..\\..\\secret.mp3").is_none());
        assert!(i.resolve("Music\\A\\y.mp3").is_none());
    }

    #[test]
    fn scan_reads_the_folder_without_touching_it() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("Library");
        std::fs::create_dir_all(lib.join("Artist").join("Album")).unwrap();
        std::fs::create_dir_all(lib.join(".musiclib")).unwrap();
        std::fs::write(lib.join("Artist").join("Album").join("01 Song.flac"), b"not really flac").unwrap();
        std::fs::write(lib.join("Artist").join("Album").join("notes.txt"), b"x").unwrap();
        std::fs::write(lib.join("Artist").join("Album").join("cover.jpg"), b"x").unwrap();
        std::fs::write(lib.join("Artist").join("Album").join("ambient.png"), b"x").unwrap();
        std::fs::write(lib.join(".musiclib").join("hidden.flac"), b"x").unwrap();
        let before = snapshot(&lib);
        let cache = dir.path().join("state").join("share.cache");
        let i = ShareIndex::scan(std::slice::from_ref(&lib), Some(&cache), ScanOptions::default());
        // The song and its cover; not the notes, the player's backdrop or
        // what is hidden.
        assert_eq!(i.file_count(), 2);
        assert_eq!(i.dir_count(), 1);
        assert_eq!(i.search("song", &[], 10)[0].name, "Library\\Artist\\Album\\01 Song.flac");
        assert!(cache.exists());
        // Rescanning with the cache reads the same.
        let again = ShareIndex::scan(std::slice::from_ref(&lib), Some(&cache), ScanOptions::default());
        assert_eq!(again.file_count(), 2);
        assert_eq!(snapshot(&lib), before);
    }

    /// Every path under `dir` with its size and modification time.
    pub(crate) fn snapshot(dir: &Path) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let md = e.metadata().unwrap();
                if md.is_dir() {
                    // NTFS updates folder times lazily; the files are what counts.
                    stack.push(e.path());
                    out.push((e.path(), 0, std::time::UNIX_EPOCH));
                } else {
                    out.push((e.path(), md.len(), md.modified().unwrap()));
                }
            }
        }
        out.sort();
        out
    }
}
