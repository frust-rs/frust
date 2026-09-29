//! `DatePicker` — a real DATE-mode picker (no time), built and driven from
//! Rust: `android.widget.DatePicker` on Android, `UIDatePicker` (mode
//! `Date`) on iOS/iPadOS, `NSDatePicker` (elements `YearMonthDay`) on macOS.
//! A **shared** control: it sits in [`super::SHARED_KINDS`] and all three
//! arms register it.
//!
//! Eight properties: the date, its optional `[min, max]` range, the
//! presentation style, enabled, a tint, a text colour, and the accessibility
//! label. [`Setter::Date`], [`Setter::Enabled`], [`Setter::DatePickerTint`],
//! [`Setter::DatePickerTextColor`] and [`Setter::ContentDescription`] are
//! [`Tier::Cheap`](super::Tier::Cheap); [`Setter::MinDate`],
//! [`Setter::MaxDate`] and [`Setter::DatePickerStyle`] are
//! [`Tier::Relayout`](super::Tier::Relayout) (a range change repopulates a
//! calendar's pages/year list; a style swap rebuilds the whole presentation).
//!
//! # `CivilDate`: a calendar date, validated at every boundary
//!
//! [`CivilDate`] is a plain proleptic-Gregorian `{year, month, day}` with no
//! time and no time zone — the value the app owns and the value every arm
//! reports. It is validated wherever it crosses a boundary: the params wire
//! ([`DatePickerProps::decode`] — an invalid date decodes as absent), the
//! event wire (`crate::events::unpack_date`), and the Apple arms'
//! `NSDate` conversion ([`foundation`]). Each date rides the params wire as
//! ONE integer — the exact `crate::events::pack_date` value the event wire
//! carries — so there is one codec for both directions, not two.
//!
//! # The controlled-component contract — `Switch`'s, date-valued
//!
//! Controlled exactly like `Switch`/`Slider` (`switch.rs`'s module doc is the
//! reference account): the platform reports the *requested* date through
//! [`crate::events::EVENT_KIND_DATE`], the app confirms it by feeding it back
//! as `date`.
//!
//! 1. **Write-back.** [`DatePickerProps::plan`] takes the date the platform
//!    last reported (`observed`) and re-plans [`Setter::Date`] when it
//!    disagrees with the app's, even though the props' own `date` did not
//!    change — a rejected pick snaps back on the next differing params (the
//!    same v1 limitation `switch.rs` documents applies).
//! 2. **Echo guard: the runtime's re-entrancy drop, and nothing else.** On
//!    Android `DatePicker.updateDate` (the write-back) and `setMinDate`/
//!    `setMaxDate` (when they clamp the current date) notify
//!    `OnDateChangedListener` **synchronously**, from inside `update` — which
//!    runs inside `crate::runtime::with_runtime`, so that echo re-enters the
//!    held runtime borrow and is dropped there before [`decode_event`] or any
//!    app callback sees it (`crate::controls`' module doc's *echo guard*).
//!    Setting the platform date from inside the change handler therefore
//!    never re-fires the handler. UIKit and AppKit send no action for a
//!    programmatic `setDate:`/`setDateValue:` at all. No per-instance
//!    suppression flag, on any arm.
//!
//! # Range: clamped on decode, never inverted on the platform
//!
//! A `date` outside `[min, max]` is clamped into it on decode (the platforms
//! clamp their own display anyway; clamping first keeps the write-back from
//! fighting them), and an inverted range (`min > max`) degrades to the
//! single day `min` rather than reaching a platform — Android's calendar
//! mode sizes its year list from `max - min`. The plan orders the two range
//! setters so the platform never passes through an inverted range either:
//! the bound that *widens* the range is written first
//! ([`DatePickerProps::plan`]). A range change also re-asserts
//! [`Setter::Date`], because narrowing a range can move the platform's date.
//!
//! # `style`: live on Apple, baked at construction on Android
//!
//! | [`DatePickerStyle`] | iOS (`preferredDatePickerStyle`) | macOS (`datePickerStyle`) | Android |
//! |---|---|---|---|
//! | `Compact` (default) | `.compact` | `textFieldAndStepper` + `presentsCalendarOverlay` | spinner mode |
//! | `Wheels` | `.wheels` | `textFieldAndStepper` (AppKit has no wheels) | spinner mode |
//! | `Inline` | `.inline` (iOS 14+, inside the 15.0 floor) | `clockAndCalendar` | calendar mode |
//!
//! Android's `datePickerMode` is a construction-time style attribute with no
//! setter, so that arm's `create` reads `props.style` directly to pick the
//! constructor style (the exact shape `spinner.rs`'s *`size_class`* section
//! describes) and a later style change is warned-and-ignored there
//! (`platform::warn_style_unsupported`).
//!
//! # Theme: `accent_ink` tint, `body_text` text colour
//!
//! `crate::api::theme` folds `accent_ink` into [`super::TINT`] (applied as
//! `UIDatePicker.tintColor`; `NSDatePicker` has no tint property — the
//! calendar's selection draws in the system accent colour — logged and
//! no-op'd) and `body_text` into [`super::TEXT_COLOR`] (applied as
//! `NSDatePicker.textColor`; UIKit exposes no public text colour on
//! `UIDatePicker`, logged and no-op'd). **Android's `DatePicker` has no tint
//! or text-colour API at all** — its colours come only from the theme it
//! was constructed against, so both fold to a logged no-op there and the
//! night-qualified construction `Context` (theme ladder L1) is the whole of
//! its theming. L1 rides the raw `dark` wire like every other control; iOS
//! re-pins `overrideUserInterfaceStyle` on every update (`crate::apple`'s
//! theme module), macOS its `NSAppearance`.

use std::fmt;

use super::{
    CONTENT_DESCRIPTION, ENABLED, Plan, Setter, TEXT_COLOR, TINT, color, owned_text, slot_of,
};
use crate::NativeWidgetError;
use crate::events::{EVENT_KIND_DATE, EventPayload, pack_date, unpack_date};
use crate::registry::SlotId;
use crate::runtime::{NativeEvent, Params};

/// The registered kind string the api layer injects as `__frustControl`.
pub(crate) const KIND: &str = "date_picker";

/// `"date"` — the app-owned date (controlled), packed with
/// `crate::events::pack_date`.
pub(crate) const DATE: &str = "date";
/// `"minDate"` — the earliest selectable date, packed like [`DATE`]; absent
/// for the platform's own floor.
pub(crate) const MIN_DATE: &str = "minDate";
/// `"maxDate"` — the latest selectable date, packed like [`DATE`]; absent
/// for the platform's own ceiling.
pub(crate) const MAX_DATE: &str = "maxDate";
/// `"style"` — [`DatePickerStyle`]'s wire spelling.
pub(crate) const STYLE: &str = "style";

/// A calendar date — year, month, day — with no time of day and no time
/// zone: what a native date picker shows and what its change handler
/// reports.
///
/// Proleptic Gregorian, `year` 1–9999, `month` **1-based** (1 = January),
/// `day` 1–31 and real for its month (no 31 April, 29 February only in a
/// leap year). Build one with [`CivilDate::new`], which refuses anything
/// else; the fields are public for reading and pattern matching, and a value
/// assembled by hand that is not a real date is treated as absent wherever
/// this crate decodes one (module doc).
///
/// Ordered chronologically (`year`, then `month`, then `day`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CivilDate {
    /// The year, 1–9999.
    pub year: i32,
    /// The month, 1–12.
    pub month: u8,
    /// The day of the month, 1–31 (and no more than the month has).
    pub day: u8,
}

