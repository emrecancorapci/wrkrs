//! Unit scanning and formatting.
//!
//! Ports units.c: the scan side reads command line arguments, the
//! format side renders values for the legacy reporter.

/// One unit ladder: a base suffix and the steps above it.
struct Ladder {
    scale: f64,
    base: &'static str,
    units: &'static [&'static str],
}

/// Formats a value down its ladder, mirroring `format_units`.
///
/// The threshold is the first step times 0.85 and never changes, and
/// the loop stops before the last step: the us ladder can only reach
/// ms, and the s ladder only reaches m before its NULL successor ends
/// the climb.
fn format_units(value: f64, ladder: &Ladder, precision: usize) -> String {
    let mut amount = value;
    let mut unit = ladder.base;
    let threshold = ladder.scale * 0.85;

    let mut index = 0;
    while index + 1 < ladder.units.len() && amount >= threshold {
        amount /= ladder.scale;
        unit = ladder.units[index];
        index += 1;
    }

    format!("{amount:.precision$}{unit}")
}

/// The seconds ladder: s to m to h, precision zero.
static TIME_UNITS_S: Ladder = Ladder {
    scale: 60.0,
    base: "s",
    units: &["m", "h"],
};

/// Formats a duration in seconds, `format_time_s`.
pub fn format_time_s(seconds: f64) -> String {
    format_units(seconds, &TIME_UNITS_S, 0)
}

/// The microseconds ladder: us to ms, the s step is unreachable.
static TIME_UNITS_US: Ladder = Ladder {
    scale: 1000.0,
    base: "us",
    units: &["ms", "s"],
};

/// Formats a latency in microseconds, `format_time_us`.
///
/// Values of a second or more switch to the seconds ladder with two
/// decimals.
pub fn format_time_us(microseconds: f64) -> String {
    if microseconds >= 1_000_000.0 {
        format_units(microseconds / 1_000_000.0, &TIME_UNITS_S, 2)
    } else {
        format_units(microseconds, &TIME_UNITS_US, 2)
    }
}

/// The binary ladder for byte counts.
static BINARY_UNITS: Ladder = Ladder {
    scale: 1024.0,
    base: "",
    units: &["K", "M", "G", "T", "P"],
};

/// The metric ladder for request rates.
static METRIC_UNITS: Ladder = Ladder {
    scale: 1000.0,
    base: "",
    units: &["k", "M", "G", "T", "P"],
};

/// Formats a byte count, `format_binary`. The B suffix is the
/// caller's, the ladder only produces the multiplier letter.
pub fn format_binary(bytes: f64) -> String {
    format_units(bytes, &BINARY_UNITS, 2)
}

/// Formats a request rate, `format_metric`.
pub fn format_metric(rate: f64) -> String {
    format_units(rate, &METRIC_UNITS, 2)
}

/// Scans a number with an optional unit suffix.
///
/// Mirrors `scan_units` from units.c:
///
/// - leading whitespace is skipped and the number may carry a sign
///   parsed with strtoull wraparound semantics
/// - the unit is at most two characters, matched case-insensitively
/// - a space between the number and the unit is allowed
/// - a unit equal to the base unit leaves the value unchanged
/// - the multiply wraps instead of overflowing
pub fn scan_units(input: &str, base: &str, units: &[&str], scale: u64) -> Option<u64> {
    // sscanf skips leading whitespace before the number.
    let rest = input.trim_start();
    let bytes = rest.as_bytes();
    let mut index = 0;

    let mut negative = false;
    if index < bytes.len() && (bytes[index] == b'+' || bytes[index] == b'-') {
        negative = bytes[index] == b'-';
        index += 1;
    }

    let digits_start = index;
    let mut value: u64 = 0;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        value = value
            .wrapping_mul(10)
            .wrapping_add(u64::from(bytes[index] - b'0'));
        index += 1;
    }
    if index == digits_start {
        return None;
    }

    // %2s skips whitespace then reads up to two non-whitespace
    // characters, anything after that is ignored.
    let remainder = rest[index..].trim_start();
    let unit: String = remainder
        .chars()
        .take_while(|character| !character.is_whitespace())
        .take(2)
        .collect();

    let mut multiplier = 1;
    if !unit.is_empty() {
        if unit.eq_ignore_ascii_case(base) {
            multiplier = 1;
        } else {
            let mut matched = false;
            for known in units {
                multiplier *= scale;
                if unit.eq_ignore_ascii_case(known) {
                    matched = true;
                    break;
                }
            }
            if !matched {
                return None;
            }
        }
    }

    let value = value.wrapping_mul(multiplier);
    Some(if negative {
        value.wrapping_neg()
    } else {
        value
    })
}

/// Scans a metric count, units are k, M, G, T, and P with a scale of
/// 1000.
pub fn scan_metric(input: &str) -> Option<u64> {
    scan_units(input, "", &["k", "M", "G", "T", "P"], 1000)
}

