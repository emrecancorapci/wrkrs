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

    /// The count in one bucket.
    fn bucket(&self, index: u64) -> u64 {
        self.data[index as usize].load(Ordering::Relaxed)
    }

    /// The value at the given percentile between 0 and 100.
    ///
    /// The rank rounds half away from zero after adding a half,
    /// mirroring the C `round((p / 100.0) * count + 0.5)`. An empty
    /// histogram answers zero.
    pub fn percentile(&self, percentile: f64) -> u64 {
        let rank = ((percentile / 100.0) * self.count() as f64 + 0.5).round() as u64;
        let mut total = 0;
        for index in self.min()..=self.max() {
            total += self.bucket(index);
            if total >= rank {
                return index;
            }
        }
        0
    }

    /// The arithmetic mean of the recorded values.
    ///
    /// The sum accumulates in wrapping u64 the way C does.
    pub fn mean(&self) -> f64 {
        let count = self.count();
        if count == 0 {
            return 0.0;
        }
        let mut sum = 0u64;
        for index in self.min()..=self.max() {
            sum = sum.wrapping_add(self.bucket(index).wrapping_mul(index));
        }
        sum as f64 / count as f64
    }

    /// The sample standard deviation of the recorded values.
    ///
    /// Aggregation runs in f64 where C uses long double, the
    /// documented deviation.
    pub fn stdev(&self) -> f64 {
        let count = self.count();
        if count < 2 {
            return 0.0;
        }
        let mean = self.mean();
        let mut sum = 0.0;
        for index in self.min()..=self.max() {
            let bucket = self.bucket(index);
            if bucket != 0 {
                sum += (index as f64 - mean).powi(2) * bucket as f64;
            }
        }
        (sum / (count - 1) as f64).sqrt()
    }

    /// The percentage of values within `n` standard deviations.
    ///
    /// An empty histogram divides zero by zero and answers NaN, the C
    /// behavior included.
    pub fn within_stdev(&self, mean: f64, stdev: f64, n: u64) -> f64 {
        let upper = mean + stdev * n as f64;
        let lower = mean - stdev * n as f64;
        let mut sum = 0u64;
        for index in self.min()..=self.max() {
            if index as f64 >= lower && index as f64 <= upper {
                sum += self.bucket(index);
            }
        }
        (sum as f64 / self.count() as f64) * 100.0
    }

    /// The coordinated omission correction, mirroring stats_correct.
    ///
    /// Every bucket at or above twice the expected interval mirrors
    /// its count down in interval steps while strictly above the
    /// interval. Mirrored buckets can land below the tracked minimum
    /// and stay invisible to the walks, matching C. A nonpositive
    /// interval is a no-op where C would spin forever.
    pub fn correct(&self, expected: i64) {
        if expected <= 0 {
            return;
        }
        let max = self.max();
        let mut value = expected.wrapping_mul(2) as u64;
        while value <= max {
            let count = self.bucket(value);
            let mut mirror = value as i64 - expected;
            while count != 0 && mirror > expected {
                self.data[mirror as usize].fetch_add(count, Ordering::Relaxed);
                self.count.fetch_add(count, Ordering::Relaxed);
                mirror -= expected;
            }
            value += 1;
        }
    }

    /// The number of buckets that hold values.
    pub fn popcount(&self) -> u64 {
        let mut count = 0;
        for index in self.min()..=self.max() {
            if self.bucket(index) != 0 {
                count += 1;
            }
        }
        count
    }

    /// The value and its bucket count at a zero based occupied slot.
    ///
    /// Past the last occupied slot the C code leaves the running
    /// occupied count in place and answers zero, preserved here.
    pub fn value_at(&self, index: u64) -> (u64, u64) {
        let mut count = 0;
        for slot in self.min()..=self.max() {
            let bucket = self.bucket(slot);
            if bucket != 0 {
                let seen = count;
                count += 1;
                if seen == index {
                    return (slot, bucket);
                }
            }
        }
        (0, count)
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

impl wrkrs_engine::StatsView for Histogram {
    fn min(&self) -> u64 {
        Histogram::min(self)
    }

    fn max(&self) -> u64 {
        Histogram::max(self)
    }

    fn mean(&self) -> f64 {
        Histogram::mean(self)
    }

    fn stdev(&self) -> f64 {
        Histogram::stdev(self)
    }

    fn percentile(&self, percentile: f64) -> u64 {
        Histogram::percentile(self, percentile)
    }

    fn popcount(&self) -> u64 {
        Histogram::popcount(self)
    }

    fn value_at(&self, slot: u64) -> (u64, u64) {
        Histogram::value_at(self, slot)
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

    fn loaded() -> Histogram {
        let histogram = Histogram::new(100);
        for value in [7u64, 3, 9, 7, 3, 3] {
            assert!(histogram.record(value));
        }
        histogram
    }

    #[test]
    fn percentile_walks_the_rank() {
        let histogram = loaded();
        // rank = round(p / 100 * 6 + 0.5). The half bump pushes even
        // count medians up: p=50 ranks 4 which lands on the 7 bucket.
        assert_eq!(histogram.percentile(0.0), 3);
        assert_eq!(histogram.percentile(50.0), 7);
        assert_eq!(histogram.percentile(75.0), 7);
        assert_eq!(histogram.percentile(99.0), 9);
        // rank 7 never fills, C answers zero for the top percentile.
        assert_eq!(histogram.percentile(100.0), 0);
    }

    #[test]
    fn an_empty_histogram_answers_zero() {
        let histogram = Histogram::new(100);
        assert_eq!(histogram.percentile(99.0), 0);
        assert_eq!(histogram.popcount(), 0);
        assert_eq!(histogram.value_at(0), (0, 0));
    }

    #[test]
    fn popcount_counts_occupied_buckets() {
        let histogram = loaded();
        assert_eq!(histogram.popcount(), 3);
    }

    #[test]
    fn value_at_reports_slots_and_leaves_the_tail_quirk() {
        let histogram = loaded();
        assert_eq!(histogram.value_at(0), (3, 3));
        assert_eq!(histogram.value_at(1), (7, 2));
        assert_eq!(histogram.value_at(2), (9, 1));
        // Past the end the count carries the occupied total.
        assert_eq!(histogram.value_at(3), (0, 3));
    }

    #[test]
    fn mean_and_stdev_match_hand_computation() {
        let histogram = loaded();
        // sum = 3*3 + 7*2 + 9 = 32, mean = 32/6
        assert!((histogram.mean() - 32.0 / 6.0).abs() < 1e-12);
        let mean = histogram.mean();
        let expected = ((3.0 * (3.0 - mean) * (3.0 - mean)
            + 2.0 * (7.0 - mean) * (7.0 - mean)
            + (9.0 - mean) * (9.0 - mean))
            / 5.0)
            .sqrt();
        assert!((histogram.stdev() - expected).abs() < 1e-12);
    }

    #[test]
    fn single_values_carry_no_stdev() {
        let histogram = Histogram::new(10);
        histogram.record(4);
        assert_eq!(histogram.mean(), 4.0);
        assert_eq!(histogram.stdev(), 0.0);
    }

    #[test]
    fn within_stdev_shares_the_middle() {
        let histogram = loaded();
        let mean = histogram.mean();
        let stdev = histogram.stdev();
        // Buckets 3 and 7 fall inside one stdev, 9 stays out.
        let expected = 5.0 / 6.0 * 100.0;
        assert!((histogram.within_stdev(mean, stdev, 1) - expected).abs() < 1e-9);
    }

    #[test]
    fn an_empty_histogram_shares_nan() {
        let histogram = Histogram::new(10);
        assert!(histogram.within_stdev(0.0, 0.0, 1).is_nan());
    }

    #[test]
    fn correction_mirrors_counts_down_in_steps() {
        let histogram = Histogram::new(200);
        for value in [100u64, 100, 130] {
            histogram.record(value);
        }
        histogram.correct(10);
        // The 100 bucket mirrors into 90..20, the 130 bucket mirrors
        // through 100 on its way down into 120..20, every step strictly
        // above the interval.
        assert_eq!(histogram.bucket(120), 1);
        assert_eq!(histogram.bucket(100), 3);
        assert_eq!(histogram.bucket(90), 3);
        assert_eq!(histogram.bucket(30), 3);
        assert_eq!(histogram.bucket(20), 3);
        assert_eq!(histogram.bucket(10), 0);
        assert_eq!(histogram.count(), 3 + 8 * 2 + 11);
    }

    #[test]
    fn corrected_values_below_the_minimum_stay_invisible() {
        let histogram = Histogram::new(200);
        histogram.record(100);
        histogram.correct(10);
        // The mirrored 90..20 buckets raise the count but the walks
        // still start at the tracked minimum of 100, so no rank fills
        // and the percentile answers zero.
        assert_eq!(histogram.count(), 1 + 8);
        assert_eq!(histogram.min(), 100);
        assert_eq!(histogram.percentile(50.0), 0);
    }

    #[test]
    fn a_nonpositive_interval_corrects_nothing() {
        let histogram = Histogram::new(200);
        histogram.record(100);
        histogram.correct(0);
        histogram.correct(-5);
        assert_eq!(histogram.count(), 1);
    }

    #[test]
    fn the_stats_view_delegates_to_the_histogram() {
        use std::sync::Arc;
        use wrkrs_engine::StatsView;

        let histogram = Arc::new(loaded());
        let view: Arc<dyn StatsView> = histogram.clone();
        assert_eq!(view.min(), 3);
        assert_eq!(view.max(), 9);
        assert!((view.mean() - 32.0 / 6.0).abs() < 1e-12);
        assert_eq!(view.popcount(), 3);
        assert_eq!(view.percentile(50.0), 7);
        assert_eq!(view.value_at(1), (7, 2));
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
