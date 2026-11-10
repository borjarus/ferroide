//! Turning file bytes into editor text and back, atomic saving, size
//! classes for large files, and fingerprints for noticing changes made by
//! other programs.

use std::{
    fs,
    hash::Hasher as _,
    io::{self, Read as _, Write as _},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::SystemTime,
};

use crate::{
    encoding::{self, TextEncoding, UnmappableError},
    line_ending::{self, LineEnding, LineEndingStats},
};

/// Above this size a file opens in "large file" mode: no syntax
/// highlighting outside the visible range, no language server, no soft
/// wrap.
pub const LARGE_FILE_BYTES: u64 = 20 * 1024 * 1024;

/// Above this size only a read-only preview of the beginning is shown.
pub const HUGE_FILE_BYTES: u64 = 500 * 1024 * 1024;

/// How much of a huge file the preview shows.
pub const PREVIEW_BYTES: usize = 4 * 1024 * 1024;

/// How a file of a given size is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SizeClass {
    Normal,
    Large,
    Huge,
}

impl SizeClass {
    pub fn of(len: u64) -> Self {
        if len > HUGE_FILE_BYTES {
            SizeClass::Huge
        } else if len > LARGE_FILE_BYTES {
            SizeClass::Large
        } else {
            SizeClass::Normal
        }
    }
}

/// How the text is written back: encoding (with or without BOM) and line
/// separator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FileFormat {
    pub encoding: TextEncoding,
    pub line_ending: LineEnding,
}

impl FileFormat {
    /// The format of a new file.
    pub fn new_file() -> Self {
        Self {
            encoding: TextEncoding::UTF8,
            line_ending: LineEnding::native(),
        }
    }
}

/// A file decoded for the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedText {
    /// With `\n` line breaks only.
    pub text: String,
    pub format: FileFormat,
    /// The file mixed line separators; saving unifies them to
    /// `format.line_ending`.
    pub mixed_line_endings: bool,
    /// Bytes that were invalid in the detected encoding became U+FFFD;
    /// saving would not restore them.
    pub had_decode_errors: bool,
    /// The bytes look like a binary file.
    pub binary: bool,
    /// Only the beginning of the file was loaded (huge-file preview).
    pub truncated: bool,
}

/// Decode file bytes for the editor.
pub fn load(bytes: &[u8]) -> LoadedText {
    load_with(bytes, None)
}

/// Decode file bytes, with a given encoding or a detected one.
pub fn load_with(bytes: &[u8], encoding: Option<TextEncoding>) -> LoadedText {
    let binary = encoding.is_none() && encoding::looks_binary(bytes);
    let decoded = match encoding {
        Some(encoding) => encoding::decode_with(bytes, encoding),
        None => encoding::decode(bytes),
    };
    let stats = LineEndingStats::of(&decoded.text);
    let line_ending = stats.dominant().unwrap_or_else(LineEnding::native);
    LoadedText {
        text: line_ending::normalize(&decoded.text).into_owned(),
        format: FileFormat {
            encoding: decoded.encoding,
            line_ending,
        },
        mixed_line_endings: stats.is_mixed(),
        had_decode_errors: decoded.had_errors,
        binary,
        truncated: false,
    }
}

/// The bytes to write for `text` (with `\n` line breaks) in `format`.
pub fn to_bytes(text: &str, format: FileFormat) -> Result<Vec<u8>, UnmappableError> {
    let text = line_ending::denormalize(text, format.line_ending);
    encoding::encode(&text, format.encoding)
}

/// Read a file from disk for the editor. Huge files are read only up to
/// [`PREVIEW_BYTES`] and marked `truncated`.
pub fn read(path: &Path) -> io::Result<(LoadedText, FileStamp)> {
    let mut file = fs::File::open(path)?;
    let metadata = file.metadata()?;
    let len = metadata.len();
    let truncated = SizeClass::of(len) == SizeClass::Huge;
    let mut bytes = Vec::new();
    if truncated {
        (&mut file)
            .take(PREVIEW_BYTES as u64)
            .read_to_end(&mut bytes)?;
    } else {
        bytes.reserve(len as usize);
        file.read_to_end(&mut bytes)?;
    }
    let stamp = FileStamp {
        len,
        modified: metadata.modified().ok(),
        hash: (!truncated).then(|| content_hash(&bytes)),
    };
    let mut loaded = load(&bytes);
    loaded.truncated = truncated;
    Ok((loaded, stamp))
}

/// What the editor knows about the file on disk, to notice changes made by
/// other programs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileStamp {
    pub len: u64,
    pub modified: Option<SystemTime>,
    /// Hash of the content, when it was read in full.
    pub hash: Option<u64>,
}

