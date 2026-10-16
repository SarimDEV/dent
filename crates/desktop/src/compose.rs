//! The reply box's rules that don't draw anything, kept apart so they can be
//! tested: pictures made ready for the agent, and long pastes folded into a
//! line of their own until they're sent.

use std::collections::BTreeMap;
use std::ops::Range;

/// The largest picture the agent is shown as a picture; a bigger one goes
/// by its path only.
pub const PICTURE_MAX: u64 = 5 * 1024 * 1024;

/// What a pasted picture is brought under when it's too big, with room left
/// for its encoding on the way to the agent.
const PASTE_TARGET: usize = 3_500_000;

/// Picture kinds the agent reads as pictures, by extension.
pub fn agent_reads(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp"
    )
}

/// A pasted picture made ready to save: its bytes and extension. Kinds the
/// agent reads (`ext` names one) stay as they are; others (TIFF from
/// Preview, BMP) become PNG, as the agent can't read them; and one too big
/// to be shown is scaled down until it fits.
pub fn ready_picture(
    bytes: &[u8],
    ext: Option<&'static str>,
) -> Result<(Vec<u8>, &'static str), String> {
    if let Some(ext) = ext
        && (bytes.len() as u64) <= PICTURE_MAX
    {
        return Ok((bytes.to_vec(), ext));
    }
    let img =
        image::load_from_memory(bytes).map_err(|e| format!("couldn't read the picture: {e}"))?;
    let png = encode(&img, image::ImageFormat::Png)?;
    if png.len() <= PASTE_TARGET {
        return Ok((png, "png"));
    }
    // Too big: smaller each time, as PNG and then as JPEG, until it fits.
    let mut side = img.width().max(img.height());
    loop {
        side = (side * 3 / 4).max(256);
        let small = img.thumbnail(side, side);
        let png = encode(&small, image::ImageFormat::Png)?;
        if png.len() <= PASTE_TARGET {
            return Ok((png, "png"));
        }
        let jpg = encode(
            &image::DynamicImage::ImageRgb8(small.to_rgb8()),
            image::ImageFormat::Jpeg,
        )?;
        if jpg.len() <= PASTE_TARGET || side == 256 {
            return Ok((jpg, "jpg"));
        }
    }
}

fn encode(img: &image::DynamicImage, format: image::ImageFormat) -> Result<Vec<u8>, String> {
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, format)
        .map_err(|e| format!("couldn't save the picture: {e}"))?;
    Ok(out.into_inner())
}

/// What to tell you about a file attached as a picture the agent won't see
/// as one: too big, or a kind it can't read. It still goes, by its path.
pub fn picture_note(path: &str, size: u64) -> Option<String> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())?;
    let picture = matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "tif" | "tiff" | "bmp" | "heic" | "heif"
    );
    if !picture {
        return None;
    }
    if !agent_reads(&ext) {
        return Some(format!(
            "The agent can't see {name} as a picture (it reads PNG, JPEG, GIF and WebP), so it gets the file's path."
        ));
    }
    (size > PICTURE_MAX)
        .then(|| format!("{name} is over 5 MB, so the agent gets its path, not the picture."))
}

/// Pasted text this long folds into one line in the box.
const FOLD_LINES: usize = 40;
const FOLD_BYTES: usize = 4096;

/// Whether pasted text folds away.
pub fn folds(text: &str) -> bool {
    text.lines().count() > FOLD_LINES || text.len() > FOLD_BYTES
}

/// The plain text a folded paste stands as in the box (what a saved draft
/// keeps, and what copying the chip gives). Numbered, so each is told apart
/// and can be found again in a saved draft.
pub fn fold_label(n: u32, text: &str) -> String {
    match text.lines().count() {
        1 => format!("[Pasted text #{n} · {} KB]", text.len().div_ceil(1024)),
        lines => format!("[Pasted text #{n} +{lines} lines]"),
    }
}

/// What a folded paste's chip says.
pub fn chip_label(text: &str) -> String {
    match text.lines().count() {
        1 => format!("Pasted text · {} KB", text.len().div_ceil(1024)),
        lines => format!("Pasted text · {lines} lines"),
    }
}