impl CivilDate {
    /// The earliest date this crate represents: 1 January of year 1.
    pub const MIN: Self = Self {
        year: 1,
        month: 1,
        day: 1,
    };

    /// The latest date this crate represents: 31 December 9999.
    pub const MAX: Self = Self {
        year: 9999,
        month: 12,
        day: 31,
    };

    /// `year`-`month`-`day`, or `None` when that is not a real calendar date
    /// in the supported range (see the type doc).
    pub fn new(year: i32, month: u8, day: u8) -> Option<Self> {
        let date = Self { year, month, day };
        date.is_valid().then_some(date)
    }

    /// Whether this is a real calendar date in the supported range.
    pub fn is_valid(self) -> bool {
        (Self::MIN.year..=Self::MAX.year).contains(&self.year)
            && (1..=12).contains(&self.month)
            && self.day >= 1
            && self.day <= days_in_month(self.year, self.month)
    }
}

impl fmt::Display for CivilDate {
    /// ISO 8601 calendar-date form, `YYYY-MM-DD`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// Whether `year` is a proleptic-Gregorian leap year.
fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// How many days `month` (1-based) of `year` has; `0` for a month outside
/// 1–12, so [`CivilDate::is_valid`] refuses every day of it.
fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// How the picker presents itself — see the module doc's *`style`* table
/// for each arm's mapping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum DatePickerStyle {
    /// The smallest footprint the platform offers.
    #[default]
    Compact,
    /// Spinning wheels.
    Wheels,
    /// A full, always-visible calendar.
    Inline,
}

impl DatePickerStyle {
    /// The wire spelling the api layer writes into [`STYLE`].
    fn from_wire(raw: &str) -> Option<Self> {
        match raw {
            "compact" => Some(Self::Compact),
            "wheels" => Some(Self::Wheels),
            "inline" => Some(Self::Inline),
            _ => None,
        }
    }
}

/// The marker type registered under [`KIND`] — by all three arms.
pub(crate) struct DatePicker;

/// Everything a `DatePicker` slot can be told, as one Rust-diffed value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DatePickerProps {
    /// The differ's slot id — **not a property**; `create` needs it for the
    /// listener/target it attaches.
    pub(crate) slot: SlotId,
    /// The app-owned date (controlled), clamped into `[min, max]` on decode;
    /// `None` leaves the platform on its own initial date (today).
    pub(crate) date: Option<CivilDate>,
    /// The earliest selectable date, `None` for the platform's own floor.
    pub(crate) min: Option<CivilDate>,
    /// The latest selectable date, `None` for the platform's own ceiling;
    /// never before `min` (normalized on decode).
    pub(crate) max: Option<CivilDate>,
    /// The presentation — module doc's *`style`* table.
    pub(crate) style: DatePickerStyle,
    /// `View.setEnabled` / `UIControl.enabled` / `NSControl.enabled`.
    pub(crate) enabled: bool,
    /// Packed ARGB tint, or `None` for the platform's own.
    pub(crate) tint: Option<i32>,
    /// Packed ARGB text colour, or `None` for the platform's own.
    pub(crate) text_color: Option<i32>,
    /// The TalkBack/VoiceOver label.
    pub(crate) content_description: Option<String>,
}

impl DatePickerProps {
    /// The state every arm's `create` diffs its first plan against: the
    /// platform's own date and range, [`DatePickerStyle::Compact`], enabled,
    /// no tint/text colour/label. Each arm reaches this state at construction
    /// (its own `create` normalizes the style on Apple and bakes it on
    /// Android — module doc).
    pub(crate) fn platform_default(slot: SlotId) -> Self {
        Self {
            slot,
            date: None,
            min: None,
            max: None,
            style: DatePickerStyle::Compact,
            enabled: true,
            tint: None,
            text_color: None,
            content_description: None,
        }
    }

    /// Decode a `DatePicker` slot's params (module doc's *Range* for the
    /// normalization).
    ///
    /// # Errors
    /// [`NativeWidgetError::Params`] when the reserved identity keys are
    /// missing.
    pub(crate) fn decode(params: &Params<'_>) -> Result<Self, NativeWidgetError> {
        let min = date_field(params, MIN_DATE);
        let max = match (min, date_field(params, MAX_DATE)) {
            (Some(min), Some(max)) if max < min => Some(min),
            (_, max) => max,
        };
        let date = date_field(params, DATE).map(|date| clamp(date, min, max));
        Ok(Self {
            slot: slot_of(params)?,
            date,
            min,
            max,
            style: params
                .string(STYLE)
                .and_then(|raw| DatePickerStyle::from_wire(&raw))
                .unwrap_or_default(),
            enabled: params.flag(ENABLED).unwrap_or(true),
            tint: color(params, TINT),
            text_color: color(params, TEXT_COLOR),
            content_description: owned_text(params, CONTENT_DESCRIPTION),
        })
    }

    /// The setter-call plan for `old` → `new`, given the date the platform
    /// last reported (`observed`, `None` until the user has picked one).
    ///
    /// Order is load-bearing: the style first; then the two range bounds,
    /// the *widening* one first so the platform never holds an inverted
    /// range (module doc's *Range*); then the date — re-planned when the
    /// app's date changed, when the platform drifted from it (the
    /// write-back), or when either bound changed (a narrowed range can have
    /// moved the platform's date).
    pub(crate) fn plan<'a>(old: &Self, new: &'a Self, observed: Option<CivilDate>) -> Plan<'a> {
        let mut plan = Plan::new();
        if old.style != new.style {
            plan.push(Setter::DatePickerStyle(new.style));
        }
        let min_changed = old.min != new.min;
        let max_changed = old.max != new.max;
        let min_setter = min_changed.then_some(Setter::MinDate(new.min));
        let max_setter = max_changed.then_some(Setter::MaxDate(new.max));
        // An absent ceiling is the latest representable date: growing it (or
        // keeping it) widens the range upward, so write it before the floor;
        // otherwise the floor moves down first.
        let ceiling = |max: Option<CivilDate>| max.unwrap_or(CivilDate::MAX);
        if ceiling(new.max) >= ceiling(old.max) {
            plan.extend(max_setter);
            plan.extend(min_setter);
        } else {
            plan.extend(min_setter);
            plan.extend(max_setter);
        }
        if let Some(date) = new.date {
            let drifted = observed.is_some_and(|platform| platform != date);
            if old.date != new.date || drifted || min_changed || max_changed {
                plan.push(Setter::Date(date));
            }
        }
        if old.enabled != new.enabled {
            plan.push(Setter::Enabled(new.enabled));
        }
        if old.tint != new.tint {
            plan.push(Setter::DatePickerTint(new.tint));
        }
        if old.text_color != new.text_color {
            plan.push(Setter::DatePickerTextColor(new.text_color));
        }
        if old.content_description != new.content_description {
            plan.push(Setter::ContentDescription(
                new.content_description.as_deref(),
            ));
        }
        plan
    }
}

/// A packed date field (`crate::events::pack_date`'s layout), `None` when
/// absent or not a real date — degrade, never a dead slot
/// (`crate::controls`' rule).
fn date_field(params: &Params<'_>, key: &str) -> Option<CivilDate> {
    params.int(key).and_then(unpack_date)
}

/// `date` clamped into the (already non-inverted) `[min, max]`.
fn clamp(date: CivilDate, min: Option<CivilDate>, max: Option<CivilDate>) -> CivilDate {
    let floored = min.map_or(date, |min| date.max(min));
    max.map_or(floored, |max| floored.min(max))
}

