//! `/proc/meminfo` parser. Compiled on every target so it is tested on every host; only the
//! Linux backend (and, later, nothing else) reads the file itself.

/// The two figures read from `/proc/meminfo`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Meminfo {
    pub(crate) total_bytes: u64,
    /// `None` on Linux before 3.14, which has no `MemAvailable`.
    pub(crate) available_bytes: Option<u64>,
}

/// Reads `MemTotal:` and `MemAvailable:` (`<n> kB`, kB = 1024). `None` when `MemTotal` is
/// missing or either line is malformed.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
pub(crate) fn parse(text: &str) -> Option<Meminfo> {
    let mut total = None;
    let mut available = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total = Some(parse_kb(rest)?);
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            available = Some(parse_kb(rest)?);
        }
    }
    Some(Meminfo {
        total_bytes: total?,
        available_bytes: available,
    })
}

fn parse_kb(rest: &str) -> Option<u64> {
    let mut parts = rest.split_whitespace();
    let value: u64 = parts.next()?.parse().ok()?;
    if parts.next()? != "kB" || parts.next().is_some() {
        return None;
    }
    value.checked_mul(1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "MemTotal:       16318412 kB\n\
        MemFree:         1203140 kB\n\
        MemAvailable:    9876544 kB\n\
        Buffers:          402116 kB\n\
        SwapTotal:       2097148 kB\n";

    #[test]
    fn parses_a_real_shaped_sample() {
        assert_eq!(
            parse(SAMPLE),
            Some(Meminfo {
                total_bytes: 16_318_412 * 1024,
                available_bytes: Some(9_876_544 * 1024),
            })
        );
    }

    #[test]
    fn missing_available_is_none() {
        let text = "MemTotal:        2048 kB\nMemFree: 100 kB\n";
        assert_eq!(
            parse(text),
            Some(Meminfo {
                total_bytes: 2048 * 1024,
                available_bytes: None,
            })
        );
    }

    #[test]
    fn malformed_unit_is_none() {
        assert_eq!(parse("MemTotal:  2048 MB\n"), None);
        assert_eq!(parse("MemTotal:  2048\n"), None);
        assert_eq!(parse("MemTotal:  abc kB\n"), None);
    }

    #[test]
    fn overflow_is_none() {
        assert_eq!(parse("MemTotal: 18446744073709551615 kB\n"), None);
    }

    #[test]
    fn empty_text_is_none() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("MemFree: 1 kB\n"), None);
    }
}
