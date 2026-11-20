use super::*;
use super::io::{blake3_plain, header, plain_meta};

// ------------------------------------------------------------ planning

/// A locator as a path (a relative one is the library folder's).
fn absolute(locator: &str, library: &Path) -> PathBuf {
    let p = Path::new(locator);
    if p.is_absolute() { p.to_owned() } else { library.join(p) }
}

/// The library folder's and the masters folder's real paths (those there).
fn real_roots(dirs: &Dirs) -> Vec<PathBuf> {
    [&dirs.library, &dirs.masters].into_iter().filter_map(|d| std::fs::canonicalize(d).ok()).collect()
}

/// A file here as it is now: its real path (links resolved) inside one of
/// `roots`, a plain file; nothing else.
fn real_file(path: &Path, roots: &[PathBuf]) -> Option<(PathBuf, Metadata)> {
    let real = std::fs::canonicalize(path).ok()?;
    if !roots.iter().any(|r| real.starts_with(r)) {
        return None;
    }
    let m = std::fs::metadata(&real).ok().filter(Metadata::is_file)?;
    Some((real, m))
}

/// When a file was changed, in unix ms.
fn mtime_ms(m: &Metadata) -> Option<i64> {
    m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_millis() as i64)
}

/// The files read again for their hash in one plan, at most.
pub(crate) struct Rehash {
    pub(super) files: usize,
    pub(super) bytes: u64,
}

impl Rehash {
    pub(crate) fn new() -> Self {
        Self { files: REHASH_FILES, bytes: REHASH_BYTES }
    }

    /// Whether a file of `size` bytes may still be read in this plan
    /// (counted when it may).
    pub(super) fn take(&mut self, size: u64) -> bool {
        let first = self.files == REHASH_FILES;
        if self.files == 0 || (size > self.bytes && !first) {
            return false;
        }
        self.files -= 1;
        self.bytes = self.bytes.saturating_sub(size);
        true
    }
}

/// What a file here hashes to.
enum Hashed {
    Known(String),
    /// Not read in this plan: it waits for the next.
    Later,
    Unreadable,
}

/// What the file at `real` (`m` its metadata) hashes to: what it hashed to
/// while its size and change time are the same, else read again when
/// `rehash` lets it (and noted for the next time).
fn hash_here(lib: &Library, real: &Path, m: &Metadata, rehash: &mut Rehash) -> Hashed {
    let key = real.to_string_lossy();
    let mtime = mtime_ms(m);
    if let Some(t) = mtime
        && let Ok(Some((size, at, hash))) = lib.file_stamp(&key)
        && size == m.len()
        && at == t
    {
        return Hashed::Known(hash);
    }
    if !rehash.take(m.len()) {
        return Hashed::Later;
    }
    let Ok(hash) = blake3_plain(real) else { return Hashed::Unreadable };
    if let Some(t) = mtime
        && let Err(e) = lib.put_file_stamp(&key, m.len(), t, &hash)
    {
        log::debug!("replica: the hash of {} not noted: {e}", real.display());
    }
    Hashed::Known(hash)
}

/// Notes what the file at `path` hashes to (by its real path, with its
/// size and change time now): a later plan takes it without reading it.
pub(super) fn stamp(lib: &Library, path: &Path, hash: &str) {
    let Ok(real) = std::fs::canonicalize(path) else { return };
    let Ok(m) = std::fs::metadata(&real) else { return };
    let Some(t) = mtime_ms(&m) else { return };
    if let Err(e) = lib.put_file_stamp(&real.to_string_lossy(), m.len(), t, hash) {
        log::debug!("replica: the hash of {} not noted: {e}", real.display());
    }
}

