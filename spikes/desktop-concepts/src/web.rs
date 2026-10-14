//! The live preview: the system web view placed over a region of the GPUI
//! window. Files an agent wrote load from memory through a `dent://` scheme
//! with a policy that blocks the network, so they can't reach anything but
//! themselves. A dev server loads from its forwarded port and talks to its
//! own origin; links anywhere else open in your browser. A small script
//! reports errors and element picks back.
//!
//! The web view is a native layer above everything GPUI draws, so dent keeps
//! a snapshot of the page and draws it underneath. When a menu or the search
//! opens, the web view hides and the snapshot stands in, so the page never
//! blanks.
//!
//! On Wayland a web view can't be placed inside another app's surface, so
//! the page gets its own window there.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gpui_kit::*;
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::http::{Response, header::CONTENT_TYPE};
use wry::{Rect, WebView, WebViewBuilder};

#[derive(Clone, Debug)]
pub enum PageEvent {
    Loaded,
    Error(String),
    /// The person pointed at an element: its tag, visible text and a selector.
    Pick {
        tag: String,
        text: String,
        selector: String,
    },
    PickCancel,
    /// A link to somewhere other than the page itself.
    External(String),
    /// Text a check read off the page.
    Probe(String),
}

/// What the web view shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A file an agent wrote, at a version: offline, from memory.
    File { name: String, version: usize },
    /// A dev server on a forwarded port.
    Server(String),
}

/// Isolated and offline: own files, inline code and data URLs only.
const POLICY: &str =
    "default-src 'self' dent: data: blob: 'unsafe-inline'; connect-src 'none'; frame-src 'none'";

const SCRIPT: &str = r#"
(() => {
  const send = (o) => window.ipc.postMessage(JSON.stringify(o));
  window.addEventListener('error', (e) => {
    const file = (e.filename || '').split('/').pop();
    send({ t: 'error', msg: e.message + (file ? ' · ' + file + ':' + e.lineno : '') });
  });
  window.addEventListener('unhandledrejection', (e) => send({ t: 'error', msg: 'Unhandled: ' + String(e.reason) }));
  const err = console.error;
  console.error = (...a) => { send({ t: 'error', msg: a.map(String).join(' ') }); err(...a); };
  // Live reload keeps your place.
  addEventListener('beforeunload', () => sessionStorage.setItem('dent-y', String(scrollY)));
  // When the page settles after a change, dent takes a fresh snapshot.
  let settle = 0;
  const settled = () => { clearTimeout(settle); settle = setTimeout(() => send({ t: 'settled' }), 220); };
  for (const ev of ['scroll', 'click', 'input', 'keyup', 'resize']) addEventListener(ev, settled, { passive: true, capture: true });
  addEventListener('DOMContentLoaded', () => {
    const y = +sessionStorage.getItem('dent-y') || 0;
    if (y) scrollTo(0, y);
    send({ t: 'loaded' });
    new MutationObserver(settled).observe(document.documentElement, { subtree: true, childList: true, characterData: true, attributes: true });
  });
  addEventListener('load', settled);
  let picking = false, box = null;
  const path = (el) => {
    const parts = [];
    while (el && el.nodeType === 1 && parts.length < 4) {
      let p = el.tagName.toLowerCase();
      if (el.id) { parts.unshift(p + '#' + el.id); break; }
      if (el.classList.length) p += '.' + [...el.classList].join('.');
      parts.unshift(p);
      el = el.parentElement;
    }
    return parts.join(' > ');
  };
  window.__dentPick = (on) => {
    picking = on;
    document.documentElement.style.cursor = on ? 'crosshair' : '';
    if (!on && box) { box.remove(); box = null; }
  };
  const show = (el) => {
    const r = el.getBoundingClientRect();
    if (!box) {
      box = document.createElement('div');
      box.dataset.dent = '';
      box.style.cssText = 'position:fixed;pointer-events:none;z-index:2147483647;outline:2px solid #171614;box-shadow:0 0 0 4px rgba(250,248,243,.9);border-radius:3px;transition:all 90ms ease-out';
      document.documentElement.appendChild(box);
    }
    Object.assign(box.style, { left: r.left + 'px', top: r.top + 'px', width: r.width + 'px', height: r.height + 'px' });
  };
  window.__dentShow = (sel) => { const el = document.querySelector(sel); if (el) show(el); };
  document.addEventListener('mousemove', (e) => { if (picking) show(e.target); }, true);
  document.addEventListener('click', (e) => {
    if (!picking) return;
    e.preventDefault(); e.stopPropagation();
    const el = e.target;
    send({ t: 'pick', tag: el.tagName.toLowerCase(), text: (el.innerText || el.getAttribute('aria-label') || '').trim().slice(0, 60), sel: path(el) });
    window.__dentPick(false);
  }, true);
  document.addEventListener('keydown', (e) => { if (picking && e.key === 'Escape') { window.__dentPick(false); send({ t: 'pick-cancel' }); } }, true);
})();
"#;

