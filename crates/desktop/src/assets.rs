//! The stock assets, plus dent's own icons (in `icons/`), which win on a
//! shared name.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

const OWN: &[(&str, &[u8])] = &[
    // The Claude mark, from Simple Icons (CC0), for the agent's model.
    ("icons/claude.svg", include_bytes!("../icons/claude.svg")),
];

/// The Book pairing, compiled in so the app needs nothing beside it (0032),
/// by file name: the window's text uses them, and inline pages load them
/// from `dent:` as `/.dent/fonts/<name>`.
pub const FONTS: &[(&str, &[u8])] = &[
    (
        "CommitMono-400-Italic.ttf",
        include_bytes!("../../../assets/fonts/CommitMono-400-Italic.ttf"),
    ),
    (
        "CommitMono-400-Regular.ttf",
        include_bytes!("../../../assets/fonts/CommitMono-400-Regular.ttf"),
    ),
    (
        "CommitMono-700-Regular.ttf",
        include_bytes!("../../../assets/fonts/CommitMono-700-Regular.ttf"),
    ),
    (
        "IBMPlexSans-Medium.ttf",
        include_bytes!("../../../assets/fonts/IBMPlexSans-Medium.ttf"),
    ),
    (
        "IBMPlexSans-Regular.ttf",
        include_bytes!("../../../assets/fonts/IBMPlexSans-Regular.ttf"),
    ),
    (
        "IBMPlexSans-SemiBold.ttf",
        include_bytes!("../../../assets/fonts/IBMPlexSans-SemiBold.ttf"),
    ),
    (
        "InstrumentSerif-Italic.ttf",
        include_bytes!("../../../assets/fonts/InstrumentSerif-Italic.ttf"),
    ),
    (
        "InstrumentSerif-Regular.ttf",
        include_bytes!("../../../assets/fonts/InstrumentSerif-Regular.ttf"),
    ),
    (
        "Literata-Italic.ttf",
        include_bytes!("../../../assets/fonts/Literata-Italic.ttf"),
    ),
    (
        "Literata-Medium.ttf",
        include_bytes!("../../../assets/fonts/Literata-Medium.ttf"),
    ),
    (
        "Literata-MediumItalic.ttf",
        include_bytes!("../../../assets/fonts/Literata-MediumItalic.ttf"),
    ),
    (
        "Literata-Regular.ttf",
        include_bytes!("../../../assets/fonts/Literata-Regular.ttf"),
    ),
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match OWN.iter().find(|(p, _)| *p == path) {
            Some((_, bytes)) => Ok(Some(Cow::Borrowed(bytes))),
            None => gpui_kit::assets::AllAssets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut all = gpui_kit::assets::AllAssets.list(path)?;
        for (p, _) in OWN {
            if p.starts_with(path) && !all.iter().any(|a| a.as_ref() == *p) {
                all.push((*p).into());
            }
        }
        Ok(all)
    }
}
