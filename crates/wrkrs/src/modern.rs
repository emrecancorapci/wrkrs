//! The modern reporter, the clean v1 layout.
//!
//! The same measurements as the legacy report without the unit
//! column quirks: fixed width columns, the percentile rows always
//! present, and one plain errors line. No banner, the report stands
//! alone.

use std::io::{self, Write};

use crate::report::{Reporter, RunReport};
use crate::stats::Histogram;
use crate::units::{format_binary, format_metric, format_time_us};

/// Width of one modern report column.
const COLUMN: usize = 13;

/// Writes one row of left aligned columns.
macro_rules! row {
    ($out:expr, $($cell:expr),+ $(,)?) => {{
        let mut line = String::from("  ");
        $( line.push_str(&format!("{:<COLUMN$} ", $cell)); )+
        writeln!($out, "{}", line.trim_end())
    }};
}

/// The modern output mode.
#[derive(Default)]
pub struct ModernReporter;

impl ModernReporter {
    /// Writes the summary block.
    fn summary(out: &mut dyn Write, run: &RunReport) -> io::Result<()> {
        let runtime_s = run.duration_us as f64 / 1_000_000.0;
        writeln!(out, "Summary")?;
        writeln!(
            out,
            "  {} requests in {}, {}B read",
            run.complete,
            format_time_us(run.duration_us as f64),
            format_binary(run.bytes as f64)
        )?;
        writeln!(
            out,
            "  {:.2} req/s, {}B/s",
            run.complete as f64 / runtime_s,
            format_binary(run.bytes as f64 / runtime_s)
        )
    }

    /// Writes the latency table: the spread row and the percentile
    /// rows.
    fn latency(out: &mut dyn Write, latency: &Histogram) -> io::Result<()> {
        let mean = latency.mean();
        let stdev = latency.stdev();
        writeln!(out, "\nLatency")?;
        row!(out, "avg", "stdev", "min", "max")?;
        row!(
            out,
            format_time_us(mean),
            format_time_us(stdev),
            format_time_us(latency.min() as f64),
            format_time_us(latency.max() as f64)
        )?;
        row!(out, "p50", "p75", "p90", "p99")?;
        row!(
            out,
            format_time_us(latency.percentile(50.0) as f64),
            format_time_us(latency.percentile(75.0) as f64),
            format_time_us(latency.percentile(90.0) as f64),
            format_time_us(latency.percentile(99.0) as f64)
        )
    }

    /// Writes the per thread request rate table.
    fn rate(out: &mut dyn Write, rate: &Histogram) -> io::Result<()> {
        let mean = rate.mean();
        let stdev = rate.stdev();
        writeln!(out, "\nRequests per second")?;
        row!(out, "avg", "stdev", "min", "max", "+/- stdev")?;
        row!(
            out,
            format_metric(mean),
            format_metric(stdev),
            format_metric(rate.min() as f64),
            format_metric(rate.max() as f64),
            format!("{:.2}%", rate.within_stdev(mean, stdev, 1))
        )
    }

    /// Writes the errors line, always present for predictability.
    fn errors(out: &mut dyn Write, run: &RunReport) -> io::Result<()> {
        writeln!(
            out,
            "\nErrors\n  connect {}, read {}, write {}, timeout {}, status {}",
            run.errors.connect,
            run.errors.read,
            run.errors.write,
            run.errors.timeout,
            run.errors.status
        )
    }
}

impl Reporter for ModernReporter {
    fn report(&self, out: &mut dyn io::Write, run: &RunReport) {
        Self::summary(out, run).expect("report writes");
        Self::latency(out, &run.latency).expect("report writes");
        Self::rate(out, &run.rate).expect("report writes");
        Self::errors(out, run).expect("report writes");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::ModernReporter;
    use crate::report::{Reporter, RunReport};
    use crate::stats::Histogram;

    fn report() -> RunReport {
        let latency = Arc::new(Histogram::new(2_000_000));
        for value in [
            34, 53, 77, 171, 500, 1190, 1200, 1500, 1500, 2000, 3400, 171_000,
        ] {
            latency.record(value);
        }
        let rate = Arc::new(Histogram::new(10_000_000));
        for value in [95_400, 98_700, 99_800, 100_100, 100_300, 105_900, 107_200] {
            rate.record(value);
        }
        RunReport {
            duration_us: 2_123_456,
            complete: 424_167,
            bytes: 16_800_000,
            errors: Default::default(),
            latency,
            rate,
        }
    }

    #[test]
    fn prints_the_modern_report() {
        let mut out = Vec::new();
        ModernReporter.report(&mut out, &report());
        let text = String::from_utf8(out).expect("ascii");
        assert!(
            text.starts_with(concat!(
                "Summary\n",
                "  424167 requests in 2.12s, 16.02MB read\n",
            )),
            "{text}"
        );
        assert!(text.contains("\nLatency\n"), "{text}");
        assert!(text.contains("\nRequests per second\n"), "{text}");
        assert!(
            text.contains("\nErrors\n  connect 0, read 0, write 0, timeout 0, status 0\n"),
            "{text}"
        );
        // No banner in the modern mode.
        assert!(!text.contains("Running"), "{text}");
    }

    #[test]
    fn every_block_carries_numbers() {
        let mut out = Vec::new();
        ModernReporter.report(&mut out, &report());
        let text = String::from_utf8(out).expect("ascii");
        // The spread and percentile rows of the latency table.
        assert!(text.contains("15.22ms"), "{text}");
        assert!(text.contains("49.07ms"), "{text}");
        assert!(text.contains("171.00ms"), "{text}");
        // The rate table carries its cells.
        assert!(text.contains("101.06k"), "{text}");
        assert!(text.contains("57.14%"), "{text}");
    }
}
