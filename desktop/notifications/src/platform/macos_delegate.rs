use crate::{
    dispatch_response, NotificationResponse, ACTION_MARK_DONE, ACTION_SNOOZE_10_MINUTES,
    ACTION_SNOOZE_1_HOUR,
};
use block::Block;
use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Protocol, Sel};
use objc::{class, msg_send, sel, sel_impl};
use std::collections::BTreeMap;
use std::sync::OnceLock;

use super::support::{nsstring, nsstring_to_string, object_to_string};

const FOREGROUND_PRESENTATION_OPTIONS: u64 = 2 | 4 | 8 | 16; // sound, alert, list, banner

pub(super) unsafe fn configure_notification_center(center: *mut Object) {
    install_foreground_delegate(center);
    install_notification_categories(center);
}

unsafe fn install_foreground_delegate(center: *mut Object) {
    let delegate = notification_delegate();
    let _: () = msg_send![center, setDelegate: delegate];
}

unsafe fn install_notification_categories(center: *mut Object) {
    let identifier = nsstring("knotq-reminder");
    let actions: *mut Object = msg_send![class!(NSMutableArray), arrayWithCapacity: 3usize];
    add_notification_action(actions, ACTION_SNOOZE_10_MINUTES, "Snooze 10 min");
    add_notification_action(actions, ACTION_SNOOZE_1_HOUR, "Snooze 1 hour");
    add_notification_action(actions, ACTION_MARK_DONE, "Mark done");
    let intents: *mut Object = msg_send![class!(NSArray), array];
    let options: u64 = 0;
    let category: *mut Object = msg_send![
        class!(UNNotificationCategory),
        categoryWithIdentifier: identifier
        actions: actions
        intentIdentifiers: intents
        options: options
    ];
    let categories: *mut Object = msg_send![class!(NSSet), setWithObject: category];
    let _: () = msg_send![center, setNotificationCategories: categories];
}

unsafe fn add_notification_action(actions: *mut Object, identifier: &str, title: &str) {
    let identifier = nsstring(identifier);
    let title = nsstring(title);
    let options: u64 = 0;
    let action: *mut Object = msg_send![
        class!(UNNotificationAction),
        actionWithIdentifier: identifier
        title: title
        options: options
    ];
    if !action.is_null() {
        let _: () = msg_send![actions, addObject: action];
    }
}

unsafe fn notification_delegate() -> *mut Object {
    static DELEGATE: OnceLock<usize> = OnceLock::new();
    *DELEGATE.get_or_init(|| {
        let class = notification_delegate_class();
        let delegate: *mut Object = msg_send![class, new];
        delegate as usize
    }) as *mut Object
}

unsafe fn notification_delegate_class() -> &'static Class {
    static CLASS: OnceLock<usize> = OnceLock::new();
    let class = *CLASS.get_or_init(|| {
        let superclass = class!(NSObject);
        let mut decl = ClassDecl::new("KnotQNotificationDelegate", superclass)
            .expect("failed to allocate KnotQNotificationDelegate");
        if let Some(protocol) = Protocol::get("UNUserNotificationCenterDelegate") {
            decl.add_protocol(protocol);
        }
        decl.add_method(
            sel!(userNotificationCenter:willPresentNotification:withCompletionHandler:),
            will_present_notification
                as extern "C" fn(&Object, Sel, *mut Object, *mut Object, *mut Object),
        );
        decl.add_method(
            sel!(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:),
            did_receive_notification_response
                as extern "C" fn(&Object, Sel, *mut Object, *mut Object, *mut Object),
        );
        decl.register() as *const Class as usize
    });
    &*(class as *const Class)
}

extern "C" fn will_present_notification(
    _this: &Object,
    _cmd: Sel,
    _center: *mut Object,
    _notification: *mut Object,
    completion_handler: *mut Object,
) {
    if completion_handler.is_null() {
        return;
    }
    unsafe {
        let completion = completion_handler as *mut Block<(u64,), ()>;
        (*completion).call((FOREGROUND_PRESENTATION_OPTIONS,));
    }
}

extern "C" fn did_receive_notification_response(
    _this: &Object,
    _cmd: Sel,
    _center: *mut Object,
    response: *mut Object,
    completion_handler: *mut Object,
) {
    unsafe {
        if !response.is_null() {
            if let Some(parsed) = notification_response(response) {
                dispatch_response(parsed);
            }
        }
        if !completion_handler.is_null() {
            let completion = completion_handler as *mut Block<(), ()>;
            (*completion).call(());
        }
    }
}

unsafe fn notification_response(response: *mut Object) -> Option<NotificationResponse> {
    let action_id: *mut Object = msg_send![response, actionIdentifier];
    let notification: *mut Object = msg_send![response, notification];
    if notification.is_null() {
        return None;
    }
    let request: *mut Object = msg_send![notification, request];
    if request.is_null() {
        return None;
    }
    let identifier: *mut Object = msg_send![request, identifier];
    let content: *mut Object = msg_send![request, content];
    let user_info: *mut Object = if content.is_null() {
        std::ptr::null_mut()
    } else {
        msg_send![content, userInfo]
    };
    Some(NotificationResponse {
        notification_id: nsstring_to_string(identifier).unwrap_or_default(),
        action_id: nsstring_to_string(action_id).unwrap_or_default(),
        user_info: user_info_to_map(user_info),
    })
}

unsafe fn user_info_to_map(user_info: *mut Object) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    if user_info.is_null() {
        return map;
    }
    let keys: *mut Object = msg_send![user_info, allKeys];
    if keys.is_null() {
        return map;
    }
    let count: usize = msg_send![keys, count];
    for index in 0..count {
        let key: *mut Object = msg_send![keys, objectAtIndex: index];
        if key.is_null() {
            continue;
        }
        let value: *mut Object = msg_send![user_info, objectForKey: key];
        let Some(key) = object_to_string(key) else {
            continue;
        };
        if let Some(value) = object_to_string(value) {
            map.insert(key, value);
        }
    }
    map
}
