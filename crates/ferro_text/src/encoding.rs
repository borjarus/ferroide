//! Character encodings: detection when a file is opened, decoding, and
//! encoding back on save.
//!
//! Detection order: byte order mark → valid UTF-8 → UTF-16 without a BOM
//! (by the pattern of zero bytes) → a statistical guess among legacy
//! encodings (`chardetng`, the detector Firefox uses).

use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE};

/// How many bytes the binary and UTF-16 heuristics look at.
const SNIFF_LEN: usize = 8 * 1024;

/// A file's encoding and whether it starts with a byte order mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextEncoding {
    encoding: &'static Encoding,
    bom: bool,
}

impl Default for TextEncoding {
    fn default() -> Self {
        Self::UTF8
    }
}

impl TextEncoding {
    pub const UTF8: TextEncoding = TextEncoding {
        encoding: &encoding_rs::UTF_8_INIT,
        bom: false,
    };
    pub const UTF8_BOM: TextEncoding = TextEncoding {
        encoding: &encoding_rs::UTF_8_INIT,
        bom: true,
    };

    pub fn new(encoding: &'static Encoding, bom: bool) -> Self {
        // Only Unicode encodings have a byte order mark.
        let bom = bom && is_unicode(encoding);
        Self { encoding, bom }
    }

    /// Look an encoding up by a WHATWG label (`"utf-8"`, `"windows-1250"`,
    /// `"latin2"`, …).
    pub fn for_label(label: &str) -> Option<Self> {
        let label = label.trim();
        if let Some(base) = label
            .strip_suffix(" with BOM")
            .or_else(|| label.strip_suffix("-bom"))
        {
            return Encoding::for_label(base.as_bytes()).map(|e| Self::new(e, true));
        }
        Encoding::for_label(label.as_bytes()).map(|e| Self::new(e, false))
    }

    pub fn encoding(&self) -> &'static Encoding {
        self.encoding
    }

    pub fn has_bom(&self) -> bool {
        self.bom
    }

    pub fn is_utf8(&self) -> bool {
        self.encoding == UTF_8
    }

    /// Status bar label: `UTF-8`, `UTF-8 with BOM`, `UTF-16 LE`,
    /// `windows-1250`…
    pub fn label(&self) -> String {
        let name = match self.encoding.name() {
            "UTF-16LE" => "UTF-16 LE",
            "UTF-16BE" => "UTF-16 BE",
            other => other,
        };
        if self.bom && self.encoding == UTF_8 {
            format!("{name} with BOM")
        } else {
            name.to_string()
        }
    }

    fn bom_bytes(&self) -> &'static [u8] {
        if !self.bom {
            return &[];
        }
        if self.encoding == UTF_8 {
            b"\xEF\xBB\xBF"
        } else if self.encoding == UTF_16LE {
            b"\xFF\xFE"
        } else if self.encoding == UTF_16BE {
            b"\xFE\xFF"
        } else {
            &[]
        }
    }
}

/// Encodings the "Reopen with Encoding" / "Save with Encoding" lists offer,
/// most common first.
pub fn common_encodings() -> Vec<TextEncoding> {
    let mut list = vec![
        TextEncoding::UTF8,
        TextEncoding::UTF8_BOM,
        TextEncoding::new(UTF_16LE, true),
        TextEncoding::new(UTF_16BE, true),
    ];
    list.extend(
        [
            encoding_rs::WINDOWS_1250,
            encoding_rs::WINDOWS_1252,
            encoding_rs::ISO_8859_2,
            encoding_rs::ISO_8859_15,
            encoding_rs::WINDOWS_1251,
            encoding_rs::KOI8_R,
            encoding_rs::SHIFT_JIS,
            encoding_rs::EUC_JP,
            encoding_rs::GBK,
            encoding_rs::GB18030,
            encoding_rs::BIG5,
            encoding_rs::EUC_KR,
        ]
        .into_iter()
        .map(|e| TextEncoding::new(e, false)),
    );
    list
}

