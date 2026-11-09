//! Requests views send to their workspace, as GPUI actions with data.
//!
//! A tool window, popup or dialog does not hold a handle to the workspace
//! (tool windows are also created when a saved layout is restored). It
//! dispatches one of these actions from its element instead; the action
//! bubbles up to the workspace root, which handles it. In tests, call the
//! matching `Workspace` method directly.

use std::path::PathBuf;

use ferro_services::TextPos;
use gpui_kit::Action;

/// Open a file in the editor, optionally at a position.
#[derive(Debug, Clone, PartialEq, Action)]
#[action(namespace = ferro, no_json)]
pub struct OpenLocation {
    pub path: PathBuf,
    /// 0-based line and column to put the caret at.
    pub position: Option<TextPos>,
    /// Open as a preview tab (italic title, replaced by the next preview).
    pub preview: bool,
    /// Move focus to the editor. False keeps focus where it is, e.g. while
    /// browsing results with the arrow keys.
    pub focus: bool,
}

impl OpenLocation {
    /// Open `path`, focused, as a regular tab.
    pub fn file(path: PathBuf) -> Self {
        Self {
            path,
            position: None,
            preview: false,
            focus: true,
        }
    }

    /// Open `path` at `position`, focused, as a regular tab.
    pub fn at(path: PathBuf, position: TextPos) -> Self {
        Self {
            path,
            position: Some(position),
            preview: false,
            focus: true,
        }
    }

    pub fn preview(mut self, preview: bool) -> Self {
        self.preview = preview;
        self
    }

    pub fn focus(mut self, focus: bool) -> Self {
        self.focus = focus;
        self
    }
}

/// Show the working-copy diff of a file in the editor area.
#[derive(Debug, Clone, PartialEq, Action)]
#[action(namespace = ferro, no_json)]
pub struct OpenDiff {
    pub path: PathBuf,
}

/// Open the 3-way merge view for a conflicted file.
#[derive(Debug, Clone, PartialEq, Action)]
#[action(namespace = ferro, no_json)]
pub struct OpenMerge {
    pub path: PathBuf,
}

/// Select a file in the Project tool window, expanding its parents.
#[derive(Debug, Clone, PartialEq, Action)]
#[action(namespace = ferro, no_json)]
pub struct RevealInProject {
    pub path: PathBuf,
}
