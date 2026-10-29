//! Reading text files the user hands the harness (`--context`, `@task`):
//! UTF-8 with or without a byte-order mark, UTF-16 with one, and a clear
//! error for anything else, so a Windows editor's or PowerShell's default
//! encoding is a message rather than mojibake or a bare "invalid UTF-8".

use std::path::Path;

/// Decode a text file's bytes: UTF-8 (a leading BOM is dropped), UTF-16 LE
/// or BE when a BOM says so. CRLF line ends are left alone; the context
/// splitter normalises them.
pub fn decode_text(bytes: &[u8]) -> Result<String, String> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return utf8(rest);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, u16::from_le_bytes);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, u16::from_be_bytes);
    }
    utf8(bytes)
}

fn utf8(bytes: &[u8]) -> Result<String, String> {
    match std::str::from_utf8(bytes) {
        Ok(s) => Ok(s.to_string()),
        Err(e) => {
            let at = e.valid_up_to();
            Err(format!(
                "not valid UTF-8 at byte {at} (byte 0x{:02X}): save the file as UTF-8 \
                 (PowerShell: Set-Content -Encoding utf8, or Out-File -Encoding utf8; \
                 Windows editors: \"UTF-8\" in Save As), then run again",
                bytes[at]
            ))
        }
    }
}

fn utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> Result<String, String> {
    if !bytes.len().is_multiple_of(2) {
        return Err("a UTF-16 byte-order mark but an odd number of bytes".into());
    }
    let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| unit(*c)).collect();
    String::from_utf16(&units).map_err(|e| format!("not valid UTF-16: {e}"))
}

/// Read and decode a text file; the error names the file and the encoding
/// problem.
pub fn read_text_file(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    decode_text(&bytes).map_err(|why| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{}: {why}", path.display()),
        )
    })
}

/// A rough token count of a text for the model's own planning: about four
/// ASCII characters per token, and one token per character outside ASCII,
/// which is close for CJK text and conservative for accented Latin.
pub fn approx_tokens(text: &str) -> u64 {
    let (ascii, other) = text.chars().fold((0u64, 0u64), |(a, o), c| {
        if c.is_ascii() {
            (a + 1, o)
        } else {
            (a, o + 1)
        }
    });
    ascii / 4 + other
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_utf8_with_and_without_bom() {
        assert_eq!(decode_text("ループ".as_bytes()).unwrap(), "ループ");
        let mut with_bom = vec![0xEF, 0xBB, 0xBF];
        with_bom.extend_from_slice("ループ\r\n\r\n予算".as_bytes());
        assert_eq!(decode_text(&with_bom).unwrap(), "ループ\r\n\r\n予算");
    }

    #[test]
    fn decodes_utf16_with_a_bom() {
        let mut le = vec![0xFF, 0xFE];
        for u in "ループ".encode_utf16() {
            le.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(decode_text(&le).unwrap(), "ループ");
        let mut be = vec![0xFE, 0xFF];
        for u in "予算".encode_utf16() {
            be.extend_from_slice(&u.to_be_bytes());
        }
        assert_eq!(decode_text(&be).unwrap(), "予算");
        assert!(decode_text(&[0xFF, 0xFE, 0x41]).is_err());
    }

    #[test]
    fn other_encodings_are_named_not_garbled() {
        // "ループ" in Shift-JIS (cp932): not UTF-8.
        let sjis = [0x83, 0x8B, 0x81, 0x5B, 0x83, 0x76];
        let err = decode_text(&sjis).unwrap_err();
        assert!(err.contains("not valid UTF-8 at byte 0"), "{err}");
        assert!(err.contains("Set-Content -Encoding utf8"), "{err}");
    }

    #[test]
    fn tokens_count_cjk_per_character() {
        assert_eq!(approx_tokens("abcdefgh"), 2);
        assert_eq!(approx_tokens("ループエンジニアリング"), 11);
        assert_eq!(approx_tokens("ab 予算"), 2);
    }
}