/// The number of the paste a chip's token id names (`paste-3`).
pub fn paste_of(id: &str) -> Option<u32> {
    id.strip_prefix("paste-")?.parse().ok()
}

/// `text` with each chip (its range in `text` and its token id) opened back
/// to what was pasted. Chips are known by their id, never by their words,
/// so typing a chip's words by hand sends them as typed.
pub fn unfold_tokens(
    text: &str,
    chips: &[(Range<usize>, &str)],
    pastes: &BTreeMap<u32, String>,
) -> String {
    let mut chips: Vec<_> = chips.to_vec();
    chips.sort_by_key(|(r, _)| r.start);
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (r, id) in chips {
        let Some(full) = paste_of(id).and_then(|n| pastes.get(&n)) else {
            continue;
        };
        if r.start < at || r.end > text.len() {
            continue;
        }
        out.push_str(&text[at..r.start]);
        out.push_str(full);
        at = r.end;
    }
    out.push_str(&text[at..]);
    out
}

/// What opening a chip at `at` in `text` replaces, and with what: the
/// paste on lines of its own, a line break before and after it unless one
/// is already there, taking the spaces that separated it from its words.
pub fn opened(text: &str, at: &Range<usize>, full: &str) -> (Range<usize>, String) {
    let full = full.trim_end_matches('\n');
    let start = text[..at.start].trim_end_matches(' ').len();
    let end = text.len() - text[at.end..].trim_start_matches(' ').len();
    let before = !(start == 0 || text[..start].ends_with('\n'));
    let after = !(end >= text.len() || text[end..].starts_with('\n'));
    (
        start..end,
        format!(
            "{}{full}{}",
            if before { "\n" } else { "" },
            if after { "\n" } else { "" }
        ),
    )
}

/// Where folded pastes sit in `text`: each one's range and number.
pub fn folded_in(text: &str, pastes: &BTreeMap<u32, String>) -> Vec<(Range<usize>, u32)> {
    let mut found: Vec<(Range<usize>, u32)> = Vec::new();
    for (n, full) in pastes {
        let label = fold_label(*n, full);
        for (at, _) in text.match_indices(&label) {
            found.push((at..at + label.len(), *n));
        }
    }
    found.sort_by_key(|(r, _)| r.start);
    found
}