/// The songs' files this device holds now, each checked: a live local
/// source whose file is inside the library or the masters folder (links
/// resolved), a plain file, the very file its source names (its hash, as
/// [`hash_here`] knows it), its size as it is on the disk. And the tracks
/// with a file here not read yet (`rehash` ran out), with its bytes: they
/// wait for the next plan.
#[allow(clippy::type_complexity)]
pub(crate) fn held_on_disk(lib: &Library, dirs: &Dirs, rehash: &mut Rehash) -> nikgob_library::Result<(HashMap<TrackId, Vec<HeldFile>>, HashMap<TrackId, u64>)> {
    let roots = real_roots(dirs);
    let mut held: HashMap<TrackId, Vec<HeldFile>> = HashMap::new();
    let mut pending: HashMap<TrackId, u64> = HashMap::new();
    for mut f in lib.replica_held_files()? {
        let Some((real, m)) = real_file(&absolute(&f.locator, &dirs.library), &roots) else { continue };
        f.size = m.len();
        match hash_here(lib, &real, &m, rehash) {
            // A file whose source names another hash is not that file
            // any more.
            Hashed::Known(h) if f.file_hash.as_deref().is_none_or(|want| want == h) => {
                f.file_hash = Some(h);
                held.entry(f.track_id).or_default().push(f);
            }
            Hashed::Later => {
                let n = pending.entry(f.track_id).or_default();
                *n = n.saturating_add(f.size);
            }
            Hashed::Known(_) | Hashed::Unreadable => {}
        }
    }
    Ok((held, pending))
}

/// Whether `s` is a BLAKE3 as the library writes it (64 lowercase hex).
fn is_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// One file of master `hash` at `path`: held when it is that very file
/// inside `roots`, waiting when it was not read yet.
fn check_master(lib: &Library, path: &Path, roots: &[PathBuf], hash: &str, rehash: &mut Rehash, held: &mut HashSet<String>, pending: &mut HashSet<String>) {
    if held.contains(hash) {
        return;
    }
    let Some((real, m)) = real_file(path, roots) else { return };
    match hash_here(lib, &real, &m, rehash) {
        Hashed::Known(h) if h == hash => {
            held.insert(hash.to_owned());
        }
        Hashed::Later => {
            pending.insert(hash.to_owned());
        }
        Hashed::Known(_) | Hashed::Unreadable => {}
    }
}

/// The masters this device holds now, each checked to be its file inside
/// the masters folder (links resolved): those its master rows keep by
/// name, and the plain files kept flat (`{hash}.<ext>` right in the
/// masters folder, a disc image's too). And those with a file here not
/// read yet: they wait for the next plan.
pub(crate) fn masters_on_disk(lib: &Library, masters: &Path, rehash: &mut Rehash) -> nikgob_library::Result<(HashSet<String>, HashSet<String>)> {
    let (mut held, mut pending) = (HashSet::new(), HashSet::new());
    let Ok(root) = std::fs::canonicalize(masters) else { return Ok((held, pending)) };
    let roots = [root.clone()];
    for (h, p) in lib.master_paths()? {
        if let Some(p) = p {
            check_master(lib, Path::new(&p), &roots, &h, rehash, &mut held, &mut pending);
        }
    }
    if let Ok(rd) = std::fs::read_dir(&root) {
        for p in rd.flatten().map(|e| e.path()) {
            let side = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json") || e.eq_ignore_ascii_case("cue"));
            if side || !std::fs::symlink_metadata(&p).is_ok_and(|m| plain_meta(&m)) {
                continue;
            }
            if let Some(stem) = p.file_stem().map(|s| s.to_string_lossy().into_owned())
                && is_hash(&stem)
            {
                check_master(lib, &p, &roots, &stem, rehash, &mut held, &mut pending);
            }
        }
    }
    pending.retain(|h| !held.contains(h));
    Ok((held, pending))
}

/// What a plan starts from: the offers of the devices online, «Избранное»'s
/// order, what this device holds on its disk and the tracks whose file
/// here waits to be read.
struct Ground {
    offers: Vec<Offer>,
    keep: Keep,
    held: HashMap<TrackId, Vec<HeldFile>>,
    pending: HashMap<TrackId, u64>,
}

fn ground(lib: &Library, a: &Assigned, peers: &[String], dirs: &Dirs) -> nikgob_library::Result<Option<Ground>> {
    let keep = match a.profile {
        MeshProfile::Full | MeshProfile::Library => Keep::All,
        MeshProfile::Favorites { cap_bytes } => Keep::Ordered { order: lib.replica_order(a.liked, &a.playlists)?, cap: cap_bytes },
        MeshProfile::SoundOnly => return Ok(None),
    };
    let (held, pending) = held_on_disk(lib, dirs, &mut Rehash::new())?;
    Ok(Some(Ground { offers: lib.replica_offers(peers)?, keep, held, pending }))
}

