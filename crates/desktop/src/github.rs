//! GitHub in replies: `#123` and `owner/repo#123` link to that issue or
//! pull request, `#123` in the session's own repository (its `origin` on
//! github.com). Hovering one, or a link to an issue or a pull request,
//! shows what it is: its title, and whether it's open, a draft, merged or
//! closed, read with `gh` when it's installed (so private repositories
//! work) and from GitHub's public API when it isn't.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How a reference is kept as a link until the session's folder is known.
pub const SCHEME: &str = "github:";

/// Where the references to issues and pull requests are in plain text,
/// and their links (`github:#123`, `github:owner/repo#123`). A `#` inside a
/// word (`a#1`, `&#123;`), a number starting with 0 and one of more than
/// seven digits aren't references.
pub fn ref_spans(s: &str) -> Vec<(Range<usize>, String)> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    for (i, _) in s.match_indices('#') {
        let digits = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        let end = i + 1 + digits;
        if digits == 0 || digits > 7 || b[i + 1] == b'0' {
            continue;
        }
        // Ends at the number: `#12.` and `#12)` are, `#12a` and `#12-x` aren't.
        if b.get(end)
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'/'))
        {
            continue;
        }
        // An `owner/repo` just before it names another repository.
        let lead = b[..i]
            .iter()
            .rev()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'/'))
            .count();
        let start = i - lead;
        let before = s[..start].chars().next_back();
        if before.is_some_and(|c| c.is_alphanumeric() || matches!(c, '&' | '#' | '@' | ':')) {
            continue;
        }
        let num = &s[i + 1..end];
        if lead == 0 {
            out.push((i..end, format!("{SCHEME}#{num}")));
        } else if let Some(repo) = slug(&s[start..i]) {
            out.push((start..end, format!("{SCHEME}{repo}#{num}")));
        }
    }
    out
}

/// `owner/repo`, if `s` is one.
fn slug(s: &str) -> Option<String> {
    let (owner, repo) = s.split_once('/')?;
    let ok = |p: &str| {
        !p.is_empty()
            && !p.starts_with('.')
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    // `docs/notes.md#3` is a heading in a file, not a repository's issue.
    let file = [
        ".md", ".rs", ".ts", ".tsx", ".html", ".txt", ".json", ".toml", ".yml", ".yaml", ".py",
    ]
    .iter()
    .any(|e| repo.ends_with(e));
    (ok(owner) && ok(repo) && !owner.contains('.') && !file).then(|| format!("{owner}/{repo}"))
}

/// The address a `github:` link goes to, its repository found from `base`
/// when it doesn't name one; none when it can't be found.
pub fn url(href: &str, base: Option<&Path>) -> Option<String> {
    let rest = href.strip_prefix(SCHEME)?;
    let (repo, num) = rest.rsplit_once('#')?;
    let repo = if repo.is_empty() {
        repo_of(base?)?
    } else {
        repo.to_string()
    };
    Some(format!("https://github.com/{repo}/issues/{num}"))
}

/// The repository and number a GitHub address to an issue or a pull
/// request names: `https://github.com/o/r/pull/12/files` is `o/r`, 12.
pub fn issue(url: &str) -> Option<(String, u64)> {
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
        .or_else(|| url.strip_prefix("https://www.github.com/"))?;
    let mut parts = rest.split(['/', '#', '?']);
    let (owner, repo, kind, num) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    if !matches!(kind, "issues" | "pull") {
        return None;
    }
    let n: u64 = num.parse().ok()?;
    Some((slug(&format!("{owner}/{repo}"))?, n))
}

/// A folder's repository is read again after this long: a remote added
/// while dent runs is found, without reading it every frame.
const REREAD: Duration = Duration::from_secs(10);

thread_local! {
    /// Each folder's repository, and when it was read.
    static REPOS: RefCell<HashMap<PathBuf, (Option<String>, Instant)>> = RefCell::default();
}

/// The GitHub repository the folder `dir` is in: its `origin`, or else the
/// first remote on github.com.
pub fn repo_of(dir: &Path) -> Option<String> {
    if let Some((r, _)) = REPOS.with(|r| {
        r.borrow()
            .get(dir)
            .filter(|(_, at)| at.elapsed() < REREAD)
            .cloned()
    }) {
        return r;
    }
    let found = config_of(dir)
        .and_then(|c| std::fs::read_to_string(c).ok())
        .and_then(|c| remote_in(&c));
    REPOS.with(|r| {
        r.borrow_mut()
            .insert(dir.to_path_buf(), (found.clone(), Instant::now()))
    });
    found
}

/// The git config of the repository `dir` is in, a worktree's included.
fn config_of(dir: &Path) -> Option<PathBuf> {
    let dot = dir
        .ancestors()
        .map(|d| d.join(".git"))
        .find(|g| g.exists())?;
    if dot.is_dir() {
        return Some(dot.join("config"));
    }
    // A worktree's `.git` is a file naming its folder in the main one.
    let line = std::fs::read_to_string(&dot).ok()?;
    let git = dot.parent()?.join(line.strip_prefix("gitdir:")?.trim());
    let common = std::fs::read_to_string(git.join("commondir"))
        .map(|c| git.join(c.trim()))
        .unwrap_or(git);
    Some(common.join("config"))
}

/// The GitHub repository a git config's remotes name.
fn remote_in(config: &str) -> Option<String> {
    let mut remotes: Vec<(String, String)> = Vec::new();
    let mut section = String::new();
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line.to_string();
        } else if let Some(("url", v)) = line.split_once('=').map(|(k, v)| (k.trim(), v.trim()))
            && let Some(name) = section
                .strip_prefix("[remote \"")
                .and_then(|r| r.strip_suffix("\"]"))
        {
            remotes.push((name.to_string(), v.to_string()));
        }
    }
    remotes.sort_by_key(|(name, _)| name != "origin");
    remotes.iter().find_map(|(_, url)| github_slug(url))
}

