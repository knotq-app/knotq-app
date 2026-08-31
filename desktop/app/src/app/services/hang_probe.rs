//! Main-thread responsiveness watchdog.
//!
//! Every other typing instrument in this app measures work it can *attribute to
//! a keystroke*: `KNOTQ_TYPING_TIMING` times input-to-frame, and a sampling
//! profiler aggregates stacks. Neither can see the failure the user actually
//! reports — when the main thread blocks, the keystrokes queue in the window
//! server and each one still processes in a few milliseconds once the thread
//! drains, so a half-second freeze is reported as a handful of 4 ms samples.
//!
//! This measures the thread itself. A background task schedules a no-op onto the
//! main thread on a fixed cadence; the gap between when it was scheduled and
//! when it ran is, by definition, how long the main thread was unavailable.
//! Enabled with `KNOTQ_TYPING_TIMING=1`.

use std::time::{Duration as StdDuration, Instant};

use crate::app::KnotQApp;

/// How often to ping. Short enough to catch a stall, long enough to be free.
const PING_INTERVAL: StdDuration = StdDuration::from_millis(50);

/// Only report gaps past this. One dropped frame at 60 Hz is ~16 ms, so this is
/// several frames — comfortably past scheduling noise and into what a person
/// notices as a hitch.
const REPORT_THRESHOLD_MS: f64 = 80.0;

pub(crate) fn spawn(cx: &mut gpui::Context<KnotQApp>) {
    if !enabled() {
        return;
    }
    cx.spawn(
        async move |weak: gpui::WeakEntity<KnotQApp>, cx: &mut gpui::AsyncApp| {
            eprintln!(
                "[hang] main-thread watchdog started (reporting gaps > {REPORT_THRESHOLD_MS:.0}ms)"
            );
            let started = Instant::now();
            let mut worst: f64 = 0.0;
            let mut last_heartbeat = Instant::now();
            loop {
                // Timed across the await, NOT after it. `cx.spawn` resumes its
                // task on the FOREGROUND executor, so by the time any statement
                // after this await runs, the main thread has already been
                // reacquired -- the delay being measured is precisely the part
                // that happens before control comes back. Timing anything after
                // the await measures zero by construction, which is the mistake
                // the first version of this file made.
                let before = Instant::now();
                cx.background_executor().timer(PING_INTERVAL).await;
                let overshoot =
                    before.elapsed().as_secs_f64() * 1000.0 - PING_INTERVAL.as_secs_f64() * 1000.0;

                if weak.update(cx, |_app, _cx| {}).is_err() {
                    break; // window gone
                }

                if overshoot > worst {
                    worst = overshoot;
                }
                if overshoot > REPORT_THRESHOLD_MS {
                    eprintln!(
                        "[hang] main thread blocked {overshoot:8.1}ms  (at t+{:.1}s)",
                        started.elapsed().as_secs_f64()
                    );
                }
                // Proof of life. A watchdog that reports nothing is
                // indistinguishable from a broken one -- this run reports the
                // worst gap it has seen so a quiet log can be trusted.
                if last_heartbeat.elapsed() >= StdDuration::from_secs(20) {
                    eprintln!("[hang] alive; worst gap so far {worst:.1}ms");
                    last_heartbeat = Instant::now();
                    worst = 0.0;
                }
            }
        },
    )
    .detach();
}

fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("KNOTQ_TYPING_TIMING").is_ok_and(|value| value != "0" && !value.is_empty())
    })
}