fn is_unicode(encoding: &'static Encoding) -> bool {
    encoding == UTF_8 || encoding == UTF_16LE || encoding == UTF_16BE
}

/// The result of decoding a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    pub text: String,
    pub encoding: TextEncoding,
    /// Some bytes were invalid in the encoding and became U+FFFD.
    pub had_errors: bool,
}

/// Whether the bytes look like a binary file rather than text: a zero byte
/// in the first 8 KiB that is not explained by UTF-16.
pub fn looks_binary(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(SNIFF_LEN)];
    head.contains(&0) && utf16_without_bom(head).is_none() && bom_encoding(head).is_none()
}

fn bom_encoding(bytes: &[u8]) -> Option<(&'static Encoding, usize)> {
    Encoding::for_bom(bytes)
}

/// UTF-16 text without a BOM: mostly-ASCII text has a zero in every other
/// byte.
fn utf16_without_bom(head: &[u8]) -> Option<&'static Encoding> {
    let pairs = head.len() / 2;
    if pairs < 2 {
        return None;
    }
    let (mut even_zeros, mut odd_zeros) = (0, 0);
    for pair in head.as_chunks::<2>().0 {
        even_zeros += usize::from(pair[0] == 0);
        odd_zeros += usize::from(pair[1] == 0);
    }
    // At least 40% of the code units have a zero high byte, and the other
    // half of the bytes almost never is zero.
    let threshold = pairs * 2 / 5;
    if odd_zeros >= threshold && even_zeros * 20 <= pairs {
        Some(UTF_16LE)
    } else if even_zeros >= threshold && odd_zeros * 20 <= pairs {
        Some(UTF_16BE)
    } else {
        None
    }
}

/// Detect the encoding of `bytes`.
pub fn detect(bytes: &[u8]) -> TextEncoding {
    if let Some((encoding, _)) = bom_encoding(bytes) {
        return TextEncoding::new(encoding, true);
    }
    // Before the UTF-8 check: ASCII in UTF-16 is valid UTF-8 full of NULs.
    let head = &bytes[..bytes.len().min(SNIFF_LEN)];
    if head.contains(&0)
        && let Some(encoding) = utf16_without_bom(head)
    {
        return TextEncoding::new(encoding, false);
    }
    if std::str::from_utf8(bytes).is_ok() {
        return TextEncoding::UTF8;
    }
    // A file cut in the middle of a character (large-file preview) is still
    // UTF-8 if everything before the last few bytes is valid.
    if let Err(err) = std::str::from_utf8(bytes)
        && err.error_len().is_none()
        && bytes.len() - err.valid_up_to() < 4
    {
        return TextEncoding::UTF8;
    }
    let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    detector.feed(bytes, true);
    TextEncoding::new(detector.guess(None, chardetng::Utf8Detection::Allow), false)
}

/// Detect the encoding and decode.
pub fn decode(bytes: &[u8]) -> Decoded {
    decode_with(bytes, detect(bytes))
}

/// Decode with a given encoding (Reopen with Encoding). A BOM matching the
/// encoding is skipped.
pub fn decode_with(bytes: &[u8], encoding: TextEncoding) -> Decoded {
    let (body, bom) = match bom_encoding(bytes) {
        Some((found, len)) if found == encoding.encoding => (&bytes[len..], true),
        _ => (bytes, false),
    };
    let (text, had_errors) = encoding.encoding.decode_without_bom_handling(body);
    Decoded {
        text: text.into_owned(),
        encoding: TextEncoding::new(encoding.encoding, bom || encoding.bom),
        had_errors,
    }
}

/// Characters the target encoding cannot represent.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{encoding} cannot represent {count} character(s), first {first:?} at byte {offset}")]
pub struct UnmappableError {
    pub encoding: String,
    pub first: char,
    /// Byte offset in the text.
    pub offset: usize,
    pub count: usize,
}

