//! Report data and output modes.
//!
//! Every mode formats the same `RunReport`: the legacy reporter is
//! byte compatible with wrk, later modes add their own views.

use std::io;
use std::sync::Arc;

use wrkrs_engine::{ErrorCounts, ScriptEngine, Summary};

use crate::stats::Histogram;

/// The aggregated measurements of one finished run.
pub struct RunReport {
    /// The wall clock runtime in microseconds.
    pub duration_us: u64,
    /// Completed requests.
    pub complete: u64,
    /// Bytes read.
    pub bytes: u64,
    /// Socket error counters.
    pub errors: ErrorCounts,
    /// The latency histogram.
    pub latency: Arc<Histogram>,
    /// The request rate histogram.
    pub rate: Arc<Histogram>,
}

impl RunReport {
    /// Calls the script done phase on the main engine. The report
    /// prints before it, the C order.
    pub fn call_done(&self, main: &mut dyn ScriptEngine) {
        if !main.capabilities().has_done {
            return;
        }
        let summary = Summary {
            duration: self.duration_us,
            requests: self.complete,
            bytes: self.bytes,
            errors: self.errors,
        };
        if let Err(error) = main.done(&summary, self.latency.clone(), self.rate.clone()) {
            // The unprotected C call aborts, we report and keep the
            // rest of the output.
            eprintln!("{}", error.raw_message());
        }
    }
}

/// Formats one finished run into an output stream.
pub trait Reporter {
    /// Writes the report.
    fn report(&self, out: &mut dyn io::Write, run: &RunReport);
}
