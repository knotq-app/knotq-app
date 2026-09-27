//! A Finder-style vibrant sidebar on macOS.
//!
//! GPUI can make a window non-opaque, but its own
//! `WindowBackgroundAppearance::Blurred` is not what an Apple sidebar uses: it
//! installs an `NSVisualEffectView` with the colorless `selection` material and
//! then, in an `updateLayer` override, strips the layer background, hides the
//! `CAChameleonLayer` that carries the desktop tint, and removes the saturation
//! filter. That is deliberate — Zed wants a neutral surface it tints itself —
//! but with the backdrop layers gone the window reads as plain glass: you can
//! see what is behind it, perfectly sharp.
//!
//! Finder's sidebar is `NSVisualEffectMaterialSidebar` blending *behind* the
//! window, untouched. So the app asks GPUI only for transparency and installs
//! that view itself, underneath everything GPUI draws.

use objc::runtime::{Object, BOOL, YES};
use objc::{class, msg_send, sel, sel_impl};

type Id = *mut Object;

/// `NSVisualEffectMaterialSidebar` — the material Finder, Mail and Notes use
/// for the sidebar, and the reason this exists at all.
const MATERIAL_SIDEBAR: i64 = 7;
/// `NSVisualEffectBlendingModeBehindWindow`: sample what is behind the window,
/// not what the app drew underneath.
const BLENDING_BEHIND_WINDOW: i64 = 0;
/// `NSVisualEffectStateActive`: keep the effect on even when the window is not
/// key, which is how a sidebar behaves.
const STATE_ACTIVE: i64 = 1;
const NS_VIEW_WIDTH_SIZABLE: u64 = 2;
const NS_VIEW_HEIGHT_SIZABLE: u64 = 16;
/// `NSWindowBelow`, so the effect view sits under GPUI's rendering layer.
const NS_WINDOW_BELOW: i64 = -1;

/// Install the effect view behind every window the app currently has.
///
/// Called once after the window opens. It is a no-op if it cannot find a window
/// or already installed one, so a second call is harmless and a failure costs
/// nothing but the blur.
pub fn install_sidebar_vibrancy() {
    unsafe {
        let app: Id = msg_send![class!(NSApplication), sharedApplication];
        if app.is_null() {
            return;
        }
        let windows: Id = msg_send![app, windows];
        if windows.is_null() {
            return;
        }
        let count: usize = msg_send![windows, count];
        for index in 0..count {
            let window: Id = msg_send![windows, objectAtIndex: index];
            if window.is_null() {
                continue;
            }
            install_in_window(window);
        }
    }
}

unsafe fn install_in_window(window: Id) {
    let content_view: Id = msg_send![window, contentView];
    if content_view.is_null() {
        return;
    }
    if first_effect_subview(content_view).is_some() {
        // GPUI's own blurred view, or a previous call's. Re-configuring it is
        // pointless: GPUI's subclass overrides `updateLayer` to strip exactly
        // the layers the blur needs, so it has to be replaced, not tuned.
        return;
    }

    let bounds: Bounds = msg_send![content_view, bounds];
    let effect: Id = msg_send![class!(NSVisualEffectView), alloc];
    let effect: Id = msg_send![effect, initWithFrame: bounds];
    if effect.is_null() {
        return;
    }
    let _: () = msg_send![effect, setMaterial: MATERIAL_SIDEBAR];
    let _: () = msg_send![effect, setBlendingMode: BLENDING_BEHIND_WINDOW];
    let _: () = msg_send![effect, setState: STATE_ACTIVE];
    let _: () = msg_send![
        effect,
        setAutoresizingMask: NS_VIEW_WIDTH_SIZABLE | NS_VIEW_HEIGHT_SIZABLE
    ];
    let _: () = msg_send![
        content_view,
        addSubview: effect
        positioned: NS_WINDOW_BELOW
        relativeTo: std::ptr::null_mut::<Object>()
    ];
}

/// Point the effect view's material at KnotQ's theme rather than the system's.
///
/// The material is light or dark according to the view's `NSAppearance`, which
/// otherwise follows macOS. Someone running KnotQ's light theme under a dark
/// system would get a dark frosted sidebar with dark labels on it, so the view
/// is told which one to be. Cheap and idempotent, but only applied on a change.
pub fn set_vibrancy_appearance(is_dark: bool) {
    use std::sync::atomic::{AtomicI8, Ordering};
    static APPLIED: AtomicI8 = AtomicI8::new(-1);
    let wanted = i8::from(is_dark);
    if APPLIED.load(Ordering::Relaxed) == wanted {
        return;
    }

    unsafe {
        let name = ns_string(if is_dark {
            "NSAppearanceNameDarkAqua"
        } else {
            "NSAppearanceNameAqua"
        });
        if name.is_null() {
            return;
        }
        let appearance: Id = msg_send![class!(NSAppearance), appearanceNamed: name];
        if appearance.is_null() {
            return;
        }
        let mut applied_to_any = false;
        for_each_effect_view(|effect| {
            applied_to_any = true;
            let _: () = msg_send![effect, setAppearance: appearance];
        });
        // GPUI renders the first frame *during* `open_window`, so the first call
        // here lands before the effect view has been installed. Only remember a
        // call that reached a view, or that one would latch and the sidebar
        // would keep the system's appearance for the rest of the session.
        if applied_to_any {
            APPLIED.store(wanted, Ordering::Relaxed);
        }
    }
}

unsafe fn ns_string(value: &str) -> Id {
    let Ok(c_string) = std::ffi::CString::new(value) else {
        return std::ptr::null_mut();
    };
    msg_send![class!(NSString), stringWithUTF8String: c_string.as_ptr()]
}

/// Run `body` for the effect view of every window the app has.
unsafe fn for_each_effect_view(mut body: impl FnMut(Id)) {
    let app: Id = msg_send![class!(NSApplication), sharedApplication];
    if app.is_null() {
        return;
    }
    let windows: Id = msg_send![app, windows];
    if windows.is_null() {
        return;
    }
    let count: usize = msg_send![windows, count];
    for index in 0..count {
        let window: Id = msg_send![windows, objectAtIndex: index];
        if window.is_null() {
            continue;
        }
        let content_view: Id = msg_send![window, contentView];
        if content_view.is_null() {
            continue;
        }
        if let Some(effect) = first_effect_subview(content_view) {
            body(effect);
        }
    }
}

/// Is there already an `NSVisualEffectView` in this content view?
unsafe fn first_effect_subview(content_view: Id) -> Option<Id> {
    let subviews: Id = msg_send![content_view, subviews];
    if subviews.is_null() {
        return None;
    }
    let count: usize = msg_send![subviews, count];
    let effect_class = class!(NSVisualEffectView);
    for index in 0..count {
        let view: Id = msg_send![subviews, objectAtIndex: index];
        if view.is_null() {
            continue;
        }
        let is_effect: BOOL = msg_send![view, isKindOfClass: effect_class];
        if is_effect == YES {
            return Some(view);
        }
    }
    None
}

/// `NSRect`, which `objc` does not define for us.
#[repr(C)]
#[derive(Clone, Copy)]
struct Bounds {
    origin: (f64, f64),
    size: (f64, f64),
}

unsafe impl objc::Encode for Bounds {
    fn encode() -> objc::Encoding {
        unsafe { objc::Encoding::from_str("{CGRect={CGPoint=dd}{CGSize=dd}}") }
    }
}
