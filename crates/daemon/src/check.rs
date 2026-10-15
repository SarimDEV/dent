//! `dent-daemon check-transcripts [DIR] [--into STATE] [--keep-empty]`: run
//! every Claude Code transcript under DIR (default `~/.claude/projects`)
//! through dent's import and report what doesn't add up. Only counts and
//! file names are printed, never what was said. Conversations sit directly
//! in each project folder; the files below them are subagents' and
//! workflows', checked only for lines dent can't read. With `--into`, every
//! conversation also becomes a session in a scratch state directory, for
//! the app to open; `--keep-empty` keeps the empty ones too, for load tests.

use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use dent_protocol::{Block, Body, Permission, Status};
use serde_json::Value;

use crate::terminal;

#[derive(Default)]
struct Tally {
    files: usize,
    lines: usize,
    unreadable_lines: usize,
    failed_files: Vec<String>,
    /// Transcripts with nothing said in them (a session opened and left).
    empty: usize,
    /// Subagent and workflow transcripts, checked only for readability.
    nested: usize,
    prompts: usize,
    pictures: usize,
    stops: usize,
    compactions: usize,
    /// Tool calls with no result, and results with no call, by file.
    no_result: Vec<(String, usize)>,
    no_call: Vec<(String, usize)>,
    /// Text and thinking blocks in the transcript that didn't come through.
    lost_blocks: Vec<(String, usize)>,
    /// Bodies that don't survive the trip through the log's JSON.
    bad_bodies: usize,
    /// Person lines left out, by why.
    skipped: BTreeMap<String, usize>,
    /// Line types and block types dent doesn't know.
    unknown_types: BTreeMap<String, usize>,
    unknown_blocks: BTreeMap<String, usize>,
    slowest: Vec<(String, f64, usize)>,
    bodies: usize,
}

const KNOWN_TYPES: &[&str] = &["user", "assistant", "system", "summary"];

pub fn run(args: &[String]) -> Result<()> {
    let dir = args
        .iter()
        .find(|a| !a.starts_with("--") && !is_value_of(args, a, "--into"))
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::paths::home().join(".claude/projects"));
    let into = args
        .iter()
        .position(|a| a == "--into")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);
    let keep_empty = args.iter().any(|a| a == "--keep-empty");
    let mut files: Vec<PathBuf> = walk(&dir);
    files.sort();
    // DIR/<project>/<conversation>.jsonl; anything deeper belongs to one.
    let depth = |p: &Path| {
        p.strip_prefix(&dir)
            .map(|r| r.components().count())
            .unwrap_or(0)
    };
    let scratch = std::env::temp_dir().join(format!("dent-check-{}", std::process::id()));
    let mut store = match &into {
        Some(state) => {
            std::fs::create_dir_all(state)?;
            Some(crate::store::Store::open(&state.join("dent.db"))?)
        }
        None => None,
    };
    let mut t = Tally::default();
    let started = Instant::now();
    for f in &files {
        let name = f
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        t.files += 1;
        if depth(f) > 2 && !keep_empty {
            t.nested += 1;
            raw_counts(f, &mut t);
            continue;
        }
        let session = crate::daemon::new_id();
        let media = match &into {
            Some(state) => state.join("attachments").join(&session),
            None => scratch.join(&name),
        };
        let clock = Instant::now();
        let imported = terminal::import(f, &media);
        let took = clock.elapsed().as_secs_f64() * 1000.0;
        let Some(imported) = imported else {
            t.failed_files.push(name);
            continue;
        };
        let raw = raw_counts(f, &mut t);
        compare(&name, &raw, &imported.bodies, &mut t);
        t.slowest.push((name.clone(), took, imported.bodies.len()));
        t.bodies += imported.bodies.len();
        if imported.bodies.is_empty() && !keep_empty {
            t.empty += 1;
            continue;
        }
        if let Some(store) = store.as_mut() {
            save(store, &session, &name, imported)?;
        }
    }
    let _ = std::fs::remove_dir_all(&scratch);
    report(&t, started.elapsed().as_secs_f64());
    Ok(())
}

