//! Spike 04: streaming Markdown, RaTeX and merman against real transcripts.
//!
//! `cargo run -p spike-markdown --release -- <stats|stream|math|mermaid>`
//! Transcripts are read locally and only counts and timings are printed.

use spike_markdown::{corpus, math, mermaid, pct, stream, visible};

use std::time::Instant;

use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("stats");
    let flag = |f: &str| args.iter().any(|a| a == f);
    match cmd {
        "stats" => stats(&corpus::load()),
        "stream" => stream_check(&corpus::load(), !flag("--raw"), flag("--bytes")),
        "math" => math::run(&corpus::load(), args.get(1).map(String::as_str)),
        "mermaid" => mermaid::run(&corpus::load(), args.get(1).map(String::as_str))?,
        "worst" => worst(),
        "dump" => {
            let t = math::typeset(args.get(1).map(String::as_str).unwrap_or("x"), true)
                .map_err(anyhow::Error::msg)?;
            for i in &t.list.items {
                println!("{i:?}");
            }
        }
        _ => anyhow::bail!("unknown command {cmd}"),
    }
    Ok(())
}

fn stats(msgs: &[corpus::Message]) {
    let mut counts = std::collections::BTreeMap::<&str, usize>::new();
    let mut bytes = Vec::new();
    let mut per_agent = std::collections::BTreeMap::<&str, usize>::new();
    for m in msgs {
        *per_agent.entry(m.agent).or_default() += 1;
        bytes.push(m.text.len() as f64);
        let mut seen = std::collections::BTreeSet::new();
        for ev in Parser::new_ext(&m.text, stream::options()).map(stream::dollars) {
            let k = match ev {
                Event::InlineMath(_) => "inline math",
                Event::DisplayMath(_) => "display math",
                Event::Start(Tag::Table(_)) => "table",
                Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(ref l)))
                    if l.as_ref() == "mermaid" =>
                {
                    "mermaid fence"
                }
                Event::Start(Tag::CodeBlock(_)) => "code block",
                Event::Start(Tag::Heading { .. }) => "heading",
                Event::Start(Tag::List(_)) => "list",
                Event::Start(Tag::Link { .. }) => "link",
                Event::Start(Tag::BlockQuote(_)) => "quote",
                Event::FootnoteReference(_) => "footnote",
                Event::Html(_) | Event::InlineHtml(_) => "html",
                Event::TaskListMarker(_) => "task list",
                Event::Start(Tag::Strong) => "bold",
                Event::Code(_) => "inline code",
                _ => continue,
            };
            seen.insert(k);
        }
        if m.text.contains("\\(") || m.text.contains("\\[") {
            seen.insert("\\( or \\[ delimiters");
        }
        if m.text
            .lines()
            .any(|l| l.starts_with('[') && l.contains("]: "))
        {
            seen.insert("link reference definition");
        }
        for k in seen {
            *counts.entry(k).or_default() += 1;
        }
    }
    let total: f64 = bytes.iter().sum();
    println!(
        "messages {} ({:?}), {:.1} MB, size p50 {:.0} B p99 {:.0} B max {:.0} B",
        msgs.len(),
        per_agent,
        total / 1e6,
        pct(&mut bytes.clone(), 50.),
        pct(&mut bytes.clone(), 99.),
        pct(&mut bytes, 100.)
    );
    println!("messages containing:");
    for (k, n) in counts {
        println!(
            "  {k:28} {n:6}  {:.2}%",
            n as f64 * 100. / msgs.len() as f64
        );
    }
}

