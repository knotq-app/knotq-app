use crate::{Error, NotificationRequest, Result};
use objc::runtime::Object;
use objc::{class, msg_send, sel, sel_impl};
use std::ffi::CStr;
use std::os::raw::c_char;

const BUNDLE_REQUIRED: &str =
    "Notifications require running from a .app bundle. Use `cargo bundle --run` or open the .app in target/debug/bundle/osx/";
const BUNDLE_ID_REQUIRED: &str =
    "Notifications require a bundle identifier (CFBundleIdentifier in Info.plist)";

pub(super) struct AutoreleasePool(*mut Object);

impl AutoreleasePool {
    pub(super) unsafe fn new() -> Self {
        let pool: *mut Object = msg_send![class!(NSAutoreleasePool), new];
        Self(pool)
    }
}

impl Drop for AutoreleasePool {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send![self.0, drain];
        }
    }
}

pub(super) unsafe fn notification_center() -> Result<*mut Object> {
    if let Some(reason) = bundle_unavailable_reason() {
        return Err(Error::Unavailable(reason));
    }
    let center: *mut Object =
        msg_send![class!(UNUserNotificationCenter), currentNotificationCenter];
    if center.is_null() {
        Err(Error::Unavailable("UNUserNotificationCenter returned nil"))
    } else {
        Ok(center)
    }
}

pub(super) unsafe fn bundle_unavailable_reason() -> Option<&'static str> {
    let bundle: *mut Object = msg_send![class!(NSBundle), mainBundle];
    if bundle.is_null() {
        return Some(BUNDLE_REQUIRED);
    }

    let bundle_url: *mut Object = msg_send![bundle, bundleURL];
    if bundle_url.is_null() {
        return Some(BUNDLE_REQUIRED);
    }
    let path_obj: *mut Object = msg_send![bundle_url, path];
    let Some(path) = nsstring_to_string(path_obj) else {
        return Some(BUNDLE_REQUIRED);
    };
    if !path.ends_with(".app") {
        return Some(BUNDLE_REQUIRED);
    }

    let identifier: *mut Object = msg_send![bundle, bundleIdentifier];
    if identifier.is_null() {
        return Some(BUNDLE_ID_REQUIRED);
    }
    None
}

pub(super) unsafe fn nsstring(value: &str) -> *mut Object {
    let string: *mut Object = msg_send![class!(NSString), alloc];
    let string: *mut Object = msg_send![
        string,
        initWithBytes: value.as_ptr()
        length: value.len()
        encoding: 4usize
    ];
    let string: *mut Object = msg_send![string, autorelease];
    string
}

pub(super) unsafe fn nsstring_to_string(value: *mut Object) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let bytes: *const c_char = msg_send![value, UTF8String];
    if bytes.is_null() {
        return None;
    }
    Some(CStr::from_ptr(bytes).to_string_lossy().into_owned())
}

pub(super) unsafe fn object_to_string(value: *mut Object) -> Option<String> {
    if value.is_null() {
        return None;
    }
    nsstring_to_string(value).or_else(|| {
        let description: *mut Object = msg_send![value, description];
        nsstring_to_string(description)
    })
}

pub(super) unsafe fn nsstring_array(values: &[String]) -> *mut Object {
    let array: *mut Object = msg_send![class!(NSMutableArray), arrayWithCapacity: values.len()];
    for value in values {
        let value = nsstring(value);
        let _: () = msg_send![array, addObject: value];
    }
    array
}

pub(super) unsafe fn user_info_dictionary(request: &NotificationRequest) -> Option<*mut Object> {
    if request.user_info.is_empty() {
        return None;
    }

    let keys: *mut Object =
        msg_send![class!(NSMutableArray), arrayWithCapacity: request.user_info.len()];
    let values: *mut Object =
        msg_send![class!(NSMutableArray), arrayWithCapacity: request.user_info.len()];

    for (key, value) in &request.user_info {
        let key = nsstring(key);
        let value = nsstring(value);
        let _: () = msg_send![keys, addObject: key];
        let _: () = msg_send![values, addObject: value];
    }

    let dictionary: *mut Object = msg_send![
        class!(NSDictionary),
        dictionaryWithObjects: values
        forKeys: keys
    ];
    Some(dictionary)
}

pub(super) unsafe fn ns_error_description(error: *mut Object) -> String {
    let description: *mut Object = msg_send![error, localizedDescription];
    nsstring_to_string(description).unwrap_or_else(|| "unknown error".to_string())
}
