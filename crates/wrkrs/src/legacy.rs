//! The legacy reporter, byte compatible with wrk.
//!
//! This module ports the print side of wrk.c: the unit column layout
//! with its trailing letter pad, the thread stats section, the latency
//! distribution, and the footer.

use std::io::{self, Write};

/// Writes one unit column, `print_units` from wrk.c.
///
/// The formatted value shrinks the pad by one space per trailing
/// letter so unit letters count toward the column width, and the
/// width doubles as a precision: a value longer than its column is
/// truncated, losing its unit letters first.
pub fn print_units(out: &mut dyn Write, msg: &str, width: usize) -> io::Result<()> {
    let bytes = msg.as_bytes();
    let mut pad = 2;
    if bytes.last().is_some_and(u8::is_ascii_alphabetic) {
        pad -= 1;
    }
    if bytes.len() >= 2 && bytes[bytes.len() - 2].is_ascii_alphabetic() {
        pad -= 1;
    }
    let width = width - pad;
    let truncated = &msg[..msg.len().min(width)];
    write!(out, "{truncated:>width$}")?;
    for _ in 0..pad {
        out.write_all(b" ")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::print_units;

    fn units(msg: &str, width: usize) -> String {
        let mut out = Vec::new();
        print_units(&mut out, msg, width).expect("write");
        String::from_utf8(out).expect("ascii")
    }

    #[test]
    fn counts_trailing_letters_toward_the_width() {
        assert_eq!(units("1.23ms", 8), "  1.23ms");
        assert_eq!(units("1.23", 8), "  1.23  ");
        // One letter keeps one pad space: nine of column plus pad.
        assert_eq!(units("1.00K", 10), "    1.00K ");
        assert_eq!(units("999.99", 10), "  999.99  ");
    }

    #[test]
    fn one_trailing_letter_keeps_one_pad() {
        assert_eq!(units("0.85m", 9), "   0.85m ");
    }

    #[test]
    fn truncates_long_values_losing_the_unit_first() {
        // Ten characters in an eight wide column: the ms is cut.
        assert_eq!(units("12345.67ms", 8), "12345.67");
        // The pad shrinks the usable width to six.
        assert_eq!(units("1234.567", 8), "1234.5  ");
    }
}
