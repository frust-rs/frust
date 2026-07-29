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
//! # One listener, five kinds, no JSON
//!
//! `FrustNativeListener` implements every listener interface
//! a v1 control needs — `View.OnClickListener`,
//! `CompoundButton.OnCheckedChangeListener`,
//! `SeekBar.OnSeekBarChangeListener` — and funnels all of them into the one
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
//! fails on any drift in the `KIND_*`/`EVENT_KIND_*` values or in
//! [`pack_value_changed`]/[`unpack_value_changed`]'s mask/shift contract.
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

// --- the typed vocabulary the app-facing api (p1-06) wraps into a signal ---

/// One decoded native-widget event, past the raw `(kind, detail)` wire —
/// [`crate::runtime::NativeRuntime::on_event`] hands this to a slot's
/// registered `Arc<dyn Fn(EventPayload) + Send + Sync>` callback (PLAN
/// 1.3), invoked on the platform main thread.
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
