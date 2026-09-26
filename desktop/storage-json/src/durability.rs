//! One durability barrier per bulk save instead of one per file.
//!
//! [`crate::files::write_atomic`]'s contract is "when this returns, the bytes
//! and the rename that published them are on the platter". On Apple platforms
//! the only syscall that provides that is `fcntl(F_FULLFSYNC)` — which is what
//! Rust's `File::sync_all`/`sync_data` both compile to there — and it is not a
//! per-file operation at all: it tells the *device* to flush its write cache,
//! measured in this repo at **4.9 ms** against 0.1 ms for the same write
//! without it (see `durable_writes`).
//!
//! That cost is invisible on an ordinary edit, which writes one scheme file and
//! one CRDT document. It is the whole story on a bulk save. Restoring a real
//! 170-scheme account onto a new device writes ~341 files; at two barriers each
//! (the file, then its directory) that is 682 device-cache flushes — measured
//! at **2.9 s of a 3.7 s** first sync, ~79% of it, for an ordering guarantee
//! the caller needs exactly once, at the end.
//!
//! Inside a [`with_durability_batch`] scope each write does only the cheap half
//! — `fsync`, which moves its bytes from the page cache down to the device —
//! and records its parent directory. The scope's commit then flushes each
//! distinct directory once and issues a single `F_FULLFSYNC`. Because that one
//! flush drains the device's cache, everything written inside the scope becomes
//! durable together, so the *scope's* guarantee is identical to what the
//! per-file barriers gave: nothing the bulk save wrote is reported durable
//! until the outermost scope has committed. Only the intermediate,
//! per-file-inside-the-batch guarantee is given up, and no caller has one —
//! a half-written bulk save is not a state any reader accepts.
//!
//! Scopes nest: only the outermost one commits, so a caller can put one barrier
//! around a workspace save *and* its paired CRDT-state save.
//!
//! The batch is a shared handle rather than plain thread-local data because the
//! bulk writers hand their files to a worker pool. A worker joins the batch
//! with [`join`]; a write on a thread that has not joined one behaves exactly
//! as it did before this existed.
//!
//! On non-Apple targets `sync_all` is already a plain `fsync`, so the batch
//! only collapses the redundant per-file directory flushes.

use std::cell::RefCell;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};

thread_local! {
    /// The batch this thread is part of, if any, and how many nested scopes on
    /// this thread opened it (only the outermost commits).
    static ACTIVE: RefCell<Option<(Batch, u32)>> = const { RefCell::new(None) };
}

/// A handle to the batch, shareable with the worker threads a bulk write uses.
pub(crate) type Batch = Arc<Mutex<BatchState>>;

#[derive(Default)]
pub(crate) struct BatchState {
    /// Distinct parent directories whose entries changed.
    dirs: HashSet<PathBuf>,
    /// A file written in this scope, used to issue the final device-cache
    /// flush. Any file on the volume would do — the flush is device-wide.
    witness: Option<PathBuf>,
}

/// Run `body` with atomic writes batching their durability barrier.
///
/// The barrier is issued before this returns, including when `body` fails: a
/// bulk save that got partway still wants what it wrote ordered ahead of
/// whatever the caller does next. A commit failure is reported only when
/// `body` itself succeeded, so the more specific error wins.
pub fn with_durability_batch<T>(body: impl FnOnce() -> Result<T>) -> Result<T> {
    if !crate::files::durable_writes() {
        return body();
    }
    /// Leaves the scope on the way out however the scope is left. A worker
    /// panic in a parallel bulk write is re-raised through here, and without
    /// this the thread would keep a batch nobody will ever commit — every
    /// later save on it would then skip its durability barrier.
    struct Scope;
    impl Drop for Scope {
        fn drop(&mut self) {
            ACTIVE.with(|active| {
                let mut active = active.borrow_mut();
                if let Some((_, depth)) = active.as_mut() {
                    *depth -= 1;
                    if *depth == 0 {
                        // The committing caller took it already on the success
                        // path; this only fires while unwinding.
                        *active = None;
                    }
                }
            });
        }
    }

    ACTIVE.with(|active| {
        let mut active = active.borrow_mut();
        match active.as_mut() {
            Some((_, depth)) => *depth += 1,
            None => *active = Some((Batch::default(), 1)),
        }
    });
    let scope = Scope;
    let result = body();
    let finished = ACTIVE.with(|active| {
        let mut active = active.borrow_mut();
        let Some((_, depth)) = active.as_mut() else {
            return None;
        };
        if *depth > 1 {
            return None;
        }
        active.take().map(|(batch, _)| batch)
    });
    // Dropped explicitly so the depth bookkeeping happens here rather than at
    // an arbitrary point below: this scope has either taken the batch (it is
    // the outermost) or left it to an outer one.
    drop(scope);
    let Some(batch) = finished else {
        return result;
    };
    let state = match Arc::try_unwrap(batch) {
        Ok(state) => state.into_inner().unwrap_or_default(),
        // A worker thread outlived the scope that shared the batch with it.
        // Bulk writers join their workers before returning, so this does not
        // happen; commit what we can see rather than skipping the barrier.
        Err(shared) => std::mem::take(&mut *shared.lock().unwrap_or_else(|e| e.into_inner())),
    };
    let committed = state.commit();
    match result {
        Ok(value) => committed.map(|()| value),
        Err(error) => Err(error),
    }
}