fn is_value_of(args: &[String], a: &String, flag: &str) -> bool {
    args.iter()
        .position(|x| x == flag)
        .is_some_and(|i| args.get(i + 1) == Some(a))
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else if p.extension().is_some_and(|x| x == "jsonl") {
            out.push(p);
        }
    }
    out
}

/// What the transcript holds, counted straight from its lines.
#[derive(Default)]
struct Raw {
    tool_calls: HashSet<String>,
    tool_results: HashSet<String>,
    texts: usize,
    thinking: usize,
}

fn raw_counts(path: &Path, t: &mut Tally) -> Raw {
    let mut raw = Raw::default();
    let Ok(f) = std::fs::File::open(path) else {
        return raw;
    };
    for line in BufReader::new(f).lines().map_while(Result::ok) {
        t.lines += 1;
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            t.unreadable_lines += 1;
            continue;
        };
        let kind = v["type"].as_str().unwrap_or("(none)");
        if !KNOWN_TYPES.contains(&kind) && v["message"].is_object() {
            // A message-carrying line of a kind dent has never seen.
            *t.unknown_types.entry(kind.to_string()).or_default() += 1;
        }
        if v["isSidechain"] == true || v["isMeta"] == true {
            continue;
        }
        let content = &v["message"]["content"];
        match kind {
            // Claude Code's own notes (limits, API errors) become errors.
            "assistant"
                if v["message"]["model"] == "<synthetic>" || v["isApiErrorMessage"] == true => {}
            "assistant" => {
                for b in content.as_array().into_iter().flatten() {
                    match b["type"].as_str().unwrap_or_default() {
                        "text" if !b["text"].as_str().unwrap_or("").is_empty() => raw.texts += 1,
                        "thinking" if !b["thinking"].as_str().unwrap_or("").trim().is_empty() => {
                            raw.thinking += 1
                        }
                        "tool_use" | "server_tool_use" => {
                            raw.tool_calls.insert(b["id"].as_str().unwrap_or("").into());
                        }
                        "text" | "thinking" | "redacted_thinking" => {}
                        other => {
                            *t.unknown_blocks
                                .entry(format!("assistant/{other}"))
                                .or_default() += 1
                        }
                    }
                }
            }
            "user" if v["isCompactSummary"] != true => {
                let parts: Vec<Value> = match content {
                    Value::String(s) => vec![serde_json::json!({"type": "text", "text": s})],
                    Value::Array(a) => a.clone(),
                    _ => Vec::new(),
                };
                for b in parts {
                    match b["type"].as_str().unwrap_or_default() {
                        "tool_result" => {
                            raw.tool_results
                                .insert(b["tool_use_id"].as_str().unwrap_or("").into());
                        }
                        "text" => {
                            let text = b["text"].as_str().unwrap_or("").trim();
                            if text.starts_with('<') && !text.starts_with("<pasted_content") {
                                let tag: String = text[1..]
                                    .chars()
                                    .take_while(|c| {
                                        c.is_ascii_alphanumeric() || *c == '-' || *c == '_'
                                    })
                                    .collect();
                                if !matches!(tag.as_str(), "command-name" | "bash-input") {
                                    *t.skipped.entry(format!("<{tag}>")).or_default() += 1;
                                }
                            }
                        }
                        "image" => {}
                        other => *t.unknown_blocks.entry(format!("user/{other}")).or_default() += 1,
                    }
                }
            }
            _ => {}
        }
    }
    raw
}

