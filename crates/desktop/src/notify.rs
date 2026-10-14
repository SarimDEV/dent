//! Notifications when a session needs you or finishes, while you're
//! looking elsewhere. macOS only shows them for a bundled app, so a
//! `cargo run` build stays quiet.

/// Ask once, at start, whether dent may notify.
pub fn init() {
    #[cfg(target_os = "macos")]
    mac::init();
}

/// Post a notification. `thread` groups a session's notifications.
pub fn post(title: &str, body: &str, thread: &str) {
    #[cfg(target_os = "macos")]
    mac::post(title, body, thread);
    #[cfg(not(target_os = "macos"))]
    let _ = (title, body, thread);
}

#[cfg(target_os = "macos")]
mod mac {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSBundle, NSError, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest,
        UNNotificationSound, UNUserNotificationCenter,
    };

    /// The notification center refuses (and can abort) without a bundle id.
    fn bundled() -> bool {
        NSBundle::mainBundle().bundleIdentifier().is_some()
    }

    pub fn init() {
        if !bundled() {
            return;
        }
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let done = RcBlock::new(|_ok: Bool, _err: *mut NSError| {});
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &done,
        );
    }

    pub fn post(title: &str, body: &str, thread: &str) {
        if !bundled() {
            return;
        }
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(title));
        content.setBody(&NSString::from_str(body));
        content.setThreadIdentifier(&NSString::from_str(thread));
        content.setSound(Some(&UNNotificationSound::defaultSound()));
        let id = NSString::from_str(&format!("{thread}-{}", crate::message_id()));
        let request =
            UNNotificationRequest::requestWithIdentifier_content_trigger(&id, &content, None);
        UNUserNotificationCenter::currentNotificationCenter()
            .addNotificationRequest_withCompletionHandler(&request, None);
    }
}