/// Replay every message in token-sized chunks and check the streaming
/// invariants: committed blocks never change, visible text only grows, and
/// the end result equals a one-shot parse.
fn stream_check(msgs: &[corpus::Message], terminate: bool, bytes: bool) {
    let mut push_us = Vec::new();
    let mut whole_us = Vec::new();
    let (mut steps, mut flicker_steps, mut flicker_msgs, mut moved, mut mismatch) = (0, 0, 0, 0, 0);
    let mut seed = 0x2545F4914F6CDD1Du64;
    let mut examples = Vec::new();
    let mut moved_examples = Vec::new();
    for (mi, m) in msgs.iter().enumerate() {
        let t = Instant::now();
        let last = stream::blocks(&m.text, 0);
        whole_us.push(t.elapsed().as_secs_f64() * 1e6);
        let last_vis = visible::styled(last.iter());

        let mut s = stream::Streamer::new(terminate);
        let mut at = 0;
        let mut flickered = false;
        let mut moved_this = false;
        while at < m.text.len() {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let mut end = if bytes {
                at + 1
            } else {
                (at + 1 + (seed % 12) as usize).min(m.text.len())
            };
            while !m.text.is_char_boundary(end) {
                end += 1;
            }
            let t = Instant::now();
            s.push(&m.text[at..end]);
            push_us.push(t.elapsed().as_secs_f64() * 1e6);
            at = end;
            steps += 1;
            let same = |a: &[stream::Block], b: &[stream::Block]| {
                a.iter().zip(b).all(|(x, y)| x.events == y.events)
            };
            if s.committed.len() > last.len() || !same(&s.committed, &last) {
                moved += 1;
                if moved_examples.len() < 40 && !moved_this {
                    let k = s
                        .committed
                        .iter()
                        .zip(&last)
                        .position(|(x, y)| x.events != y.events)
                        .unwrap_or(0);
                    moved_examples.push(format!(
                        "#{mi} block {k}\nsrc: {:?}\nnow:  {:?}\nlast: {:?}",
                        &m.text[s.committed[k].range.start
                            ..at.min(s.committed[k].range.start + 300)
                                .max(s.committed[k].range.start)],
                        s.committed[k].events.iter().take(12).collect::<Vec<_>>(),
                        last.get(k)
                            .map(|b| b.events.iter().take(12).collect::<Vec<_>>())
                    ));
                }
                moved_this = true;
            }
            let vis = visible::styled(s.visible());
            if !visible::is_prefix(&vis, &last_vis) {
                flicker_steps += 1;
                if !flickered && examples.len() < 400 {
                    examples.push(describe(mi, &m.text[..at], &vis, &last_vis));
                }
                flickered = true;
            }
        }
        flicker_msgs += flickered as usize;
        s.finish();
        if s.committed.len() != last.len()
            || !s
                .committed
                .iter()
                .zip(&last)
                .all(|(x, y)| x.events == y.events)
        {
            mismatch += 1;
        }
    }
    println!(
        "terminator {terminate}, {} chunks: {} messages, {steps} steps",
        if bytes { "1-byte" } else { "1-12 byte" },
        msgs.len()
    );
    println!("  committed block changed: {moved} steps");
    println!("  final != one-shot parse: {mismatch} messages");
    println!(
        "  flicker: {flicker_steps} steps ({:.3}%), {flicker_msgs} messages ({:.2}%)",
        flicker_steps as f64 * 100. / steps as f64,
        flicker_msgs as f64 * 100. / msgs.len() as f64
    );
    println!(
        "  push: p50 {:.1} µs, p99 {:.1} µs, max {:.0} µs; one-shot parse p50 {:.1} µs, max {:.0} µs",
        pct(&mut push_us.clone(), 50.),
        pct(&mut push_us.clone(), 99.),
        pct(&mut push_us, 100.),
        pct(&mut whole_us.clone(), 50.),
        pct(&mut whole_us, 100.)
    );
    // Private: real message text, for local debugging only (scratch, never committed).
    if let Ok(path) = std::env::var("SPIKE_EXAMPLES") {
        let _ = std::fs::write(&path, examples.join("\n\n"));
        let _ = std::fs::write(format!("{path}.moved"), moved_examples.join("\n\n"));
    }
}

fn describe(mi: usize, prefix: &str, now: &visible::Styled, last: &visible::Styled) -> String {
    let i = now
        .iter()
        .zip(last)
        .position(|(a, b)| a != b)
        .unwrap_or(now.len().min(last.len()));
    let show = |v: &visible::Styled| {
        v[i.saturating_sub(30)..(i + 30).min(v.len())]
            .iter()
            .map(|(c, f)| {
                format!(
                    "{c}{}",
                    if *f != 0 {
                        format!("[{f:x}]")
                    } else {
                        String::new()
                    }
                )
            })
            .collect::<String>()
    };
    let mut k = prefix.len().saturating_sub(120);
    while !prefix.is_char_boundary(k) {
        k += 1;
    }
    let tail = &prefix[k..];
    format!(
        "#{mi}\nsrc tail: {tail:?}\nnow:  {}\nlast: {}",
        show(now),
        show(last)
    )
}

/// Synthetic worst cases: one huge block that stays the uncommitted tail the
/// whole time, so every chunk re-parses all of it.
fn worst() {
    let code = format!(
        "```rust\n{}```\n",
        "let x = compute(a, b) + 1; // note\n".repeat(3000)
    );
    let table = format!(
        "| a | b | c |\n| --- | --- | --- |\n{}",
        "| **cell** | `code` | 12.5 ms |\n".repeat(2000)
    );
    let para =
        "Words with **bold**, `code` and [links](https://x.y) in one long paragraph. ".repeat(700);
    for (name, text) in [("code block", code), ("table", table), ("paragraph", para)] {
        let mut s = stream::Streamer::new(true);
        let mut times = Vec::new();
        let mut at = 0;
        while at < text.len() {
            let end = (at + 8).min(text.len());
            let t = Instant::now();
            s.push(&text[at..end]);
            times.push(t.elapsed().as_secs_f64() * 1e3);
            at = end;
        }
        let last = times.len() / 10;
        println!(
            "{name:10} {:>4} KB: push p50 {:.2} ms, last 10% p50 {:.2} ms, max {:.2} ms",
            text.len() / 1024,
            pct(&mut times.clone(), 50.),
            pct(&mut times[times.len() - last..].to_vec(), 50.),
            pct(&mut times, 100.)
        );
    }
}