/// The wire value of `date` for a params body — the same packed integer the
/// event wire carries (module doc).
pub(crate) fn wire(date: CivilDate) -> i64 {
    pack_date(date)
}

/// Decode an [`EVENT_KIND_DATE`] firing into the typed vocabulary: records
/// the write-back drift signal (`observed`) and decodes to an
/// [`EventPayload::Date`].
///
/// `None` when `event.kind` is not the date kind (defensive — the listener/
/// target is only ever attached for this one kind), or when `detail` is not
/// a real date (`crate::events::unpack_date`), in which case `observed` is
/// left untouched.
///
/// Pure and host-testable, and the ONE decoder all three arms' `on_event`
/// call — kind/detail parity by construction.
pub(crate) fn decode_event(
    observed: &mut Option<CivilDate>,
    event: NativeEvent,
) -> Option<EventPayload> {
    if event.kind != EVENT_KIND_DATE {
        return None;
    }
    let date = unpack_date(event.detail)?;
    *observed = Some(date);
    Some(EventPayload::Date(date))
}

/// `NSDate` ↔ [`CivilDate`] for the two Apple arms — both the controls'
/// setters and the two target classes (`crate::apple::events`,
/// `crate::appkit::events`) convert through here, so the arms cannot map a
/// date differently.
///
/// Conversion runs in a **Gregorian** `NSCalendar` in the current time zone,
/// deliberately not `NSCalendar.currentCalendar`: [`CivilDate`] is a
/// Gregorian date on every arm (Android's `DatePicker` reports Gregorian
/// fields whatever the user's locale), and a user whose system calendar is,
/// say, Buddhist or Japanese would otherwise report a different year for the
/// same absolute day. The picker itself keeps displaying the user's own
/// calendar — only the Rust-side mapping is pinned. A date becomes an
/// `NSDate` at local **noon**, so no DST transition at midnight can push it
/// onto a neighbouring day.
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub(crate) mod foundation {
    use objc2::rc::Retained;
    use objc2_foundation::{
        NSCalendar, NSCalendarIdentifierGregorian, NSCalendarUnit, NSDate, NSDateComponents,
    };

    use super::CivilDate;

    /// A Gregorian calendar in the current time zone (module doc).
    fn gregorian() -> Option<Retained<NSCalendar>> {
        // SAFETY: `NSCalendarIdentifierGregorian` is an immutable
        // Foundation-exported `NSString` constant, initialized before any
        // Rust code runs; reading the extern static is sound.
        let identifier = unsafe { NSCalendarIdentifierGregorian };
        NSCalendar::calendarWithIdentifier(identifier)
    }

    /// `date` at local noon, or `None` if Foundation cannot build it (it
    /// can for every valid [`CivilDate`]; the `Option` is Foundation's).
    pub(crate) fn ns_date(date: CivilDate) -> Option<Retained<NSDate>> {
        let calendar = gregorian()?;
        let components = NSDateComponents::new();
        components.setYear(date.year as isize);
        components.setMonth(isize::from(date.month));
        components.setDay(isize::from(date.day));
        components.setHour(12);
        calendar.dateFromComponents(&components)
    }

    /// The civil date `date` falls on in the current time zone, or `None`
    /// when it is outside [`CivilDate`]'s range (before year 1 — a BC era —
    /// or after 9999).
    pub(crate) fn civil_date(date: &NSDate) -> Option<CivilDate> {
        let calendar = gregorian()?;
        // Era 1 is AD in the Gregorian calendar; era 0 years count backwards.
        if calendar.component_fromDate(NSCalendarUnit::Era, date) != 1 {
            return None;
        }
        let year = calendar.component_fromDate(NSCalendarUnit::Year, date);
        let month = calendar.component_fromDate(NSCalendarUnit::Month, date);
        let day = calendar.component_fromDate(NSCalendarUnit::Day, date);
        CivilDate::new(
            i32::try_from(year).ok()?,
            u8::try_from(month).ok()?,
            u8::try_from(day).ok()?,
        )
    }

    /// `date` as an `NSDate`, `None` staying `None` — the nullable-range
    /// setters' argument (`nil` restores the platform's own bound).
    pub(crate) fn optional_ns_date(date: Option<CivilDate>) -> Option<Retained<NSDate>> {
        date.and_then(ns_date)
    }
}

#[cfg(target_os = "android")]
pub(crate) mod platform {
    //! The Android half: build the `DatePicker` in the mode `props.style`
    //! asks for, apply the shared plan, and attach the ONE listener through
    //! `DatePicker.init(year, month, day, listener)`.
    //!
    //! # Construction: the mode is a style resource, not a setter
    //!
    //! `android:datePickerMode` is read once, in the constructor, from the
    //! style it is handed — there is no setter and no `ContextThemeWrapper`
    //! is involved. So `create` calls the four-argument constructor
    //! `DatePicker(context, null, 0, <style>)` with a framework style
    //! resolved from `android.R.style` at runtime (never a hard-coded id):
    //!
    //! - `Inline` → `Widget_Material_DatePicker`, whose `datePickerMode` is
    //!   `calendar`;
    //! - `Compact`/`Wheels` → `Widget_DatePicker`, which sets no
    //!   `datePickerMode`, so the constructor's own default — spinner mode —
    //!   applies; its legacy `calendarViewShown` is then switched off and the
    //!   spinners on explicitly (`setCalendarViewShown`/`setSpinnersShown`,
    //!   both meaningful in spinner mode only).
    //!
    //! Both styles resolve their colours against the night-qualified
    //! construction `Context` (theme ladder L1), exactly as the one-argument
    //! constructor would. Should the style field ever be missing from a
    //! device's framework, `create` falls back to that one-argument
    //! constructor (the theme's own default mode) with one warning, rather
    //! than a dead slot.
    //!
    //! # Months are 0-based on this side of the wire only
    //!
    //! `DatePicker.init`/`updateDate`/`getMonth` and `java.util.Calendar`
    //! count months from 0; [`CivilDate`] and the event wire count from 1.
    //! Every conversion is in this module (and in the Kotlin listener's
    //! `onDateChanged`, for events) — nothing else sees a 0-based month.
    //!
    //! # Range bounds are local-midnight millis
    //!
    //! `setMinDate`/`setMaxDate` take epoch milliseconds, which `DatePicker`
    //! turns back into a day in the device's default time zone; the bound is
    //! therefore built through `java.util.Calendar.getInstance()` (the same
    //! default zone) at local midnight, not computed from UTC in Rust. An
    //! absent bound restores `DatePicker`'s own documented default (1900-01-01
    //! / 2100-12-31).

    use std::sync::Once;

    use jni::objects::{JObject, JValue};
    use jni::refs::Global;
    use jni::{jni_sig, jni_str};

    use super::{CivilDate, DatePicker, DatePickerProps, DatePickerStyle, decode_event};
    use crate::NativeWidgetError;
    use crate::android::{NativeCtx, NativeView};
    use crate::controls::platform::{FRAME_CAPACITY, apply as apply_shared};
    use crate::controls::{Plan, Setter};
    use crate::events::EventPayload;
    use crate::runtime::{NativeEvent, NativeWidget, Params};

    /// `android.widget.DatePicker` — the framework class.
    const CLASS: &str = "android.widget.DatePicker";
    /// `android.R$style` — the framework's public style ids, read at runtime
    /// (module doc's *Construction*).
    const R_STYLE_CLASS: &str = "android.R$style";
    /// `java.util.Calendar` — the range bounds' millis (module doc).
    const CALENDAR_CLASS: &str = "java.util.Calendar";

