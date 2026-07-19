//! Background, coalescing, atomic write-back.
//!
//! A [`Writer`] owns one background thread and a queue keyed by target path.
//! Callers hand it a **pre-serialized byte snapshot** of a document (a copy, made
//! on the caller's thread), so the worker never touches the live document tree —
//! there is no shared mutable state between them beyond the mutex-guarded queue.
//!
//! # Coalescing
//!
//! The queue holds at most one pending snapshot per path. Enqueuing a path that
//! is already pending does not add a second entry — it replaces the pending
//! bytes with the newer snapshot, so a burst of edits collapses to a single
//! write of the latest data.
//!
//! # Atomicity & portability
//!
//! Each write goes to a uniquely-named temp file in the target's directory, is
//! flushed to disk, then `rename`d over the target. On every mainstream OS a
//! same-directory rename is atomic; Rust's [`std::fs::rename`] replaces an
//! existing file on Windows too (`MOVEFILE_REPLACE_EXISTING`). A fallback
//! remove-then-rename covers filesystems that reject replace-on-rename.
//!
//! Per-file writes are atomic. Cross-file atomicity (a "transaction" spanning
//! several files) is **not** provided — see the crate docs for why that needs a
//! journal and is deliberately out of scope.

use std::collections::{HashMap, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

/// A write that failed on the background thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteError {
    /// The file that could not be written.
    pub path: PathBuf,
    /// The underlying error, rendered.
    pub message: String,
}

/// A queued file operation. One per path; a newer op replaces the pending one.
enum Op {
    /// Write these bytes (a document snapshot).
    Write(Vec<u8>),
    /// Delete the file (from a document removal).
    Delete,
}

/// Mutex-guarded queue state.
struct State {
    /// Latest pending operation per target path.
    pending: HashMap<PathBuf, Op>,
    /// FIFO of distinct pending paths (one entry per path).
    order: VecDeque<PathBuf>,
    /// Whether the worker is mid-write.
    active: bool,
    /// Set to request shutdown once the queue drains.
    shutdown: bool,
}

/// State shared between the enqueueing threads and the worker.
struct Shared {
    state: Mutex<State>,
    /// Signaled when work arrives or shutdown is requested.
    work: Condvar,
    /// Signaled when the queue is empty and the worker is idle.
    idle: Condvar,
    errors: Mutex<Vec<WriteError>>,
}

/// Handle to the background writer thread.
pub(crate) struct Writer {
    shared: Arc<Shared>,
    handle: Option<JoinHandle<()>>,
}

impl Writer {
    /// Spawns the background writer thread.
    pub(crate) fn start() -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                pending: HashMap::new(),
                order: VecDeque::new(),
                active: false,
                shutdown: false,
            }),
            work: Condvar::new(),
            idle: Condvar::new(),
            errors: Mutex::new(Vec::new()),
        });

        let worker = Arc::clone(&shared);
        let handle = std::thread::Builder::new()
            .name("datastore-writer".to_string())
            .spawn(move || run(worker))
            .expect("spawn datastore writer thread");

        Self {
            shared,
            handle: Some(handle),
        }
    }

    /// Queues a write of `bytes` to `path`, coalescing with any pending op for
    /// the same path (latest wins).
    pub(crate) fn write(&self, path: PathBuf, bytes: Vec<u8>) {
        self.enqueue(path, Op::Write(bytes));
    }

    /// Queues a delete of `path`, superseding any pending write for it (so an
    /// edit-then-remove ends in deletion, not a re-created file).
    pub(crate) fn delete(&self, path: PathBuf) {
        self.enqueue(path, Op::Delete);
    }

    fn enqueue(&self, path: PathBuf, op: Op) {
        {
            let mut state = self.shared.state.lock().unwrap();
            if !state.pending.contains_key(&path) {
                state.order.push_back(path.clone());
            }
            state.pending.insert(path, op);
        }
        self.shared.work.notify_one();
    }

    /// Blocks until every queued write has completed.
    pub(crate) fn flush(&self) {
        let mut state = self.shared.state.lock().unwrap();
        while !state.order.is_empty() || state.active {
            state = self.shared.idle.wait(state).unwrap();
        }
    }

    /// Drains and returns any write errors accumulated so far.
    pub(crate) fn take_errors(&self) -> Vec<WriteError> {
        std::mem::take(&mut *self.shared.errors.lock().unwrap())
    }
}

impl Drop for Writer {
    /// Requests shutdown, lets the worker drain the queue, then joins it — so
    /// dropping the owning store flushes every pending write.
    fn drop(&mut self) {
        {
            let mut state = self.shared.state.lock().unwrap();
            state.shutdown = true;
        }
        self.shared.work.notify_all();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Worker loop: drain the queue, writing each snapshot atomically. Exits only
/// once shutdown is requested *and* the queue is empty.
fn run(shared: Arc<Shared>) {
    loop {
        let job = {
            let mut state = shared.state.lock().unwrap();
            loop {
                if let Some(path) = state.order.pop_front() {
                    let op = state.pending.remove(&path).expect("pending/order consistent");
                    state.active = true;
                    break Some((path, op));
                }
                if state.shutdown {
                    break None;
                }
                state.active = false;
                shared.idle.notify_all();
                state = shared.work.wait(state).unwrap();
            }
        };

        match job {
            Some((path, op)) => {
                let result = match &op {
                    Op::Write(bytes) => atomic_write(&path, bytes),
                    Op::Delete => remove_file_idempotent(&path),
                };
                if let Err(error) = result {
                    shared.errors.lock().unwrap().push(WriteError {
                        path,
                        message: error.to_string(),
                    });
                }

                let mut state = shared.state.lock().unwrap();
                state.active = false;
                if state.order.is_empty() {
                    shared.idle.notify_all();
                }
            }
            None => return,
        }
    }
}

/// Global sequence for unique temp names (no wall-clock needed).
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Writes `bytes` to `path` as atomically as the OS allows: temp file in the
/// same directory, flushed, then renamed over the target.
fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let temp = temp_sibling(path);
    {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }

    match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(_) => {
            // Some filesystems refuse rename-onto-existing; remove then retry.
            let _ = fs::remove_file(path);
            let result = fs::rename(&temp, path);
            if result.is_err() {
                let _ = fs::remove_file(&temp);
            }
            result
        }
    }
}

/// Deletes `path`, treating an already-absent file as success.
fn remove_file_idempotent(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// A unique temp path beside `path`, in the same directory (so the rename is a
/// same-filesystem, atomic move). Uniqueness comes from the pid plus a counter.
fn temp_sibling(path: &Path) -> PathBuf {
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("data");
    path.with_file_name(format!(".{name}.{pid}.{seq}.tmp"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesces_repeated_paths_into_one_queue_entry() {
        // White-box: exercise the queue's coalescing directly, without a worker.
        let mut state = State {
            pending: HashMap::new(),
            order: VecDeque::new(),
            active: false,
            shutdown: false,
        };
        let path = PathBuf::from("settings.yaml");

        for revision in [b"v1".to_vec(), b"v2".to_vec(), b"v3".to_vec()] {
            if !state.pending.contains_key(&path) {
                state.order.push_back(path.clone());
            }
            state.pending.insert(path.clone(), Op::Write(revision));
        }
        // A later delete supersedes the pending write, still one queue entry.
        state.pending.insert(path.clone(), Op::Delete);

        assert_eq!(state.order.len(), 1, "one queue entry for the path");
        assert!(matches!(state.pending[&path], Op::Delete), "latest op wins");
    }
}
