//! The live preview: the system web view placed over a region of the GPUI
//! window. Files an agent wrote load from memory through a `dent://` scheme
//! and may use the network like any page: fonts, styles and scripts from the
//! web, and requests out. A dev server loads from its forwarded port and talks to its
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
use wry::http::Response;
use wry::http::header::{ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE};
use wry::{Rect, WebView, WebViewBuilder};

#[derive(Clone, Debug)]
pub enum PageEvent {
    /// A load began, before any of the page's own scripts ran.
    Started,
    Loaded,
    /// An uncaught error or rejected promise: the page is broken.
    Error(String),
    /// A line the page logged with console.error: worth listing, not alarm.
    Logged(String),
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
    /// Text you copied in the page, for the clipboard.
    Copy(String),
    /// How many matches ⌘F found in the page.
    Found(usize),
    /// Files dropped on the page, to attach.
    Dropped(Vec<std::path::PathBuf>),
}

/// What the web view shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A file an agent wrote, at a version, from memory.
    File { name: String, version: usize },
    /// A dev server on a forwarded port.
    Server(String),
}

const SCRIPT: &str = r#"
(() => {
  const send = (o) => window.ipc.postMessage(JSON.stringify(o));
  // A load begins: what the last one reported no longer stands.
  send({ t: 'start' });
  window.addEventListener('error', (e) => {
    const file = (e.filename || '').split('/').pop();
    send({ t: 'error', msg: e.message + (file ? ' · ' + file + ':' + e.lineno : '') });
  });
  window.addEventListener('unhandledrejection', (e) => send({ t: 'error', msg: 'Unhandled: ' + String(e.reason) }));
  const err = console.error;
  console.error = (...a) => { send({ t: 'logged', msg: a.map(String).join(' ') }); err(...a); };
  // Live reload keeps your place, page by page: a new version of this page
  // opens where you were; another page opens at its top.
  const place = 'dent-y:' + location.host + location.pathname.replace(/^\/\d+\//, '/');
  addEventListener('beforeunload', () => sessionStorage.setItem(place, String(scrollY)));
  // When the page settles after a change, dent takes a fresh snapshot.
  let settle = 0;
  const settled = () => { clearTimeout(settle); settle = setTimeout(() => send({ t: 'settled' }), 220); };
  for (const ev of ['scroll', 'click', 'input', 'keyup', 'resize']) addEventListener(ev, settled, { passive: true, capture: true });
  // ⌘C and ⌘X reach dent first: it asks the page for what's selected.
  window.__dentCopy = (cut) => {
    const a = document.activeElement;
    let text = '';
    if (a && (a.tagName === 'INPUT' || a.tagName === 'TEXTAREA') && a.selectionStart != null) {
      text = a.value.substring(a.selectionStart, a.selectionEnd);
    } else {
      text = String(getSelection());
    }
    if (text) send({ t: 'copy', text });
    if (cut && text) document.execCommand('delete');
  };
  // ⌘F in the page: every match tinted, the current one stronger and
  // scrolled to, and the count sent back.
  window.__dentFind = (q, at, tint, strong) => {
    const H = window.CSS && CSS.highlights;
    if (H) { H.delete('dent-find'); H.delete('dent-find-now'); }
    if (!q) { send({ t: 'found', n: 0 }); return; }
    let style = document.querySelector('style[data-dent-find]');
    if (!style) {
      style = document.createElement('style');
      style.dataset.dentFind = '';
      document.documentElement.appendChild(style);
    }
    style.textContent = `::highlight(dent-find){background-color:${tint}}::highlight(dent-find-now){background-color:${strong}}`;
    const low = q.toLowerCase();
    const ranges = [];
    const walk = document.createTreeWalker(document.body || document.documentElement, NodeFilter.SHOW_TEXT, {
      acceptNode: (n) => {
        const p = n.parentElement;
        if (!p || p.closest('script,style,noscript,template,[data-dent]')) return NodeFilter.FILTER_REJECT;
        return p.getClientRects().length ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_REJECT;
      },
    });
    for (let n; (n = walk.nextNode()) && ranges.length < 10000;) {
      const t = n.data.toLowerCase();
      for (let i = t.indexOf(low); i >= 0; i = t.indexOf(low, i + low.length)) {
        const r = new Range();
        r.setStart(n, i);
        r.setEnd(n, Math.min(n.data.length, i + q.length));
        ranges.push(r);
      }
    }
    const now = ranges[Math.min(at, ranges.length - 1)];
    if (H && window.Highlight) {
      H.set('dent-find', new Highlight(...ranges));
      if (now) H.set('dent-find-now', new Highlight(now));
    }
    if (now) {
      const r = now.getBoundingClientRect();
      if (r.top < 0 || r.bottom > innerHeight) scrollBy(0, r.top - innerHeight / 3);
    }
    send({ t: 'found', n: ranges.length });
  };
  // A press inside the page: it may keep the keyboard.
  addEventListener('pointerdown', () => send({ t: 'down' }), { passive: true, capture: true });
  addEventListener('DOMContentLoaded', () => {
    const y = +sessionStorage.getItem(place) || 0;
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

/// Another machine's page or video: its own files (pictures, styles, the
/// video's ranges) are fetched through the link by the window, never
/// looked for on this Mac's disk ([decision 0068]).
///
/// [decision 0068]: ../../../docs/decisions/0068-files-across-machines.md
#[derive(Clone, Debug, PartialEq)]
pub struct FarRoot {
    /// The session, qualified with its machine.
    pub session: String,
    pub cwd: String,
    /// The folder on that machine the page's own files are found from.
    pub dir: String,
}

/// A file a far page asked for, waiting for the window to fetch it.
pub struct FarAsk {
    pub far: FarRoot,
    /// Its path from `far.dir`.
    pub rel: String,
    /// The `Range` asked for (a player's), if any.
    pub range: Option<String>,
    pub ty: &'static str,
    pub responder: wry::RequestAsyncResponder,
}

pub struct Preview {
    web: WebView,
    /// The showing page's machine, when it's another.
    far: Shared<Option<FarRoot>>,
    far_asks: Shared<Vec<FarAsk>>,
    files: Shared<HashMap<String, Arc<[u8]>>>,
    /// The folder a page's own images, styles and scripts load from.
    root: Shared<Option<std::path::PathBuf>>,
    /// The showing page's folder inside `root`, for links from the site's root.
    page_dir: Shared<String>,
    events: Shared<Vec<PageEvent>>,
    /// Set from any thread when something arrives; the app checks it each tick.
    pending: Arc<AtomicBool>,
    /// The page changed since its last snapshot.
    settled: Arc<AtomicBool>,
    /// You pressed inside the page since dent last had the keyboard: only
    /// then may the page keep it.
    pressed: Arc<AtomicBool>,
    /// The dev server origin pages may navigate within.
    origin: Shared<Option<String>>,
    snapshot: Shared<Option<Snapshot>>,
    /// The latest snapshot as the web view gave it, for a page's card in
    /// the chat to show; taken once by the app.
    still: Shared<Option<(Source, Vec<u8>, ImageFormat)>>,
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
    let ext = path
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "wasm" => "application/wasm",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "m4a" | "aac" => "audio/mp4",
        "wav" => "audio/wav",
        "aiff" | "aif" => "audio/aiff",
        "flac" => "audio/flac",
        "ogg" | "oga" => "audio/ogg",
        "txt" | "md" | "csv" => "text/plain; charset=utf-8",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
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
        let far: Shared<Option<FarRoot>> = Arc::default();
        let far_asks: Shared<Vec<FarAsk>> = Arc::default();
        let (fr, fa, far_pending) = (far.clone(), far_asks.clone(), pending.clone());
        let root: Shared<Option<std::path::PathBuf>> = Arc::default();
        let r = root.clone();
        let page_dir: Shared<String> = Arc::default();
        let pd = page_dir.clone();
        let settled = Arc::new(AtomicBool::new(false));
        let ipc_settled = settled.clone();
        let pressed = Arc::new(AtomicBool::new(false));
        let ipc_pressed = pressed.clone();
        let (nav_origin, nav_push, win_push, ipc_push, drop_push) = (
            origin.clone(),
            push.clone(),
            push.clone(),
            push.clone(),
            push.clone(),
        );
        let builder = WebViewBuilder::new()
            .with_asynchronous_custom_protocol("dent".into(), move |_, req, responder| {
                let mut path = req.uri().path().trim_start_matches('/').to_string();
                // Another machine's page: what isn't the page itself is the
                // window's to fetch from there.
                if let Some(far) = fr.lock().ok().and_then(|f| f.clone()) {
                    let own = f.lock().ok().and_then(|m| {
                        m.get(&path)
                            .or_else(|| m.get(&percent_decode(&path)))
                            .cloned()
                    });
                    if let Some(b) = own {
                        let resp = Response::builder()
                            .status(200)
                            .header(CONTENT_TYPE, kind(&path))
                            .body(Cow::Owned(b.to_vec()))
                            .unwrap_or_else(|_| Response::new(Cow::Borrowed(&[][..])));
                        responder.respond(resp);
                        return;
                    }
                    let rel = path
                        .split_once('/')
                        .map_or(path.clone(), |(_, r)| r.to_string());
                    let range = req
                        .headers()
                        .get(RANGE)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string);
                    if let Ok(mut q) = fa.lock() {
                        q.push(FarAsk {
                            far,
                            rel: percent_decode(&rel),
                            range,
                            ty: kind(&path),
                            responder,
                        });
                    }
                    far_pending.store(true, Ordering::Release);
                    return;
                }
                let respond = |r: Response<Cow<'static, [u8]>>| responder.respond(r);
                let root = r.lock().ok().and_then(|r| r.clone());
                // Video and sound play from disk a piece at a time, as the
                // player asks for them, so a long film never loads whole.
                if crate::data::media_kind(&path).is_some()
                    && let Some(full) = root.as_deref().and_then(|r| disk_path(r, &path))
                {
                    let range = req.headers().get(RANGE).and_then(|v| v.to_str().ok());
                    return respond(ranged(&full, range, kind(&path)));
                }
                // A name with spaces arrives encoded (`Sign-ups%20by%20week`).
                let mut body = f
                    .lock()
                    .ok()
                    .and_then(|m| {
                        m.get(&path)
                            .or_else(|| m.get(&percent_decode(&path)))
                            .cloned()
                    })
                    .or_else(|| from_disk(root.as_deref()?, &path));
                // A link from the site's root (`/styles.css`) skips the version
                // in the address: find it from the page's folder upwards.
                if body.is_none()
                    && let Some(root) = &root
                    && let Ok(dir) = pd.lock()
                    && let Some((found, name)) = from_site_root(root, &dir, &path)
                {
                    body = Some(found);
                    path = name;
                }
                let (status, bytes): (u16, Vec<u8>) = match body {
                    Some(b) => (200, b.to_vec()),
                    None => (404, b"Not found".to_vec()),
                };
                respond(
                    Response::builder()
                        .status(status)
                        .header(CONTENT_TYPE, kind(&path))
                        .body(Cow::Owned(bytes))
                        .unwrap_or_else(|_| Response::new(Cow::Borrowed(&[][..]))),
                )
            })
            .with_initialization_script(SCRIPT)
            // Pages stay on their own origin; anything else opens in the browser.
            .with_navigation_handler(move |url| {
                if std::env::var_os("DENT_DEBUG_WEB").is_some() {
                    eprintln!("web: navigate {url}");
                }
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
                    "start" => PageEvent::Started,
                    "error" => PageEvent::Error(s("msg")),
                    "logged" => PageEvent::Logged(s("msg")),
                    "pick" => PageEvent::Pick {
                        tag: s("tag"),
                        text: s("text"),
                        selector: s("sel"),
                    },
                    "pick-cancel" => PageEvent::PickCancel,
                    "probe" => PageEvent::Probe(s("msg")),
                    "copy" => PageEvent::Copy(s("text")),
                    "found" => {
                        PageEvent::Found(v.get("n").and_then(|n| n.as_u64()).unwrap_or(0) as usize)
                    }
                    // Settling only makes the snapshot stale; it wakes nothing.
                    "settled" => {
                        ipc_settled.store(true, Ordering::Release);
                        return;
                    }
                    "down" => {
                        ipc_pressed.store(true, Ordering::Release);
                        return;
                    }
                    _ => return,
                });
            })
            // Files dropped on a page attach to your message, as anywhere
            // else in dent, instead of the page opening them.
            .with_drag_drop_handler(move |e| {
                if let wry::DragDropEvent::Drop { paths, .. } = e {
                    drop_push(PageEvent::Dropped(paths));
                }
                true
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
            far,
            far_asks,
            files,
            root,
            page_dir,
            events,
            pending,
            settled,
            pressed,
            origin,
            snapshot,
            still: Arc::default(),
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
    /// Files far pages asked for since the last call.
    pub fn take_far(&self) -> Vec<FarAsk> {
        self.far_asks
            .lock()
            .map(|mut q| std::mem::take(&mut *q))
            .unwrap_or_default()
    }

    /// Show a page of another machine's, its own files fetched from there.
    pub fn show_far(&mut self, name: &str, v: usize, bytes: &[u8], far: FarRoot) {
        self.set_far(Some(far));
        self.show_page(name, v, bytes, None);
    }

    /// Show a video, a sound or a PDF of another machine's, played from
    /// there a range at a time.
    pub fn show_far_file(&mut self, name: &str, far: FarRoot) {
        self.set_far(Some(far));
        self.show_media(name, None);
    }

    fn set_far(&mut self, far: Option<FarRoot>) {
        let moved = match self.far.lock() {
            Ok(mut f) => std::mem::replace(&mut *f, far.clone()) != far,
            Err(_) => false,
        };
        // Another page's files, or another machine's: shown afresh.
        if moved {
            self.showing = None;
        }
    }

    pub fn show(&mut self, name: &str, v: usize, bytes: &[u8], root: Option<&std::path::Path>) {
        self.set_far(None);
        self.show_page(name, v, bytes, root);
    }

    fn show_page(&mut self, name: &str, v: usize, bytes: &[u8], root: Option<&std::path::Path>) {
        // A page outside the project (a scratch folder, say) finds its
        // images and styles beside itself.
        let (name, root) = split_root(name, root);
        let name = name.as_str();
        let moved = match self.root.lock() {
            Ok(mut r) => std::mem::replace(&mut *r, root.clone()) != root,
            Err(_) => false,
        };
        if let Ok(mut d) = self.page_dir.lock() {
            *d = name
                .rsplit_once('/')
                .map(|(d, _)| d.to_string())
                .unwrap_or_default();
        }
        let src = Source::File {
            name: name.to_string(),
            version: v,
        };
        if self.showing.as_ref() == Some(&src) && !moved {
            return;
        }
        let key = format!("{v}/{name}");
        if let Ok(mut m) = self.files.lock() {
            m.insert(key, bytes.into());
        }
        self.load(src);
    }

    /// Show a video, a sound or a PDF from disk: a PDF as itself, the others
    /// in a page of their own that plays them. Showing it again is a no-op.
    pub fn show_file(&mut self, file: &std::path::Path) {
        let (Some(dir), Some(name)) = (file.parent(), file.file_name()) else {
            return;
        };
        self.set_far(None);
        let name = name.to_string_lossy().into_owned();
        self.show_media(&name, Some(dir.to_path_buf()));
    }

    fn show_media(&mut self, name: &str, root: Option<std::path::PathBuf>) {
        let name = name.to_string();
        let moved = match self.root.lock() {
            Ok(mut r) => std::mem::replace(&mut *r, root.clone()) != root,
            Err(_) => false,
        };
        if let Ok(mut d) = self.page_dir.lock() {
            d.clear();
        }
        let pdf = crate::data::media_kind(&name) == Some("PDF");
        let page = if pdf {
            name.clone()
        } else {
            format!("{name}.html")
        };
        let src = Source::File {
            name: page.clone(),
            version: 0,
        };
        if self.showing.as_ref() == Some(&src) && !moved {
            return;
        }
        if !pdf && let Ok(mut m) = self.files.lock() {
            m.insert(format!("0/{page}"), media_page(&name).into_bytes().into());
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
        let r = self.web.load_url(&url);
        if std::env::var_os("DENT_DEBUG_WEB").is_some() {
            eprintln!("web: load {url} -> {r:?}");
        }
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
        if !on {
            self.release_focus();
        }
        let _ = self.web.set_visible(on);
    }

    /// Hand the keyboard back to dent. A click into the page makes it the
    /// window's key target, and it stays that even after you click back
    /// into dent or the page hides, so typing would land in the page.
    pub fn release_focus(&self) {
        self.pressed.store(false, Ordering::Release);
        let _ = self.web.focus_parent();
    }

    /// The editing keys, done in the page while it has the keyboard: dent
    /// sees them first. Returns whether the page took the key.
    pub fn edit(&self, key: &str, shift: bool, clipboard: Option<String>) -> bool {
        if !self.has_focus() {
            return false;
        }
        let js = match (key, shift) {
            ("c", false) => "window.__dentCopy && window.__dentCopy(false)".to_string(),
            ("x", false) => "window.__dentCopy && window.__dentCopy(true)".to_string(),
            ("a", false) => "document.execCommand('selectAll')".to_string(),
            ("z", false) => "document.execCommand('undo')".to_string(),
            ("z", true) => "document.execCommand('redo')".to_string(),
            ("v", false) => match clipboard {
                Some(text) => format!(
                    "document.execCommand('insertText', false, {})",
                    serde_json::to_string(&text).unwrap_or_default()
                ),
                None => return true,
            },
            _ => return false,
        };
        let _ = self.web.evaluate_script(&js);
        true
    }

    /// The page took the keyboard without being pressed (it does on load):
    /// give it back.
    pub fn keep_keys_unless_pressed(&self, pointer: Point<Pixels>) {
        // The pointer over the page may be a press the page hasn't told us
        // about yet: leave it be.
        let over = self.visible && self.bounds.is_some_and(|b| b.contains(&pointer));
        if !over && !self.pressed.load(Ordering::Acquire) && self.has_focus() {
            let _ = self.web.focus_parent();
        }
    }

    /// ⌘F in the page: tint each match of `needle`, mark match `at` and
    /// scroll to it; the page answers with how many it found.
    pub fn find(&self, needle: &str, at: usize, tint: Hsla, strong: Hsla) {
        let css = |c: Hsla| {
            let c = Rgba::from(c);
            format!(
                "rgba({},{},{},{:.2})",
                (c.r * 255.).round(),
                (c.g * 255.).round(),
                (c.b * 255.).round(),
                c.a
            )
        };
        self.script(&format!(
            "window.__dentFind && window.__dentFind({}, {at}, {:?}, {:?})",
            serde_json::to_string(needle).unwrap_or_default(),
            css(tint),
            css(strong),
        ));
    }

    /// Give the page the keyboard, as a click into it does (walkthrough).
    pub fn take_focus(&self) {
        self.pressed.store(true, Ordering::Release);
        let _ = self.web.focus();
    }

    /// Whether keys typed now go to the page.
    #[cfg(target_os = "macos")]
    pub fn has_focus(&self) -> bool {
        use wry::WebViewExtMacOS;
        let view = self.web.webview();
        let Some(window) = view.window() else {
            return false;
        };
        window.firstResponder().is_some_and(|r| {
            r.downcast_ref::<objc2_app_kit::NSView>()
                .is_some_and(|v| v.isDescendantOf(&view))
        })
    }

    #[cfg(not(target_os = "macos"))]
    pub fn has_focus(&self) -> bool {
        false
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
    /// The page's last picture, if it shows what's showing. `any_size`
    /// takes one of another size too, as while the panel is being resized.
    pub fn snapshot(&self, size: Size<Pixels>, any_size: bool) -> Option<Snapshot> {
        let snap = self.snapshot.lock().ok()?.clone()?;
        let same_size = (snap.size.width - size.width).abs() < px(1.)
            && (snap.size.height - size.height).abs() < px(1.);
        (Some(&snap.source) == self.showing.as_ref() && (same_size || any_size)).then_some(snap)
    }

    /// The page's latest picture as the web view gave it, once.
    pub fn take_still(&self) -> Option<(Source, Vec<u8>, ImageFormat)> {
        self.still.lock().ok()?.take()
    }

    /// Ask the web view for a picture of itself. It arrives a moment later.
    pub fn take_snapshot(&self) {
        let (Some(size), Some(source)) = (self.bounds.map(|b| b.size), self.showing.clone()) else {
            return;
        };
        if !self.visible {
            return;
        }
        let (slot, retired, still) = (
            self.snapshot.clone(),
            self.retired.clone(),
            self.still.clone(),
        );
        let store = move |bytes: Vec<u8>, format: ImageFormat| {
            if let Ok(mut s) = still.lock() {
                *s = Some((source.clone(), bytes.clone(), format));
            }
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

/// A page's own file from disk: `path` is `{version}/{relative path}`, and
/// nothing outside `root` is served.
/// The name a page loads under and the folder its own files come from: the
/// project for a page inside it, the page's folder for one outside.
fn split_root(name: &str, root: Option<&std::path::Path>) -> (String, Option<std::path::PathBuf>) {
    let path = std::path::Path::new(name);
    if path.is_absolute()
        && let (Some(dir), Some(file)) = (path.parent(), path.file_name())
    {
        return (file.to_string_lossy().into_owned(), Some(dir.to_path_buf()));
    }
    (name.to_string(), root.map(|p| p.to_path_buf()))
}

/// A path from the site's root, found in the nearest folder from the page's
/// own up to the project that has it, the way a static site is served from
/// its public folder. A folder means its index.html. Gives the bytes and the
/// name they were found under.
fn from_site_root(root: &std::path::Path, dir: &str, path: &str) -> Option<(Arc<[u8]>, String)> {
    let rel = percent_decode(path);
    let rel = if rel.is_empty() || rel.ends_with('/') {
        format!("{rel}index.html")
    } else {
        rel
    };
    let root = root.canonicalize().ok()?;
    let mut at = root.join(dir);
    loop {
        if let Ok(full) = at.join(&rel).canonicalize()
            && full.starts_with(&root)
        {
            let full = if full.is_dir() {
                full.join("index.html")
            } else {
                full
            };
            if full.is_file() {
                let name = full.file_name()?.to_string_lossy().into_owned();
                return std::fs::read(&full).ok().map(|b| (b.into(), name));
            }
        }
        if at == root || !at.pop() {
            return None;
        }
    }
}

fn from_disk(root: &std::path::Path, path: &str) -> Option<Arc<[u8]>> {
    std::fs::read(disk_path(root, path)?).ok().map(Into::into)
}

/// The file a `{version}/{path}` address names under `root`, if it's there
/// and inside it.
fn disk_path(root: &std::path::Path, path: &str) -> Option<std::path::PathBuf> {
    let rel = path.split_once('/')?.1;
    let rel = percent_decode(rel);
    let root = root.canonicalize().ok()?;
    let full = root.join(rel).canonicalize().ok()?;
    (full.starts_with(&root) && full.is_file()).then_some(full)
}

/// The most of a file one answer carries when the player asks for "the rest".
const RANGE_CHUNK: u64 = 4 << 20;

/// The bytes of `full` a `Range` header asks for (`bytes=a-b`, `bytes=a-`,
/// `bytes=-n`): inclusive bounds, clamped to the file; `None` if it can't
/// be met.
pub fn byte_range(range: &str, total: u64) -> Option<(u64, u64)> {
    let spec = range
        .trim()
        .strip_prefix("bytes=")?
        .split(',')
        .next()?
        .trim();
    let (a, b) = spec.split_once('-')?;
    let last = total.checked_sub(1)?;
    let (start, end) = match (a.trim(), b.trim()) {
        ("", n) => {
            let n: u64 = n.parse().ok()?;
            (total.saturating_sub(n), last)
        }
        (a, "") => {
            let a: u64 = a.parse().ok()?;
            (a, last)
        }
        (a, b) => (a.parse().ok()?, b.parse::<u64>().ok()?.min(last)),
    };
    // At most a chunk at a time, whatever was asked: a player asking for
    // all of a long recording at once gets its start and asks on, rather
    // than dent reading gigabytes into memory.
    let end = end.min(start.saturating_add(RANGE_CHUNK - 1));
    (start <= end).then_some((start, end))
}

/// A media file's answer: the piece asked for, or all of it when nothing
/// was asked.
fn ranged(full: &std::path::Path, range: Option<&str>, ty: &str) -> Response<Cow<'static, [u8]>> {
    use std::io::{Read, Seek, SeekFrom};
    let answer =
        |status: u16, headers: &[(wry::http::header::HeaderName, String)], body: Vec<u8>| {
            let mut b = Response::builder()
                .status(status)
                .header(ACCEPT_RANGES, "bytes");
            for (k, v) in headers {
                b = b.header(k, v.as_str());
            }
            b.body(Cow::Owned(body))
                .unwrap_or_else(|_| Response::new(Cow::Borrowed(&[][..])))
        };
    let Ok(mut f) = std::fs::File::open(full) else {
        return answer(404, &[], b"Not found".to_vec());
    };
    let total = f.metadata().map(|m| m.len()).unwrap_or(0);
    let Some(range) = range else {
        let mut all = Vec::new();
        let _ = f.read_to_end(&mut all);
        return answer(200, &[(CONTENT_TYPE, ty.to_string())], all);
    };
    let Some((start, end)) = byte_range(range, total) else {
        return answer(
            416,
            &[(CONTENT_RANGE, format!("bytes */{total}"))],
            Vec::new(),
        );
    };
    let mut piece = vec![0; (end - start + 1) as usize];
    if f.seek(SeekFrom::Start(start)).is_err() || f.read_exact(&mut piece).is_err() {
        return answer(500, &[], Vec::new());
    }
    answer(
        206,
        &[
            (CONTENT_TYPE, ty.to_string()),
            (CONTENT_LENGTH, piece.len().to_string()),
            (CONTENT_RANGE, format!("bytes {start}-{end}/{total}")),
        ],
        piece,
    )
}

/// The page a video or a sound plays in: centred, at most its own size,
/// on the page's own background in either theme.
fn media_page(name: &str) -> String {
    let src: String = name
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect();
    let player = if crate::data::media_kind(name) == Some("Sound") {
        format!(r#"<audio src="{src}" controls preload="metadata"></audio>"#)
    } else {
        format!(r#"<video src="{src}" controls playsinline preload="metadata"></video>"#)
    };
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><meta name="color-scheme" content="light dark"><style>html,body{{margin:0;height:100%;background:Canvas}}body{{display:flex;align-items:center;justify-content:center}}video{{max-width:100%;max-height:100%;outline:none}}audio{{width:min(480px,90%)}}</style></head><body>{player}</body></html>"#
    )
}

/// `%20` and friends back to their characters, for paths in links.
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod root_tests {
    use super::{byte_range, from_site_root, kind, media_page, split_root};

    #[test]
    fn ranges_are_read_as_the_player_asks() {
        assert_eq!(byte_range("bytes=0-1", 100), Some((0, 1)));
        assert_eq!(byte_range("bytes=10-", 100), Some((10, 99)));
        assert_eq!(byte_range("bytes=-10", 100), Some((90, 99)));
        assert_eq!(byte_range("bytes=50-500", 100), Some((50, 99)));
        assert_eq!(byte_range("bytes=0-", 100 << 20), Some((0, (4 << 20) - 1)));
        // All of a long file asked for at once, or its last half: a chunk.
        assert_eq!(
            byte_range(&format!("bytes=0-{}", (2u64 << 30) - 1), 2 << 30),
            Some((0, (4 << 20) - 1))
        );
        assert_eq!(
            byte_range("bytes=-1073741824", 2 << 30),
            Some((1 << 30, (1 << 30) + (4 << 20) - 1))
        );
        assert_eq!(byte_range("bytes=200-", 100), None);
        assert_eq!(byte_range("bytes=0-1", 0), None);
    }

    #[test]
    fn a_video_page_names_its_file_safely() {
        let p = media_page("my clip \"1\".mp4");
        assert!(p.contains(r#"<video src="my%20clip%20%221%22.mp4""#));
        assert!(media_page("talk.m4a").contains("<audio"));
    }

    #[test]
    fn pages_outside_the_project_load_from_their_own_folder() {
        let proj = std::path::Path::new("/p");
        assert_eq!(
            split_root("/tmp/x/index.html", Some(proj)),
            ("index.html".into(), Some("/tmp/x".into()))
        );
        assert_eq!(
            split_root("site/index.html", Some(proj)),
            ("site/index.html".into(), Some("/p".into()))
        );
        assert_eq!(kind("a/Photo.JPG"), "image/jpeg");
        assert_eq!(kind("f.woff2"), "font/woff2");
        assert_eq!(kind("index.html"), "text/html; charset=utf-8");
    }

    #[test]
    fn links_from_the_site_root_are_found_above_the_page() {
        let dir = std::env::temp_dir().join(format!("dent-site-{}", std::process::id()));
        let public = dir.join("web/public");
        std::fs::create_dir_all(public.join("joined")).unwrap();
        std::fs::create_dir_all(public.join("privacy")).unwrap();
        std::fs::write(public.join("styles.css"), "h1{}").unwrap();
        std::fs::write(public.join("privacy/index.html"), "<p>p</p>").unwrap();
        let css = from_site_root(&dir, "web/public/joined", "styles.css").unwrap();
        assert_eq!((&*css.0, css.1.as_str()), (&b"h1{}"[..], "styles.css"));
        assert_eq!(
            from_site_root(&dir, "web/public/joined", "privacy/")
                .unwrap()
                .1,
            "index.html"
        );
        assert!(from_site_root(&dir, "web/public/joined", "../../etc/passwd").is_none());
        assert!(from_site_root(&dir, "web/public/joined", "missing.css").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