    /// `DatePicker`'s own default floor when no `min` is set.
    const PLATFORM_MIN: CivilDate = CivilDate {
        year: 1900,
        month: 1,
        day: 1,
    };
    /// `DatePicker`'s own default ceiling when no `max` is set.
    const PLATFORM_MAX: CivilDate = CivilDate {
        year: 2100,
        month: 12,
        day: 31,
    };

    /// Build the picker for `style` — module doc's *Construction*.
    fn new_date_picker<'local>(
        ctx: &mut NativeCtx<'local, '_>,
        style: DatePickerStyle,
    ) -> Result<JObject<'local>, NativeWidgetError> {
        let (field, spinner) = match style {
            DatePickerStyle::Inline => (jni_str!("Widget_Material_DatePicker"), false),
            DatePickerStyle::Compact | DatePickerStyle::Wheels => {
                (jni_str!("Widget_DatePicker"), true)
            }
        };
        let styles = ctx.class(R_STYLE_CLASS)?;
        let style_res = match ctx.run_jni(&format!("android.R.style.{field}"), |env| {
            env.get_static_field(&styles, field, jni_sig!("I"))?.i()
        }) {
            Ok(style_res) => style_res,
            Err(error) => {
                log::warn!(
                    "frust-native-widgets: {error} — building the DatePicker with the theme's own \
                     default mode instead"
                );
                return ctx.new_view(CLASS);
            }
        };
        let class = ctx.class(CLASS)?;
        let context = ctx.context()?;
        let view = ctx.run_jni("new DatePicker(Context, AttributeSet, int, int)", |env| {
            env.new_object(
                &class,
                jni_sig!("(Landroid/content/Context;Landroid/util/AttributeSet;II)V"),
                &[
                    JValue::Object(context),
                    JValue::Object(&JObject::null()),
                    JValue::Int(0),
                    JValue::Int(style_res),
                ],
            )
        })?;
        if spinner {
            ctx.call_void(
                &view,
                jni_str!("setCalendarViewShown"),
                jni_sig!("(Z)V"),
                &[JValue::Bool(false)],
            )?;
            ctx.call_void(
                &view,
                jni_str!("setSpinnersShown"),
                jni_sig!("(Z)V"),
                &[JValue::Bool(true)],
            )?;
        }
        Ok(view)
    }

    /// The picker's current date, read back through `getYear`/`getMonth`/
    /// `getDayOfMonth` — `init`'s argument when the app supplied no date.
    fn current_date(
        ctx: &mut NativeCtx<'_, '_>,
        view: &JObject<'_>,
    ) -> Result<CivilDate, NativeWidgetError> {
        let (year, month0, day) = ctx.run_jni("DatePicker.get{Year,Month,DayOfMonth}", |env| {
            let year = env
                .call_method(view, jni_str!("getYear"), jni_sig!("()I"), &[])?
                .i()?;
            let month0 = env
                .call_method(view, jni_str!("getMonth"), jni_sig!("()I"), &[])?
                .i()?;
            let day = env
                .call_method(view, jni_str!("getDayOfMonth"), jni_sig!("()I"), &[])?
                .i()?;
            Ok((year, month0, day))
        })?;
        u8::try_from(month0 + 1)
            .ok()
            .zip(u8::try_from(day).ok())
            .and_then(|(month, day)| CivilDate::new(year, month, day))
            .ok_or_else(|| {
                NativeWidgetError::Platform(format!(
                    "android native-widgets: DatePicker reported {year}-{month0}(0-based)-{day}, \
                     not a supported date"
                ))
            })
    }

    /// `date` at local midnight as epoch millis, through
    /// `java.util.Calendar` (module doc's *Range bounds*).
    fn epoch_millis(
        ctx: &mut NativeCtx<'_, '_>,
        date: CivilDate,
    ) -> Result<i64, NativeWidgetError> {
        let class = ctx.class(CALENDAR_CLASS)?;
        ctx.run_jni("Calendar.getTimeInMillis", |env| {
            let calendar = env
                .call_static_method(
                    &class,
                    jni_str!("getInstance"),
                    jni_sig!("()Ljava/util/Calendar;"),
                    &[],
                )?
                .l()?;
            env.call_method(&calendar, jni_str!("clear"), jni_sig!("()V"), &[])?
                .v()?;
            env.call_method(
                &calendar,
                jni_str!("set"),
                jni_sig!("(III)V"),
                &[
                    JValue::Int(date.year),
                    JValue::Int(i32::from(date.month) - 1),
                    JValue::Int(i32::from(date.day)),
                ],
            )?
            .v()?;
            env.call_method(&calendar, jni_str!("getTimeInMillis"), jni_sig!("()J"), &[])?
                .j()
        })
    }

    /// A live picker's retained state.
    pub(crate) struct DatePickerState {
        /// The picker's own global reference (the second one — see
        /// `button.rs`'s note).
        view: Global<JObject<'static>>,
        /// The date the platform last reported, `None` while untouched —
        /// [`DatePickerProps::plan`]'s write-back drift signal. Written from
        /// [`NativeWidget::on_event`] via [`decode_event`].
        observed: Option<CivilDate>,
    }

    impl NativeWidget for DatePicker {
        type Props = DatePickerProps;
        type State = DatePickerState;

        fn decode_props(params: &Params<'_>) -> Result<Self::Props, NativeWidgetError> {
            DatePickerProps::decode(params)
        }

        fn create(
            ctx: &mut NativeCtx<'_, '_>,
            props: &Self::Props,
        ) -> Result<(NativeView, Self::State), NativeWidgetError> {
            let view = new_date_picker(ctx, props.style)?;
            // The style is baked by the constructor and the date is set by
            // `init` below, so the diffed plan must not re-report either
            // (`spinner.rs`'s create, the same shape).
            let mut default = DatePickerProps::platform_default(props.slot);
            default.style = props.style;
            default.date = props.date;
            let plan = DatePickerProps::plan(&default, props, None);
            ctx.with_frame(FRAME_CAPACITY, |ctx| apply_all(ctx, &view, &plan))?;
            // Read after the range setters ran, so an app that supplied no
            // date still hands `init` the platform's (possibly clamped) own.
            let date = match props.date {
                Some(date) => date,
                None => current_date(ctx, &view)?,
            };
            // Attached last, the other controls' create order: nothing above
            // can reach the runtime as an event.
            let listener = ctx.new_listener(props.slot)?;
            ctx.init_date_picker(&view, date, &listener)?;
            let handle = ctx.retain(&view)?;
            let retained = ctx.retain(&view)?;
            let listener_ref = ctx.retain(&listener)?;
            Ok((
                NativeView::with_extra(handle, vec![listener_ref]),
                DatePickerState {
                    view: retained,
                    observed: None,
                },
            ))
        }

        fn update(
            ctx: &mut NativeCtx<'_, '_>,
            state: &mut Self::State,
            old: &Self::Props,
            new: &Self::Props,
        ) -> Result<(), NativeWidgetError> {
            let plan = DatePickerProps::plan(old, new, state.observed);
            // `updateDate`/`setMinDate`/`setMaxDate` echo synchronously into
            // the listener; that echo re-enters `with_runtime` mid-borrow and
            // is dropped there (module doc's *Echo guard*) — no suppression
            // state is held across this call.
            let applied = ctx.with_frame(FRAME_CAPACITY, |ctx| apply_all(ctx, &state.view, &plan));
            if applied.is_ok() {
                state.observed = None;
            }
            applied
        }

        fn on_event(state: &mut Self::State, event: NativeEvent) -> Option<EventPayload> {
            decode_event(&mut state.observed, event)
        }

        fn dispose(
            ctx: &mut NativeCtx<'_, '_>,
            state: Self::State,
        ) -> Result<(), NativeWidgetError> {
            // Detach so a stray in-flight change can't fire after this
            // slot's instance is gone; dropping `state` releases its global
            // reference either way.
            ctx.set_on_date_changed_listener(&state.view, &JObject::null())
        }
    }

    /// Execute a whole [`Plan`], front to back — the range bounds before the
    /// date, the order the shared plan already guarantees.
    fn apply_all(
        ctx: &mut NativeCtx<'_, '_>,
        view: &JObject<'_>,
        plan: &Plan<'_>,
    ) -> Result<(), NativeWidgetError> {
        for setter in plan {
            apply(ctx, view, setter)?;
        }
        Ok(())
    }

    /// Execute one planned property write — this control's own setters
    /// here, the shared ones (`Enabled`, `ContentDescription`) through
    /// `crate::controls::platform::apply`.
    fn apply(
        ctx: &mut NativeCtx<'_, '_>,
        view: &JObject<'_>,
        setter: &Setter<'_>,
    ) -> Result<(), NativeWidgetError> {
        match *setter {
            Setter::Date(date) => ctx.call_void(
                view,
                jni_str!("updateDate"),
                jni_sig!("(III)V"),
                &[
                    JValue::Int(date.year),
                    JValue::Int(i32::from(date.month) - 1),
                    JValue::Int(i32::from(date.day)),
                ],
            ),
            Setter::MinDate(min) => {
                let millis = epoch_millis(ctx, min.unwrap_or(PLATFORM_MIN))?;
                ctx.call_void(
                    view,
                    jni_str!("setMinDate"),
                    jni_sig!("(J)V"),
                    &[JValue::Long(millis)],
                )
            }
            Setter::MaxDate(max) => {
                let millis = epoch_millis(ctx, max.unwrap_or(PLATFORM_MAX))?;
                ctx.call_void(
                    view,
                    jni_str!("setMaxDate"),
                    jni_sig!("(J)V"),
                    &[JValue::Long(millis)],
                )
            }
            Setter::DatePickerStyle(style) => {
                warn_style_unsupported(style);
                Ok(())
            }
            Setter::DatePickerTint(_) | Setter::DatePickerTextColor(_) => {
                log_no_colour_api();
                Ok(())
            }
            Setter::Enabled(_) | Setter::ContentDescription(_) => apply_shared(ctx, view, setter),
            ref other => {
                log::warn!(
                    "frust-native-widgets: control 'date_picker' planned a setter its Android \
                     arm does not implement ({other:?}) — ignored"
                );
                Ok(())
            }
        }
    }

    /// One-time warning that a post-create style change is not applied —
    /// module doc's *Construction*: the mode is baked into the constructor
    /// style, so only a rebuilt view could change it.
    pub(crate) fn warn_style_unsupported(style: DatePickerStyle) {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            log::warn!(
                "frust-native-widgets: Android's DatePicker bakes its spinner/calendar mode into \
                 the style it was constructed with, so a later style change to {style:?} is not \
                 applied — the picker keeps the mode it was created in"
            );
        });
    }

    /// One-time note that `DatePicker` has no tint or text-colour API — the
    /// module doc's *Theme* section: a documented platform gap, not a
    /// defect, so debug level.
    fn log_no_colour_api() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            log::debug!(
                "frust-native-widgets: Android's DatePicker exposes no tint or text-colour setter \
                 — its colours come from the (night-qualified) construction theme only"
            );
        });
    }
}

