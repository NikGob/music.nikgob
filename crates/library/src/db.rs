//! The [`Library`] handle: one writer connection plus a small pool of
//! read-only connections.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, RwLock};
use std::panic::Location;
use std::time::{Duration, Instant};

use nikgob_core::model::{DeviceId, Event, UnixMs};
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};

use crate::error::Result;
use crate::migrations;
use crate::util::{meta_get, meta_set};

/// Read-only connections kept for UI-adjacent reads.
const READ_POOL_SIZE: usize = 4;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// Pages one step of [`Library::backup_to`] copies (4 MB of 4 KB pages).
const BACKUP_STEP_PAGES: i32 = 1024;
/// The pause between two steps of a backup.
const BACKUP_PAUSE: Duration = Duration::from_millis(20);
/// Starts over a backup takes before it ends in one step.
const BACKUP_RESTARTS: u32 = 5;
/// A read or a write that waits this long for its connection is logged…
const SLOW_WAIT: Duration = Duration::from_millis(250);
/// …and a write that keeps the writer this long.
const SLOW_WRITE: Duration = Duration::from_millis(500);

/// Cheap-to-clone handle to the library database.
///
/// Writes serialise through one connection. Reads use a pool of read-only
/// connections (WAL lets them run while a long import transaction is open), so
/// a UI query never waits behind a write. [`Library::open_in_memory`] has no
/// pool: reads share the writer connection.
#[derive(Clone)]
pub struct Library {
    inner: Arc<Inner>,
}

/// The listening events of a stamp ([`crate::events::stamp`]).
pub(crate) type KeptEvents = ((i64, i64, u64), Arc<Vec<Event>>);

struct Inner {
    writer: Mutex<Connection>,
    /// Where the last write that took the writer was asked from.
    writing: Mutex<Option<&'static Location<'static>>>,
    /// See [`Library::listening_events`].
    listening: Mutex<Option<KeptEvents>>,
    readers: Option<ReadPool>,
    device: RwLock<DeviceId>,
    last_ts: AtomicI64,
    /// See [`Library::set_album_likes_tracks`]; the SQL reads it through
    /// `album_likes_tracks()`.
    album_likes_tracks: Arc<AtomicBool>,
    /// The database file (none in memory), for [`Library::backup_to`].
    path: Option<std::path::PathBuf>,
}

struct ReadPool {
    conns: Mutex<Vec<Connection>>,
    available: Condvar,
}

struct PooledConn<'a> {
    pool: &'a ReadPool,
    conn: Option<Connection>,
}

impl ReadPool {
    fn checkout(&self) -> PooledConn<'_> {
        let mut conns = lock(&self.conns);
        loop {
            if let Some(conn) = conns.pop() {
                return PooledConn {
                    pool: self,
                    conn: Some(conn),
                };
            }
            conns = self
                .available
                .wait(conns)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

impl Drop for PooledConn<'_> {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            lock(&self.pool.conns).push(conn);
            self.pool.available.notify_one();
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic while holding a connection leaves SQLite itself consistent
    // (open transactions roll back on drop), so poisoning is ignored.
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The functions the library's SQL calls: `album_likes_tracks()`, the
/// mode of [`Library::set_album_likes_tracks`]; `various_artists(name)`,
/// a compilation's artist line ([`nikgob_core::names::is_various_artists`]);
/// `foreign_script(name)`, a name in a script the user may not read
/// ([`nikgob_core::names::foreign_script`]).
fn add_functions(conn: &Connection, album_likes_tracks: &Arc<AtomicBool>) -> Result<()> {
    use rusqlite::functions::FunctionFlags;
    let flag = Arc::clone(album_likes_tracks);
    conn.create_scalar_function("album_likes_tracks", 0, FunctionFlags::SQLITE_UTF8, move |_| {
        Ok(flag.load(Ordering::Relaxed))
    })?;
    conn.create_scalar_function(
        "various_artists",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| Ok(nikgob_core::names::is_various_artists(&ctx.get::<String>(0)?)),
    )?;
    conn.create_scalar_function(
        "foreign_script",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| Ok(nikgob_core::names::foreign_script(&ctx.get::<String>(0)?)),
    )?;
    Ok(())
}

fn configure(conn: &Connection, writer: bool) -> Result<()> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    // Hot reads (queue rows, cards, likes) keep their statements prepared.
    conn.set_prepared_statement_cache_capacity(64);
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    // 16 MB of pages for the writer, 2 MB for each reader: their full-table
    // reads (recs, stats, export) would otherwise each keep a copy of most of
    // the database, which the system's file cache already holds once. No
    // memory mapping: on Windows a file mapped by the readers cannot be
    // truncated by the writer's checkpoint, and an I/O error in a mapping
    // ends the process instead of failing the query.
    conn.pragma_update(None, "cache_size", if writer { -16_000 } else { -2_000 })?;
    if writer {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
    } else {
        conn.pragma_update(None, "query_only", "ON")?;
    }
    Ok(())
}

impl Library {
    /// Opens (creating and migrating if needed) the database at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let mut writer = Connection::open(path)?;
        configure(&writer, true)?;
        migrations::migrate(&mut writer)?;

