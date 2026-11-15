//! The bundled design assets (Phase 3 plan T0.5): the Aegis fonts (`design/fonts`, OFL) and the baked icons
//! (`design/icons`, from `design/scripts/gen-icons.py`), served to GPUI under `icons/`. gpui-kit's own assets serve
//! everything else.

use std::borrow::Cow;

use gpui::{AssetSource, SharedString};

#[derive(rust_embed::RustEmbed)]
#[folder = "../../design/icons"]
#[prefix = "icons/"]
struct Icons;

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        match Icons::get(path) {
            Some(file) => Ok(Some(file.data)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        let mut found: Vec<SharedString> = Icons::iter()
            .filter(|name| name.starts_with(path))
            .map(|name| name.into_owned().into())
            .collect();
        found.extend(gpui_kit::assets::Assets.list(path)?);
        Ok(found)
    }
}

/// One of the baked icons, by its asset path (`icons/...`).
pub fn icon(path: &str) -> Option<Cow<'static, [u8]>> {
    Icons::get(path).map(|file| file.data)
}

/// The text families the theme names: Archivo (text), Archivo Expanded Tight (display) and JetBrains Mono.
pub fn fonts() -> Vec<Cow<'static, [u8]>> {
    [
        include_bytes!("../../../design/fonts/Archivo-Regular.ttf").as_slice(),
        include_bytes!("../../../design/fonts/Archivo-Medium.ttf"),
        include_bytes!("../../../design/fonts/Archivo-SemiBold.ttf"),
        include_bytes!("../../../design/fonts/ArchivoExpandedTight-ExtraBold.ttf"),
        include_bytes!("../../../design/fonts/JetBrainsMono-Regular.ttf"),
        include_bytes!("../../../design/fonts/JetBrainsMono-Medium.ttf"),
        include_bytes!("../../../design/fonts/JetBrainsMono-SemiBold.ttf"),
    ]
    .into_iter()
    .map(Cow::Borrowed)
    .collect()
}

pub const TEXT_FAMILY: &str = "Archivo";
pub const DISPLAY_FAMILY: &str = "Archivo Expanded Tight";
pub const MONO_FAMILY: &str = "JetBrains Mono";