fn compare(name: &str, raw: &Raw, bodies: &[Body], t: &mut Tally) {
    let (mut calls, mut results) = (HashSet::new(), HashSet::new());
    let (mut texts, mut thinking) = (0, 0);
    for b in bodies {
        // The log keeps bodies as JSON: each must read back as itself.
        match serde_json::to_string(b).map(|j| serde_json::from_str::<Body>(&j)) {
            Ok(Ok(back)) if back == *b => {}
            _ => t.bad_bodies += 1,
        }
        match b {
            Body::User { attachments, .. } => {
                t.prompts += 1;
                t.pictures += attachments.len();
            }
            Body::Assistant { blocks, .. } => {
                for bl in blocks {
                    match bl {
                        Block::Text { .. } => texts += 1,
                        Block::Thinking { .. } => thinking += 1,
                        Block::Tool { id, .. } => {
                            calls.insert(id.clone());
                        }
                    }
                }
            }
            Body::ToolResult { tool, .. } => {
                results.insert(tool.clone());
            }
            Body::TurnEnded {
                outcome: dent_protocol::Outcome::Interrupted { .. },
            } => t.stops += 1,
            Body::Notice { text } if text == "Conversation compacted" => t.compactions += 1,
            Body::Compacted { .. } => t.compactions += 1,
            _ => {}
        }
    }
    let lost = (raw.texts + raw.thinking).saturating_sub(texts + thinking)
        + raw.tool_calls.difference(&calls).count()
        + raw.tool_results.difference(&results).count();
    if lost > 0 {
        t.lost_blocks.push((name.into(), lost));
    }
    let no_result = calls.difference(&results).count();
    if no_result > 0 {
        t.no_result.push((name.into(), no_result));
    }
    let no_call = results.difference(&calls).count();
    if no_call > 0 {
        t.no_call.push((name.into(), no_call));
    }
}

fn save(
    store: &mut crate::store::Store,
    session: &str,
    agent_session: &str,
    imported: terminal::Imported,
) -> Result<()> {
    let now = crate::daemon::now_ms();
    let mut put = |b: Body| store.append(session, now, &b, None).map(|_| ());
    put(Body::Created {
        agent: "claude".into(),
        cwd: imported.cwd,
        model: None,
        permission: Permission::Ask,
        title: imported.title,
    })?;
    put(Body::Agent {
        agent_session: Some(agent_session.into()),
        version: None,
    })?;
    for b in imported.bodies {
        put(b)?;
    }
    put(Body::Status {
        status: Status::Done,
    })
    .context("save")
}

fn report(t: &Tally, secs: f64) {
    println!(
        "{} transcripts, {} lines, {} events, in {secs:.1}s",
        t.files, t.lines, t.bodies
    );
    println!("  unreadable lines: {}", t.unreadable_lines);
    println!(
        "  transcripts that wouldn't import: {}",
        t.failed_files.len()
    );
    println!(
        "  {} conversations ({} with nothing said), {} subagent and workflow transcripts",
        t.files - t.nested - t.failed_files.len(),
        t.empty,
        t.nested
    );
    for f in t.failed_files.iter().take(10) {
        println!("    {f}");
    }
    println!(
        "  prompts {}, pictures {}, stops {}, compactions {}",
        t.prompts, t.pictures, t.stops, t.compactions
    );
    println!("  events that don't survive the log: {}", t.bad_bodies);
    let worst = |label: &str, v: &[(String, usize)]| {
        let total: usize = v.iter().map(|x| x.1).sum();
        println!("  {label}: {total} in {} transcripts", v.len());
        let mut v = v.to_vec();
        v.sort_by_key(|x| std::cmp::Reverse(x.1));
        for (f, n) in v.iter().take(5) {
            println!("    {n:>5}  {f}");
        }
    };
    worst("blocks in the transcript missing from dent", &t.lost_blocks);
    worst("tool calls with no result", &t.no_result);
    worst("results with no tool call", &t.no_call);
    println!("  person lines left out as tooling, by tag:");
    for (k, n) in &t.skipped {
        println!("    {n:>6}  {k}");
    }
    println!("  unknown message line types: {:?}", t.unknown_types);
    println!("  unknown block types: {:?}", t.unknown_blocks);
    let mut slow = t.slowest.clone();
    slow.sort_by(|a, b| b.1.total_cmp(&a.1));
    println!("  slowest imports:");
    for (f, ms, n) in slow.iter().take(5) {
        println!("    {ms:>7.1} ms  {n:>6} events  {f}");
    }
}