impl FileStamp {
    /// A stamp of bytes held in memory (files of the project service).
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self {
            len: bytes.len() as u64,
            modified: None,
            hash: Some(content_hash(bytes)),
        }
    }

    /// Size and modification time of `path`, without reading it.
    pub fn of_metadata(path: &Path) -> io::Result<Self> {
        let metadata = fs::metadata(path)?;
        Ok(Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            hash: None,
        })
    }

    /// Whether `current` (usually from [`FileStamp::of_metadata`]) may
    /// describe different content. Equal size and time count as unchanged;
    /// when both carry a hash, the hash decides.
    pub fn differs_from(&self, current: &FileStamp) -> bool {
        if let (Some(a), Some(b)) = (self.hash, current.hash) {
            return a != b;
        }
        self.len != current.len || self.modified != current.modified
    }
}

/// A 64-bit hash of file content. Stamps live only in memory, so the hash
/// needs to be stable within one run, not across versions.
pub fn content_hash(bytes: &[u8]) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    hasher.write(bytes);
    hasher.finish()
}

/// Write `bytes` to `path` atomically: into a temporary file next to it,
/// flushed to disk, then renamed over the original. A crash or full disk
/// leaves either the old or the new content, never a mix. The original's
/// permissions are kept; a symbolic link keeps pointing at its (updated)
/// target.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<FileStamp> {
    let target = resolve_symlink(path);
    let dir = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = target
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?
        .to_string_lossy();
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let temp = dir.join(format!(
        ".{name}.ferro-save-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if let Ok(metadata) = fs::metadata(&target) {
            let permissions = metadata.permissions();
            if permissions.readonly() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("{} is read-only", target.display()),
                ));
            }
            fs::set_permissions(&temp, permissions)?;
        }
        fs::rename(&temp, &target)?;
        #[cfg(unix)]
        if let Ok(dir) = fs::File::open(dir) {
            // Persist the rename itself.
            let _ = dir.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result?;
    let metadata = fs::metadata(&target)?;
    Ok(FileStamp {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        hash: Some(content_hash(bytes)),
    })
}

fn resolve_symlink(path: &Path) -> PathBuf {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
        }
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_and_save_round_trip_keeps_the_format() {
        let original = "\u{FEFF}line one\r\nline two\r\n".as_bytes();
        let loaded = load(original);
        assert_eq!(loaded.text, "line one\nline two\n");
        assert_eq!(loaded.format.line_ending, LineEnding::CrLf);
        assert_eq!(loaded.format.encoding, TextEncoding::UTF8_BOM);
        assert!(!loaded.mixed_line_endings);
        assert_eq!(to_bytes(&loaded.text, loaded.format).unwrap(), original);
    }

    #[test]
    fn size_classes() {
        assert_eq!(SizeClass::of(10), SizeClass::Normal);
        assert_eq!(SizeClass::of(LARGE_FILE_BYTES + 1), SizeClass::Large);
        assert_eq!(SizeClass::of(HUGE_FILE_BYTES + 1), SizeClass::Huge);
    }

    #[test]
    fn atomic_write_replaces_and_stamps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        fs::write(&path, "old").unwrap();
        let stamp = write_atomic(&path, b"new content").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new content");
        assert_eq!(stamp.len, 11);
        // Only the file itself is left; no temporary files.
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        let (loaded, read_stamp) = read(&path).unwrap();
        assert_eq!(loaded.text, "new content");
        assert!(!stamp.differs_from(&read_stamp));
        fs::write(&path, "changed by someone else").unwrap();
        let now = FileStamp::of_metadata(&path).unwrap();
        assert!(stamp.differs_from(&now));
    }

    #[test]
    fn atomic_write_creates_new_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.rs");
        write_atomic(&path, b"fn main() {}\n").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"fn main() {}\n");
    }

    #[test]
    fn read_only_files_are_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("locked.txt");
        fs::write(&path, "keep").unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions.clone()).unwrap();
        let err = write_atomic(&path, b"lost").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read_to_string(&path).unwrap(), "keep");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        fs::set_permissions(&path, permissions).unwrap();
    }

    #[test]
    fn unmappable_text_is_not_saved_silently() {
        let format = FileFormat {
            encoding: TextEncoding::new(encoding_rs::WINDOWS_1250, false),
            line_ending: LineEnding::Lf,
        };
        assert!(to_bytes("zażółć", format).is_ok());
        assert!(to_bytes("漢", format).is_err());
    }
}