/// A picture of the page as it last looked, and the size it was taken at.
#[derive(Clone)]
pub struct Snapshot {
    pub image: Arc<RenderImage>,
    pub size: Size<Pixels>,
    pub source: Source,
}

type Shared<T> = Arc<Mutex<T>>;

pub struct Preview {
    web: WebView,
    files: Shared<HashMap<String, Arc<[u8]>>>,
    events: Shared<Vec<PageEvent>>,
    /// Set from any thread when something arrives; the app checks it each tick.
    pending: Arc<AtomicBool>,
    /// The page changed since its last snapshot.
    settled: Arc<AtomicBool>,
    /// The dev server origin pages may navigate within.
    origin: Shared<Option<String>>,
    snapshot: Shared<Option<Snapshot>>,
    /// Pictures replaced by newer ones, for the window to free.
    retired: Shared<Vec<Arc<RenderImage>>>,
    /// A snapshot is wanted; taken at most every `SNAP_EVERY`.
    want_snapshot: std::cell::Cell<bool>,
    last_snapshot: std::cell::Cell<Option<std::time::Instant>>,
    /// The page on screen.
    pub showing: Option<Source>,
    bounds: Option<Bounds<Pixels>>,
    visible: bool,
    /// The page lives in its own window (Wayland), not inside dent's.
    #[cfg(target_os = "linux")]
    own_window: Option<gtk::Window>,
}

/// GPUI speaks XCB on X11; the web view wants an Xlib handle. Both name the
/// same X window.
#[cfg(target_os = "linux")]
struct X11Parent<'a>(&'a Window);

#[cfg(target_os = "linux")]
impl raw_window_handle::HasWindowHandle for X11Parent<'_> {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        use raw_window_handle::{RawWindowHandle, XlibWindowHandle};
        let raw = raw_window_handle::HasWindowHandle::window_handle(self.0)?.as_raw();
        let raw = match raw {
            RawWindowHandle::Xcb(h) => {
                RawWindowHandle::Xlib(XlibWindowHandle::new(h.window.get() as _))
            }
            other => other,
        };
        // SAFETY: the window outlives the web view, which is its child.
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(raw) })
    }
}

fn kind(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("css") => "text/css",
        Some("js") => "text/javascript",
        _ => "text/html; charset=utf-8",
    }
}

