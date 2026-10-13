//! The live preview: the system web view placed over a region of the GPUI
//! window. Pages load from memory through a `dent://` scheme with a policy
//! that blocks the network, so a preview is offline and can't reach anything
//! but its own files. A small script reports errors and element picks back.

use std::borrow::Cow;
use std::collections::HashMap;
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
  const err = console.error;
  console.error = (...a) => { send({ t: 'error', msg: a.map(String).join(' ') }); err(...a); };
  // Live reload keeps your place.
  addEventListener('beforeunload', () => sessionStorage.setItem('dent-y', String(scrollY)));
  addEventListener('DOMContentLoaded', () => {
    const y = +sessionStorage.getItem('dent-y') || 0;
    if (y) scrollTo(0, y);
    send({ t: 'loaded' });
  });
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

pub struct Preview {
    web: WebView,
    files: Arc<Mutex<HashMap<String, Arc<[u8]>>>>,
    events: Arc<Mutex<Vec<PageEvent>>>,
    /// The file and version on screen.
    pub showing: Option<(String, usize)>,
    bounds: Option<Bounds<Pixels>>,
    visible: bool,
}

impl Preview {
    pub fn new(window: &Window) -> Option<Self> {
        let files: Arc<Mutex<HashMap<String, Arc<[u8]>>>> = Arc::default();
        let events: Arc<Mutex<Vec<PageEvent>>> = Arc::default();
        let (f, e) = (files.clone(), events.clone());
        let web = WebViewBuilder::new()
            .with_custom_protocol("dent".into(), move |_, req| {
                let path = req.uri().path().trim_start_matches('/').to_string();
                let body = f.lock().ok().and_then(|m| m.get(&path).cloned());
                let (status, bytes): (u16, Vec<u8>) = match body {
                    Some(b) => (200, b.to_vec()),
                    None => (404, b"Not found".to_vec()),
                };
                let kind = if path.ends_with(".svg") {
                    "image/svg+xml"
                } else if path.ends_with(".png") {
                    "image/png"
                } else {
                    "text/html; charset=utf-8"
                };
                Response::builder()
                    .status(status)
                    .header(CONTENT_TYPE, kind)
                    .header("Content-Security-Policy", POLICY)
                    .body(Cow::Owned(bytes))
                    .unwrap_or_else(|_| Response::new(Cow::Borrowed(&[][..])))
            })
            .with_initialization_script(SCRIPT)
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
                let ev = match s("t").as_str() {
                    "loaded" => PageEvent::Loaded,
                    "error" => PageEvent::Error(s("msg")),
                    "pick" => PageEvent::Pick {
                        tag: s("tag"),
                        text: s("text"),
                        selector: s("sel"),
                    },
                    _ => return,
                };
                if let Ok(mut q) = e.lock() {
                    q.push(ev);
                }
            })
            .with_visible(false)
            .with_bounds(Rect {
                position: LogicalPosition::new(0., 0.).into(),
                size: LogicalSize::new(10., 10.).into(),
            })
            .build_as_child(window)
            .map_err(|e| eprintln!("preview: no web view ({e})"))
            .ok()?;
        Some(Self {
            web,
            files,
            events,
            showing: None,
            bounds: None,
            visible: false,
        })
    }

    /// Show `bytes` as `name`, version `v`. Loading the same thing again is a no-op.
    pub fn show(&mut self, name: &str, v: usize, bytes: &[u8]) {
        if self
            .showing
            .as_ref()
            .is_some_and(|(n, x)| n == name && *x == v)
        {
            return;
        }
        let key = format!("{v}/{name}");
        if let Ok(mut m) = self.files.lock() {
            m.insert(key.clone(), bytes.into());
        }
        self.showing = Some((name.to_string(), v));
        let _ = self.web.load_url(&format!("dent://localhost/{key}"));
    }

    pub fn reload(&mut self) {
        if let Some((n, v)) = self.showing.take() {
            let key = format!("{v}/{n}");
            self.showing = Some((n, v));
            let _ = self.web.load_url(&format!("dent://localhost/{key}"));
        }
    }

    pub fn place(&mut self, b: Bounds<Pixels>) {
        if self.bounds == Some(b) {
            return;
        }
        self.bounds = Some(b);
        let _ = self.web.set_bounds(Rect {
            position: LogicalPosition::new(b.origin.x.as_f32() as f64, b.origin.y.as_f32() as f64)
                .into(),
            size: LogicalSize::new(b.size.width.as_f32() as f64, b.size.height.as_f32() as f64)
                .into(),
        });
    }

    pub fn set_visible(&mut self, on: bool) {
        if self.visible != on {
            self.visible = on;
            let _ = self.web.set_visible(on);
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

    pub fn drain(&self) -> Vec<PageEvent> {
        self.events
            .lock()
            .map(|mut q| std::mem::take(&mut *q))
            .unwrap_or_default()
    }
}
