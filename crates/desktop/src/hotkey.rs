//! A shortcut that works from any app: it brings dent forward with a new
//! session's field ready (decision 0051). macOS's `RegisterEventHotKey`
//! needs no Accessibility permission, unlike watching every key.

/// The one used until you pick another. The choices (dent › Global
/// Shortcut), by the name kept in prefs: "opt-shift-space" (⌥⇧Space),
/// "ctrl-opt-cmd-n" (⌃⌥⌘N), and "off".
pub const DEFAULT: &str = "opt-shift-space";

/// The shortcut chosen, by name; set from prefs before [`set`] uses it.
static CHOSEN: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// The chosen shortcut's name.
pub fn chosen() -> String {
    let c = CHOSEN.lock().map(|c| c.clone()).unwrap_or_default();
    if c.is_empty() { DEFAULT.to_string() } else { c }
}

/// Remember a choice without registering it (prefs, at start).
pub fn choose(name: &str) {
    if let Ok(mut c) = CHOSEN.lock() {
        *c = name.to_string();
    }
}

/// Presses arrive here, on the main thread; the app listens.
static PRESSED: std::sync::OnceLock<futures::channel::mpsc::UnboundedSender<()>> =
    std::sync::OnceLock::new();

/// Where presses are sent. Call once, before [`set`].
pub fn listen() -> futures::channel::mpsc::UnboundedReceiver<()> {
    let (tx, rx) = futures::channel::mpsc::unbounded();
    let _ = PRESSED.set(tx);
    rx
}

/// Use this shortcut from now on (a name from [`CHOICES`]; "off" for none).
pub fn set(name: &str) {
    choose(name);
    #[cfg(target_os = "macos")]
    mac::set(name);
    #[cfg(not(target_os = "macos"))]
    let _ = name;
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;
    use std::sync::Mutex;

    #[repr(C)]
    struct EventTypeSpec {
        class: u32,
        kind: u32,
    }

    #[repr(C)]
    struct HotKeyId {
        signature: u32,
        id: u32,
    }

    type Handler = extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32;

    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn GetApplicationEventTarget() -> *mut c_void;
        fn InstallEventHandler(
            target: *mut c_void,
            handler: Handler,
            count: u32,
            types: *const EventTypeSpec,
            data: *mut c_void,
            out: *mut *mut c_void,
        ) -> i32;
        fn RegisterEventHotKey(
            key: u32,
            modifiers: u32,
            id: HotKeyId,
            target: *mut c_void,
            options: u32,
            out: *mut *mut c_void,
        ) -> i32;
        fn UnregisterEventHotKey(hotkey: *mut c_void) -> i32;
    }

    const KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
    const PRESSED: u32 = 5;
    const CMD: u32 = 1 << 8;
    const SHIFT: u32 = 1 << 9;
    const OPTION: u32 = 1 << 11;
    const CONTROL: u32 = 1 << 12;
    const SPACE: u32 = 49;
    const N: u32 = 45;

    /// The registered shortcut (as an address, so it can sit in a static),
    /// and whether the handler is in.
    static STATE: Mutex<(usize, bool)> = Mutex::new((0, false));

    extern "C" fn pressed(_: *mut c_void, _: *mut c_void, _: *mut c_void) -> i32 {
        if let Some(tx) = super::PRESSED.get() {
            let _ = tx.unbounded_send(());
        }
        0
    }

    pub fn set(name: &str) {
        let Ok(mut st) = STATE.lock() else { return };
        // SAFETY: Carbon's event calls take the application target and the
        // structs above as declared; the hot key is only ever unregistered
        // once, from the address it was registered under.
        unsafe {
            if !st.1 {
                let spec = EventTypeSpec {
                    class: KEYBOARD,
                    kind: PRESSED,
                };
                let mut out = std::ptr::null_mut();
                InstallEventHandler(
                    GetApplicationEventTarget(),
                    pressed,
                    1,
                    &spec,
                    std::ptr::null_mut(),
                    &mut out,
                );
                st.1 = true;
            }
            if st.0 != 0 {
                UnregisterEventHotKey(st.0 as *mut c_void);
                st.0 = 0;
            }
            let (key, mods) = match name {
                "opt-shift-space" => (SPACE, OPTION | SHIFT),
                "ctrl-opt-cmd-n" => (N, CONTROL | OPTION | CMD),
                _ => return,
            };
            let id = HotKeyId {
                signature: u32::from_be_bytes(*b"dent"),
                id: 1,
            };
            let mut out = std::ptr::null_mut();
            if RegisterEventHotKey(key, mods, id, GetApplicationEventTarget(), 0, &mut out) == 0 {
                st.0 = out as usize;
            }
        }
    }
}
