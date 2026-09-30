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
//!
//! Every call here is public AppKit. There is no supported way to blur harder
//! than a material does — `NSVisualEffectView` exposes no radius, and each
//! material carries the one Apple chose — so the material *is* the control, and
//! `KNOTQ_SIDEBAR_MATERIAL` picks among them.

use objc::runtime::{Object, BOOL, YES};
use objc::{class, msg_send, sel, sel_impl};

type Id = *mut Object;

/// `NSVisualEffectMaterial` values. `Sidebar` is what Finder, Mail and Notes
/// use, and is the default. The others are here because how see-through a
/// sidebar should be is a judgement call: the material is the only public
/// control over it, since `NSVisualEffectView` exposes no blur radius — each
/// material carries the one Apple chose.
const MATERIAL_SIDEBAR: i64 = 7;
const MATERIAL_UNDER_WINDOW_BACKGROUND: i64 = 21;
const MATERIAL_HUD_WINDOW: i64 = 13;
const MATERIAL_POPOVER: i64 = 6;
const MATERIAL_MENU: i64 = 5;
const MATERIAL_WINDOW_BACKGROUND: i64 = 12;

/// Which material to use, from `KNOTQ_SIDEBAR_MATERIAL`. Defaults to Finder's.
fn material() -> i64 {
    match std::env::var("KNOTQ_SIDEBAR_MATERIAL")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "under-window" => MATERIAL_UNDER_WINDOW_BACKGROUND,
        "hud" => MATERIAL_HUD_WINDOW,
        "popover" => MATERIAL_POPOVER,
        "menu" => MATERIAL_MENU,
        "window" => MATERIAL_WINDOW_BACKGROUND,
        _ => MATERIAL_SIDEBAR,
    }
}
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

/// Install the effect view behind every window the app currently has, and
/// report whether any window ended up with one.
///
/// Called once after the window opens. A second call is harmless: a window that
/// already has an effect view is left alone and still counts as installed.
///
/// The caller needs the return value, not a best effort. A failure used to cost
/// nothing but the blur, back when the look was opt-in; now that it is the macOS
/// default it costs the whole window, because the vibrant look has already made
/// that window transparent by the time this runs — so "no effect view" means
/// "see-through app", and the only safe response is to put the window back.
///
/// Every early return below is a shape AppKit should never hand us for an open
/// window. Reporting them is what turns a future break into a diagnosable
/// fallback rather than an invisible window.
#[must_use]
pub fn install_sidebar_vibrancy() -> bool {
    unsafe {
        let app: Id = msg_send![class!(NSApplication), sharedApplication];
        if app.is_null() {
            return false;
        }
        let windows: Id = msg_send![app, windows];
        if windows.is_null() {
            return false;
        }
        let count: usize = msg_send![windows, count];
        let mut installed_in_any = false;
        for index in 0..count {
            let window: Id = msg_send![windows, objectAtIndex: index];
            if window.is_null() {
                continue;
            }
            installed_in_any |= install_in_window(window);
        }
        installed_in_any
    }
}

/// Returns whether the window has an effect view when this returns.
unsafe fn install_in_window(window: Id) -> bool {
    let content_view: Id = msg_send![window, contentView];
    if content_view.is_null() {
        return false;
    }
    if first_effect_subview(content_view).is_some() {
        // Already vibrant, from an earlier call — nothing to do, and reporting
        // it as installed is correct. This cannot be GPUI's own blurred view:
        // `window_background_appearance` asks only for `Transparent`, never
        // `Blurred`, precisely because GPUI's subclass overrides `updateLayer`
        // to strip the layers the blur needs.
        return true;
    }

    let bounds: Bounds = msg_send![content_view, bounds];
    let effect: Id = msg_send![class!(NSVisualEffectView), alloc];
    let effect: Id = msg_send![effect, initWithFrame: bounds];
    if effect.is_null() {
        return false;
    }
    let _: () = msg_send![effect, setMaterial: material()];
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
    true
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
