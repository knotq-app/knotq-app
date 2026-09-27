//! A small, dependency-free parallel map for the per-document work that
//! dominates a bulk sync.
//!
//! Materializing a workspace decodes every scheme document into its items, and
//! that work is independent per scheme: on a first sync restoring a real
//! 170-scheme account it is the single largest phase, and it runs twice (once
//! in the pull, once in the merge that adopts it). It is also embarrassingly
//! parallel, and every phone this ships to has at least four cores sitting idle
//! while it runs.
//!
//! Deliberately not `rayon`: this needs one function, a global thread pool would
//! outlive the one call that wants it, and the mobile core cares about binary
//! size. `std::thread::scope` borrows the input directly, so nothing has to be
//! cloned into the workers.
//!
//! Work is handed out one index at a time through an atomic cursor rather than
//! in equal slices, because scheme sizes are wildly uneven — a real account has
//! a median of ~22 items and one scheme with 3000 — and a static split finishes
//! as slowly as its unluckiest chunk.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Below this many items the serial path wins: thread spawn costs more than the
/// work, and this must stay invisible to the interactive edit path, which
/// materializes a handful of schemes at most.
const MIN_ITEMS_FOR_THREADS: usize = 24;

/// More workers than this stops helping (the work is memory-bound) and starts
/// competing with the UI thread on a phone's efficiency cores.
const MAX_WORKERS: usize = 8;

fn worker_count(items: usize) -> usize {
    if items < MIN_ITEMS_FOR_THREADS {
        return 1;
    }
    let available = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1);
    available.min(MAX_WORKERS).min(items).max(1)
}

/// Apply `map` to every element of `items`, returning the results in the input's
/// order.
///
/// Runs serially for small inputs. A panic in a worker is re-raised on the
/// calling thread, so this cannot silently drop results.
pub(crate) fn map_ordered<T, R, F>(items: &[T], map: F) -> Vec<R>
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync,
{
    let workers = worker_count(items.len());
    if workers <= 1 {
        return items.iter().map(map).collect();
    }
    let cursor = AtomicUsize::new(0);
    let map = &map;
    let cursor = &cursor;
    let mut gathered: Vec<(usize, R)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(move || {
                    let mut mine: Vec<(usize, R)> = Vec::new();
                    loop {
                        let index = cursor.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(index) else {
                            break;
                        };
                        mine.push((index, map(item)));
                    }
                    mine
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| match handle.join() {
                Ok(results) => results,
                // Never swallow a worker panic into a short result vector: that
                // would silently materialize a workspace with schemes missing.
                Err(payload) => std::panic::resume_unwind(payload),
            })
            .collect()
    });
    gathered.sort_unstable_by_key(|(index, _)| *index);
    gathered.into_iter().map(|(_, value)| value).collect()
}