impl Preview {
    pub fn new(window: &Window) -> Option<Self> {
        let files: Shared<HashMap<String, Arc<[u8]>>> = Arc::default();
        let events: Shared<Vec<PageEvent>> = Arc::default();
        let pending = Arc::new(AtomicBool::new(false));
        let origin: Shared<Option<String>> = Arc::default();
        let snapshot: Shared<Option<Snapshot>> = Arc::default();
        let push = {
            let (events, pending) = (events.clone(), pending.clone());
            move |ev: PageEvent| {
                if let Ok(mut q) = events.lock() {
                    q.push(ev);
                }
                pending.store(true, Ordering::Release);
            }
        };
        let f = files.clone();
        let settled = Arc::new(AtomicBool::new(false));
        let ipc_settled = settled.clone();
        let (nav_origin, nav_push, win_push, ipc_push) =
            (origin.clone(), push.clone(), push.clone(), push.clone());
        let builder = WebViewBuilder::new()
            .with_custom_protocol("dent".into(), move |_, req| {
                let path = req.uri().path().trim_start_matches('/').to_string();
                let body = f.lock().ok().and_then(|m| m.get(&path).cloned());
                let (status, bytes): (u16, Vec<u8>) = match body {
                    Some(b) => (200, b.to_vec()),
                    None => (404, b"Not found".to_vec()),
                };
                Response::builder()
                    .status(status)
                    .header(CONTENT_TYPE, kind(&path))
                    .header("Content-Security-Policy", POLICY)
                    .body(Cow::Owned(bytes))
                    .unwrap_or_else(|_| Response::new(Cow::Borrowed(&[][..])))
            })
            .with_initialization_script(SCRIPT)
            // Pages stay on their own origin; anything else opens in the browser.
            .with_navigation_handler(move |url| {
                let own = url.starts_with("dent:")
                    || url.starts_with("about:")
                    || url.starts_with("data:")
                    || url.starts_with("blob:")
                    || nav_origin
                        .lock()
                        .ok()
                        .and_then(|o| o.clone())
                        .is_some_and(|o| url.starts_with(&o));
                if !own {
                    nav_push(PageEvent::External(url));
                }
                own
            })
            .with_new_window_req_handler(move |url, _| {
                win_push(PageEvent::External(url));
                wry::NewWindowResponse::Deny
            })
            .with_ipc_handler(move |req| {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(req.body()) else {
                    return;
                };
                let s = |k: &str| {
                    v.get(k)
                        .and_then(|x| x.as_str())
                        .unwrap_or_default()
                        .to_string()
                };
                ipc_push(match s("t").as_str() {
                    "loaded" => PageEvent::Loaded,
                    "error" => PageEvent::Error(s("msg")),
                    "pick" => PageEvent::Pick {
                        tag: s("tag"),
                        text: s("text"),
                        selector: s("sel"),
                    },
                    "pick-cancel" => PageEvent::PickCancel,
                    "probe" => PageEvent::Probe(s("msg")),
                    // Settling only makes the snapshot stale; it wakes nothing.
                    "settled" => {
                        ipc_settled.store(true, Ordering::Release);
                        return;
                    }
                    _ => return,
                });
            })
            .with_visible(false)
            // The composer keeps focus when a page is made.
            .with_focused(false);

        #[cfg(target_os = "linux")]
        let (web, own_window) = {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            let wayland = matches!(
                HasWindowHandle::window_handle(window).map(|h| h.as_raw()),
                Ok(RawWindowHandle::Wayland(_))
            );
            if wayland {
                use gtk::prelude::*;
                use wry::WebViewBuilderExtUnix;
                let w = gtk::Window::new(gtk::WindowType::Toplevel);
                w.set_default_size(900, 760);
                // Closing the window only hides it; dent shows it again.
                w.connect_delete_event(|w, _| {
                    w.hide();
                    gtk::glib::Propagation::Stop
                });
                let web = builder
                    .with_visible(true)
                    // Straight into the window, so the page fills it.
                    .build_gtk(&w)
                    .map_err(|e| eprintln!("preview: no web view ({e})"))
                    .ok()?;
                (web, Some(w))
            } else {
                let web = builder
                    .with_bounds(Rect {
                        position: LogicalPosition::new(0., 0.).into(),
                        size: LogicalSize::new(10., 10.).into(),
                    })
                    .build_as_child(&X11Parent(window))
                    .map_err(|e| eprintln!("preview: no web view ({e})"))
                    .ok()?;
                (web, None)
            }
        };
        #[cfg(not(target_os = "linux"))]
        let web = builder
            .with_bounds(Rect {
                position: LogicalPosition::new(0., 0.).into(),
                size: LogicalSize::new(10., 10.).into(),
            })
            .build_as_child(window)
            .map_err(|e| eprintln!("preview: no web view ({e})"))
            .ok()?;

        // wry activates the whole app whenever it makes a web view. A
        // background run hands activation straight back.
        #[cfg(target_os = "macos")]
        if crate::BACKGROUND.load(Ordering::Relaxed)
            && let Some(mtm) = objc2::MainThreadMarker::new()
        {
            objc2_app_kit::NSApplication::sharedApplication(mtm).deactivate();
        }

        Some(Self {
            web,
            files,
            events,
            pending,
            settled,
            origin,
            snapshot,
            retired: Arc::default(),
            want_snapshot: std::cell::Cell::new(false),
            last_snapshot: std::cell::Cell::new(None),
            showing: None,
            bounds: None,
            visible: false,
            #[cfg(target_os = "linux")]
            own_window,
        })
    }

