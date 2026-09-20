//! The latency and rate histogram, a port of stats.c.
//!
//! Buckets are atomic counters indexed by value. Reads happen after the
//! run or through the stats view handed to scripts, the same lifecycle
//! as the C struct, so `Relaxed` ordering carries the atomicity and
//! nothing more.

use std::sync::atomic::{AtomicU64, Ordering};

/// A histogram over values up to an inclusive maximum.
pub struct Histogram {
    count: AtomicU64,
    min: AtomicU64,
    max: AtomicU64,
    limit: u64,
    data: Box<[AtomicU64]>,
}

impl Histogram {
    /// Allocates a histogram accepting values up to `max` inclusive,
    /// mirroring stats_alloc.
    pub fn new(max: u64) -> Histogram {
        let limit = max.wrapping_add(1);
        let data = (0..limit).map(|_| AtomicU64::new(0)).collect();
        Histogram {
            count: AtomicU64::new(0),
            min: AtomicU64::new(u64::MAX),
            max: AtomicU64::new(0),
            limit,
            data,
        }
    }

    /// Records one value, returning false when it exceeds the limit.
    ///
    /// The caller counts a timeout for every false, the way wrk does.
    pub fn record(&self, value: u64) -> bool {
        if value >= self.limit {
            return false;
        }
        self.data[value as usize].fetch_add(1, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);

        let mut min = self.min.load(Ordering::Relaxed);
        while value < min {
            match self
                .min
                .compare_exchange(min, value, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => break,
                Err(current) => min = current,
            }
        }

        let mut max = self.max.load(Ordering::Relaxed);
        while value > max {
            match self
                .max
                .compare_exchange(max, value, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => break,
                Err(current) => max = current,
            }
        }
        true
    }

    /// The number of recorded values.
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// The smallest recorded value.
    pub fn min(&self) -> u64 {
        self.min.load(Ordering::Relaxed)
    }

    /// The largest recorded value.
    pub fn max(&self) -> u64 {
        self.max.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use super::Histogram;

    #[test]
    fn records_values_and_tracks_the_range() {
        let histogram = Histogram::new(100);
        assert_eq!(histogram.count(), 0);
        assert_eq!(histogram.min(), u64::MAX);
        assert_eq!(histogram.max(), 0);

        for value in [7u64, 3, 9, 7, 3, 3] {
            assert!(histogram.record(value));
        }
        assert_eq!(histogram.count(), 6);
        assert_eq!(histogram.min(), 3);
        assert_eq!(histogram.max(), 9);
    }

    #[test]
    fn the_maximum_value_is_inclusive() {
        let histogram = Histogram::new(10);
        assert!(histogram.record(10));
        assert!(!histogram.record(11));
        assert_eq!(histogram.count(), 1);
    }

    #[test]
    fn zero_is_a_valid_value() {
        let histogram = Histogram::new(10);
        assert!(histogram.record(0));
        assert_eq!(histogram.min(), 0);
        assert_eq!(histogram.max(), 0);
    }

    #[test]
    fn concurrent_recording_keeps_the_count() {
        let histogram = Arc::new(Histogram::new(10_000));
        let workers: Vec<_> = (0..4u64)
            .map(|worker| {
                let histogram = Arc::clone(&histogram);
                thread::spawn(move || {
                    for value in (worker * 2_500)..((worker + 1) * 2_500) {
                        assert!(histogram.record(value));
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().expect("worker finishes");
        }
        assert_eq!(histogram.count(), 10_000);
        assert_eq!(histogram.min(), 0);
        assert_eq!(histogram.max(), 9_999);
    }
}