#[cfg(target_os = "ios")]
pub(crate) mod platform {
    //! The iOS half: build a `UIDatePicker` in `Date` mode and apply the
    //! shared plan — no echo guard of its own (UIKit sends no `ValueChanged`
    //! for a programmatic `setDate:`; `switch.rs`'s module doc is the
    //! reference account).
    //!
    //! # Construction-time normalization
    //!
    //! `create` pins `datePickerMode = Date` (a fresh picker is
    //! `DateAndTime`) and `preferredDatePickerStyle = Compact` — the
    //! [`DatePickerProps::platform_default`] style, rather than UIKit's own
    //! `.automatic`, so the diffed create plan starts from the truth.

    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_ui_kit::{UIDatePicker, UIDatePickerMode, UIDatePickerStyle};

    use super::foundation::{ns_date, optional_ns_date};
    use super::{DatePicker, DatePickerProps, DatePickerStyle, KIND, decode_event};
    use crate::NativeWidgetError;
    use crate::apple::{FrustNativeControlTarget, NativeCtx, NativeView};
    use crate::controls::platform;
    use crate::controls::{Plan, Setter};
    use crate::events::EventPayload;
    use crate::runtime::{NativeEvent, NativeWidget, Params};

    /// A live picker's retained state — `SwitchState`'s shape.
    pub(crate) struct DatePickerState {
        view: Retained<UIDatePicker>,
        /// The target `create` attached as the `ValueChanged` action.
        /// `UIControl` holds targets weakly, so this is its only retain
        /// (`crate::apple::events`' *Target retention*).
        target: Retained<FrustNativeControlTarget>,
        /// The date the platform last reported, `None` while untouched —
        /// [`DatePickerProps::plan`]'s write-back drift signal.
        observed: Option<super::CivilDate>,
    }

    impl NativeWidget for DatePicker {
        type Props = DatePickerProps;
        type State = DatePickerState;

        fn decode_props(params: &Params<'_>) -> Result<Self::Props, NativeWidgetError> {
            DatePickerProps::decode(params)
        }

        fn create(
            ctx: &mut NativeCtx<'_, '_>,
            props: &Self::Props,
        ) -> Result<(NativeView, Self::State), NativeWidgetError> {
            let mtm = ctx.mtm();
            let view = UIDatePicker::new(mtm);
            let default = DatePickerProps::platform_default(props.slot);
            // Module doc's *Construction-time normalization*.
            view.setDatePickerMode(UIDatePickerMode::Date);
            view.setPreferredDatePickerStyle(ui_style(default.style));
            let plan = DatePickerProps::plan(&default, props, None);
            apply_all(mtm, &view, &plan);
            // Attached after the initial plan, the other controls' order.
            let target = FrustNativeControlTarget::attach_date_picker(mtm, props.slot, &view);
            let handle = NativeView::new(Retained::clone(&view).into_super().into_super(), mtm);
            Ok((
                handle,
                DatePickerState {
                    view,
                    target,
                    observed: None,
                },
            ))
        }

        fn update(
            ctx: &mut NativeCtx<'_, '_>,
            state: &mut Self::State,
            old: &Self::Props,
            new: &Self::Props,
        ) -> Result<(), NativeWidgetError> {
            let plan = DatePickerProps::plan(old, new, state.observed);
            apply_all(ctx.mtm(), &state.view, &plan);
            // Nothing on this arm can fail, so the drift signal is cleared
            // unconditionally (`Switch`'s iOS arm).
            state.observed = None;
            Ok(())
        }

        fn on_event(state: &mut Self::State, event: NativeEvent) -> Option<EventPayload> {
            decode_event(&mut state.observed, event)
        }

        fn dispose(
            _ctx: &mut NativeCtx<'_, '_>,
            state: Self::State,
        ) -> Result<(), NativeWidgetError> {
            // Detach so a stray in-flight action can't reach a torn-down
            // slot; dropping `state` afterwards releases the target's retain.
            state.target.detach_date_picker(&state.view);
            Ok(())
        }
    }

