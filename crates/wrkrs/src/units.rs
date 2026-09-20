//! Unit scanning for command line arguments.
//!
//! Ports the scan side of units.c. The format side (format_time_us and
//! friends) lands with the legacy reporter.

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
    use super::{scan_metric, scan_time, scan_units};

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