/// The best files of «Избранное»'s tracks whose size no device told and
/// that were not asked about yet (`known`), in its order, at most
/// [`SIZE_ASKS`]; a track held (or whose file here waits to be read) is
/// not asked about.
fn sizes_to_ask(g: &Ground, known: &HashMap<String, Option<u64>>) -> Vec<Offer> {
    let Keep::Ordered { order, .. } = &g.keep else { return Vec::new() };
    let best = best_offers(&g.offers);
    let none: Vec<HeldFile> = Vec::new();
    let mut seen = HashSet::new();
    order
        .iter()
        .filter(|t| seen.insert(**t) && !g.pending.contains_key(t))
        .filter_map(|t| best.get(t))
        .filter(|o| o.size == 0 && !known.contains_key(&o.file_hash))
        .filter(|o| !held_satisfies(o, g.held.get(&o.track_id).unwrap_or(&none)))
        .take(SIZE_ASKS)
        .cloned()
        .collect()
}

/// The sizes learned put into the offers that told none.
fn learn(offers: &mut [Offer], sizes: &HashMap<String, Option<u64>>) {
    for o in offers.iter_mut().filter(|o| o.size == 0) {
        if let Some(Some(n)) = sizes.get(&o.file_hash) {
            o.size = *n;
        }
    }
}

/// The size of the file `hash` of track `track` on device `peer`, from the
/// answer to its first byte.
async fn size_of(sh: &Arc<Shared>, peer: &str, track: TrackId, hash: &str) -> Option<u64> {
    let uri = format!("/v1/media/tracks/{track}/audio?profile={}&hash={hash}", nikgob_view::MeshFormat::Orig.key());
    let req = http::Request::get(uri).header(http::header::RANGE, "bytes=0-0").body(Body::empty()).ok()?;
    let resp = tokio::time::timeout(HEAD_WAIT, sh.request(peer, req)).await.ok()?.ok()?;
    let total = match resp.status() {
        http::StatusCode::PARTIAL_CONTENT => header(&resp, "content-range").and_then(|v| v.rsplit_once('/')).and_then(|(_, n)| n.trim().parse().ok()),
        http::StatusCode::OK => header(&resp, "content-length").and_then(|v| v.trim().parse().ok()),
        _ => None,
    };
    drop(resp);
    total.filter(|n| *n > 0)
}

/// The masters to pull: those the devices list that this device neither
/// keeps nor holds a copy of (`held`), each with the devices listing it,
/// in hash order.
pub(crate) fn plan_masters(lists: &HashMap<String, (Instant, Vec<MasterItem>)>, held: &HashSet<String>) -> Vec<(MasterItem, Vec<String>)> {
    let mut by_hash: BTreeMap<String, (MasterItem, Vec<String>)> = BTreeMap::new();
    let mut peers: Vec<&String> = lists.keys().collect();
    peers.sort();
    for peer in peers {
        for item in &lists[peer].1 {
            if held.contains(&item.hash) {
                continue;
            }
            by_hash.entry(item.hash.clone()).or_insert_with(|| (item.clone(), Vec::new())).1.push(peer.clone());
        }
    }
    by_hash.into_values().collect()
}

// ------------------------------------------------- what a device is to hold

/// The bytes the plan `keep` gives a device and, of them, the bytes the
/// device holds: `offers` are the files the devices it pulls from have,
/// `held` its own (as its catalog tells them). A file of a track makes
/// the track's planned file unnecessary exactly as [`plan_tracks`] has
/// it: the very hash, or a better quality.
pub(super) fn planned_bytes(keep: &Keep, offers: Vec<Offer>, held: &[Offer]) -> (u64, u64) {
    let mut by_track: HashMap<TrackId, Vec<HeldFile>> = HashMap::new();
    for o in held {
        by_track.entry(o.track_id).or_default().push(HeldFile {
            track_id: o.track_id,
            file_hash: Some(o.file_hash.clone()),
            size: o.size,
            quality_score: o.quality_score,
            locator: String::new(),
        });
    }
    let none: Vec<HeldFile> = Vec::new();
    let mut done = 0u64;
    let mut goal = 0u64;
    for w in plan_tracks(keep, offers, &HashMap::new(), &HashMap::new()) {
        goal = goal.saturating_add(w.size);
        // The best offer is the first (`want_of` sorts them).
        if w.offers.first().is_some_and(|best| held_satisfies(best, by_track.get(&w.track_id).unwrap_or(&none))) {
            done = done.saturating_add(w.size);
        }
    }
    (done, goal)
}

