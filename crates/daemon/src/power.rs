//! The machine going to sleep: every connected device is told
//! ([`dent_link::Goodbye::Sleep`]) before it does, so a client reads the
//! machine as asleep at once rather than as gone after a silence
//! ([decision 0062](../../../docs/decisions/0062-liveness-and-retry.md)).
//! On macOS it comes from IOKit's system power notifications, on Linux from
//! logind's `PrepareForSleep`; elsewhere there's nothing yet, and a
//! sleeping machine reads as unreachable.

/// Call `changed(true)` each time the machine is about to sleep, and
/// `changed(false)` once it's awake again. Sleep waits for it (up to
/// IOKit's 30 s), so it should be quick.
#[cfg(target_os = "macos")]
pub fn watch(changed: impl Fn(bool) + Send + Sync + 'static) {
    mac::watch(Box::new(changed));
}

/// On Linux, a delay lock holds sleep back (up to logind's
/// `InhibitDelayMaxSec`, 5 s by default) until `changed(true)` returns.
/// Called inside the runtime.
#[cfg(target_os = "linux")]
pub fn watch(changed: impl Fn(bool) + Send + Sync + 'static) {
    linux::watch(std::sync::Arc::new(changed));
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn watch(_changed: impl Fn(bool) + Send + Sync + 'static) {}

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::Arc;

    use futures_util::StreamExt;
    use zbus::zvariant::OwnedFd;

    #[zbus::proxy(
        interface = "org.freedesktop.login1.Manager",
        default_service = "org.freedesktop.login1",
        default_path = "/org/freedesktop/login1"
    )]
    trait Manager {
        fn inhibit(&self, what: &str, who: &str, why: &str, mode: &str) -> zbus::Result<OwnedFd>;
        #[zbus(signal)]
        fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;
    }

    pub fn watch(changed: Arc<dyn Fn(bool) + Send + Sync>) {
        tokio::spawn(async move {
            if let Err(e) = run(changed).await {
                eprintln!("dent-daemon: can't hear when the machine sleeps: {e}");
            }
        });
    }

    /// The lock that makes sleep wait for the goodbyes; `None` when it's
    /// refused (the machine's policy), and then the goodbyes race the sleep.
    async fn hold(m: &ManagerProxy<'_>) -> Option<OwnedFd> {
        m.inhibit(
            "sleep",
            "dent",
            "Telling devices this machine is going to sleep",
            "delay",
        )
        .await
        .map_err(|e| eprintln!("dent-daemon: sleep won't wait for devices to be told: {e}"))
        .ok()
    }

    async fn run(changed: Arc<dyn Fn(bool) + Send + Sync>) -> zbus::Result<()> {
        let bus = zbus::Connection::system().await?;
        let m = ManagerProxy::new(&bus).await?;
        let mut sleeps = m.receive_prepare_for_sleep().await?;
        let mut lock = hold(&m).await;
        while let Some(s) = sleeps.next().await {
            let start = s.args()?.start;
            let f = changed.clone();
            let _ = tokio::task::spawn_blocking(move || f(start)).await;
            if start {
                // Told: sleep can go ahead.
                lock = None;
            } else if lock.is_none() {
                lock = hold(&m).await;
            }
        }
        Ok(())
    }
}

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
    const SYSTEM_HAS_POWERED_ON: u32 = 0xe000_0300;

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
    static CHANGED: OnceLock<Box<dyn Fn(bool) + Send + Sync>> = OnceLock::new();

    extern "C" fn on_power(_: *mut c_void, _: IoObject, kind: u32, arg: *mut c_void) {
        let root = ROOT.load(Ordering::Relaxed);
        match kind {
            // Asked whether it may sleep (idle sleep): yes; the daemon
            // keeps it awake by other means while agents work.
            CAN_SYSTEM_SLEEP => unsafe {
                IOAllowPowerChange(root, arg as isize);
            },
            SYSTEM_WILL_SLEEP => {
                if let Some(f) = CHANGED.get() {
                    f(true);
                }
                unsafe {
                    IOAllowPowerChange(root, arg as isize);
                }
            }
            SYSTEM_HAS_POWERED_ON => {
                if let Some(f) = CHANGED.get() {
                    f(false);
                }
            }
            _ => {}
        }
    }

    pub fn watch(changed: Box<dyn Fn(bool) + Send + Sync>) {
        if CHANGED.set(changed).is_err() {
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
