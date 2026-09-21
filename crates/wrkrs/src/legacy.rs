//! The legacy reporter, byte compatible with wrk.
//!
//! This module ports the print side of wrk.c: the unit column layout
//! with its trailing letter pad, the thread stats section, the latency
//! distribution, and the footer.

use std::io::{self, Write};

use crate::stats::Histogram;
use crate::units::format_time_us;

/// The legacy reporter, byte compatible with wrk.
#[derive(Default)]
pub struct LegacyReporter {
    /// Whether the latency distribution prints, the cfg.latency of
    /// the C report.
    pub latency: bool,
}

impl LegacyReporter {
    /// Writes the thread stats column header.
    pub fn stats_header(out: &mut dyn Write) -> io::Result<()> {
        writeln!(
            out,
            "  Thread Stats{:>6}{:>11}{:>8}{:>12}",
            "Avg", "Stdev", "Max", "+/- Stdev"
        )
    }

    /// Writes one stats row: name, mean, stdev, max, and the share
    /// of samples within one stdev.
    pub fn stats_row(
        out: &mut dyn Write,
        name: &str,
        stats: &Histogram,
        format: fn(f64) -> String,
    ) -> io::Result<()> {
        let mean = stats.mean();
        let stdev = stats.stdev();
        write!(out, "    {name:<10}")?;
        print_units(out, &format(mean), 8)?;
        print_units(out, &format(stdev), 10)?;
        print_units(out, &format(stats.max() as f64), 9)?;
        writeln!(out, "{:>8.2}%", stats.within_stdev(mean, stdev, 1))
    }

    /// Writes the latency distribution section.
    pub fn distribution(out: &mut dyn Write, latency: &Histogram) -> io::Result<()> {
        writeln!(out, "  Latency Distribution")?;
        for percentile in [50.0, 75.0, 90.0, 99.0] {
            write!(out, "{percentile:>7.0}%")?;
            print_units(
                out,
                &format_time_us(latency.percentile(percentile) as f64),
                10,
            )?;
            writeln!(out)?;
        }
        Ok(())
    }
}

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
    use std::sync::Arc;

    use super::{LegacyReporter, print_units};

    fn units(msg: &str, width: usize) -> String {
        let mut out = Vec::new();
        print_units(&mut out, msg, width).expect("write");
        String::from_utf8(out).expect("ascii")
    }

    fn histogram_of(values: &[u64]) -> Arc<super::Histogram> {
        let stats = Arc::new(super::Histogram::new(2_000_000));
        for value in values {
            stats.record(*value);
        }
        stats
    }

    fn row(name: &str, stats: &Arc<super::Histogram>, format: fn(f64) -> String) -> String {
        let mut out = Vec::new();
        LegacyReporter::stats_row(&mut out, name, stats, format).expect("write");
        String::from_utf8(out).expect("ascii")
    }

    #[test]
    fn lays_out_the_stats_header() {
        let mut out = Vec::new();
        LegacyReporter::stats_header(&mut out).expect("write");
        assert_eq!(
            String::from_utf8(out).expect("ascii"),
            "  Thread Stats   Avg      Stdev     Max   +/- Stdev\n"
        );
    }

    #[test]
    fn lays_out_one_stats_row() {
        let stats = histogram_of(&[1500, 1500, 1500, 1500, 1500]);
        assert_eq!(
            row("Latency", &stats, super::super::units::format_time_us),
            "    Latency     1.50ms    0.00us   1.50ms  100.00%\n"
        );
    }

    #[test]
    fn lays_out_the_distribution() {
        let stats = histogram_of(&[1500, 1500, 1500, 1500, 1500]);
        let mut out = Vec::new();
        LegacyReporter::distribution(&mut out, &stats).expect("write");
        assert_eq!(
            String::from_utf8(out).expect("ascii"),
            concat!(
                "  Latency Distribution\n",
                "     50%    1.50ms\n",
                "     75%    1.50ms\n",
                "     90%    1.50ms\n",
                "     99%    1.50ms\n"
            )
        );
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
