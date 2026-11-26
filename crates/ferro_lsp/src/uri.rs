//! `file://` URIs ↔ paths.
//!
//! Servers spell the same file in different ways (`file:///C:/a`,
//! `file:///c%3A/a`); [`uri_to_path`] normalizes the drive letter to upper
//! case, so paths compare equal to the ones the editor uses.

use std::path::{Path, PathBuf};

/// Bytes kept as they are in a path URI; everything else is percent-encoded.
fn keep(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/')
}

/// The `file://` URI of an absolute path.
pub fn path_to_uri(path: &Path) -> String {
    let mut text = path.to_string_lossy().into_owned();
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        text = rest.to_string();
    }
    let text = text.replace('\\', "/");
    let mut out = String::from("file://");
    let (drive, rest) = match text.as_bytes() {
        [letter, b':', ..] if letter.is_ascii_alphabetic() => (
            Some(format!("/{}:", letter.to_ascii_uppercase() as char)),
            &text[2..],
        ),
        _ => (None, text.as_str()),
    };
    if let Some(drive) = drive {
        out.push_str(&drive);
    } else if !rest.starts_with('/') {
        out.push('/');
    }
    for &b in rest.as_bytes() {
        if keep(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The path of a `file://` URI; `None` for other schemes.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // An authority (`file://localhost/…`) is dropped.
    let rest = match rest.find('/') {
        Some(0) => rest,
        Some(i) => &rest[i..],
        None => return None,
    };
    let decoded = percent_decode(rest);
    let bytes = decoded.as_bytes();
    // `/C:/…` → `C:/…`
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
        let mut path = String::with_capacity(decoded.len());
        path.push(bytes[1].to_ascii_uppercase() as char);
        path.push_str(&decoded[2..]);
        if cfg!(windows) {
            path = path.replace('/', "\\");
        }
        return Some(PathBuf::from(path));
    }
    Some(PathBuf::from(decoded))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(value) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(value);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// [`path_to_uri`] as `lsp_types::Uri`.
pub fn lsp_uri(path: &Path) -> lsp_types::Uri {
    path_to_uri(path)
        .parse()
        .unwrap_or_else(|_| "file:///".parse().expect("a valid URI"))
}

/// A path for comparing: on Windows, `\\?\` is dropped and the drive letter
/// is upper case.
pub fn normalize(path: &Path) -> PathBuf {
    uri_to_path(&path_to_uri(path)).unwrap_or_else(|| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let path = if cfg!(windows) {
            PathBuf::from(r"C:\work\my project\src\zażółć #1.rs")
        } else {
            PathBuf::from("/work/my project/src/zażółć #1.rs")
        };
        let uri = path_to_uri(&path);
        assert!(uri.starts_with("file:///"), "{uri}");
        assert!(uri.contains("my%20project"), "{uri}");
        assert!(uri.contains("%23"), "{uri}");
        assert_eq!(uri_to_path(&uri).unwrap(), path);
        assert!(lsp_uri(&path).as_str().starts_with("file:///"));
    }

    #[test]
    fn drive_letters_are_normalized() {
        let path = uri_to_path("file:///c%3A/work/a.rs").unwrap();
        let text = path.to_string_lossy().replace('\\', "/");
        assert_eq!(text, "C:/work/a.rs");
        assert_eq!(path_to_uri(Path::new(r"c:\a")), "file:///C:/a");
    }

    #[test]
    fn other_schemes_have_no_path() {
        assert_eq!(uri_to_path("https://example.com/a"), None);
        assert_eq!(
            uri_to_path("file://localhost/tmp/a").unwrap(),
            PathBuf::from("/tmp/a")
        );
    }
}
