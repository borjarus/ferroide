//! The application's asset source: GPUI Kit's default component icons plus
//! the extra Lucide icons FerroIDE uses. Only the icons listed here are
//! embedded, not the whole 1,800-icon catalog.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

gpui_kit::assets::icon_assets!(
    pub ExtraIcons,
    [
        ArrowDownToLine,
        ArrowLeft,
        ArrowRight,
        ArrowUpFromLine,
        Box,
        Braces,
        Bug,
        ChevronsDownUp,
        CircleDot,
        CircleMinus,
        CircleSlash,
        CircleStop,
        Clock,
        CodeXml,
        Columns2,
        Container,
        Contrast,
        Database,
        FileCode,
        FileDiff,
        FilePlus,
        FileType,
        FlaskConical,
        FolderGit2,
        FolderPlus,
        FolderTree,
        GitBranch,
        GitCommitHorizontal,
        GitGraph,
        GitMerge,
        GitPullRequestArrow,
        Hammer,
        HardDriveDownload,
        Hash,
        KeyRound,
        Keyboard,
        Languages,
        Layers,
        LayoutPanelLeft,
        Lightbulb,
        ListChecks,
        ListFilter,
        ListOrdered,
        ListTodo,
        ListTree,
        Maximize2,
        Minimize2,
        Package,
        Parentheses,
        Pencil,
        Pin,
        PinOff,
        Regex,
        Rows2,
        ServerCog,
        Shapes,
        Sigma,
        SquareFunction,
        Tag,
        Trash,
        Type,
        Variable,
        WholeWord,
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