/// The masters' bytes here and, of them, those a device holds a verified
/// copy of (`verified`: its `replica` hashes); a file shared by several
/// masters once.
pub(super) fn masters_bytes(masters: &[(String, u64)], verified: &HashSet<String>) -> (u64, u64) {
    masters.iter().fold((0u64, 0u64), |(goal, done), (hash, size)| {
        (goal.saturating_add(*size), if verified.contains(hash) { done.saturating_add(*size) } else { done })
    })
}

/// What the songs' part of this device's plan gives it and what of them it
/// holds, in bytes: the best file of every track the devices it pulls from
/// offer (`keep`: all of them) and, of those bytes, the files it holds —
/// `held` its checked files, a better quality satisfying as
/// [`plan_tracks`] has it. None: «Избранное»'s cap is another measure.
pub(super) fn songs_progress(keep: &Keep, offers: &[Offer], held: &HashMap<TrackId, Vec<HeldFile>>) -> Option<(u64, u64)> {
    if !matches!(keep, Keep::All) {
        return None;
    }
    // The size of the planned file of a track, told by any device that has
    // that very file (`want_of` takes the largest of them).
    let mut sizes: HashMap<(TrackId, &str), u64> = HashMap::new();
    for o in offers {
        let n = sizes.entry((o.track_id, o.file_hash.as_str())).or_default();
        *n = (*n).max(o.size);
    }
    let none: Vec<HeldFile> = Vec::new();
    let mut done = 0u64;
    let mut goal = 0u64;
    for best in best_offers(offers).into_values() {
        let size = sizes.get(&(best.track_id, best.file_hash.as_str())).copied().unwrap_or(best.size);
        goal = goal.saturating_add(size);
        if held_satisfies(&best, held.get(&best.track_id).unwrap_or(&none)) {
            done = done.saturating_add(size);
        }
    }
    Some((done, goal))
}

/// What the masters' part of this device's plan gives it and what of them
/// it holds, in bytes: every master the devices it pulls from list
/// (`lists`: a file shared by several once) and, of those bytes, the
/// masters whose files it holds verified (`held`).
pub(super) fn masters_progress(lists: &HashMap<String, (Instant, Vec<MasterItem>)>, held: &HashSet<String>) -> (u64, u64) {
    let mut sizes: BTreeMap<&str, u64> = BTreeMap::new();
    for (_, list) in lists.values() {
        for m in list {
            let n = sizes.entry(m.hash.as_str()).or_default();
            *n = (*n).max(m.size);
        }
    }
    sizes.into_iter().fold((0u64, 0u64), |(done, goal), (hash, size)| {
        (if held.contains(hash) { done.saturating_add(size) } else { done }, goal.saturating_add(size))
    })
}

/// The masters' part of a device's numbers, once every device of `peers`
/// listed its masters; None while one did not (the device's copy is not
/// known then).
pub(super) fn masters_plan(
    lists: &HashMap<String, (Instant, Vec<MasterItem>)>,
    peers: &[String],
    held: &HashSet<String>,
) -> Option<(u64, u64)> {
    peers.iter().all(|p| lists.contains_key(p)).then(|| masters_progress(lists, held))
}

/// What device `target` (profile `profile`) is to hold of this computer's
/// music and how much of it it holds, in bytes: the planned file of every
/// track (`peers`: the devices it pulls from, itself not among them) and,
/// with «Полная копия», every master. Its holdings are what its catalog
/// tells (`node_file` rows, synced here) and its `replica` rows for the
/// masters. None: no such plan, or nothing counted.
pub(crate) fn copy_stats(lib: &Library, target: &str, profile: MeshProfile, peers: &[String]) -> nikgob_library::Result<Option<MeshCopy>> {
    let keep = match profile {
        MeshProfile::Full | MeshProfile::Library => Keep::All,
        _ => return Ok(None),
    };
    let (mut done, mut goal) = planned_bytes(&keep, lib.replica_offers(peers)?, &lib.replica_offers(&[target.to_owned()])?);
    if profile == MeshProfile::Full {
        let (masters, held) = masters_bytes(&lib.master_sizes()?, &lib.replica_hashes(target)?);
        done = done.saturating_add(held);
        goal = goal.saturating_add(masters);
    }
    Ok((goal > 0).then_some(MeshCopy { done, goal }))
}

