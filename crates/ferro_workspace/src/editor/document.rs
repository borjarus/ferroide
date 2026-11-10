//! An editor tab's file: reading it in any encoding and line separator,
//! saving it back atomically in the same format, and noticing when another
//! program changed it (Stage 3).
//!
//! Files inside the project go through the project service; anything else
//! (e.g. the raw `settings.toml`) is read and written on disk directly.

use std::{
    future::Future,
    io,
    path::{Path, PathBuf},
    pin::Pin,
};

use ferro_services::Services;
use ferro_text::{
    Rope,
    encoding::{TextEncoding, UnmappableError},
    file::{self, FileFormat, FileStamp, LoadedText, SizeClass},
};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// What the editor knows about the file behind a tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskInfo {
    /// Encoding and line separator the file is written back with.
    pub format: FileFormat,
    /// The file as last read or written; `None` before the first load.
    pub stamp: Option<FileStamp>,
    pub size: SizeClass,
    pub mixed_line_endings: bool,
    pub decode_errors: bool,
    /// Only the beginning of a huge file is shown; the tab is read-only.
    pub truncated: bool,
    pub binary: bool,
}

impl Default for DiskInfo {
    fn default() -> Self {
        Self {
            format: FileFormat::new_file(),
            stamp: None,
            size: SizeClass::Normal,
            mixed_line_endings: false,
            decode_errors: false,
            truncated: false,
            binary: false,
        }
    }
}

impl DiskInfo {
    fn new(loaded: &LoadedText, stamp: FileStamp) -> Self {
        Self {
            format: loaded.format,
            stamp: Some(stamp),
            size: SizeClass::of(stamp.len),
            mixed_line_endings: loaded.mixed_line_endings,
            decode_errors: loaded.had_decode_errors,
            truncated: loaded.truncated,
            binary: loaded.binary,
        }
    }

    /// Large and huge files: no highlighting, analysis or soft wrap.
    pub fn is_large(&self) -> bool {
        self.size >= SizeClass::Large
    }

    pub fn is_read_only(&self) -> bool {
        self.truncated
    }
}

/// A file read for the editor.
#[derive(Debug, Clone)]
pub struct Loaded {
    /// With `\n` line breaks.
    pub text: String,
    pub info: DiskInfo,
}

/// Whether `path` belongs to the open project.
pub fn in_project(path: &Path, services: &Services) -> bool {
    services
        .project
        .roots()
        .iter()
        .any(|root| path.starts_with(root))
}

fn decode(bytes: &[u8], encoding: Option<TextEncoding>) -> Loaded {
    let size = SizeClass::of(bytes.len() as u64);
    let shown = if size == SizeClass::Huge {
        &bytes[..file::PREVIEW_BYTES.min(bytes.len())]
    } else {
        bytes
    };
    let mut loaded = file::load_with(shown, encoding);
    loaded.truncated = size == SizeClass::Huge;
    let stamp = if loaded.truncated {
        FileStamp {
            len: bytes.len() as u64,
            modified: None,
            hash: None,
        }
    } else {
        FileStamp::of_bytes(bytes)
    };
    Loaded {
        info: DiskInfo::new(&loaded, stamp),
        text: loaded.text,
    }
}

/// Read and decode `path`, detecting its encoding unless one is given
/// (Reopen with Encoding). Runs on a background thread.
pub fn load(
    path: &Path,
    services: &Services,
    encoding: Option<TextEncoding>,
) -> BoxFuture<anyhow::Result<Loaded>> {
    if in_project(path, services) {
        let read = services.project.read_bytes(path);
        return Box::pin(async move { Ok(decode(&read.await?, encoding)) });
    }
    let path = path.to_path_buf();
    Box::pin(async move {
        let described = |err: io::Error| anyhow::anyhow!("{}: {err}", path.display());
        if encoding.is_none() {
            // Reads only the preview of a huge file.
            let (loaded, stamp) = file::read(&path).map_err(described)?;
            return Ok(Loaded {
                info: DiskInfo::new(&loaded, stamp),
                text: loaded.text,
            });
        }
        let bytes = std::fs::read(&path).map_err(described)?;
        let mut loaded = decode(&bytes, encoding);
        if let Some(stamp) = &mut loaded.info.stamp {
            stamp.modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        }
        Ok(loaded)
    })
}