    /// The page has its own window rather than a place in dent's.
    pub fn detached(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.own_window.is_some()
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }

    /// Show `bytes` as `name`, version `v`. Loading the same thing again is a no-op.
    pub fn show(&mut self, name: &str, v: usize, bytes: &[u8]) {
        let src = Source::File {
            name: name.to_string(),
            version: v,
        };
        if self.showing.as_ref() == Some(&src) {
            return;
        }
        let key = format!("{v}/{name}");
        if let Ok(mut m) = self.files.lock() {
            m.insert(key, bytes.into());
        }
        self.load(src);
    }

    /// Show a dev server. Pages may navigate within its origin.
    pub fn show_server(&mut self, url: &str) {
        let src = Source::Server(url.to_string());
        if self.showing.as_ref() == Some(&src) {
            return;
        }
        if let Ok(mut o) = self.origin.lock() {
            *o = Some(origin_of(url));
        }
        self.load(src);
    }

    fn load(&mut self, src: Source) {
        // A page in its own window is named after itself.
        #[cfg(target_os = "linux")]
        if let Some(w) = &self.own_window {
            use gtk::prelude::*;
            let name = match &src {
                Source::File { name, version } => {
                    format!("{} v{version}", name.rsplit('/').next().unwrap_or(name))
                }
                Source::Server(url) => url.clone(),
            };
            w.set_title(&format!("{name} · dent"));
        }
        let url = match &src {
            Source::File { name, version } => format!("dent://localhost/{version}/{name}"),
            Source::Server(url) => url.clone(),
        };
        self.showing = Some(src);
        let _ = self.web.load_url(&url);
    }

    pub fn reload(&mut self) {
        if let Some(src) = self.showing.take() {
            self.load(src);
        }
    }

    pub fn place(&mut self, b: Bounds<Pixels>) {
        if self.bounds == Some(b) || self.detached() {
            return;
        }
        let resized = self.bounds.is_none_or(|old| old.size != b.size);
        self.bounds = Some(b);
        let _ = self.web.set_bounds(Rect {
            position: LogicalPosition::new(b.origin.x.as_f32() as f64, b.origin.y.as_f32() as f64)
                .into(),
            size: LogicalSize::new(b.size.width.as_f32() as f64, b.size.height.as_f32() as f64)
                .into(),
        });
        if resized {
            // The page reflows at a new size; the page reports when it settles.
            let _ = self
                .web
                .evaluate_script("dispatchEvent(new Event('resize'))");
        }
    }

    pub fn set_visible(&mut self, on: bool) {
        if self.visible == on {
            return;
        }
        self.visible = on;
        #[cfg(target_os = "linux")]
        if let Some(w) = &self.own_window {
            use gtk::prelude::*;
            // Its own window comes forward when you open a page and steps
            // away when you leave it. Menus in dent never cover it, so it
            // doesn't hide for them (see `preview_visible`).
            if on {
                w.show_all();
                w.present();
            } else {
                w.hide();
            }
            return;
        }
        let _ = self.web.set_visible(on);
    }

    /// Bring a page's own window forward (Wayland).
    pub fn raise(&self) {
        #[cfg(target_os = "linux")]
        if let Some(w) = &self.own_window {
            use gtk::prelude::*;
            w.present();
        }
    }