/// The batch the calling thread belongs to, to hand to a worker thread.
pub(crate) fn current() -> Option<Batch> {
    ACTIVE.with(|active| active.borrow().as_ref().map(|(batch, _)| Arc::clone(batch)))
}

/// Make `batch` the calling thread's batch for the duration of `body`.
///
/// Used by a bulk writer's worker threads so their writes land in the same
/// batch — and therefore under the same single barrier — as the thread that
/// opened it.
pub(crate) fn join<T>(batch: &Option<Batch>, body: impl FnOnce() -> T) -> T {
    let Some(batch) = batch else {
        return body();
    };
    // Worker threads are fresh, so there is nothing to restore; put the
    // previous value back regardless so this is safe on any thread.
    let previous = ACTIVE.with(|active| active.borrow_mut().replace((Arc::clone(batch), 1)));
    let result = body();
    ACTIVE.with(|active| *active.borrow_mut() = previous);
    result
}

/// Whether the calling thread is inside a batch.
pub(crate) fn batching() -> bool {
    ACTIVE.with(|active| active.borrow().is_some())
}

/// Note that `path` was published by a rename, so the batch's commit flushes
/// its directory (and can use it as the final flush's witness).
pub(crate) fn record(path: &Path) {
    ACTIVE.with(|active| {
        let active = active.borrow();
        let Some((batch, _)) = active.as_ref() else {
            return;
        };
        let Ok(mut state) = batch.lock() else {
            return;
        };
        if let Some(dir) = path.parent() {
            if !state.dirs.contains(dir) {
                state.dirs.insert(dir.to_path_buf());
            }
        }
        if state.witness.is_none() {
            state.witness = Some(path.to_path_buf());
        }
    });
}

impl BatchState {
    fn commit(self) -> Result<()> {
        for dir in &self.dirs {
            // Best effort per directory: a directory that has since been
            // removed (a save that pruned its own tree) must not fail the
            // save that wrote it.
            if let Ok(handle) = fs::File::open(dir) {
                let _ = flush_to_device(&handle);
            }
        }
        let Some(witness) = self.witness else {
            return Ok(());
        };
        // The one real barrier. `sync_all` is `fcntl(F_FULLFSYNC)` on Apple
        // platforms, which flushes the device's write cache and therefore
        // makes every write above durable, not just this file's.
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&witness)
            .and_then(|handle| handle.sync_all())
            .with_context(|| format!("flush batched writes via {}", witness.display()))
    }
}

/// Push a file's bytes from the page cache down to the device without asking
/// the device to flush its own cache.
///
/// This is the cheap half of a durable write, and on Apple platforms it is the
/// only half Rust's standard library will not give us: both `sync_all` and
/// `sync_data` are `fcntl(F_FULLFSYNC)` there, deliberately, because each is
/// documented to provide full durability on its own.
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) fn flush_to_device(file: &fs::File) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;
    // SAFETY: `file` owns the descriptor for the duration of the call.
    if unsafe { libc::fsync(file.as_raw_fd()) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
pub(crate) fn flush_to_device(file: &fs::File) -> std::io::Result<()> {
    file.sync_all()
}
