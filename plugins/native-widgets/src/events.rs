//! The typed native-widget event vocabulary plus the primitive `detail`
//! codec every interactive control's
//! Android listener packs into — the layer above [`crate::runtime`]'s raw
//! `(kind, detail)` transport.
//!
//! # Native-widget events bypass `RenderRoot::event` — again, in this module
//!
//! Every [`EventPayload`] decoded here is a **platform interaction
//! surfacing as a Rust callback**, delivered on the platform main thread
//! straight from `dev.frust.nativewidgets.FrustNativeListener` through
//! `crate::android`'s JNI export and
//! [`crate::runtime::NativeRuntime::on_event`] — never
//! through `frust-core`'s `EventCtx`. No pointer capture, no focus, none of
//! frust's fire-on-up-inside press semantics apply (see the crate doc's
//! *Native-widget events bypass `RenderRoot::event`*, and
//! `docs/CODE_STANDARDS.md`'s Interaction Semantics, which this vocabulary
//! is deliberately exempt from — a native control's interaction is entirely
//! platform-owned). The app-facing api wraps a decoded
//! [`EventPayload`] straight into a signal write, which is what wakes
//! exactly one frust frame.
//!
//! # One listener, seven kinds, no JSON
//!
//! Kinds 1-5 are emitted on Android; the sixth,
//! [`EVENT_KIND_SELECTION`], is appended for the Apple-arm-only segmented
//! control and is emitted only by the two Apple target classes today (its
//! Kotlin twin exists purely to keep the shared table whole). The seventh,
//! [`EVENT_KIND_DATE`], is appended after it for the date picker and is
//! emitted by all three arms.
//!
//! `FrustNativeListener` implements every listener interface
//! a v1 control needs — `View.OnClickListener`,
//! `CompoundButton.OnCheckedChangeListener`,
//! `SeekBar.OnSeekBarChangeListener`, `DatePicker.OnDateChangedListener` —
//! and funnels all of them into the one
//! native method `nativeOnEvent(slotId, kind, detail)`
//! (`crate::android::Java_dev_frust_nativewidgets_FrustNativeListener_nativeOnEvent`).
//! `detail` packs a primitive payload into a `jlong`; a value listener can
//! fire at drag rate, and allocating a JSON string per event on the main
//! thread is exactly what `docs/CODE_STANDARDS.md`'s no-JSON-on-the-hot-path
//! rule is about.
//!
//! **The `EVENT_KIND_*` constants below are LAW, shared verbatim with
//! `FrustNativeListener.kt`'s companion `KIND_*` constants — edit both
//! tables together.** This is mechanically enforced, not just a comment:
//! `plugins/native-widgets/tests/kotlin_conformance.rs` scans both files and
//! fails on any drift in the `KIND_*`/`EVENT_KIND_*` values, in
//! [`pack_value_changed`]/[`unpack_value_changed`]'s mask/shift contract, or
//! in [`pack_date`]'s field layout (Kotlin's `onDateChanged` packs the same
//! three fields by hand).
//!
//! # Slider values are platform-space until they leave this module
//!
//! `SeekBar` is always zero-based (`setMin` needs API 26; this plugin's
//! floor is 24 — `crate::controls::slider`'s module doc), so
//! `onProgressChanged` reports `progress - min` under the hood.
//! `crate::controls::slider::decode_event` (the only caller with a `min` to
//! add back) is what restores the app-space value the callback actually
//! sees; every other decode function in this module has no such conversion
//! because no other control maps its range.

use crate::controls::date_picker::CivilDate;
use crate::runtime::NativeEvent;

// --- the kind vocabulary -----------------------------------------------

