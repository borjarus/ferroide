//! The application's asset source: GPUI Kit's default component icons plus
//! the extra Lucide icons FerroIDE uses. Only the icons listed here are
//! embedded, not the whole 1,800-icon catalog.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

gpui_kit::assets::icon_assets!(
    pub ExtraIcons,
    [
        Box,
        Braces,
        Bug,
        CircleStop,
        CodeXml,
        Columns2,
        Contrast,
        Database,
        FileCode,
        FileType,
        FolderGit2,
        FolderPlus,
        FolderTree,
        GitBranch,
        GitCommitHorizontal,
        GitGraph,
        GitPullRequestArrow,
        Hammer,
        HardDriveDownload,
        Keyboard,
        Languages,
        LayoutPanelLeft,
        ListTodo,
        ListTree,
        Maximize2,
        Minimize2,
        Pin,
        PinOff,
        Rows2,
        X,
    ]
);

/// The asset source to install with `Application::with_assets`.
#[derive(Clone, Copy, Debug, Default)]
pub struct FerroAssets;

impl AssetSource for FerroAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FerroIcon;

    #[test]
    fn every_ferro_icon_is_embedded() {
        for icon in FerroIcon::ALL {
            let path = icon.path();
            let bytes = FerroAssets.load(&path).unwrap();
            assert!(
                bytes.is_some_and(|b| !b.is_empty()),
                "{icon:?} -> {path} is not embedded"
            );
        }
    }
}