/// `owner/repo` from a remote's address on github.com, in any of git's
/// forms (`git@github.com:o/r.git`, `https://github.com/o/r`, `ssh://…`).
fn github_slug(url: &str) -> Option<String> {
    let at = url.find("github.com")? + "github.com".len();
    let path = url[at..].trim_start_matches([':', '/']);
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.split('/');
    slug(&format!("{}/{}", parts.next()?, parts.next()?))
}

/// What an issue or a pull request is now.
#[derive(Clone, Debug, PartialEq)]
pub struct Info {
    pub title: String,
    pub pull: bool,
    pub state: State,
    pub author: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    Open,
    Draft,
    Merged,
    Closed,
    /// An issue closed as not planned.
    NotPlanned,
}

impl State {
    pub fn word(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Draft => "Draft",
            Self::Merged => "Merged",
            Self::Closed => "Closed",
            Self::NotPlanned => "Not planned",
        }
    }
}

impl Info {
    /// Its icon, as GitHub draws it.
    pub fn icon(&self) -> &'static str {
        match (self.pull, self.state) {
            (true, State::Merged) => "git-merge",
            (true, State::Draft) => "git-pull-request-draft",
            (true, State::Closed | State::NotPlanned) => "git-pull-request-closed",
            (true, State::Open) => "git-pull-request",
            (false, State::Open | State::Draft) => "circle-dot",
            (false, State::Closed | State::Merged) => "circle-check",
            (false, State::NotPlanned) => "circle-slash",
        }
    }
}

/// What the issues API says of one (a pull request is an issue there too).
pub fn info_from(v: &serde_json::Value) -> Option<Info> {
    let title = v["title"].as_str()?.trim().to_string();
    let pull = v.get("pull_request").is_some_and(|p| !p.is_null());
    let closed = v["state"] == "closed";
    let state = if pull && !v["pull_request"]["merged_at"].is_null() {
        State::Merged
    } else if closed && v["state_reason"] == "not_planned" {
        State::NotPlanned
    } else if closed {
        State::Closed
    } else if v["draft"] == true {
        State::Draft
    } else {
        State::Open
    };
    let author = v["user"]["login"].as_str().unwrap_or_default().to_string();
    Some(Info {
        title,
        pull,
        state,
        author,
    })
}

/// Ask GitHub about `repo`'s `n`: through `gh`, signed in, else the
/// public API. Blocks for up to a few seconds; run it off the main thread.
pub fn fetch(repo: &str, n: u64) -> Option<Info> {
    let path = format!("repos/{repo}/issues/{n}");
    let gh = ["/opt/homebrew/bin/gh", "/usr/local/bin/gh"]
        .into_iter()
        .find(|p| Path::new(p).exists())
        .unwrap_or("gh");
    let mut gh = std::process::Command::new(gh);
    gh.args(["api", &path]);
    let mut curl = std::process::Command::new("curl");
    curl.args([
        "-sfL",
        "--max-time",
        "6",
        "-H",
        "Accept: application/vnd.github+json",
        &format!("https://api.github.com/{path}"),
    ]);
    [gh, curl].into_iter().find_map(|c| {
        let out = run(c, Duration::from_secs(6))?;
        info_from(&serde_json::from_slice(&out).ok()?)
    })
}

