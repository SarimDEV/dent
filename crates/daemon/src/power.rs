//! The machine going to sleep: every connected device is told
//! ([`dent_link::Goodbye::Sleep`]) before it does, so a client reads the
//! machine as asleep at once rather than as gone after a silence
//! ([decision 0062](../../../docs/decisions/0062-liveness-and-retry.md)).
//! On macOS it comes from IOKit's system power notifications; elsewhere
//! there's nothing yet, and a sleeping machine reads as unreachable.

/// Call `will_sleep` each time the machine is about to sleep. It has a
/// moment (sleep waits for it, up to IOKit's 30 s) and should be quick.
#[cfg(target_os = "macos")]
pub fn watch(will_sleep: impl Fn() + Send + Sync + 'static) {
    mac::watch(Box::new(will_sleep));
}

#[cfg(not(target_os = "macos"))]
pub fn watch(_will_sleep: impl Fn() + Send + Sync + 'static) {}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU32, Ordering};

    type IoConnect = u32;
    type IoObject = u32;
    type CfRef = *const c_void;
    type Callback = extern "C" fn(*mut c_void, IoObject, u32, *mut c_void);

    const CAN_SYSTEM_SLEEP: u32 = 0xe000_0270;
    const SYSTEM_WILL_SLEEP: u32 = 0xe000_0280;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IORegisterForSystemPower(
            refcon: *mut c_void,
            port: *mut *mut c_void,
            callback: Callback,
            notifier: *mut IoObject,
        ) -> IoConnect;
        fn IONotificationPortGetRunLoopSource(port: *mut c_void) -> CfRef;
        fn IOAllowPowerChange(root: IoConnect, id: isize) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopDefaultMode: CfRef;
        fn CFRunLoopGetCurrent() -> CfRef;
        fn CFRunLoopAddSource(rl: CfRef, source: CfRef, mode: CfRef);
        fn CFRunLoopRun();
    }

    static ROOT: AtomicU32 = AtomicU32::new(0);
    static WILL_SLEEP: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

    extern "C" fn on_power(_: *mut c_void, _: IoObject, kind: u32, arg: *mut c_void) {
        let root = ROOT.load(Ordering::Relaxed);
        match kind {
            // Asked whether it may sleep (idle sleep): yes; the daemon
            // keeps it awake by other means while agents work.
            CAN_SYSTEM_SLEEP => unsafe {
                IOAllowPowerChange(root, arg as isize);
            },
            SYSTEM_WILL_SLEEP => {
                if let Some(f) = WILL_SLEEP.get() {
                    f();
                }
                unsafe {
                    IOAllowPowerChange(root, arg as isize);
                }
            }
            _ => {}
        }
    }

    pub fn watch(will_sleep: Box<dyn Fn() + Send + Sync>) {
        if WILL_SLEEP.set(will_sleep).is_err() {
            return;
        }
        let spawned = std::thread::Builder::new()
            .name("power".into())
            .spawn(|| unsafe {
                let mut port: *mut c_void = std::ptr::null_mut();
                let mut notifier: IoObject = 0;
                let root = IORegisterForSystemPower(
                    std::ptr::null_mut(),
                    &mut port,
                    on_power,
                    &mut notifier,
                );
                if root == 0 || port.is_null() {
                    eprintln!("dent-daemon: can't hear when the machine sleeps");
                    return;
                }
                ROOT.store(root, Ordering::Relaxed);
                CFRunLoopAddSource(
                    CFRunLoopGetCurrent(),
                    IONotificationPortGetRunLoopSource(port),
                    kCFRunLoopDefaultMode,
                );
                CFRunLoopRun();
            });
        if let Err(e) = spawned {
            eprintln!("dent-daemon: can't hear when the machine sleeps: {e}");
        }
    }
}