    pub fn pick(&self, on: bool) {
        let _ = self
            .web
            .evaluate_script(&format!("window.__dentPick && window.__dentPick({on})"));
    }

    /// Scripted input for the walkthrough: point at an element and click it.
    pub fn script(&self, js: &str) {
        let _ = self.web.evaluate_script(js);
    }

    /// Something arrived since the last drain.
    pub fn pending(&self) -> bool {
        self.pending.load(Ordering::Acquire)
    }

    /// Events from the page. A load or a settle also refreshes the snapshot.
    pub fn drain(&self) -> Vec<PageEvent> {
        self.pending.store(false, Ordering::Release);
        let events = self
            .events
            .lock()
            .map(|mut q| std::mem::take(&mut *q))
            .unwrap_or_default();
        if events.iter().any(|e| matches!(e, PageEvent::Loaded)) {
            self.want_snapshot.set(true);
        }
        events
    }

    /// Take a wanted snapshot once enough time has passed since the last one.
    /// Returns true while one is still owed, so the caller checks back.
    pub fn tick(&self) -> bool {
        if self.settled.swap(false, Ordering::AcqRel) {
            self.want_snapshot.set(true);
        }
        // A hidden page can't be pictured; it asks again once it's back.
        if !self.want_snapshot.get() || !self.visible {
            return false;
        }
        let due = self
            .last_snapshot
            .get()
            .is_none_or(|at| at.elapsed() >= SNAP_EVERY);
        if due {
            self.want_snapshot.set(false);
            self.last_snapshot.set(Some(std::time::Instant::now()));
            self.take_snapshot();
        }
        self.want_snapshot.get()
    }

    /// Pictures the window should free from its atlas.
    pub fn retired(&self) -> Vec<Arc<RenderImage>> {
        self.retired
            .lock()
            .map(|mut r| std::mem::take(&mut *r))
            .unwrap_or_default()
    }

    /// The last picture of the page, if it still matches what's showing at
    /// this size.
    pub fn snapshot(&self, size: Size<Pixels>) -> Option<Snapshot> {
        let snap = self.snapshot.lock().ok()?.clone()?;
        let same_size = (snap.size.width - size.width).abs() < px(1.)
            && (snap.size.height - size.height).abs() < px(1.);
        (Some(&snap.source) == self.showing.as_ref() && same_size).then_some(snap)
    }

    /// A snapshot of what's showing exists, at any size.
    pub fn has_snapshot(&self) -> bool {
        self.snapshot
            .lock()
            .ok()
            .and_then(|s| s.as_ref().map(|s| Some(&s.source) == self.showing.as_ref()))
            .unwrap_or(false)
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Ask the web view for a picture of itself. It arrives a moment later.
    pub fn take_snapshot(&self) {
        let (Some(size), Some(source)) = (self.bounds.map(|b| b.size), self.showing.clone()) else {
            return;
        };
        if !self.visible {
            return;
        }
        let (slot, retired) = (self.snapshot.clone(), self.retired.clone());
        let store = move |bytes: Vec<u8>, format: ImageFormat| {
            let (slot, retired, source) = (slot.clone(), retired.clone(), source.clone());
            // Decoding a full-panel picture takes a few milliseconds; keep it
            // off the main thread.
            std::thread::spawn(move || {
                let Some(image) = decode(&bytes, format) else {
                    return;
                };
                if let Ok(mut s) = slot.lock() {
                    let old = s.replace(Snapshot {
                        image,
                        size,
                        source,
                    });
                    if let (Some(old), Ok(mut r)) = (old, retired.lock()) {
                        r.push(old.image);
                    }
                }
                // No redraw: the snapshot sits under the live page, and the
                // frame that hides the page draws it.
            });
        };
        snapshot::take(&self.web, store);
    }
}

const SNAP_EVERY: std::time::Duration = std::time::Duration::from_millis(700);

/// Into GPUI's BGRA layout.
fn decode(bytes: &[u8], format: ImageFormat) -> Option<Arc<RenderImage>> {
    let format = match format {
        ImageFormat::Tiff => image::ImageFormat::Tiff,
        _ => image::ImageFormat::Png,
    };
    let mut rgba = image::load_from_memory_with_format(bytes, format)
        .ok()?
        .into_rgba8();
    for px in rgba.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new([image::Frame::new(rgba)])))
}