/// What `cmd` prints, if it succeeds within `limit`.
fn run(mut cmd: std::process::Command, limit: Duration) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reading = std::thread::spawn(move || {
        let mut out = Vec::new();
        stdout.read_to_end(&mut out).map(|_| out).ok()
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return reading.join().ok().flatten(),
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if started.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// What dent knows of one issue or pull request.
#[derive(Clone, Debug, PartialEq)]
pub enum Look {
    Reading,
    Found(Info),
    /// GitHub didn't say: no `gh` and a private repository, or offline.
    Unknown,
}

/// How long the pointer rests on a link before its card shows.
const REST: Duration = Duration::from_millis(400);
/// What a card says is read again on a hover after this long.
const STALE: Duration = Duration::from_secs(120);

/// The colour of a state, as GitHub marks it.
fn state_ink(i: &Info, t: &crate::theme::Theme) -> gpui_kit::Hsla {
    match i.state {
        State::Open => t.done.0,
        State::Merged => t.spectrum[0],
        // An issue closed as done is GitHub's purple; a pull request
        // closed unmerged is red.
        State::Closed if !i.pull => t.spectrum[0],
        State::Closed => t.removed,
        State::Draft | State::NotPlanned => t.quiet,
    }
}

impl crate::app::Dent {
    /// The card a GitHub link shows once the pointer rests on it: what it
    /// is, read from GitHub the first time (and again after a while).
    pub fn github_card(
        &mut self,
        cx: &mut gpui_kit::Context<Self>,
    ) -> Option<gpui_kit::AnyElement> {
        use crate::ds::{LABEL, Typed, UI, radius, space};
        use gpui_kit::prelude::*;
        use gpui_kit::*;
        if self.lightbox.is_some() || self.menu.is_some() {
            return None;
        }
        let h = crate::select::hovered_link()?;
        let (repo, n) = issue(&h.url)?;
        let rested = h.since.elapsed();
        if rested < REST {
            if self.github_wait != Some(h.since) {
                self.github_wait = Some(h.since);
                let wait = REST - rested;
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(wait).await;
                    let _ = this.update(cx, |_, cx| cx.notify());
                })
                .detach();
            }
            return None;
        }
        let key = (repo.clone(), n);
        let fresh = self
            .github
            .get(&key)
            .is_some_and(|(_, at)| at.elapsed() < STALE);
        if !fresh && self.github_asking.insert(key.clone()) {
            let (r, k) = (repo.clone(), key.clone());
            let asked = cx.background_executor().spawn(async move { fetch(&r, n) });
            cx.spawn(async move |this, cx| {
                let got = asked.await;
                let _ = this.update(cx, |this, cx| {
                    this.github_asking.remove(&k);
                    let look = match got {
                        Some(i) => Look::Found(i),
                        // What it said before stands, if it said anything.
                        None => match this.github.get(&k) {
                            Some((l @ Look::Found(_), _)) => l.clone(),
                            _ => Look::Unknown,
                        },
                    };
                    this.github.insert(k, (look, Instant::now()));
                    cx.notify();
                });
            })
            .detach();
        }
        let look = self
            .github
            .get(&key)
            .map_or(Look::Reading, |(l, _)| l.clone());
        let t = self.t;
        let (mark, ink, title, note) = match &look {
            Look::Found(i) => {
                let what = if i.pull { "Pull request" } else { "Issue" };
                let mut note = format!("{what} #{n} in {repo} · {}", i.state.word());
                if !i.author.is_empty() {
                    note.push_str(&format!(" · by {}", i.author));
                }
                (i.icon(), state_ink(i, &t), i.title.clone(), note)
            }
            Look::Reading => (
                "circle-dot-dashed",
                t.faint,
                format!("{repo} #{n}"),
                "Reading it from GitHub…".to_string(),
            ),
            Look::Unknown => (
                "circle-dot-dashed",
                t.quiet,
                format!("{repo} #{n}"),
                "GitHub didn't say what this is. Click the link to open it.".to_string(),
            ),
        };
        let card = crate::ui::floating(&t)
            .id("github-card")
            .relative()
            .rounded(px(radius::MD))
            .px(px(space::S3))
            .py(px(space::S2))
            .max_w(px(380.))
            .flex()
            .items_start()
            .gap(px(space::S2))
            .child(crate::ui::probe(&self.probes, "github-card"))
            // Painted after the text: if this frame no longer draws the
            // link where the pointer rested, the card goes.
            .child(
                canvas(
                    |_, window, _| crate::select::check_hover(window),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_0(),
            )
            .child(div().pt(px(2.)).child(crate::ui::icon(mark, 14., ink)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .gap(px(space::HALF))
                    .child(
                        div()
                            .ty(UI)
                            .text_color(t.ink)
                            .font_weight(FontWeight::MEDIUM)
                            .whitespace_normal()
                            .child(title),
                    )
                    .child(
                        div()
                            .ty(LABEL)
                            .text_color(t.quiet)
                            .whitespace_normal()
                            .child(note),
                    ),
            );
        // Above the link, or below it when there's no room above.
        Some(
            deferred(
                anchored()
                    .position(point(h.at.origin.x, h.at.origin.y - px(space::S1)))
                    .anchor(Anchor::BottomLeft)
                    .snap_to_window_with_margin(px(space::S2))
                    .child(card),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs(s: &str) -> Vec<(&str, String)> {
        ref_spans(s).into_iter().map(|(r, u)| (&s[r], u)).collect()
    }

    #[test]
    fn references_are_found_where_they_stand_alone() {
        assert_eq!(
            refs("Fixed in #12, see (#3) and SarimDEV/dent#45."),
            [
                ("#12", "github:#12".into()),
                ("#3", "github:#3".into()),
                ("SarimDEV/dent#45", "github:SarimDEV/dent#45".into()),
            ]
        );
        // Not in a word, an entity, an anchor or a colour-like run, nor a
        // number with a leading 0.
        assert!(refs("a#1 &#123; page#top #12a #12-b #0 #01 #12345678 x:#5").is_empty());
        assert!(refs("see docs/notes.md#3").is_empty());
        assert_eq!(refs("vercel/next.js#7").len(), 1);
        assert_eq!(refs("#7"), [("#7", "github:#7".into())]);
    }

    #[test]
    fn a_reference_goes_to_its_repository() {
        assert_eq!(
            url("github:o/r#5", None).as_deref(),
            Some("https://github.com/o/r/issues/5")
        );
        // No repository to find it from: no link.
        assert_eq!(url("github:#5", None), None);
        assert_eq!(
            issue("https://github.com/o/r/pull/12/files"),
            Some(("o/r".into(), 12))
        );
        assert_eq!(
            issue("https://github.com/o/r/issues/7#issuecomment-1"),
            Some(("o/r".into(), 7))
        );
        assert_eq!(issue("https://github.com/o/r/blob/main/x.rs"), None);
        assert_eq!(issue("https://gitlab.com/o/r/issues/7"), None);
    }

    #[test]
    fn the_repository_is_read_from_the_folders_remotes() {
        for (url, want) in [
            ("git@github.com:SarimDEV/dent.git", Some("SarimDEV/dent")),
            ("https://github.com/o/r", Some("o/r")),
            ("https://github.com/o/r.git/", Some("o/r")),
            ("ssh://git@github.com/o/r.git", Some("o/r")),
            ("https://gitlab.com/o/r.git", None),
        ] {
            assert_eq!(github_slug(url).as_deref(), want, "{url}");
        }
        let config = "[core]\n\tbare = false\n[remote \"fork\"]\n\turl = git@github.com:me/r.git\n[remote \"origin\"]\n\turl = https://github.com/o/r.git\n";
        assert_eq!(remote_in(config).as_deref(), Some("o/r"));
        let dir = std::env::temp_dir().join(format!("dent-gh-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("src/deep")).unwrap();
        std::fs::write(dir.join(".git/config"), config).unwrap();
        assert_eq!(repo_of(&dir.join("src/deep")).as_deref(), Some("o/r"));
        assert_eq!(
            url("github:#9", Some(&dir)).as_deref(),
            Some("https://github.com/o/r/issues/9")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn what_github_says_of_one() {
        let pr = serde_json::json!({
            "title": "Zoom pictures ", "state": "closed", "user": {"login": "sarim"},
            "pull_request": {"merged_at": "2026-10-18T00:00:00Z"},
        });
        let i = info_from(&pr).unwrap();
        assert_eq!(
            (i.title.as_str(), i.pull, i.state),
            ("Zoom pictures", true, State::Merged)
        );
        assert_eq!(i.icon(), "git-merge");
        let draft = serde_json::json!({"title": "t", "state": "open", "draft": true, "pull_request": {"merged_at": null}});
        assert_eq!(info_from(&draft).unwrap().state, State::Draft);
        let issue =
            serde_json::json!({"title": "t", "state": "closed", "state_reason": "not_planned"});
        let i = info_from(&issue).unwrap();
        assert_eq!(
            (i.pull, i.state, i.icon()),
            (false, State::NotPlanned, "circle-slash")
        );
    }
}