    /// The `UIDatePickerStyle` for `style` — module doc's table (top of file).
    fn ui_style(style: DatePickerStyle) -> UIDatePickerStyle {
        match style {
            DatePickerStyle::Compact => UIDatePickerStyle::Compact,
            DatePickerStyle::Wheels => UIDatePickerStyle::Wheels,
            DatePickerStyle::Inline => UIDatePickerStyle::Inline,
        }
    }

    /// Execute a whole [`Plan`], front to back.
    fn apply_all(mtm: MainThreadMarker, view: &UIDatePicker, plan: &Plan<'_>) {
        for setter in plan {
            apply(mtm, view, setter);
        }
    }

    /// Execute one planned property write against `view`.
    fn apply(mtm: MainThreadMarker, view: &UIDatePicker, setter: &Setter<'_>) {
        match *setter {
            // No action is sent for a programmatic date (module doc), so the
            // write-back's snap-back is a plain write.
            Setter::Date(date) => match ns_date(date) {
                Some(ns) => view.setDate(&ns),
                None => log::warn!(
                    "frust-native-widgets: iOS could not build an NSDate for {date} — not applied"
                ),
            },
            Setter::MinDate(min) => view.setMinimumDate(optional_ns_date(min).as_deref()),
            Setter::MaxDate(max) => view.setMaximumDate(optional_ns_date(max).as_deref()),
            Setter::DatePickerStyle(style) => view.setPreferredDatePickerStyle(ui_style(style)),
            Setter::Enabled(enabled) => view.setEnabled(enabled),
            Setter::DatePickerTint(argb) => {
                let color = platform::optional_ui_color(argb);
                // SAFETY: objc2 marks `setTintColor:` unsafe only because the
                // header leaves the argument's nullability unannotated;
                // passing `None` is `UIView`'s own documented "restore the
                // inherited tint" behaviour, exactly `DatePickerTint(None)`'s
                // meaning (`crate::controls::platform::set_image_tint`
                // closes the same question for `UIImageView`).
                unsafe { view.setTintColor(color.as_deref()) };
            }
            Setter::DatePickerTextColor(argb) => {
                log::debug!(
                    "frust-native-widgets: iOS DatePickerTextColor({argb:?}) not applied — \
                     UIDatePicker exposes no public text colour"
                );
            }
            Setter::ContentDescription(label) => {
                platform::set_accessibility_label(view, label, mtm);
            }
            ref other => platform::warn_unexpected_setter(KIND, other),
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) mod platform {
    //! The macOS half: build an `NSDatePicker` showing year/month/day only
    //! and apply the shared plan — no echo guard of its own (AppKit sends no
    //! action for a programmatic `setDateValue:`; `crate::appkit::events`'
    //! *No echo guard*).
    //!
    //! # Construction-time normalization
    //!
    //! `create` pins single-date mode, the `YearMonthDay` elements (no
    //! time), and the [`DatePickerProps::platform_default`] `Compact`
    //! presentation (`textFieldAndStepper` with the calendar overlay on)
    //! before the diffed create plan runs.
    //!
    //! # One action, one kind
    //!
    //! The picker's single target/action pair is wired with
    //! [`crate::events::EVENT_KIND_DATE`]; `frustAction:` reads the sender's
    //! `dateValue`, converts it through [`super::foundation`] and packs it
    //! with [`crate::events::pack_date`], and `on_event` decodes it through
    //! the SAME [`decode_event`] the other two arms call.

    use objc2::rc::Retained;
    use objc2_app_kit::{
        NSColor, NSDatePicker, NSDatePickerElementFlags, NSDatePickerMode,
        NSDatePickerStyle as AppKitStyle,
    };

    use super::foundation::{ns_date, optional_ns_date};
    use super::{DatePicker, DatePickerProps, DatePickerStyle, KIND, decode_event};
    use crate::NativeWidgetError;
    use crate::appkit::{FrustNativeControlTarget, NativeCtx, NativeView};
    use crate::controls::platform;
    use crate::controls::{Plan, Setter};
    use crate::events::{EVENT_KIND_DATE, EventPayload};
    use crate::runtime::{NativeEvent, NativeWidget, Params};

    /// A live picker's retained state — the iOS arm's shape.
    pub(crate) struct DatePickerState {
        /// The picker, kept typed for its own date/range setters.
        view: Retained<NSDatePicker>,
        /// The target `create` attached. `NSControl.target` is weak, so this
        /// field is its only retain (`crate::appkit::events`' *Target
        /// retention*).
        target: Retained<FrustNativeControlTarget>,
        /// The date the platform last reported, `None` while untouched —
        /// [`DatePickerProps::plan`]'s write-back drift signal.
        observed: Option<super::CivilDate>,
    }

    impl NativeWidget for DatePicker {
        type Props = DatePickerProps;
        type State = DatePickerState;

        fn decode_props(params: &Params<'_>) -> Result<Self::Props, NativeWidgetError> {
            DatePickerProps::decode(params)
        }

        fn create(
            ctx: &mut NativeCtx<'_, '_>,
            props: &Self::Props,
        ) -> Result<(NativeView, Self::State), NativeWidgetError> {
            let mtm = ctx.mtm();
            let view = NSDatePicker::new(mtm);
            let default = DatePickerProps::platform_default(props.slot);
            // Module doc's *Construction-time normalization*.
            view.setDatePickerMode(NSDatePickerMode::Single);
            view.setDatePickerElements(NSDatePickerElementFlags::YearMonthDay);
            set_style(&view, default.style);
            let plan = DatePickerProps::plan(&default, props, None);
            apply_all(&view, &plan);
            // Attached after the initial plan, the other controls' order.
            let target = FrustNativeControlTarget::attach(mtm, &view, props.slot, EVENT_KIND_DATE);
            let handle = NativeView::new(Retained::clone(&view).into_super().into_super(), mtm);
            Ok((
                handle,
                DatePickerState {
                    view,
                    target,
                    observed: None,
                },
            ))
        }

        fn update(
            _ctx: &mut NativeCtx<'_, '_>,
            state: &mut Self::State,
            old: &Self::Props,
            new: &Self::Props,
        ) -> Result<(), NativeWidgetError> {
            apply_all(
                &state.view,
                &DatePickerProps::plan(old, new, state.observed),
            );
            // Cleared unconditionally, as on iOS: nothing here can fail.
            state.observed = None;
            Ok(())
        }

        fn on_event(state: &mut Self::State, event: NativeEvent) -> Option<EventPayload> {
            decode_event(&mut state.observed, event)
        }

        fn dispose(
            _ctx: &mut NativeCtx<'_, '_>,
            state: Self::State,
        ) -> Result<(), NativeWidgetError> {
            // Detach so a stray in-flight action can't reach a torn-down
            // slot; dropping `state` afterwards releases the target's retain.
            state.target.detach(&state.view);
            Ok(())
        }
    }

    /// Apply `style` — module doc's table (top of file): AppKit has no
    /// wheels, so `Wheels` is the plain text field and stepper, and
    /// `Compact` adds the click-to-open calendar overlay.
    fn set_style(view: &NSDatePicker, style: DatePickerStyle) {
        let (appkit, overlay) = match style {
            DatePickerStyle::Compact => (AppKitStyle::TextFieldAndStepper, true),
            DatePickerStyle::Wheels => (AppKitStyle::TextFieldAndStepper, false),
            DatePickerStyle::Inline => (AppKitStyle::ClockAndCalendar, false),
        };
        view.setDatePickerStyle(appkit);
        view.setPresentsCalendarOverlay(overlay);
    }

    /// Execute a whole [`Plan`], front to back.
    fn apply_all(view: &NSDatePicker, plan: &Plan<'_>) {
        for setter in plan {
            apply(view, setter);
        }
    }

    /// Execute one planned property write against `view`.
    fn apply(view: &NSDatePicker, setter: &Setter<'_>) {
        match *setter {
            // `setDateValue:` sends no action (module doc's echo guard).
            Setter::Date(date) => match ns_date(date) {
                Some(ns) => view.setDateValue(&ns),
                None => log::warn!(
                    "frust-native-widgets: macOS could not build an NSDate for {date} — not \
                     applied"
                ),
            },
            Setter::MinDate(min) => view.setMinDate(optional_ns_date(min).as_deref()),
            Setter::MaxDate(max) => view.setMaxDate(optional_ns_date(max).as_deref()),
            Setter::DatePickerStyle(style) => set_style(view, style),
            Setter::Enabled(enabled) => platform::set_enabled(view, enabled),
            // No AppKit tint on `NSDatePicker` — `set_tint`'s generic "this
            // view has no tint property" branch logs it (module doc's
            // *Theme*).
            Setter::DatePickerTint(argb) => platform::set_tint(view, "DatePickerTint", argb),
            Setter::DatePickerTextColor(argb) => {
                let color = argb.map_or_else(NSColor::controlTextColor, platform::ns_color);
                view.setTextColor(&color);
            }
            Setter::ContentDescription(label) => platform::set_accessibility_label(view, label),
            ref other => platform::warn_unexpected_setter(KIND, other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::Tier;
    use crate::runtime::with_identity;

    fn date(year: i32, month: u8, day: u8) -> CivilDate {
        CivilDate::new(year, month, day).expect("a real date")
    }

    fn decode(body: &str) -> DatePickerProps {
        let raw = with_identity(KIND, 5, body);
        DatePickerProps::decode(&Params::new(&raw)).expect("decodes")
    }

    fn field(key: &str, value: CivilDate) -> String {
        format!("\"{key}\":{}", wire(value))
    }

    // --- CivilDate ------------------------------------------------------

    #[test]
    fn civil_date_validates_every_field_and_the_month_length() {
        assert!(CivilDate::new(1, 1, 1).is_some());
        assert!(CivilDate::new(9999, 12, 31).is_some());
        assert!(CivilDate::new(0, 1, 1).is_none(), "year 0");
        assert!(CivilDate::new(10_000, 1, 1).is_none(), "year 10000");
        assert!(CivilDate::new(-1, 1, 1).is_none(), "negative year");
        assert!(CivilDate::new(2026, 0, 1).is_none(), "month 0");
        assert!(CivilDate::new(2026, 13, 1).is_none(), "month 13");
        assert!(CivilDate::new(2026, 1, 0).is_none(), "day 0");
        assert!(CivilDate::new(2026, 1, 32).is_none(), "day 32");
        assert!(CivilDate::new(2026, 4, 31).is_none(), "31 April");
        assert!(CivilDate::new(2024, 2, 29).is_some(), "leap year");
        assert!(CivilDate::new(2000, 2, 29).is_some(), "400-year leap");
        assert!(CivilDate::new(1900, 2, 29).is_none(), "century non-leap");
        assert!(CivilDate::new(2026, 2, 29).is_none(), "common year");
        assert_eq!(CivilDate::MIN, date(1, 1, 1));
        assert_eq!(CivilDate::MAX, date(9999, 12, 31));
    }

    #[test]
    fn civil_dates_order_chronologically_and_display_as_iso_8601() {
        assert!(date(2026, 1, 31) < date(2026, 2, 1));
        assert!(date(2025, 12, 31) < date(2026, 1, 1));
        assert_eq!(date(7, 3, 9).to_string(), "0007-03-09");
        assert_eq!(date(2026, 9, 29).to_string(), "2026-09-29");
    }

    // --- decode ---------------------------------------------------------

    #[test]
    fn absent_fields_decode_to_the_platform_defaults_and_plan_nothing() {
        let props = decode("");
        assert_eq!(props, DatePickerProps::platform_default(5));
        assert!(
            DatePickerProps::plan(&DatePickerProps::platform_default(5), &props, None).is_empty()
        );
    }

    #[test]
    fn dates_decode_from_their_packed_wire_integers() {
        let props = decode(&format!(
            "{},{},{},\"style\":\"inline\",\"enabled\":false,\"tint\":7,\"textColor\":8",
            field(DATE, date(2026, 9, 29)),
            field(MIN_DATE, date(2026, 1, 1)),
            field(MAX_DATE, date(2026, 12, 31)),
        ));
        assert_eq!(props.date, Some(date(2026, 9, 29)));
        assert_eq!(props.min, Some(date(2026, 1, 1)));
        assert_eq!(props.max, Some(date(2026, 12, 31)));
        assert_eq!(props.style, DatePickerStyle::Inline);
        assert!(!props.enabled);
        assert_eq!(props.tint, Some(7));
        assert_eq!(props.text_color, Some(8));
    }

    #[test]
    fn an_invalid_wire_date_decodes_as_absent() {
        // 31 April, packed by hand — never a date the platform is handed.
        let props = decode(&format!("\"date\":{}", (2026 << 16) | (4 << 8) | 31));
        assert_eq!(props.date, None);
        let negative = decode("\"minDate\":-5,\"maxDate\":0");
        assert_eq!((negative.min, negative.max), (None, None));
    }

    #[test]
    fn an_unknown_style_decodes_to_compact() {
        assert_eq!(
            decode("\"style\":\"sideways\"").style,
            DatePickerStyle::Compact
        );
        assert_eq!(
            decode("\"style\":\"wheels\"").style,
            DatePickerStyle::Wheels
        );
    }

    #[test]
    fn the_date_is_clamped_into_the_range_on_decode() {
        let before = decode(&format!(
            "{},{}",
            field(DATE, date(2020, 5, 5)),
            field(MIN_DATE, date(2026, 1, 1))
        ));
        assert_eq!(before.date, Some(date(2026, 1, 1)));
        let after = decode(&format!(
            "{},{}",
            field(DATE, date(2030, 5, 5)),
            field(MAX_DATE, date(2026, 12, 31))
        ));
        assert_eq!(after.date, Some(date(2026, 12, 31)));
    }

    #[test]
    fn an_inverted_range_degrades_to_the_single_day_min() {
        let props = decode(&format!(
            "{},{},{}",
            field(DATE, date(2026, 6, 1)),
            field(MIN_DATE, date(2026, 12, 1)),
            field(MAX_DATE, date(2026, 1, 1)),
        ));
        assert_eq!(props.min, Some(date(2026, 12, 1)));
        assert_eq!(props.max, Some(date(2026, 12, 1)));
        assert_eq!(props.date, Some(date(2026, 12, 1)));
    }

    // --- plan -----------------------------------------------------------

    #[test]
    fn the_create_plan_sets_exactly_the_non_default_fields_in_order() {
        let props = decode(&format!(
            "{},{},{},\"style\":\"wheels\",\"enabled\":false,\"tint\":1,\"textColor\":2,\
             \"contentDescription\":\"birthday\"",
            field(DATE, date(2026, 9, 29)),
            field(MIN_DATE, date(2000, 1, 1)),
            field(MAX_DATE, date(2030, 12, 31)),
        ));
        assert_eq!(
            DatePickerProps::plan(&DatePickerProps::platform_default(5), &props, None),
            vec![
                Setter::DatePickerStyle(DatePickerStyle::Wheels),
                Setter::MinDate(Some(date(2000, 1, 1))),
                Setter::MaxDate(Some(date(2030, 12, 31))),
                Setter::Date(date(2026, 9, 29)),
                Setter::Enabled(false),
                Setter::DatePickerTint(Some(1)),
                Setter::DatePickerTextColor(Some(2)),
                Setter::ContentDescription(Some("birthday")),
            ]
        );
    }

    #[test]
    fn a_date_change_alone_plans_exactly_one_setter() {
        let old = decode(&field(DATE, date(2026, 9, 29)));
        let new = decode(&field(DATE, date(2026, 9, 30)));
        assert_eq!(
            DatePickerProps::plan(&old, &new, None),
            vec![Setter::Date(date(2026, 9, 30))]
        );
        assert!(
            DatePickerProps::plan(&new, &new, None).is_empty(),
            "unchanged props plan nothing — the zero-FFI property"
        );
    }

    #[test]
    fn a_pick_the_app_confirmed_writes_the_date_back_once() {
        let before = decode(&field(DATE, date(2026, 9, 29)));
        let after = decode(&field(DATE, date(2026, 10, 3)));
        assert_eq!(
            DatePickerProps::plan(&before, &after, Some(date(2026, 10, 3))),
            vec![Setter::Date(date(2026, 10, 3))]
        );
    }

    #[test]
    fn a_platform_drift_is_written_back_even_when_the_props_date_did_not_change() {
        // The user picked 3 October; the app kept 29 September and changed
        // something else — the picker must snap back.
        let old = decode(&field(DATE, date(2026, 9, 29)));
        let new = decode(&format!(
            "{},\"enabled\":false",
            field(DATE, date(2026, 9, 29))
        ));
        assert_eq!(
            DatePickerProps::plan(&old, &new, Some(date(2026, 10, 3))),
            vec![Setter::Date(date(2026, 9, 29)), Setter::Enabled(false)],
            "the app rejected the pick: the confirmed date is re-asserted"
        );
        assert_eq!(
            DatePickerProps::plan(&old, &new, Some(date(2026, 9, 29))),
            vec![Setter::Enabled(false)],
            "an observed date matching the app's is no drift"
        );
    }

    #[test]
    fn a_range_that_moves_later_writes_the_ceiling_first() {
        let old = decode(&format!(
            "{},{},{}",
            field(DATE, date(2026, 1, 15)),
            field(MIN_DATE, date(2026, 1, 1)),
            field(MAX_DATE, date(2026, 1, 31)),
        ));
        let new = decode(&format!(
            "{},{},{}",
            field(DATE, date(2026, 3, 15)),
            field(MIN_DATE, date(2026, 3, 1)),
            field(MAX_DATE, date(2026, 3, 31)),
        ));
        assert_eq!(
            DatePickerProps::plan(&old, &new, None),
            vec![
                Setter::MaxDate(Some(date(2026, 3, 31))),
                Setter::MinDate(Some(date(2026, 3, 1))),
                Setter::Date(date(2026, 3, 15)),
            ],
            "floor-first would pass through [March 1, January 31]"
        );
    }

    #[test]
    fn a_range_that_moves_earlier_writes_the_floor_first() {
        let old = decode(&format!(
            "{},{}",
            field(MIN_DATE, date(2026, 3, 1)),
            field(MAX_DATE, date(2026, 3, 31)),
        ));
        let new = decode(&format!(
            "{},{}",
            field(MIN_DATE, date(2026, 1, 1)),
            field(MAX_DATE, date(2026, 1, 31)),
        ));
        assert_eq!(
            DatePickerProps::plan(&old, &new, None),
            vec![
                Setter::MinDate(Some(date(2026, 1, 1))),
                Setter::MaxDate(Some(date(2026, 1, 31))),
            ],
            "ceiling-first would pass through [March 1, January 31]"
        );
    }

    #[test]
    fn a_range_change_reasserts_an_unchanged_date() {
        let old = decode(&format!(
            "{},{}",
            field(DATE, date(2026, 6, 15)),
            field(MIN_DATE, date(2026, 1, 1))
        ));
        let new = decode(&format!(
            "{},{}",
            field(DATE, date(2026, 6, 15)),
            field(MIN_DATE, date(2026, 6, 1))
        ));
        assert_eq!(
            DatePickerProps::plan(&old, &new, None),
            vec![
                Setter::MinDate(Some(date(2026, 6, 1))),
                Setter::Date(date(2026, 6, 15)),
            ]
        );
    }

    #[test]
    fn clearing_a_bound_and_the_colours_plans_the_nullable_setters() {
        let set = decode(&format!(
            "{},\"tint\":1,\"textColor\":2",
            field(MAX_DATE, date(2026, 12, 31))
        ));
        let cleared = decode("");
        assert_eq!(
            DatePickerProps::plan(&set, &cleared, None),
            vec![
                Setter::MaxDate(None),
                Setter::DatePickerTint(None),
                Setter::DatePickerTextColor(None),
            ]
        );
    }

    #[test]
    fn every_planned_setter_reports_its_documented_tier() {
        let props = decode(&format!(
            "{},{},{},\"style\":\"inline\",\"enabled\":false,\"tint\":1,\"textColor\":2,\
             \"contentDescription\":\"d\"",
            field(DATE, date(2026, 9, 29)),
            field(MIN_DATE, date(2000, 1, 1)),
            field(MAX_DATE, date(2030, 12, 31)),
        ));
        for setter in DatePickerProps::plan(&DatePickerProps::platform_default(5), &props, None) {
            let expected = if matches!(
                setter,
                Setter::MinDate(_) | Setter::MaxDate(_) | Setter::DatePickerStyle(_)
            ) {
                Tier::Relayout
            } else {
                Tier::Cheap
            };
            assert_eq!(setter.tier(), expected, "{setter:?}");
        }
    }

    // --- events ---------------------------------------------------------

    fn picked(date: CivilDate) -> NativeEvent {
        NativeEvent {
            kind: EVENT_KIND_DATE,
            detail: pack_date(date),
        }
    }

    #[test]
    fn a_user_pick_decodes_and_updates_the_drift_signal() {
        let mut observed = None;
        assert_eq!(
            decode_event(&mut observed, picked(date(2026, 10, 3))),
            Some(EventPayload::Date(date(2026, 10, 3)))
        );
        assert_eq!(observed, Some(date(2026, 10, 3)));
    }

    #[test]
    fn an_invalid_report_decodes_to_nothing_and_leaves_observed_untouched() {
        let mut observed = Some(date(2026, 1, 1));
        let corrupt = NativeEvent {
            kind: EVENT_KIND_DATE,
            detail: (2026 << 16) | (2 << 8) | 30,
        };
        assert_eq!(decode_event(&mut observed, corrupt), None);
        assert_eq!(observed, Some(date(2026, 1, 1)));
    }

    #[test]
    fn a_misrouted_kind_decodes_to_nothing() {
        let mut observed = None;
        let selection = NativeEvent {
            kind: crate::events::EVENT_KIND_SELECTION,
            detail: pack_date(date(2026, 1, 1)),
        };
        assert_eq!(decode_event(&mut observed, selection), None);
        assert_eq!(observed, None);
    }

    // There is no per-instance suppression parameter to test an echo
    // against: `DatePicker.updateDate`'s listener notification is
    // synchronous, so the ONLY guard against a `Setter::Date` echo is
    // `crate::runtime::with_runtime`'s re-entrancy drop one layer up
    // (module doc's *Echo guard*) — pinned by
    // `runtime::tests::the_thread_local_runtime_is_reentrancy_tolerant`,
    // exactly as `slider.rs`'s tests note for `Setter::Progress`.
}
