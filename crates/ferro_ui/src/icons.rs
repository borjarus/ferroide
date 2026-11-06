//! Semantic icon names. Views ask for `FerroIcon::Commit`, not for a Lucide
//! file, so the icon set can change in one place.

use gpui_kit::{SharedString, assets::IconName, component::Icon};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FerroIcon {
    // Tool windows
    Project,
    Commit,
    Structure,
    Run,
    Debug,
    Problems,
    Git,
    Terminal,
    Todo,
    Notifications,
    Database,
    Build,
    // Toolbar and actions
    Search,
    Settings,
    Stop,
    Menu,
    Branch,
    Close,
    Pin,
    Unpin,
    SplitRight,
    SplitDown,
    Maximize,
    Restore,
    Ellipsis,
    ChevronDown,
    ChevronRight,
    // Files
    Folder,
    FolderOpen,
    File,
    FileCode,
    FileText,
    FileConfig,
    // Welcome screen
    NewProject,
    OpenProject,
    GetFromVcs,
    // Appearance
    ThemeDark,
    ThemeLight,
    HighContrast,
    Language,
    Keyboard,
    // Status
    Info,
    Ok,
    Warning,
    Error,
    Loading,
    Memory,
}

impl FerroIcon {
    pub const ALL: [FerroIcon; 47] = [
        FerroIcon::Project,
        FerroIcon::Commit,
        FerroIcon::Structure,
        FerroIcon::Run,
        FerroIcon::Debug,
        FerroIcon::Problems,
        FerroIcon::Git,
        FerroIcon::Terminal,
        FerroIcon::Todo,
        FerroIcon::Notifications,
        FerroIcon::Database,
        FerroIcon::Build,
        FerroIcon::Search,
        FerroIcon::Settings,
        FerroIcon::Stop,
        FerroIcon::Menu,
        FerroIcon::Branch,
        FerroIcon::Close,
        FerroIcon::Pin,
        FerroIcon::Unpin,
        FerroIcon::SplitRight,
        FerroIcon::SplitDown,
        FerroIcon::Maximize,
        FerroIcon::Restore,
        FerroIcon::Ellipsis,
        FerroIcon::ChevronDown,
        FerroIcon::ChevronRight,
        FerroIcon::Folder,
        FerroIcon::FolderOpen,
        FerroIcon::File,
        FerroIcon::FileCode,
        FerroIcon::FileText,
        FerroIcon::FileConfig,
        FerroIcon::NewProject,
        FerroIcon::OpenProject,
        FerroIcon::GetFromVcs,
        FerroIcon::ThemeDark,
        FerroIcon::ThemeLight,
        FerroIcon::HighContrast,
        FerroIcon::Language,
        FerroIcon::Keyboard,
        FerroIcon::Info,
        FerroIcon::Ok,
        FerroIcon::Warning,
        FerroIcon::Error,
        FerroIcon::Loading,
        FerroIcon::Memory,
    ];

    pub fn name(self) -> IconName {
        use FerroIcon::*;
        match self {
            Project => IconName::FolderTree,
            Commit => IconName::GitCommitHorizontal,
            Structure => IconName::ListTree,
            Run => IconName::Play,
            Debug => IconName::Bug,
            Problems => IconName::CircleAlert,
            Git => IconName::GitGraph,
            Terminal => IconName::SquareTerminal,
            Todo => IconName::ListTodo,
            Notifications => IconName::Bell,
            Database => IconName::Database,
            Build => IconName::Hammer,
            Search => IconName::Search,
            Settings => IconName::Settings,
            Stop => IconName::CircleStop,
            Menu => IconName::Menu,
            Branch => IconName::GitBranch,
            Close => IconName::X,
            Pin => IconName::Pin,
            Unpin => IconName::PinOff,
            SplitRight => IconName::Columns2,
            SplitDown => IconName::Rows2,
            Maximize => IconName::Maximize2,
            Restore => IconName::Minimize2,
            Ellipsis => IconName::EllipsisVertical,
            ChevronDown => IconName::ChevronDown,
            ChevronRight => IconName::ChevronRight,
            Folder => IconName::Folder,
            FolderOpen => IconName::FolderOpen,
            File => IconName::File,
            FileCode => IconName::FileCode,
            FileText => IconName::FileText,
            FileConfig => IconName::Braces,
            NewProject => IconName::FolderPlus,
            OpenProject => IconName::FolderOpen,
            GetFromVcs => IconName::FolderGit2,
            ThemeDark => IconName::Moon,
            ThemeLight => IconName::Sun,
            HighContrast => IconName::Contrast,
            Language => IconName::Languages,
            Keyboard => IconName::Keyboard,
            Info => IconName::Info,
            Ok => IconName::CircleCheck,
            Warning => IconName::TriangleAlert,
            Error => IconName::CircleX,
            Loading => IconName::LoaderCircle,
            Memory => IconName::MemoryStick,
        }
    }

    pub fn path(self) -> SharedString {
        self.name().path()
    }

    /// Icon for a file, by extension.
    pub fn for_file(name: &str) -> FerroIcon {
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
        match ext.as_deref() {
            Some(
                "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "java" | "kt" | "go" | "c" | "cpp"
                | "h" | "cs" | "php" | "rb",
            ) => FerroIcon::FileCode,
            Some("toml" | "json" | "yaml" | "yml" | "xml" | "ini") => FerroIcon::FileConfig,
            Some("md" | "txt" | "rst") => FerroIcon::FileText,
            _ => FerroIcon::File,
        }
    }
}

impl From<FerroIcon> for Icon {
    fn from(icon: FerroIcon) -> Self {
        Icon::new(icon.name())
    }
}
