//! Notifications when a session needs you or finishes, while you're
//! looking elsewhere. macOS only shows them for a bundled app, so a
//! `cargo run` build stays quiet.

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
        content.setSound(Some(&UNNotificationSound::defaultSound()));
        let id = NSString::from_str(&format!("{thread}-{}", crate::message_id()));
        let request =
            UNNotificationRequest::requestWithIdentifier_content_trigger(&id, &content, None);
        UNUserNotificationCenter::currentNotificationCenter()
            .addNotificationRequest_withCompletionHandler(&request, None);
    }
}