/// [`map_ordered`], for work that needs unique access to each element.
///
/// Used where the elements are disjoint `&mut` borrows out of one collection —
/// each scheme document in a merge, for instance — so the work is independent
/// even though it mutates.
pub(crate) fn map_ordered_mut<T, R, F>(items: &mut [T], map: F) -> Vec<R>
where
    T: Send,
    R: Send,
    F: Fn(&mut T) -> R + Sync,
{
    let workers = worker_count(items.len());
    if workers <= 1 {
        return items.iter_mut().map(map).collect();
    }
    // Hand each worker its own disjoint slice of `&mut` elements. A shared
    // atomic cursor cannot be used here: two workers would need `&mut` to the
    // same slice to index it. The elements are re-chunked round-robin so an
    // uneven cost distribution still spreads across the workers.
    let mut lanes: Vec<Vec<(usize, &mut T)>> = (0..workers).map(|_| Vec::new()).collect();
    for (index, item) in items.iter_mut().enumerate() {
        lanes[index % workers].push((index, item));
    }
    let map = &map;
    let mut gathered: Vec<(usize, R)> = std::thread::scope(|scope| {
        let handles: Vec<_> = lanes
            .into_iter()
            .map(|lane| {
                scope.spawn(move || {
                    lane.into_iter()
                        .map(|(index, item)| (index, map(item)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| match handle.join() {
                Ok(results) => results,
                Err(payload) => std::panic::resume_unwind(payload),
            })
            .collect()
    });
    gathered.sort_unstable_by_key(|(index, _)| *index);
    gathered.into_iter().map(|(_, value)| value).collect()
}

/// Run every closure in `jobs`, returning the first error if any failed.
///
/// Used for bulk file writes, where the jobs are independent and each is
/// dominated by a syscall.
pub fn try_for_each<F>(jobs: Vec<F>) -> anyhow::Result<()>
where
    F: FnOnce() -> anyhow::Result<()> + Send,
{
    let workers = worker_count(jobs.len());
    if workers <= 1 {
        for job in jobs {
            job()?;
        }
        return Ok(());
    }
    let cursor = AtomicUsize::new(0);
    // Each job runs at most once, taken by whichever worker claims its index.
    let slots: Vec<std::sync::Mutex<Option<F>>> = jobs
        .into_iter()
        .map(|job| std::sync::Mutex::new(Some(job)))
        .collect();
    let slots = &slots;
    let cursor = &cursor;
    let errors: Vec<anyhow::Error> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(move || {
                    let mut mine: Vec<anyhow::Error> = Vec::new();
                    loop {
                        let index = cursor.fetch_add(1, Ordering::Relaxed);
                        let Some(slot) = slots.get(index) else {
                            break;
                        };
                        let job = slot.lock().ok().and_then(|mut slot| slot.take());
                        if let Some(job) = job {
                            if let Err(error) = job() {
                                mine.push(error);
                            }
                        }
                    }
                    mine
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| match handle.join() {
                Ok(errors) => errors,
                Err(payload) => std::panic::resume_unwind(payload),
            })
            .collect()
    });
    match errors.into_iter().next() {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_keep_the_input_order_on_both_paths() {
        for count in [1_usize, 23, 24, 200] {
            let items: Vec<usize> = (0..count).collect();
            let doubled = map_ordered(&items, |value| value * 2);
            assert_eq!(
                doubled,
                items.iter().map(|value| value * 2).collect::<Vec<_>>(),
                "{count} items"
            );
        }
    }

    #[test]
    fn mutable_work_keeps_the_input_order_on_both_paths() {
        for count in [1_usize, 23, 24, 200] {
            let mut items: Vec<usize> = (0..count).collect();
            let seen = map_ordered_mut(&mut items, |value| {
                *value += 1;
                *value
            });
            assert_eq!(seen, (1..=count).collect::<Vec<_>>(), "{count} items");
            assert_eq!(items, (1..=count).collect::<Vec<_>>(), "{count} items");
        }
    }

    #[test]
    fn every_job_runs_exactly_once() {
        let counter = std::sync::atomic::AtomicUsize::new(0);
        let jobs: Vec<_> = (0..500)
            .map(|_| {
                let counter = &counter;
                move || -> anyhow::Result<()> {
                    counter.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                }
            })
            .collect();
        try_for_each(jobs).expect("no job fails");
        assert_eq!(counter.load(Ordering::Relaxed), 500);
    }

    #[test]
    fn a_failing_job_is_reported() {
        let jobs: Vec<Box<dyn FnOnce() -> anyhow::Result<()> + Send>> = (0..100)
            .map(|index| -> Box<dyn FnOnce() -> anyhow::Result<()> + Send> {
                if index == 57 {
                    Box::new(|| Err(anyhow::anyhow!("job 57 failed")))
                } else {
                    Box::new(|| Ok(()))
                }
            })
            .collect();
        let error = try_for_each(jobs).expect_err("the failing job is reported");
        assert!(error.to_string().contains("job 57 failed"));
    }
}