/// Scans a time in seconds, units are m and h with a scale of 60, the
/// base unit s is accepted as a no-op.
pub fn scan_time(input: &str) -> Option<u64> {
    scan_units(input, "s", &["m", "h"], 60)
}

#[cfg(test)]
mod tests {
    use super::{
        format_binary, format_metric, format_time_s, format_time_us, scan_metric, scan_time,
        scan_units,
    };

    #[test]
    fn formats_bytes_through_the_binary_ladder() {
        assert_eq!(format_binary(0.0), "0.00");
        assert_eq!(format_binary(849.0), "849.00");
        // 1024 * 0.85 is 870.4.
        assert_eq!(format_binary(870.0), "870.00");
        assert_eq!(format_binary(871.0), "0.85K");
        assert_eq!(format_binary(1024.0 * 1024.0), "1.00M");
        assert_eq!(format_binary(1024.0 * 1024.0 * 1024.0 * 5.0), "5.00G");
    }

    #[test]
    fn formats_rates_through_the_metric_ladder() {
        assert_eq!(format_metric(849.0), "849.00");
        assert_eq!(format_metric(850.0), "0.85k");
        assert_eq!(format_metric(1500.0), "1.50k");
        // The climb repeats while the amount clears the threshold,
        // so 850k is already 0.85M.
        assert_eq!(format_metric(849_999.0), "850.00k");
        assert_eq!(format_metric(850_000.0), "0.85M");
    }

    #[test]
    fn formats_seconds_with_precision_zero() {
        assert_eq!(format_time_s(10.0), "10s");
        assert_eq!(format_time_s(50.0), "50s");
    }

    #[test]
    fn climbs_the_seconds_ladder_at_51() {
        // 60 * 0.85 is the fixed threshold.
        assert_eq!(format_time_s(50.999), "51s");
        assert_eq!(format_time_s(51.0), "1m");
        // The h step is unreachable, its successor ends the climb.
        assert_eq!(format_time_s(3600.0 * 3.0), "180m");
    }

    #[test]
    fn formats_microseconds_below_the_second() {
        assert_eq!(format_time_us(0.0), "0.00us");
        assert_eq!(format_time_us(849.0), "849.00us");
        assert_eq!(format_time_us(850.0), "0.85ms");
        assert_eq!(format_time_us(1000.0), "1.00ms");
        assert_eq!(format_time_us(999_999.0), "1000.00ms");
    }

    #[test]
    fn switches_to_the_seconds_ladder_at_one_second() {
        assert_eq!(format_time_us(1_000_000.0), "1.00s");
        assert_eq!(format_time_us(50_999_999.0), "51.00s");
        // 51 seconds crosses the fixed 51 threshold into minutes.
        assert_eq!(format_time_us(51_000_000.0), "0.85m");
        assert_eq!(format_time_us(3_600_000_000.0), "60.00m");
    }

    #[test]
    fn scans_plain_numbers() {
        assert_eq!(scan_metric("0"), Some(0));
        assert_eq!(scan_metric("100"), Some(100));
        assert_eq!(scan_time("10"), Some(10));
    }

    #[test]
    fn scans_metric_units_case_insensitively() {
        assert_eq!(scan_metric("1k"), Some(1_000));
        assert_eq!(scan_metric("1K"), Some(1_000));
        assert_eq!(scan_metric("2m"), Some(2_000_000));
        assert_eq!(scan_metric("1P"), Some(1_000_000_000_000_000));
    }

    #[test]
    fn scans_time_units() {
        assert_eq!(scan_time("10s"), Some(10));
        assert_eq!(scan_time("90S"), Some(90));
        assert_eq!(scan_time("2m"), Some(120));
        assert_eq!(scan_time("1h"), Some(3_600));
        assert_eq!(scan_time("1H"), Some(3_600));
    }

    #[test]
    fn allows_space_between_number_and_unit() {
        assert_eq!(scan_metric("10 k"), Some(10_000));
        assert_eq!(scan_time("2 m"), Some(120));
    }

    #[test]
    fn skips_leading_whitespace() {
        assert_eq!(scan_metric(" 10"), Some(10));
    }

    #[test]
    fn rejects_unknown_units() {
        assert_eq!(scan_metric("10x"), None);
        assert_eq!(scan_metric("10kk"), None);
        assert_eq!(scan_metric("10xyz"), None);
        assert_eq!(scan_time("500ms"), None);
        assert_eq!(scan_metric("k"), None);
        assert_eq!(scan_metric(""), None);
    }

    #[test]
    fn wraps_like_strtoull() {
        let two_pow_64: u64 = 0;
        let _ = two_pow_64;
        // 2^64 wraps to 0
        assert_eq!(scan_metric("18446744073709551616"), Some(0));
        assert_eq!(scan_metric("-1"), Some(u64::MAX));
        assert_eq!(scan_time("-2s"), Some(u64::MAX - 1));
    }

    #[test]
    fn unit_equal_to_base_is_a_no_op() {
        assert_eq!(scan_units("5", "s", &["m", "h"], 60), Some(5));
        assert_eq!(scan_time("5s"), Some(5));
    }
}