/// `View.OnClickListener.onClick` — `detail` unused (`0`).
pub(crate) const EVENT_KIND_CLICK: i32 = 1;
/// `CompoundButton.OnCheckedChangeListener.onCheckedChanged` — `detail`
/// packs the checked state, see [`pack_bool`]/[`unpack_bool`].
pub(crate) const EVENT_KIND_TOGGLED: i32 = 2;
/// `SeekBar.OnSeekBarChangeListener.onProgressChanged` — `detail` packs the
/// platform-space progress plus `fromUser`, see
/// [`pack_value_changed`]/[`unpack_value_changed`].
pub(crate) const EVENT_KIND_VALUE_CHANGED: i32 = 3;
/// `SeekBar.OnSeekBarChangeListener.onStartTrackingTouch` — `detail` unused.
pub(crate) const EVENT_KIND_DRAG_START: i32 = 4;
/// `SeekBar.OnSeekBarChangeListener.onStopTrackingTouch` — `detail` unused.
pub(crate) const EVENT_KIND_DRAG_END: i32 = 5;
/// A segmented control's `ValueChanged` action (`UISegmentedControl` on iOS,
/// `NSSegmentedControl`'s action on macOS) — `detail` packs the reported
/// segment index, see [`pack_index`]/[`unpack_index`].
///
/// **Appended, Apple-arm-only in this build.** Kotlin's `KIND_SELECTION`
/// carries the same value so the two tables stay one table
/// (`tests/kotlin_conformance.rs`), but no Android listener ever emits it:
/// the segmented control has no Android arm yet (`crate::controls::segmented`'s
/// module doc).
pub(crate) const EVENT_KIND_SELECTION: i32 = 6;
/// A date picker's committed date change (`DatePicker.OnDateChangedListener`
/// on Android, `UIDatePicker`'s `ValueChanged` action on iOS,
/// `NSDatePicker`'s action on macOS) — `detail` packs the reported civil
/// date, see [`pack_date`]/[`unpack_date`].
///
/// **Appended** after [`EVENT_KIND_SELECTION`], never renumbered into the
/// table: kinds 1-6 are shipped wire values. Unlike `SELECTION`, all three
/// arms emit it (`crate::controls::date_picker` is a shared control).
pub(crate) const EVENT_KIND_DATE: i32 = 7;

// --- the typed vocabulary the app-facing api wraps into a signal -----------

/// One decoded native-widget event, past the raw `(kind, detail)` wire —
/// [`crate::runtime::NativeRuntime::on_event`] hands this to a slot's
/// registered `Arc<dyn Fn(EventPayload) + Send + Sync>` callback, invoked on
/// the platform main thread.
///
/// **Bypasses `RenderRoot::event` entirely** — see the module doc.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum EventPayload {
    /// `Button` was tapped.
    Click,
    /// `Switch` was toggled to this checked state.
    Toggled(bool),
    /// `Slider` moved to this **app-space** value
    /// (`crate::controls::slider`'s platform-space mapping already undone).
    ValueChanged {
        /// The new value, in the app's own `[min, max]`.
        value: i32,
        /// Whether a user touch caused this change, as `SeekBar` reports it
        /// — also `false` for a programmatic `setProgress` (the
        /// controlled-component write-back), but such a call's echo never
        /// reaches `crate::controls::slider::decode_event` in production at
        /// all: it is dropped one layer up, before
        /// `NativeWidget::on_event`/`decode_event` ever run, by
        /// `crate::runtime::with_runtime`'s re-entrancy guard
        /// (`crate::controls`'s module doc's *echo guard* — safety that is an
        /// incidental property of `update_params` being called inside
        /// `with_runtime`, not a designed invariant). The bit is still
        /// decoded here in case a future caller wants it.
        from_user: bool,
    },
    /// `Slider`'s drag gesture began (`onStartTrackingTouch`).
    DragStart,
    /// `Slider`'s drag gesture ended (`onStopTrackingTouch`).
    DragEnd,
    /// A segmented control reported this segment as the **requested**
    /// selection — controlled, like [`Self::Toggled`]: the app confirms it by
    /// feeding the index back as props (`crate::controls::segmented`).
    Selected(usize),
    /// A date picker reported this date as the **requested** one —
    /// controlled, like [`Self::Selected`]: the app confirms it by feeding
    /// the date back as props (`crate::controls::date_picker`). Always a
    /// valid civil date ([`unpack_date`] refuses anything else).
    Date(CivilDate),
}

// --- the detail codec -------------------------------------------------------

/// Pack a boolean into `detail` — [`EVENT_KIND_TOGGLED`]'s whole payload.
pub(crate) fn pack_bool(value: bool) -> i64 {
    value as i64
}

/// [`pack_bool`]'s inverse: any non-zero `detail` is `true`.
pub(crate) fn unpack_bool(detail: i64) -> bool {
    detail != 0
}

