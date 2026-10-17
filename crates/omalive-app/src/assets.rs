//! Lucide icons beyond gpui-kit's default bundle, drawn with square caps
//! and mitred joins to match Omarchy's square geometry.
use gpui_kit::{AssetSource, SharedString};
use std::borrow::Cow;

gpui_kit::assets::icon_assets!(
    OmaliveIcons,
    [AudioLines, Mic, MicOff, Video, VideoOff, Webcam, Moon]
);

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        let bytes = match OmaliveIcons.load(path)? {
            Some(bytes) => Some(bytes),
            None => gpui_kit::assets::Assets.load(path)?,
        };
        Ok(bytes.map(|bytes| {
            if !path.ends_with(".svg") {
                return bytes;
            }
            Cow::Owned(
                String::from_utf8_lossy(&bytes)
                    .replace("stroke-linecap=\"round\"", "stroke-linecap=\"square\"")
                    .replace("stroke-linejoin=\"round\"", "stroke-linejoin=\"miter\"")
                    .into_bytes(),
            )
        }))
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(OmaliveIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_omarchy::IconName;

    #[test]
    fn every_icon_omalive_draws_resolves_squared() {
        for name in [
            IconName::AudioLines,
            IconName::Mic,
            IconName::MicOff,
            IconName::Video,
            IconName::VideoOff,
            IconName::Webcam,
            IconName::Moon,
            IconName::Check,
            IconName::TriangleAlert,
        ] {
            let path = name.path();
            let svg = Assets
                .load(&path)
                .unwrap()
                .unwrap_or_else(|| panic!("{path}"));
            assert!(
                !String::from_utf8_lossy(&svg).contains("stroke-linecap=\"round\""),
                "{path}"
            );
        }
    }
}