        let mut readers = Vec::with_capacity(READ_POOL_SIZE);
        for _ in 0..READ_POOL_SIZE {
            let conn = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            configure(&conn, false)?;
            readers.push(conn);
        }
        Self::from_parts(
            writer,
            Some(ReadPool {
                conns: Mutex::new(readers),
                available: Condvar::new(),
            }),
            Some(path.to_path_buf()),
        )
    }

    /// A private in-memory database (tests). Reads share the writer connection.
    pub fn open_in_memory() -> Result<Self> {
        let mut writer = Connection::open_in_memory()?;
        configure(&writer, true)?;
        migrations::migrate(&mut writer)?;
        Self::from_parts(writer, None, None)
    }

    fn from_parts(writer: Connection, readers: Option<ReadPool>, path: Option<std::path::PathBuf>) -> Result<Self> {
        let album_likes_tracks = Arc::new(AtomicBool::new(false));
        add_functions(&writer, &album_likes_tracks)?;
        if let Some(pool) = &readers {
            for conn in pool.conns.lock().unwrap_or_else(PoisonError::into_inner).iter() {
                add_functions(conn, &album_likes_tracks)?;
            }
        }
        let device = match meta_get(&writer, "device_id")? {
            Some(id) => DeviceId(id),
            None => {
                let id = DeviceId::random();
                meta_set(&writer, "device_id", id.as_str())?;
                id
            }
        };
        Ok(Self {
            inner: Arc::new(Inner {
                writer: Mutex::new(writer),
                writing: Mutex::new(None),
                listening: Mutex::new(None),
                readers,
                device: RwLock::new(device),
                last_ts: AtomicI64::new(0),
                album_likes_tracks,
                path,
            }),
        })
    }

    /// How an album like works: off (the default), it saves the album
    /// alone and the album reads as liked exactly while it is saved; on, it
    /// likes every track of the album too, and the album reads as liked
    /// while all of them are (see [`Self::like_release`]).
    pub fn set_album_likes_tracks(&self, on: bool) {
        self.inner.album_likes_tracks.store(on, Ordering::Relaxed);
    }

    pub fn album_likes_tracks(&self) -> bool {
        self.inner.album_likes_tracks.load(Ordering::Relaxed)
    }