/// Pack a platform-space progress plus `fromUser` into `detail` —
/// [`EVENT_KIND_VALUE_CHANGED`]'s whole payload: the progress in the low 32
/// bits, the flag in bit 32. Progress is never negative
/// (`crate::controls::slider`'s `span`/`progress` are both clamped `>= 0`),
/// so the low 32 bits round-trip exactly for every value `SeekBar` can
/// report.
pub(crate) fn pack_value_changed(platform_value: i32, from_user: bool) -> i64 {
    (platform_value as i64 & 0xFFFF_FFFF) | ((from_user as i64) << 32)
}

/// [`pack_value_changed`]'s inverse.
pub(crate) fn unpack_value_changed(detail: i64) -> (i32, bool) {
    let platform_value = (detail & 0xFFFF_FFFF) as i32;
    let from_user = (detail >> 32) & 1 != 0;
    (platform_value, from_user)
}

/// Pack a platform-reported segment index into `detail` —
/// [`EVENT_KIND_SELECTION`]'s whole payload. **Layout: the whole 64-bit
/// `detail` is the platform's own signed index, sign-extended** — no mask, no
/// flag bits (unlike [`pack_value_changed`]). Taking the signed `NSInteger`
/// verbatim keeps the platforms' "no segment selected" sentinel
/// (`UISegmentedControlNoSegment`, `-1` on both Apple arms) representable, so
/// [`unpack_index`] can refuse it rather than a caller having to guess.
pub(crate) fn pack_index(platform_index: isize) -> i64 {
    platform_index as i64
}

/// [`pack_index`]'s inverse: the reported segment, or `None` for a negative
/// (no-selection) report — which carries no requested segment to deliver.
pub(crate) fn unpack_index(detail: i64) -> Option<usize> {
    usize::try_from(detail).ok()
}

/// Pack a civil date into `detail` — [`EVENT_KIND_DATE`]'s whole payload.
///
/// **Layout** (the low 32 bits of the `i64`; bits 32-63 are always zero):
///
/// | bits | field | range |
/// |---|---|---|
/// | 16-31 | `year` | 1-9999 (fits the 16-bit field with room to spare) |
/// | 8-15 | `month` | **1-based**, 1-12 (Android's `monthOfYear` is 0-based — the Kotlin listener adds 1 before packing) |
/// | 0-7 | `day` | 1-31 |
///
/// Unlike [`pack_value_changed`]'s `progress | fromUser << 32`, no flag bit
/// rides along: a date report carries no `fromUser`, because no arm can
/// tell (and the runtime's re-entrancy drop already swallows the only
/// programmatic echo — `crate::controls::date_picker`'s module doc).
/// `FrustNativeListener.onDateChanged` duplicates this arithmetic by hand;
/// `tests/kotlin_conformance.rs` pins the masks, the shifts and the month
/// offset against this body.
pub(crate) fn pack_date(date: CivilDate) -> i64 {
    ((i64::from(date.year) & 0xFFFF) << 16)
        | ((i64::from(date.month) & 0xFF) << 8)
        | (i64::from(date.day) & 0xFF)
}

/// [`pack_date`]'s inverse — validated: `None` for any `detail` that is not
/// exactly a packed, real calendar date (stray high bits, a zero or
/// out-of-range field, a 31st of a 30-day month, a 29 February outside a
/// leap year), so a corrupted report can never reach an app callback as a
/// date the calendar does not have.
pub(crate) fn unpack_date(detail: i64) -> Option<CivilDate> {
    if detail >> 32 != 0 {
        return None;
    }
    let year = ((detail >> 16) & 0xFFFF) as i32;
    let month = ((detail >> 8) & 0xFF) as u8;
    let day = (detail & 0xFF) as u8;
    CivilDate::new(year, month, day)
}

