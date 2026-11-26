//! Hexadecimal dumps of raw body bytes.

use std::fmt::Write as _;

/// Raw bytes a hexadecimal view shows by default.
pub const HEX_VIEW_BYTES: usize = 8 * 1024;

/// Formats `bytes` as offset, hexadecimal and ASCII columns, 16 bytes per line.
#[must_use]
pub fn dump(bytes: &[u8]) -> String {
    let mut text = String::new();
    for (line, chunk) in bytes.chunks(16).enumerate() {
        let _ = write!(text, "{:08x} ", line * 16);
        for column in 0..16 {
            match chunk.get(column) {
                Some(byte) => {
                    let _ = write!(text, " {byte:02x}");
                }
                None => text.push_str("   "),
            }
        }
        text.push_str("  |");
        text.extend(chunk.iter().map(|&byte| {
            if byte.is_ascii_graphic() || byte == b' ' {
                char::from(byte)
            } else {
                '.'
            }
        }));
        text.push_str("|\n");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::dump;

    #[test]
    fn formats_hex_with_offsets_and_ascii() {
        assert_eq!(
            dump(b"\x89PNG\r\n\x1a\n0123456789"),
            "00000000  89 50 4e 47 0d 0a 1a 0a 30 31 32 33 34 35 36 37  |.PNG....01234567|\n\
             00000010  38 39                                            |89|\n"
        );
    }
}