/// `text` with each folded paste it holds whole taken out, and the chips
/// they showed as: a queued message shows its pastes as chips again, the
/// words around them closing up.
pub fn refold(text: &str, pastes: &BTreeMap<u32, String>) -> (String, Vec<String>) {
    // Only text this long can hold a folded paste.
    if text.lines().nth(FOLD_LINES).is_none() && text.len() <= FOLD_BYTES {
        return (text.to_string(), Vec::new());
    }
    let mut out = text.to_string();
    let mut chips = Vec::new();
    for full in pastes.values() {
        let Some(at) = out.find(full.as_str()) else {
            continue;
        };
        if full.trim().is_empty() {
            continue;
        }
        let (a, b) = (&out[..at], &out[at + full.len()..]);
        let (a, b) = (
            a.trim_end_matches([' ', '\n']),
            b.trim_start_matches([' ', '\n']),
        );
        let gap = if a.is_empty() || b.is_empty() {
            ""
        } else {
            " "
        };
        out = format!("{a}{gap}{b}");
        chips.push(chip_label(full));
    }
    (out, chips)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_pastes_fold_and_open_again() {
        let long: String = (0..60).map(|i| format!("line {i}\n")).collect();
        assert!(folds(&long));
        assert!(!folds("a short\npaste"));
        assert!(folds(&"x".repeat(5000)));
        let pastes = BTreeMap::from([(1, long.clone()), (2, "y".repeat(5000))]);
        let label = fold_label(1, &long);
        assert_eq!(label, "[Pasted text #1 +60 lines]");
        assert_eq!(fold_label(2, &pastes[&2]), "[Pasted text #2 · 5 KB]");
        let typed = format!("look at this:\n{label}\nand {}", fold_label(2, &pastes[&2]));
        // A saved draft finds its chips again by their words.
        let chips: Vec<(Range<usize>, String)> = folded_in(&typed, &pastes)
            .into_iter()
            .map(|(r, n)| (r, format!("paste-{n}")))
            .collect();
        assert_eq!(chips.len(), 2);
        let chips: Vec<(Range<usize>, &str)> = chips
            .iter()
            .map(|(r, id)| (r.clone(), id.as_str()))
            .collect();
        assert_eq!(
            unfold_tokens(&typed, &chips, &pastes),
            format!("look at this:\n{long}\nand {}", "y".repeat(5000))
        );
        // Nothing folded: the words as typed.
        assert_eq!(unfold_tokens("plain", &[], &pastes), "plain");
    }

    #[test]
    fn chips_open_by_their_id_not_their_words() {
        let long: String = (0..60).map(|i| format!("line {i}\n")).collect();
        let pastes = BTreeMap::from([(1, long.clone())]);
        assert_eq!(chip_label(&long), "Pasted text · 60 lines");
        let label = fold_label(1, &long);
        // The words typed by hand, and the chip after them.
        let text = format!("{label} and {label}");
        let chip = text.len() - label.len()..text.len();
        assert_eq!(
            unfold_tokens(&text, &[(chip, "paste-1")], &pastes),
            format!("{label} and {long}")
        );
        // An unknown id leaves its words.
        assert_eq!(unfold_tokens("x", &[(0..1, "paste-9")], &pastes), "x");
    }

    #[test]
    fn an_opened_chip_takes_lines_of_its_own() {
        let t = "see [chip] why";
        assert_eq!(opened(t, &(4..10), "a\nb\n"), (3..11, "\na\nb\n".into()));
        let t = "[chip]";
        assert_eq!(opened(t, &(0..6), "a\nb"), (0..6, "a\nb".into()));
        let t = "see\n[chip]\nwhy";
        assert_eq!(opened(t, &(4..10), "a\nb\n"), (4..10, "a\nb".into()));
    }

    #[test]
    fn pictures_the_agent_cant_read_become_png() {
        let img = image::DynamicImage::new_rgb8(4, 4);
        let tiff = encode(&img, image::ImageFormat::Tiff).unwrap();
        let (bytes, ext) = ready_picture(&tiff, None).unwrap();
        assert_eq!(ext, "png");
        assert_eq!(
            image::guess_format(&bytes).unwrap(),
            image::ImageFormat::Png
        );
        // A PNG stays as it was.
        let png = encode(&img, image::ImageFormat::Png).unwrap();
        assert_eq!(ready_picture(&png, Some("png")).unwrap(), (png, "png"));
        assert!(ready_picture(b"not a picture", None).is_err());
    }

    #[test]
    fn a_picture_too_big_to_show_shrinks_until_it_fits() {
        // Noise compresses badly: a big PNG.
        let mut img = image::RgbImage::new(1800, 1800);
        let mut x: u32 = 1;
        for p in img.pixels_mut() {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            *p = image::Rgb([x as u8, (x >> 8) as u8, (x >> 16) as u8]);
        }
        let png = encode(
            &image::DynamicImage::ImageRgb8(img),
            image::ImageFormat::Png,
        )
        .unwrap();
        assert!(png.len() as u64 > PICTURE_MAX);
        let (bytes, _) = ready_picture(&png, Some("png")).unwrap();
        assert!(bytes.len() <= PASTE_TARGET);
    }

    #[test]
    fn says_when_a_picture_goes_by_path() {
        assert!(picture_note("/a/shot.png", 1000).is_none());
        assert!(picture_note("/a/notes.txt", 10 << 20).is_none());
        assert!(
            picture_note("/a/shot.png", 6 << 20)
                .unwrap()
                .contains("over 5 MB")
        );
        assert!(
            picture_note("/a/scan.tiff", 1000)
                .unwrap()
                .contains("can't see")
        );
    }

    #[test]
    fn a_queued_message_shows_its_pastes_folded() {
        let long: String = (0..60).map(|i| format!("line {i}\n")).collect();
        let pastes = BTreeMap::from([(1, long.clone()), (2, "short".to_string())]);
        let sent = format!("look at this {long} please");
        assert_eq!(
            refold(&sent, &pastes),
            (
                "look at this please".to_string(),
                vec!["Pasted text · 60 lines".to_string()]
            )
        );
        // Short text is left alone, even if a paste's words are in it.
        assert_eq!(refold("short", &pastes), ("short".to_string(), vec![]));
    }
}
