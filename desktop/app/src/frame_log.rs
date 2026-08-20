//! An env-gated counter for diagnosing repaint storms.
//!
//! A GPUI window redraws whenever something marks it dirty, and a view that
//! marks itself dirty on a timer — or on every message from a background
//! service — will redraw at the display's full rate without ever looking wrong.
//! The symptom is latency, not a visual artifact: the main thread ends up
//! parked in the display link's `nextDrawable` and cannot service key events
//! promptly.
//!
//! Set `KNOTQ_FRAME_LOG=1` to get one line per second on stderr counting the
//! renders and the things that force them. A healthy idle app prints nothing;
//! a healthy app under typing prints roughly one render per keystroke.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

macro_rules! counters {
    ($($name:ident => $label:literal),+ $(,)?) => {
        $(pub(crate) static $name: AtomicU64 = AtomicU64::new(0);)+
        const COUNTERS: &[(&AtomicU64, &str)] = &[$((&$name, $label)),+];
    };
}

counters! {
    RENDERS => "renders",
    SYNC_RUNS => "sync_runs",
    WORKSPACE_REPLACED => "workspace_replaced",
    SCROLL_RESTORES => "scroll_restores",
    FORCED_REFRESHES => "forced_refreshes",
}

pub(crate) fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("KNOTQ_FRAME_LOG").is_ok_and(|value| value != "0" && !value.is_empty())
    })
}

/// Bumps `counter` and, at most once a second, prints every counter's rate.
///
/// Called from the render path, so the disabled case must stay a single relaxed
/// load of a `OnceLock`-backed bool.
pub(crate) fn count(counter: &AtomicU64) {
    if !enabled() {
        return;
    }
    counter.fetch_add(1, Ordering::Relaxed);
    report_if_due();
}

fn report_if_due() {
    static LAST: OnceLock<std::sync::Mutex<Instant>> = OnceLock::new();
    let last = LAST.get_or_init(|| std::sync::Mutex::new(Instant::now()));
    let mut last = match last.try_lock() {
        Ok(last) => last,
        // Another thread is already printing this second's line.
        Err(_) => return,
    };
    let elapsed = last.elapsed();
    if elapsed.as_secs_f64() < 1.0 {
        return;
    }
    *last = Instant::now();

    let secs = elapsed.as_secs_f64();
    let mut line = String::from("frame_log");
    let mut any = false;
    for (counter, label) in COUNTERS {
        let count = counter.swap(0, Ordering::Relaxed);
        if count > 0 {
            any = true;
        }
        line.push_str(&format!(" {label}={:.0}/s", count as f64 / secs));
    }
    if any {
        eprintln!("{line}");
    }
}