/// Why a save failed.
#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error(transparent)]
    Unmappable(#[from] UnmappableError),
    #[error("{0}")]
    Io(String),
}

/// Encode `text` in `format` and write it to `path` atomically. Returns
/// the stamp of what was written. The rope snapshot becomes a string on the
/// background thread that runs this.
pub fn save(
    path: &Path,
    services: &Services,
    text: Rope,
    format: FileFormat,
) -> BoxFuture<Result<FileStamp, SaveError>> {
    let in_project = in_project(path, services);
    let project = services.project.clone();
    let path = path.to_path_buf();
    Box::pin(async move {
        let bytes = file::to_bytes(&text.to_string(), format)?;
        if in_project {
            let stamp = FileStamp::of_bytes(&bytes);
            project
                .write_file(&path, bytes)
                .await
                .map_err(|err| SaveError::Io(err.to_string()))?;
            Ok(stamp)
        } else {
            file::write_atomic(&path, &bytes).map_err(|err| SaveError::Io(err.to_string()))
        }
    })
}

/// What a check for changes on disk found.
#[derive(Debug, Clone)]
pub enum DiskState {
    Unchanged,
    /// Only the modification time moved; the content is the same.
    Touched(FileStamp),
    /// Different content, already read.
    Changed(Loaded),
    Deleted,
}

/// Compare the file on disk with `known` (the stamp of the last load or
/// save). Cheap when nothing changed: a metadata call for files on disk.
pub fn check(
    path: &Path,
    services: &Services,
    known: FileStamp,
    encoding: TextEncoding,
) -> BoxFuture<DiskState> {
    let path: PathBuf = path.to_path_buf();
    if in_project(&path, services) {
        let read = services.project.read_bytes(&path);
        return Box::pin(async move {
            let Ok(bytes) = read.await else {
                return DiskState::Deleted;
            };
            let stamp = FileStamp::of_bytes(&bytes);
            if !known.differs_from(&stamp) {
                DiskState::Unchanged
            } else {
                DiskState::Changed(decode(&bytes, Some(encoding)))
            }
        });
    }
    Box::pin(async move {
        let current = match FileStamp::of_metadata(&path) {
            Ok(stamp) => stamp,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return DiskState::Deleted,
            Err(_) => return DiskState::Unchanged,
        };
        if !known.differs_from(&current) {
            return DiskState::Unchanged;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            return DiskState::Unchanged;
        };
        let full = FileStamp {
            hash: Some(file::content_hash(&bytes)),
            ..current
        };
        if known.hash.is_some() && known.hash == full.hash {
            return DiskState::Touched(full);
        }
        let mut loaded = decode(&bytes, Some(encoding));
        loaded.info.stamp = Some(full);
        DiskState::Changed(loaded)
    })
}

/// A unified diff of two versions of a file, for the Compare view.
pub fn unified_diff(path: &Path, disk: &str, editor: &str) -> String {
    let name = path.to_string_lossy().replace('\\', "/");
    let body = similar::TextDiff::from_lines(disk, editor)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{name}"), &format!("b/{name}"))
        .to_string();
    format!("diff --git a/{name} b/{name}\n{body}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_has_headers_and_hunks() {
        let diff = unified_diff(Path::new("src/a.rs"), "a\nb\n", "a\nc\n");
        assert!(diff.contains("--- a/src/a.rs"));
        assert!(diff.contains("-b"));
        assert!(diff.contains("+c"));
    }
}