/// Encode `text` (BOM included when the encoding has one).
pub fn encode(text: &str, encoding: TextEncoding) -> Result<Vec<u8>, UnmappableError> {
    let mut out = Vec::with_capacity(text.len() + 3);
    out.extend_from_slice(encoding.bom_bytes());
    let target = encoding.encoding;
    if target == UTF_8 {
        out.extend_from_slice(text.as_bytes());
    } else if target == UTF_16LE || target == UTF_16BE {
        // encoding_rs only decodes UTF-16; encoding it is simple.
        for unit in text.encode_utf16() {
            let bytes = if target == UTF_16LE {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            };
            out.extend_from_slice(&bytes);
        }
    } else {
        let (bytes, _, unmappable) = target.encode(text);
        if unmappable {
            let unmappable: Vec<(usize, char)> = text
                .char_indices()
                .filter(|(_, c)| {
                    let mut buf = [0; 4];
                    target.encode(c.encode_utf8(&mut buf)).2
                })
                .collect();
            let (offset, first) = unmappable[0];
            return Err(UnmappableError {
                encoding: encoding.label(),
                first,
                offset,
                count: unmappable.len(),
            });
        }
        out.extend_from_slice(&bytes);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_and_boms() {
        assert_eq!(detect("zażółć".as_bytes()), TextEncoding::UTF8);
        let with_bom = b"\xEF\xBB\xBFabc";
        let decoded = decode(with_bom);
        assert_eq!(decoded.text, "abc");
        assert_eq!(decoded.encoding, TextEncoding::UTF8_BOM);
        assert_eq!(encode("abc", decoded.encoding).unwrap(), with_bom);
        assert_eq!(decoded.encoding.label(), "UTF-8 with BOM");
    }

    #[test]
    fn utf16_round_trip_with_and_without_bom() {
        for encoding in [
            TextEncoding::new(UTF_16LE, true),
            TextEncoding::new(UTF_16BE, true),
            TextEncoding::new(UTF_16LE, false),
            TextEncoding::new(UTF_16BE, false),
        ] {
            let text = "fn main() {\n    println!(\"zażółć 漢字\");\n}\n";
            let bytes = encode(text, encoding).unwrap();
            let decoded = decode(&bytes);
            assert_eq!(decoded.encoding, encoding, "{}", encoding.label());
            assert_eq!(decoded.text, text);
            assert!(!looks_binary(&bytes));
        }
    }

    #[test]
    fn legacy_polish_text() {
        let text = "Zażółć gęślą jaźń. Pchnąć w tę łódź jeża lub ośm skrzyń fig. \
                    Mężny bądź, chroń pułk twój i sześć flag.";
        let cp1250 = TextEncoding::new(encoding_rs::WINDOWS_1250, false);
        let bytes = encode(text, cp1250).unwrap();
        let decoded = decode(&bytes);
        assert_eq!(decoded.text, text);
        assert!(!decoded.had_errors);
    }

    #[test]
    fn unmappable_characters_are_reported() {
        let latin2 = TextEncoding::new(encoding_rs::ISO_8859_2, false);
        let err = encode("ok 漢 and 字", latin2).unwrap_err();
        assert_eq!(err.first, '漢');
        assert_eq!(err.count, 2);
        assert_eq!(err.offset, 3);
    }

    #[test]
    fn binary_detection() {
        assert!(looks_binary(
            b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0\x03\0>\0"
        ));
        assert!(!looks_binary(b"plain text"));
    }

    #[test]
    fn reopen_with_another_encoding() {
        let bytes = "ł".as_bytes();
        let latin2 = TextEncoding::for_label("latin2").unwrap();
        let decoded = decode_with(bytes, latin2);
        assert_eq!(decoded.text.chars().count(), 2);
        assert_eq!(
            TextEncoding::for_label("utf-8 with BOM"),
            Some(TextEncoding::UTF8_BOM)
        );
    }

    #[test]
    fn truncated_utf8_is_still_utf8() {
        let bytes = "abcż".as_bytes();
        assert_eq!(detect(&bytes[..bytes.len() - 1]), TextEncoding::UTF8);
    }
}
