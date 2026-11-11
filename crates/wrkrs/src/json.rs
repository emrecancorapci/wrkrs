//! The JSON reporter, the machine readable v1 object.
//!
//! One object per run, pretty printed. Latency values are
//! microseconds, rate values are requests per second, and the
//! throughput fields carry full float precision.

use std::io;

use serde::Serialize;

use crate::report::{Reporter, RunReport};
use crate::stats::Histogram;

/// The error counters.
#[derive(Serialize)]
pub(crate) struct Errors {
    pub(crate) connect: u64,
    pub(crate) read: u64,
    pub(crate) write: u64,
    pub(crate) timeout: u64,
    pub(crate) status: u64,
}

/// One statistics block.
#[derive(Serialize)]
pub(crate) struct Stats {
    pub(crate) avg: f64,
    pub(crate) stdev: f64,
    pub(crate) min: u64,
    pub(crate) max: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) p50: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) p75: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) p90: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) p99: Option<u64>,
    pub(crate) within_1_stdev: f64,
}

impl Stats {
    /// Builds the block for a histogram, percentiles included when
    /// asked for.
    fn of(stats: &Histogram, percentiles: bool) -> Stats {
        let mean = stats.mean();
        let stdev = stats.stdev();
        let percentile = |p: f64| percentiles.then(|| stats.percentile(p));
        Stats {
            avg: mean,
            stdev,
            min: stats.min(),
            max: stats.max(),
            p50: percentile(50.0),
            p75: percentile(75.0),
            p90: percentile(90.0),
            p99: percentile(99.0),
            within_1_stdev: stats.within_stdev(mean, stdev, 1),
        }
    }
}

/// The whole run as one object.
#[derive(Serialize)]
pub(crate) struct JsonReport<'a> {
    pub(crate) duration_us: u64,
    pub(crate) requests: u64,
    pub(crate) bytes: u64,
    pub(crate) requests_per_s: f64,
    pub(crate) bytes_per_s: f64,
    pub(crate) errors: Errors,
    pub(crate) latency: Stats,
    pub(crate) rate: Stats,
    /// The wrkrs build that produced the report.
    pub(crate) version: &'a str,
}

/// The JSON output mode.
#[derive(Default)]
pub struct JsonReporter;

impl Reporter for JsonReporter {
    fn report(&self, out: &mut dyn io::Write, run: &RunReport) {
        let report = json_report(run);
        serde_json::to_writer_pretty(&mut *out, &report).expect("report writes");
        let _ = out.write_all(b"\n");
    }
}

/// Builds the v1 report object for a run, shared by every mode that
/// renders the JSON shape.
pub(crate) fn json_report(run: &RunReport) -> JsonReport<'static> {
    json_report_with_version(run, env!("CARGO_PKG_VERSION"))
}

/// Builds the report object carrying an explicit version, the seam
/// other modes pin their fixtures through.
pub(crate) fn json_report_with_version(
    run: &RunReport,
    version: &'static str,
) -> JsonReport<'static> {
    let runtime_s = run.duration_us as f64 / 1_000_000.0;
    JsonReport {
        duration_us: run.duration_us,
        requests: run.complete,
        bytes: run.bytes,
        requests_per_s: run.complete as f64 / runtime_s,
        bytes_per_s: run.bytes as f64 / runtime_s,
        errors: Errors {
            connect: run.errors.connect,
            read: run.errors.read,
            write: run.errors.write,
            timeout: run.errors.timeout,
            status: run.errors.status,
        },
        // Latency in microseconds, the rate in requests per
        // second, percentiles only for latency.
        latency: Stats::of(&run.latency, true),
        rate: Stats::of(&run.rate, false),
        version,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::JsonReporter;
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
            duration_us: 2_000_000,
            complete: 1000,
            bytes: 2000,
            errors: wrkrs_engine::ErrorCounts {
                connect: 0,
                read: 2,
                write: 0,
                timeout: 0,
                status: 7,
            },
            latency,
            rate,
        }
    }

    #[test]
    fn writes_one_parsable_object() {
        let mut out = Vec::new();
        JsonReporter.report(&mut out, &report());
        let text = String::from_utf8(out).expect("utf8");
        assert!(text.ends_with("}\n"));
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(value["duration_us"], 2_000_000);
        assert_eq!(value["requests"], 1000);
        assert_eq!(value["errors"]["read"], 2);
        assert_eq!(value["errors"]["status"], 7);
        assert_eq!(value["requests_per_s"], 500.0);
        assert_eq!(value["latency"]["max"], 171_000);
        assert_eq!(value["latency"]["p99"], 171_000);
        // The rate block carries no percentiles.
        assert!(value["rate"].get("p50").is_none());
    }
}
