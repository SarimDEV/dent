//! The stock assets, plus dent's own icons (in `icons/`), which win on a
//! shared name.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

const OWN: &[(&str, &[u8])] = &[
    // The Claude mark, from Simple Icons (CC0), for the agent's model.
    ("icons/claude.svg", include_bytes!("../icons/claude.svg")),
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
