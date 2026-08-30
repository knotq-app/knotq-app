//! macOS input-to-frame latency mitigation.
//!
//! GPUI normally draws from its CVDisplayLink callback. On a display in a
//! low-power state, the first callback after input can arrive noticeably late.
//! AppKit also exposes an immediate path through GPUI's layer-backed NSView:
//! `displayIfNeeded` invokes GPUI's `displayLayer:`, which renders and presents
//! with a Core Animation transaction before restarting the display link.
//!
//! This request must not run inline from `EntityInputHandler`: GPUI temporarily
//! removes its platform input handler while invoking us, and an inline frame
//! would re-enter it. Dispatching to the main queue runs after the current
//! AppKit input callback unwinds. The retained NSView keeps the public raw
//! window handle alive until that block completes.

use gpui::Window;

#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(target_os = "macos")]
static REQUEST_PENDING: AtomicBool = AtomicBool::new(false);

/// Request an immediate AppKit display pass after a text-input mutation.
///
/// This is macOS-only and can be disabled for same-binary A/B measurements with
/// `KNOTQ_MACOS_IMMEDIATE_INPUT_FRAME=0`.
pub(crate) fn request_after_text_input(window: &Window) {
    #[cfg(target_os = "macos")]
    request_after_text_input_macos(window);

    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

#[cfg(target_os = "macos")]
fn request_after_text_input_macos(window: &Window) {
    use dispatch::Queue;
    use objc::{
        msg_send,
        runtime::{Object, YES},
        sel, sel_impl,
    };
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    if !enabled() || !mark_request_pending(&REQUEST_PENDING) {
        return;
    }

    let Ok(window_handle) = HasWindowHandle::window_handle(window) else {
        REQUEST_PENDING.store(false, Ordering::Release);
        return;
    };
    let RawWindowHandle::AppKit(appkit_handle) = window_handle.as_raw() else {
        REQUEST_PENDING.store(false, Ordering::Release);
        return;
    };

    let view = appkit_handle.ns_view.as_ptr().cast::<Object>();
    // SAFETY: GPUI's public AppKit raw-window handle is its layer-backed NSView.
    // Text input runs on AppKit's main thread, so retaining it here is valid.
    unsafe {
        let _: *mut Object = msg_send![view, retain];
    }
    let view_address = view as usize;

    Queue::main().exec_async(move || {
        let view = view_address as *mut Object;
        // SAFETY: This block runs on the main queue and owns the retain above.
        // Both selectors are public NSView APIs. `displayIfNeeded` synchronously
        // reaches GPUI's `displayLayer:` implementation only after the input
        // callback that scheduled us has returned.
        unsafe {
            let _: () = msg_send![view, setNeedsDisplay: YES];
            let _: () = msg_send![view, displayIfNeeded];
            let _: () = msg_send![view, release];
        }
        REQUEST_PENDING.store(false, Ordering::Release);
    });
}

#[cfg(target_os = "macos")]
fn enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(
        || match std::env::var("KNOTQ_MACOS_IMMEDIATE_INPUT_FRAME") {
            Ok(value) => value != "0" && !value.is_empty(),
            Err(_) => true,
        },
    )
}

#[cfg(target_os = "macos")]
fn mark_request_pending(pending: &AtomicBool) -> bool {
    pending
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn pending_request_coalesces_until_completed() {
        let pending = AtomicBool::new(false);

        assert!(mark_request_pending(&pending));
        assert!(!mark_request_pending(&pending));
        pending.store(false, Ordering::Release);
        assert!(mark_request_pending(&pending));
    }
}