/// A new plan: what `a` wants and this device lacks.
pub(super) async fn replan(sh: &Arc<Shared>, lib: &Library, dirs: &Dirs, a: &Assigned, peers: &[String], st: &mut Puller, ctl: &Ctl) {
    let ground = {
        let (l, a, p, d) = (lib.clone(), a.clone(), peers.to_vec(), dirs.clone());
        match blocking(move || ground(&l, &a, &p, &d)).await {
            Ok(g) => g,
            Err(e) => {
                log::warn!("replica: the songs to keep not worked out: {e}");
                st.planned_at = Some(Instant::now());
                return;
            }
        }
    };
    let (songs, songs_seen) = match ground {
        None => (Vec::new(), None),
        Some(mut g) => {
            // «Избранное» charges each file with its real size: one no
            // device told is asked for it first, the sizes learned kept
            // for the next plans (so the asks go on to the next files; one
            // asked in vain is asked again once all the others were); one
            // still not known is never taken as free.
            learn(&mut g.offers, &st.sizes);
            let ask = sizes_to_ask(&g, &st.sizes);
            if ask.is_empty() {
                st.sizes.retain(|_, n| n.is_some());
            }
            for o in ask {
                let n = size_of(sh, &o.node_id, o.track_id, &o.file_hash).await;
                if st.sizes.len() >= SIZES_MAX {
                    st.sizes.clear();
                }
                st.sizes.insert(o.file_hash.clone(), n);
                tokio::time::sleep(PACE).await;
            }
            learn(&mut g.offers, &st.sizes);
            // What the plan gives this device and what of it it holds, as
            // its own disk tells (the cards' numbers).
            match tokio::task::spawn_blocking(move || {
                let seen = songs_progress(&g.keep, &g.offers, &g.held);
                (plan_tracks(&g.keep, g.offers, &g.held, &g.pending), seen)
            })
            .await
            {
                Ok((w, seen)) => (w, seen),
                Err(e) => {
                    log::warn!("replica: the songs to keep not worked out: {e}");
                    st.planned_at = Some(Instant::now());
                    return;
                }
            }
        }
    };
    let mut jobs: Vec<Job> = songs.into_iter().map(Job::Track).collect();
    // The masters' part of the numbers, once every device the plan pulls
    // from said what it has (None while one has not: the numbers stay as
    // they were).
    let mut masters_seen = Some((0u64, 0u64));
    if a.profile == MeshProfile::Full {
        for peer in peers {
            let fresh = st.masters.get(peer).is_some_and(|(at, _)| at.elapsed() < MASTERS_AGAIN);
            if fresh {
                continue;
            }
            match masters_of(sh, peer).await {
                Ok(list) => {
                    st.masters.insert(peer.clone(), (Instant::now(), list));
                }
                Err(e) => log::info!("replica: the masters of {peer} not listed: {e}"),
            }
        }
        st.masters.retain(|p, _| peers.contains(p));
        // Held, or a file here not read yet (it waits): not pulled.
        let (held, pending) = {
            let (l, m) = (lib.clone(), dirs.masters.clone());
            match blocking(move || masters_on_disk(&l, &m, &mut Rehash::new())).await {
                Ok((held, pending)) => (held, pending),
                Err(e) => {
                    log::warn!("replica: the masters here not read: {e}");
                    st.planned_at = Some(Instant::now());
                    return;
                }
            }
        };
        let skip: HashSet<String> = held.iter().chain(pending.iter()).cloned().collect();
        masters_seen = masters_plan(&st.masters, peers, &held);
        jobs.extend(plan_masters(&st.masters, &skip).into_iter().map(|(item, from)| Job::Master { item, from }));
    }
    jobs.retain(|j| !st.waiting(&j.key()));
    let all: u64 = jobs.iter().map(Job::size).sum();
    let goes_on = st.cut;
    st.cut = jobs.len() > QUEUE_MAX;
    jobs.truncate(QUEUE_MAX);
    if goes_on {
        st.total = ctl.received.load(Ordering::Relaxed) + all;
    } else {
        ctl.received.store(0, Ordering::Relaxed);
        st.total = all;
        st.last = None;
        st.speed = 0.0;
    }
    if !jobs.is_empty() {
        log::info!("replica: {} files to pull ({} MB)", jobs.len(), all >> 20);
    }
    st.queue = jobs.into();
    st.planned_at = Some(Instant::now());
    st.plan_for = Some(a.clone());
    if let (Some((done, goal)), Some((masters_done, masters_goal))) = (songs_seen, masters_seen) {
        st.copy = Some(MeshCopy {
            done: done.saturating_add(masters_done),
            goal: goal.saturating_add(masters_goal),
        });
    }
}

