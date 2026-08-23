//! Measuring the latency between typing a character and the frame that shows it.
//!
//! Everything here is off unless `KNOTQ_TYPING_TIMING` is set, and compiles to
//! a load of a cached `bool` when it is not. It exists because "typing feels
//! behind" is not a number, and the two halves of a keystroke — applying the
//! edit to the model, and re-laying-out the document — are paid in different
//! crates, so neither one alone tells you where the time went.
//!
//! A keystroke enters at `replace_text_in_range` and becomes visible at the end
//! of the `paint` that follows. [`mark_input`] stamps the start, [`finish_frame`]
//! reports at the end. Everything in between — the model command, the CRDT
//! update, relayout, painting — falls inside that window, which is the point:
//! it is measured from the same place the user is waiting.
//!
//! What it does NOT include is the compositor: the GPU work and the wait for
//! the display's next refresh happen after `paint` returns. Add roughly a frame
//! to every number here to get what an eye sees.
//!
//! ```sh
//! KNOTQ_TYPING_TIMING=1 app/local/run-app.sh          # measure
//! KNOTQ_TYPING_TIMING=1 KNOTQ_SHAPE_CACHE=0 …         # measure without line reuse
//! ```

use std::cell::RefCell;
use std::time::{Duration, Instant};

fn env_flag(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(value) => !(value == "0" || value.is_empty()),
        Err(_) => default,
    }
}

/// Whether the probe is on. Read once: this is checked on every relayout.
pub fn enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| env_flag("KNOTQ_TYPING_TIMING", false))
}

/// Whether a relayout may reuse the previous pass's shaped lines.
///
/// On by default — this is the optimisation, not an experiment. The switch
/// exists so the cost it removes can be measured in the same binary, against
/// the same document, instead of being inferred by comparing two builds.
pub fn shape_reuse_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| env_flag("KNOTQ_SHAPE_CACHE", true))
}

/// Whether paint may skip rows outside the viewport.
///
/// On by default — like [`shape_reuse_enabled`], the switch exists so the cost
/// it removes can be measured against the same document in the same binary,
/// rather than inferred by comparing two builds under two different loads.
pub fn paint_cull_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| env_flag("KNOTQ_PAINT_CULL", true))
}

#[derive(Default)]
struct Pending {
    input_at: Option<Instant>,
    relayout: Duration,
    reshaped: usize,
    rows: usize,
}

thread_local! {
    static PENDING: RefCell<Pending> = RefCell::new(Pending::default());
}

/// A character arrived. Later keystrokes inside one frame keep the FIRST stamp,
/// so a burst reports the age of the oldest character the frame is showing —
/// which is the latency the typist actually perceives.
pub fn mark_input() {
    if !enabled() {
        return;
    }
    PENDING.with(|pending| {
        let mut pending = pending.borrow_mut();
        if pending.input_at.is_none() {
            pending.input_at = Some(Instant::now());
        }
    });
}

/// One relayout finished: how long it took, and how many of the document's rows
/// it had to re-shape rather than reuse.
pub fn record_relayout(elapsed: Duration, reshaped: usize, rows: usize) {
    if !enabled() {
        return;
    }
    PENDING.with(|pending| {
        let mut pending = pending.borrow_mut();
        pending.relayout += elapsed;
        pending.reshaped += reshaped;
        pending.rows += rows;
    });
}

/// A frame finished painting. If it was showing a character that had not been
/// reported yet, report how long that took.
pub fn finish_frame() {
    if !enabled() {
        return;
    }
    PENDING.with(|pending| {
        let mut pending = pending.borrow_mut();
        let Some(input_at) = pending.input_at.take() else {
            return;
        };
        let total = input_at.elapsed().as_secs_f64() * 1000.0;
        let relayout = pending.relayout.as_secs_f64() * 1000.0;
        eprintln!(
            "[typing] {total:6.2}ms to screen  (relayout {relayout:5.2}ms, \
             reshaped {}/{} rows)",
            pending.reshaped, pending.rows
        );
        *pending = Pending::default();
    });
}
