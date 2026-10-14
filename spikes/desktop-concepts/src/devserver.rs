//! A stand-in for a dev server an agent started on another machine, which dent
//! forwards to a local port. It serves a small admin dashboard that reads a
//! live API, so the preview can show a real app talking to its own backend.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::OnceLock;
use std::time::Instant;

static PORT: OnceLock<u16> = OnceLock::new();

/// The forwarded address, starting the server the first time it's asked for.
pub fn url() -> String {
    let port = *PORT.get_or_init(|| {
        let Ok(listener) = TcpListener::bind("127.0.0.1:0") else {
            return 0;
        };
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
        let started = Instant::now();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                std::thread::spawn(move || serve(stream, started));
            }
        });
        port
    });
    format!("http://127.0.0.1:{port}/")
}

fn serve(mut stream: TcpStream, started: Instant) {
    let mut line = String::new();
    if BufReader::new(&stream).read_line(&mut line).is_err() {
        return;
    }
    let path = line.split_whitespace().nth(1).unwrap_or("/");
    let (kind, body) = match path.split('?').next().unwrap_or("/") {
        "/" | "/index.html" => ("text/html; charset=utf-8", PAGE.to_string()),
        "/api/signups" => ("application/json", signups(started.elapsed().as_secs())),
        _ => ("text/plain", "Not found".to_string()),
    };
    let status = if kind == "text/plain" {
        "404 Not Found"
    } else {
        "200 OK"
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// Fourteen days of signups; today keeps counting while you watch.
fn signups(secs: u64) -> String {
    let days = [38, 41, 36, 52, 61, 47, 44, 58, 66, 71, 63, 80, 86, 0u64];
    let today = 40 + secs / 3;
    let list: Vec<String> = days
        .iter()
        .enumerate()
        .map(|(i, n)| (if i == 13 { today } else { *n }).to_string())
        .collect();
    format!(
        r#"{{"days":[{}],"recent":[{{"name":"Ada Byrne","plan":"Pro","when":"{}s ago"}},{{"name":"Kenji Mori","plan":"Team","when":"2m ago"}},{{"name":"Lena Ortiz","plan":"Pro","when":"9m ago"}},{{"name":"Sam Okafor","plan":"Free","when":"14m ago"}}]}}"#,
        list.join(","),
        secs % 50 + 4
    )
}

const PAGE: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Quillmate admin</title>
<style>
  :root { --ink: #1d1b2e; --quiet: #6b6880; --line: #e6e3ef; --bg: #fbfaff; --brand: #5b4bdb; --brand-soft: #ebe8fd; }
  * { box-sizing: border-box; }
  body { margin: 0; font: 14px/1.5 -apple-system, "Segoe UI", system-ui, sans-serif; color: var(--ink); background: var(--bg); }
  header { display: flex; align-items: center; gap: 12px; padding: 14px 24px; border-bottom: 1px solid var(--line); background: #fff; }
  header b { font-size: 15px; }
  header nav { display: flex; gap: 16px; margin-left: auto; color: var(--quiet); }
  header nav a { color: inherit; text-decoration: none; }
  main { padding: 24px; display: grid; gap: 20px; max-width: 980px; }
  .kpis { display: grid; grid-template-columns: repeat(auto-fit, minmax(170px, 1fr)); gap: 12px; }
  .kpi { background: #fff; border: 1px solid var(--line); border-radius: 10px; padding: 14px 16px; }
  .kpi span { color: var(--quiet); font-size: 12px; }
  .kpi strong { display: block; font-size: 26px; font-variant-numeric: tabular-nums; }
  .card { background: #fff; border: 1px solid var(--line); border-radius: 10px; padding: 16px; }
  .card h2 { margin: 0 0 12px; font-size: 14px; }
  svg { width: 100%; height: 180px; display: block; }
  table { width: 100%; border-collapse: collapse; }
  td { padding: 8px 0; border-top: 1px solid var(--line); }
  td:last-child { color: var(--quiet); text-align: right; }
  .plan { font-size: 12px; padding: 2px 8px; border-radius: 99px; background: var(--brand-soft); color: var(--brand); }
  .live { display: inline-flex; align-items: center; gap: 6px; color: var(--quiet); font-size: 12px; }
  .live i { width: 7px; height: 7px; border-radius: 50%; background: #2f9e6a; }
</style>
</head>
<body>
<header><b>Quillmate admin</b><span class="live"><i></i>live</span>
  <nav><a href="/">Signups</a><a href="https://quillmate.example/docs">Docs</a></nav></header>
<main>
  <section class="kpis">
    <div class="kpi"><span>Signups today</span><strong id="today">–</strong></div>
    <div class="kpi"><span>Last 14 days</span><strong id="total">–</strong></div>
    <div class="kpi"><span>Paid share</span><strong>62%</strong></div>
  </section>
  <section class="card"><h2>Signups per day</h2><svg id="chart" viewBox="0 0 700 180" preserveAspectRatio="none"></svg></section>
  <section class="card"><h2>Latest</h2><table id="recent"></table></section>
</main>
<script>
async function refresh() {
  const r = await fetch('/api/signups');
  const d = await r.json();
  const max = Math.max(...d.days, 1);
  const w = 700 / d.days.length;
  document.getElementById('chart').innerHTML = d.days.map((n, i) => {
    const h = n / max * 160;
    return `<rect x="${i * w + 6}" y="${176 - h}" width="${w - 12}" height="${h}" rx="4" fill="${i === d.days.length - 1 ? '#5b4bdb' : '#cfc8f7'}"/>`;
  }).join('');
  document.getElementById('today').textContent = d.days[d.days.length - 1];
  document.getElementById('total').textContent = d.days.reduce((a, b) => a + b, 0).toLocaleString();
  document.getElementById('recent').innerHTML = d.recent.map(x =>
    `<tr><td>${x.name}</td><td><span class="plan">${x.plan}</span></td><td>${x.when}</td></tr>`).join('');
}
refresh();
setInterval(refresh, 2000);
</script>
</body>
</html>
"##;
