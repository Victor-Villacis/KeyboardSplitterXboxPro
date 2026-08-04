//! Byte-level decoding of the legacy XML files.
//!
//! The legacy app wrote every file with .NET `Encoding.Unicode` = UTF-16LE with
//! BOM. We honor any BOM (UTF-16LE/BE, UTF-8), defensively accept BOM-less
//! UTF-8, and apply a heuristic for BOM-less UTF-16LE (ASCII `<` followed by a
//! NUL byte — a hand-truncated or tool-mangled legacy file).

use encoding_rs::{UTF_16LE, UTF_8};

/// Decode legacy XML bytes to a string.
///
/// Returns the decoded text and whether malformed sequences were replaced
/// (U+FFFD) during decoding.
pub fn decode_xml(bytes: &[u8]) -> (String, bool) {
    if encoding_rs::Encoding::for_bom(bytes).is_none()
        && bytes.len() >= 2
        && bytes[0] != 0x00
        && bytes[1] == 0x00
    {
        let (text, had_errors) = UTF_16LE.decode_without_bom_handling(bytes);
        return (text.into_owned(), had_errors);
    }
    // BOM sniffing: a UTF-16LE/BE or UTF-8 BOM wins; otherwise UTF-8.
    let (text, _, had_errors) = UTF_8.decode(bytes);
    (text.into_owned(), had_errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16le_with_bom(text: &str) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xFE];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn utf16le_bom_is_decoded_and_stripped() {
        let (text, bad) = decode_xml(&utf16le_with_bom("<a>x</a>"));
        assert_eq!(text, "<a>x</a>");
        assert!(!bad);
    }

    #[test]
    fn utf16be_bom_is_decoded() {
        let mut bytes = vec![0xFE, 0xFF];
        for unit in "<a/>".encode_utf16() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        let (text, bad) = decode_xml(&bytes);
        assert_eq!(text, "<a/>");
        assert!(!bad);
    }

    #[test]
    fn plain_utf8_is_accepted() {
        let (text, bad) = decode_xml("<a>\u{00E9}</a>".as_bytes());
        assert_eq!(text, "<a>\u{00E9}</a>");
        assert!(!bad);
    }

    #[test]
    fn utf8_bom_is_stripped() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"<a/>");
        let (text, bad) = decode_xml(&bytes);
        assert_eq!(text, "<a/>");
        assert!(!bad);
    }

    #[test]
    fn bomless_utf16le_heuristic() {
        let bytes = &utf16le_with_bom("<a>x</a>")[2..].to_vec();
        let (text, bad) = decode_xml(bytes);
        assert_eq!(text, "<a>x</a>");
        assert!(!bad);
    }

    #[test]
    fn malformed_utf8_is_flagged_not_fatal() {
        let (_, bad) = decode_xml(&[b'<', 0xC3, b'>']);
        assert!(bad);
    }
}
