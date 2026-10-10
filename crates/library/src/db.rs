//! The [`Library`] handle: one writer connection plus a small pool of
//! read-only connections.

use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, RwLock};
use std::time::Duration;

use nikgob_core::model::{DeviceId, UnixMs};
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};

use crate::error::Result;
use crate::migrations;
use crate::util::{meta_get, meta_set};

/// Read-only connections kept for UI-adjacent reads.
const READ_POOL_SIZE: usize = 4;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

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

struct Inner {
    writer: Mutex<Connection>,
    readers: Option<ReadPool>,
    device: RwLock<DeviceId>,
    last_ts: AtomicI64,
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

fn configure(conn: &Connection, writer: bool) -> Result<()> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "cache_size", -16_000)?;
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
        )
    }

    /// A private in-memory database (tests). Reads share the writer connection.
    pub fn open_in_memory() -> Result<Self> {
        let mut writer = Connection::open_in_memory()?;
        configure(&writer, true)?;
        migrations::migrate(&mut writer)?;
        Self::from_parts(writer, None)
    }

    fn from_parts(writer: Connection, readers: Option<ReadPool>) -> Result<Self> {
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
                readers,
                device: RwLock::new(device),
                last_ts: AtomicI64::new(0),
            }),
        })
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

    /// Runs `f` against a read-only snapshot.
    pub(crate) fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        match &self.inner.readers {
            Some(pool) => {
                let pooled = pool.checkout();
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

    /// Runs `f` inside one write transaction (committed if `f` succeeds).
    pub(crate) fn write<T>(&self, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let mut conn = lock(&self.inner.writer);
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }
}

impl std::fmt::Debug for Library {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Library")
            .field("pooled_reads", &self.inner.readers.is_some())
            .finish_non_exhaustive()
    }
}