/// Decode an [`EVENT_KIND_CLICK`] firing — `Button`'s whole event surface,
/// no echo guard (a click is never caused by `update`'s own setters, unlike
/// `Switch`/`Slider` — see `crate::controls`'s controlled-component note).
///
/// `None` when `event.kind` is not the click kind — defensive, since a
/// `Button`'s listener is only ever attached as an `OnClickListener` and so
/// can only ever report this one kind.
pub(crate) fn decode_click(event: NativeEvent) -> Option<EventPayload> {
    (event.kind == EVENT_KIND_CLICK).then_some(EventPayload::Click)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bool_round_trips() {
        assert!(unpack_bool(pack_bool(true)));
        assert!(!unpack_bool(pack_bool(false)));
    }

    #[test]
    fn value_changed_round_trips_across_the_i32_range() {
        for (value, from_user) in [
            (0, false),
            (100, true),
            (i32::MAX, false),
            (i32::MAX, true),
            (0, true),
        ] {
            let (decoded_value, decoded_from_user) =
                unpack_value_changed(pack_value_changed(value, from_user));
            assert_eq!(decoded_value, value);
            assert_eq!(decoded_from_user, from_user);
        }
    }

    #[test]
    fn a_segment_index_round_trips_and_no_selection_decodes_to_none() {
        for index in [0usize, 1, 7, 63] {
            assert_eq!(
                unpack_index(pack_index(index as isize)),
                Some(index),
                "index {index}"
            );
        }
        // `UISegmentedControlNoSegment` / `NSSegmentedControl`'s `-1`.
        assert_eq!(unpack_index(pack_index(-1)), None);
        assert_eq!(unpack_index(i64::MIN), None);
    }

    #[test]
    fn a_date_round_trips_across_every_field_boundary() {
        // Year 1..9999, month 1..12, day 1..31 — each boundary with the
        // other two fields at both of theirs, where the calendar allows it.
        for (year, month, day) in [
            (1, 1, 1),
            (1, 12, 31),
            (9999, 1, 1),
            (9999, 12, 31),
            (2026, 9, 29),
            (2024, 2, 29),
            (2000, 2, 29),
            (1970, 1, 1),
            (255, 12, 31),
            (256, 1, 1),
        ] {
            let date = CivilDate::new(year, month, day)
                .unwrap_or_else(|| panic!("{year}-{month}-{day} is a real date"));
            let detail = pack_date(date);
            assert_eq!(detail >> 32, 0, "only the low 32 bits are used");
            assert_eq!(unpack_date(detail), Some(date), "{year}-{month}-{day}");
        }
    }

    #[test]
    fn the_date_layout_is_year_month_day_high_to_low() {
        let date = CivilDate::new(2026, 9, 29).unwrap();
        assert_eq!(pack_date(date), (2026 << 16) | (9 << 8) | 29);
        assert_eq!(
            pack_date(CivilDate::new(9999, 12, 31).unwrap()),
            0x270F_0C1F
        );
        assert_eq!(pack_date(CivilDate::new(1, 1, 1).unwrap()), 0x0001_0101);
    }

    #[test]
    fn an_invalid_packed_date_decodes_to_none() {
        let pack = |year: i64, month: i64, day: i64| (year << 16) | (month << 8) | day;
        for (what, detail) in [
            ("year 0", pack(0, 1, 1)),
            ("year 10000", pack(10_000, 1, 1)),
            ("month 0", pack(2026, 0, 1)),
            ("month 13", pack(2026, 13, 1)),
            ("day 0", pack(2026, 1, 0)),
            ("day 32", pack(2026, 1, 32)),
            ("31 April", pack(2026, 4, 31)),
            ("29 Feb, common year", pack(2026, 2, 29)),
            ("29 Feb, century non-leap", pack(1900, 2, 29)),
            ("a stray high bit", pack(2026, 1, 1) | (1 << 40)),
            ("negative", -1),
            ("zero", 0),
        ] {
            assert_eq!(unpack_date(detail), None, "{what}");
        }
    }

    #[test]
    fn the_kind_table_is_append_only() {
        // Kinds 1-5 are shipped wire values; SELECTION and then DATE are
        // appended after them, never renumbered into the middle.
        assert_eq!(
            [
                EVENT_KIND_CLICK,
                EVENT_KIND_TOGGLED,
                EVENT_KIND_VALUE_CHANGED,
                EVENT_KIND_DRAG_START,
                EVENT_KIND_DRAG_END,
                EVENT_KIND_SELECTION,
                EVENT_KIND_DATE,
            ],
            [1, 2, 3, 4, 5, 6, 7]
        );
    }

    #[test]
    fn click_decodes_only_for_the_click_kind() {
        assert_eq!(
            decode_click(NativeEvent {
                kind: EVENT_KIND_CLICK,
                detail: 0,
            }),
            Some(EventPayload::Click)
        );
        assert_eq!(
            decode_click(NativeEvent {
                kind: EVENT_KIND_TOGGLED,
                detail: 1,
            }),
            None,
            "a misrouted kind decodes to nothing rather than a false click"
        );
    }
}