    /// Identity stamped into `updated_by` / `Event::device`. Generated on first
    /// open and persisted.
    pub fn device_id(&self) -> DeviceId {
        self.inner
            .device
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Replaces the device id (e.g. with the node's public key once sync exists).
    pub fn set_device_id(&self, id: DeviceId) -> Result<()> {
        self.write(|tx| meta_set(tx, "device_id", id.as_str()))?;
        *self
            .inner
            .device
            .write()
            .unwrap_or_else(PoisonError::into_inner) = id;
        Ok(())
    }

    /// Timestamp for a user-data write: the wall clock, but strictly increasing
    /// within this handle, so two quick edits never share a timestamp (they
    /// would otherwise tie in last-writer-wins merges and in "recent" orderings).
    pub(crate) fn tick(&self) -> UnixMs {
        let now = crate::util::now_ms();
        let mut prev = self.inner.last_ts.load(Ordering::Relaxed);
        loop {
            let next = now.max(prev + 1);
            match self.inner.last_ts.compare_exchange_weak(
                prev,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return next,
                Err(actual) => prev = actual,
            }
        }
    }

    /// Schema version of the open database.
    pub fn schema_version(&self) -> Result<u32> {
        self.read(migrations::user_version)
    }

    /// Writes one consistent copy of the whole database into `dest` (a new
    /// or empty file) with SQLite's online backup, from a connection of its
    /// own (read-only; the readers' pool is never taken), in short steps of
    /// [`BACKUP_STEP_PAGES`] with a pause between them, so no read snapshot
    /// is held for long and the writer and checkpoints go on. A write in
    /// between makes SQLite start the copy over; after
    /// [`BACKUP_RESTARTS`] of those it ends in one step. Blocking: never
    /// call it from the player loop.
    pub fn backup_to(&self, dest: &Path) -> Result<()> {
        self.backup_in_steps(dest, BACKUP_STEP_PAGES)
    }

    pub(crate) fn backup_in_steps(&self, dest: &Path, step_pages: i32) -> Result<()> {
        use rusqlite::backup::{Backup, StepResult};
        let from = self
            .inner
            .path
            .clone()
            .ok_or_else(|| crate::error::LibraryError::Invalid("the library is in memory".into()))?;
        let src = Connection::open_with_flags(&from, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        src.busy_timeout(BUSY_TIMEOUT)?;
        let mut dst = Connection::open(dest)?;
        {
            let backup = Backup::new(&src, &mut dst)?;
            let mut waits = 0;
            let mut restarts = 0;
            let mut left = i32::MAX;
            loop {
                let pages = if restarts >= BACKUP_RESTARTS { -1 } else { step_pages };
                match backup.step(pages)? {
                    StepResult::Done => break,
                    StepResult::More => {
                        let now = backup.progress().remaining;
                        if now > left {
                            restarts += 1;
                        }
                        left = now;
                        std::thread::sleep(BACKUP_PAUSE);
                    }
                    StepResult::Busy | StepResult::Locked if waits < 50 => {
                        waits += 1;
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    _ => return Err(crate::error::LibraryError::Invalid("the library stayed busy for its copy".into())),
                }
            }
        }
        dst.close().map_err(|(_, e)| e)?;
        Ok(())
    }

    /// The listening events kept from the last read, with their stamp.
    pub(crate) fn listening_kept(&self) -> Option<KeptEvents> {
        lock(&self.inner.listening).clone()
    }

    pub(crate) fn keep_listening(&self, stamp: (i64, i64, u64), events: &[Event]) {
        *lock(&self.inner.listening) = Some((stamp, Arc::new(events.to_vec())));
    }

    /// Runs `f` against a read-only snapshot.
    #[track_caller]
    pub(crate) fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        match &self.inner.readers {
            Some(pool) => {
                let me = Location::caller();
                let asked = Instant::now();
                let pooled = pool.checkout();
                let waited = asked.elapsed();
                if waited >= SLOW_WAIT {
                    log::warn!("library: the read at {me} waited {} ms for a connection", waited.as_millis());
                }
                let conn = pooled.conn.as_ref().expect("checked-out connection");
                let tx = conn.unchecked_transaction()?;
                f(&tx)
            }
            None => {
                let conn = lock(&self.inner.writer);
                let tx = conn.unchecked_transaction()?;
                f(&tx)
            }
        }
    }

    /// Runs `f` on the writer connection outside any transaction (`VACUUM`).
    pub(crate) fn with_writer<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = lock(&self.inner.writer);
        f(&conn)
    }

    /// Runs `f` inside one write transaction (committed if `f` succeeds).
    /// There is one writer: a write that waits long behind another, or
    /// holds the others up long itself, is logged with both places.
    #[track_caller]
    pub(crate) fn write<T>(&self, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let me = Location::caller();
        let asked = Instant::now();
        let mut conn = match self.inner.writer.try_lock() {
            Ok(conn) => conn,
            Err(_) => {
                let holder = *lock(&self.inner.writing);
                let conn = lock(&self.inner.writer);
                let waited = asked.elapsed();
                if waited >= SLOW_WAIT {
                    let by = holder.map_or_else(|| "another write".to_owned(), |h| format!("the write at {h}"));
                    log::warn!("library: the write at {me} waited {} ms behind {by}", waited.as_millis());
                }
                conn
            }
        };
        *lock(&self.inner.writing) = Some(me);
        let began = Instant::now();
        let out = (|| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let out = f(&tx)?;
            tx.commit()?;
            Ok(out)
        })();
        let held = began.elapsed();
        if held >= SLOW_WRITE {
            log::warn!("library: the write at {me} held the writer for {} ms", held.as_millis());
        }
        out
    }

    /// [`Self::write`] whose transaction is rolled back whatever `f` returns.
    pub(crate) fn write_rolled_back<T>(&self, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let mut conn = lock(&self.inner.writer);
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let out = f(&tx);
        tx.rollback()?;
        out
    }
}

impl std::fmt::Debug for Library {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Library")
            .field("pooled_reads", &self.inner.readers.is_some())
            .finish_non_exhaustive()
    }
}
