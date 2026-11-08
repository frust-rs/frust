//! The Android logger's level: `Info` unless the `debug.frust.log` system property names
//! another, in the `off`/`error`/`warn`/`info`/`debug`/`trace` vocabulary the desktop, iOS and
//! web shells take from `FRUST_LOG`.
//!
//! An app process inherits no environment from `adb`, so the property stands in for the env
//! var: `adb shell setprop debug.frust.log debug` raises the level without a rebuild (cleared on
//! reboot), in a profile or release build as much as a debug one. The default stays `Info` in
//! every build because `Debug` also surfaces wgpu's and naga's shader-compilation logging, tens
//! of thousands of lines in the first second on a phone. `log`'s static `max_level_*` caps still
//! bound whatever the property asks for.

use log::LevelFilter;

/// The system property that sets the level; `debug.*` properties need no root.
pub(crate) const LEVEL_PROPERTY: &str = "debug.frust.log";

/// The level when the property is unset or does not name one.
pub(crate) const DEFAULT_LEVEL: LevelFilter = LevelFilter::Info;

/// The level `raw` (a property value) names, `None` when it is not one: `LevelFilter`'s own
/// case-insensitive `FromStr`, after trimming.
pub(crate) fn parse_level(raw: &str) -> Option<LevelFilter> {
    raw.trim().parse().ok()
}

#[cfg(target_os = "android")]
pub(crate) use android::property_value;

#[cfg(target_os = "android")]
mod android {
    use std::ffi::{CStr, c_char, c_int};

    /// Android's `PROP_VALUE_MAX`: the longest property value, including the NUL.
    const PROP_VALUE_MAX: usize = 92;

    unsafe extern "C" {
        /// bionic's `__system_property_get(name, value)`: copies the NUL-terminated value (at
        /// most `PROP_VALUE_MAX` bytes) into `value`, returns its length (0 when unset).
        fn __system_property_get(name: *const c_char, value: *mut c_char) -> c_int;
    }

    /// The raw value of [`super::LEVEL_PROPERTY`], `None` when unset or empty.
    pub(crate) fn property_value() -> Option<String> {
        let name = CStr::from_bytes_with_nul(b"debug.frust.log\0").expect("static NUL-terminated");
        let mut buf = [0 as c_char; PROP_VALUE_MAX];
        // SAFETY: `name` is a valid NUL-terminated string for the call's duration, and `buf` is
        // writable for `PROP_VALUE_MAX` bytes, the most `__system_property_get` ever writes
        // (including the NUL).
        let len = unsafe { __system_property_get(name.as_ptr(), buf.as_mut_ptr()) };
        if len <= 0 {
            return None;
        }
        // SAFETY: on a positive return the callee wrote a NUL-terminated string into `buf`.
        let value = unsafe { CStr::from_ptr(buf.as_ptr()) };
        let value = value.to_string_lossy().into_owned();
        (!value.trim().is_empty()).then_some(value)
    }
}

#[cfg(test)]
mod tests {
    use log::LevelFilter;

    use super::{DEFAULT_LEVEL, LEVEL_PROPERTY, parse_level};

    #[test]
    fn every_level_word_parses_trimmed_and_case_insensitively() {
        for (value, level) in [
            ("off", LevelFilter::Off),
            ("error", LevelFilter::Error),
            ("warn", LevelFilter::Warn),
            ("info", LevelFilter::Info),
            ("debug", LevelFilter::Debug),
            ("trace", LevelFilter::Trace),
            (" Debug ", LevelFilter::Debug),
            ("TRACE", LevelFilter::Trace),
        ] {
            assert_eq!(parse_level(value), Some(level), "{value:?}");
        }
    }

    #[test]
    fn a_word_that_is_not_a_level_parses_to_none() {
        for value in ["loud", "verbose", "warning", "none", "", "  "] {
            assert_eq!(parse_level(value), None, "{value:?}");
        }
    }

    #[test]
    fn the_default_is_info_and_the_property_needs_no_root() {
        assert_eq!(DEFAULT_LEVEL, LevelFilter::Info);
        assert!(
            LEVEL_PROPERTY.starts_with("debug."),
            "settable without root"
        );
    }
}
