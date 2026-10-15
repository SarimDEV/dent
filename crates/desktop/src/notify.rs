//! Notifications when a session needs you or finishes, while you're
//! looking elsewhere. macOS only shows them for a bundled app, so a
//! `cargo run` build stays quiet.

/// Whether notifications play their sound (dent › Notification Sound).
pub static SOUND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Take a session's notifications out of Notification Center: you've
/// opened it, so they've done their job.
pub fn clear(thread: &str) {
    #[cfg(target_os = "macos")]
    mac::clear(thread);
    #[cfg(not(target_os = "macos"))]
    let _ = thread;
}

/// The ids posted for `thread`, taken out of `posted`.
fn taken(posted: &mut Vec<(String, String)>, thread: &str) -> Vec<String> {
    let (gone, kept): (Vec<_>, Vec<_>) = posted.drain(..).partition(|(t, _)| t == thread);
    *posted = kept;
    gone.into_iter().map(|(_, id)| id).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn opening_a_session_takes_only_its_notifications() {
        let mut p = vec![
            ("a".to_string(), "a-1".to_string()),
            ("b".to_string(), "b-1".to_string()),
            ("a".to_string(), "a-2".to_string()),
        ];
        assert_eq!(super::taken(&mut p, "a"), ["a-1", "a-2"]);
        assert_eq!(p, [("b".to_string(), "b-1".to_string())]);
    }
}

/// The Mac's About panel for dent.
pub fn about() {
    #[cfg(target_os = "macos")]
    mac::about();
}

/// Ask once, at start, whether dent may notify.
pub fn init() {
    #[cfg(target_os = "macos")]
    mac::init();
}

/// The session whose notification was clicked, once.
pub fn take_clicked() -> Option<String> {
    #[cfg(target_os = "macos")]
    return mac::CLICKED.lock().ok().and_then(|mut c| c.take());
    #[cfg(not(target_os = "macos"))]
    None
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
    use std::sync::Mutex;

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{AnyThread, define_class, msg_send};
    use objc2_foundation::{NSBundle, NSError, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest,
        UNNotificationResponse, UNNotificationSound, UNUserNotificationCenter,
        UNUserNotificationCenterDelegate,
    };

    pub static CLICKED: Mutex<Option<String>> = Mutex::new(None);
    /// What's been posted and not cleared: (session, notification id).
    static POSTED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

    define_class!(
        // SAFETY: NSObject has no subclassing requirements, and this class
        // has no Drop.
        #[unsafe(super(NSObject))]
        #[name = "DentNotificationDelegate"]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl UNUserNotificationCenterDelegate for Delegate {
            /// A click on a notification opens its session.
            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive(
                &self,
                _center: &UNUserNotificationCenter,
                response: &UNNotificationResponse,
                done: &block2::DynBlock<dyn Fn()>,
            ) {
                let id = response
                    .notification()
                    .request()
                    .content()
                    .threadIdentifier()
                    .to_string();
                if let Ok(mut c) = CLICKED.lock() {
                    *c = Some(id);
                }
                done.call(());
            }
        }
    );

    /// The notification center refuses (and can abort) without a bundle id.
    fn bundled() -> bool {
        NSBundle::mainBundle().bundleIdentifier().is_some()
    }

    pub fn init() {
        if !bundled() {
            return;
        }
        let center = UNUserNotificationCenter::currentNotificationCenter();
        // The center holds its delegate weakly: this one lives for the app.
        let delegate: Retained<Delegate> = unsafe { msg_send![Delegate::alloc(), init] };
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        std::mem::forget(delegate);
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
        if super::SOUND.load(std::sync::atomic::Ordering::Relaxed) {
            content.setSound(Some(&UNNotificationSound::defaultSound()));
        }
        let key = format!("{thread}-{}", crate::message_id());
        if let Ok(mut p) = POSTED.lock() {
            p.push((thread.to_string(), key.clone()));
        }
        let id = NSString::from_str(&key);
        let request =
            UNNotificationRequest::requestWithIdentifier_content_trigger(&id, &content, None);
        UNUserNotificationCenter::currentNotificationCenter()
            .addNotificationRequest_withCompletionHandler(&request, None);
    }

    pub fn clear(thread: &str) {
        if !bundled() {
            return;
        }
        let ids: Vec<Retained<NSString>> = match POSTED.lock() {
            Ok(mut p) => super::taken(&mut p, thread)
                .iter()
                .map(|id| NSString::from_str(id))
                .collect(),
            Err(_) => return,
        };
        if ids.is_empty() {
            return;
        }
        let ids = objc2_foundation::NSArray::from_retained_slice(&ids);
        UNUserNotificationCenter::currentNotificationCenter()
            .removeDeliveredNotificationsWithIdentifiers(&ids);
    }

    pub fn about() {
        if let Some(mtm) = objc2::MainThreadMarker::new() {
            let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
            app.orderFrontStandardAboutPanel(None);
        }
    }
}