fn origin_of(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, r)) => {
            let host = r.split('/').next().unwrap_or(r);
            format!("{scheme}://{host}")
        }
        None => url.to_string(),
    }
}

#[cfg(target_os = "macos")]
mod snapshot {
    use block2::RcBlock;
    use gpui_kit::ImageFormat;
    use objc2_app_kit::NSImage;
    use objc2_foundation::NSError;
    use wry::{WebView, WebViewExtMacOS};

    pub fn take(web: &WebView, store: impl Fn(Vec<u8>, ImageFormat) + 'static) {
        let view = web.webview();
        let handler = RcBlock::new(move |image: *mut NSImage, _err: *mut NSError| {
            // SAFETY: WebKit passes a valid image or null, on the main thread.
            let Some(image) = (unsafe { image.as_ref() }) else {
                return;
            };
            if let Some(data) = image.TIFFRepresentation() {
                store(data.to_vec(), ImageFormat::Tiff);
            }
        });
        // SAFETY: called on the main thread with a live web view.
        unsafe { view.takeSnapshotWithConfiguration_completionHandler(None, &handler) };
    }
}

#[cfg(target_os = "linux")]
mod snapshot {
    use gpui_kit::ImageFormat;
    use webkit2gtk::{SnapshotOptions, SnapshotRegion, WebViewExt};
    use wry::{WebView, WebViewExtUnix};

    pub fn take(web: &WebView, store: impl Fn(Vec<u8>, ImageFormat) + 'static) {
        web.webview().snapshot(
            SnapshotRegion::Visible,
            SnapshotOptions::NONE,
            None::<&gtk::gio::Cancellable>,
            move |result| {
                let Ok(surface) = result else { return };
                let Ok(image) = cairo::ImageSurface::try_from(surface) else {
                    return;
                };
                let mut png = Vec::new();
                if image.write_to_png(&mut png).is_ok() {
                    store(png, ImageFormat::Png);
                }
            },
        );
    }
}

#[cfg(target_os = "windows")]
mod snapshot {
    use gpui_kit::ImageFormat;
    use webview2_com::CapturePreviewCompletedHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG;
    use windows::Win32::System::Com::{IStream, STATFLAG_NONAME, STATSTG, STREAM_SEEK_SET};
    use windows::Win32::UI::Shell::SHCreateMemStream;
    use wry::{WebView, WebViewExtWindows};

    pub fn take(web: &WebView, store: impl Fn(Vec<u8>, ImageFormat) + 'static) {
        // SAFETY: an empty in-memory stream, owned here.
        let Some(stream): Option<IStream> = (unsafe { SHCreateMemStream(None) }) else {
            return;
        };
        let out = stream.clone();
        let handler = CapturePreviewCompletedHandler::create(Box::new(move |result| {
            if result.is_err() {
                return Ok(());
            }
            // SAFETY: the stream holds the PNG WebView2 just wrote.
            unsafe {
                let mut stat = STATSTG::default();
                out.Stat(&mut stat, STATFLAG_NONAME)?;
                out.Seek(0, STREAM_SEEK_SET, None)?;
                let mut png = vec![0u8; stat.cbSize as usize];
                let mut read = 0u32;
                let _ = out.Read(png.as_mut_ptr().cast(), png.len() as u32, Some(&mut read));
                png.truncate(read as usize);
                store(png, ImageFormat::Png);
            }
            Ok(())
        }));
        // SAFETY: called on the UI thread with a live web view.
        unsafe {
            let _ = web.webview().CapturePreview(
                COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
                &stream,
                &handler,
            );
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
mod snapshot {
    pub fn take(_: &wry::WebView, _: impl Fn(Vec<u8>, gpui_kit::ImageFormat) + 'static) {}
}
