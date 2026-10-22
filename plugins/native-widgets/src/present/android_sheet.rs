//! The Android sheet arm: a Material
//! `com.google.android.material.bottomsheet.BottomSheetDialog` built, shown
//! and resolved by `FrustNativePresenter.kt`, over `super::android_host`'s
//! shared JNI plumbing (`with_presenter`, the generation-keyed live entry,
//! the one `nativeOnOutcome` callback) — the alert arm's shape
//! (`super::android_alert`), presented over the same resumed `Activity`.
//!
//! # The contract
//!
//! Three Rust → Kotlin members beside `android_host`'s table — plain
//! `@JvmStatic fun`s, so the frozen export set is unchanged; each hops to
//! the main thread itself (inline when already there):
//!
//! | Kotlin | Contract |
//! |---|---|
//! | `showSheet(generation, specJson, image: ByteArray?)` | Builds and shows the sheet; `specJson` is [`wire::sheet_json`], the image bytes travel beside it (`null` for none) |
//! | `dismissSheet(generation)` | Takes the live sheet down, if it is still `generation`'s |
//! | `selectSheetDetent(generation, index)` | Moves the live sheet to the detent at spec `index` |
//!
//! # Mapping
//!
//! | [`SheetSpec`] | Material |
//! |---|---|
//! | `content` | a vertical `LinearLayout` in a `NestedScrollView`: title (`textAppearanceTitleLarge`), message, an `ImageView` decoded with `BitmapFactory` (undecodable bytes omit the row, logged), up to three full-width `MaterialButton` rows — `Default` filled (wearing `tint`), `Destructive` filled in `colorError`, `Cancel` outlined |
//! | `detents` | [`wire::sheet_state`]: `Medium` → `STATE_HALF_EXPANDED` at `halfExpandedRatio = 0.5`, `Large` → `STATE_EXPANDED`, `Custom(f)` → half expanded at `f` clamped into Material's `(0, 1)` rule (`f = 1` is `Large`); `isFitToContents = false`, `skipCollapsed = true`, the sheet laid out full height so a half state shows the head of the content |
//! | `selected` | the state applied before `show` ([`wire::initial_detent`]) |
//! | `grabber` | a `BottomSheetDragHandleView` above the content |
//! | `dismissible` | `setCancelable` (which also gates `isHideable`); `isDraggable` unless the sheet is neither dismissible nor has a second detent |
//! | `corner_radius` / `dark` | the sheet background's corner size (dp) / the dialog context's night bit |
//! | `scrolling_expands` / `largest_undimmed` | ignored: the scroll view always hands nested scrolls to the behavior, and the dialog dims at every height |
//!
//! `BottomSheetBehavior` holds one half-expanded ratio at a time, so of
//! several half detents the user drags between the selected (else smallest)
//! one and the full height; [`SheetHandle`](super::SheetHandle)'s
//! `select_detent` reaches the others. A drag that settles where no detent
//! rests (half expanded with no half detent, expanded with no full one,
//! collapsed) returns to the detent it left.
//!
//! # Resolution
//!
//! Every terminal path reports through `nativeOnOutcome`, exactly once
//! under the presenter's own live-dialog guard (shared with the alert) and
//! `android_host`'s generation-keyed entry:
//!
//! - **An action row** → the dialog dismisses, and its dismiss listener
//!   reports `OUTCOME_ACTION` — [`SheetOutcome::Action`] resolves after the
//!   sheet has finished dismissing, as on iOS.
//! - **A swipe-down, back key or outside tap** (only when `dismissible`) →
//!   the cancel listener → `OUTCOME_CANCELLED` →
//!   [`SheetOutcome::Dismissed`]`(`[`DismissReason::User`](super::DismissReason::User)`)`.
//! - **[`SheetHandle::dismiss`](super::SheetHandle::dismiss)** →
//!   `dismissSheet` → `OUTCOME_DISMISSED` → `Dismissed(Programmatic)`.
//! - **The hosting Activity destroyed** → `OUTCOME_HOST_LOST` →
//!   [`SheetOutcome::HostLost`].
//! - **No resumed Activity** → [`PresentError::NoHost`], synchronously here
//!   or from `showSheet`; an exception building the sheet → `OUTCOME_FAILED`.
//!
//! Detent changes are not outcomes: `BottomSheetCallback.onStateChanged`
//! reports `OUTCOME_DETENT` with the detent's spec index when the user
//! settles the sheet at a different detent, streamed to
//! [`SheetSpec::on_detent`] with the presentation left live. The detent
//! applied at show and a [`select_detent`](SheetHost::select_detent) move are
//! not echoed back.
//!
//! # A displaced presentation
//!
//! As in the alert arm: an abandoned presentation's resolver is resolved
//! (and discarded) as [`install_live_streaming`] displaces it, and
//! `showSheet` takes its dialog — an alert's or a sheet's — down first.
//!
//! # `unsafe`
//!
//! None — the raw FFI boundary is `android_host`'s exported
//! `nativeOnOutcome`.

#[cfg(not(target_os = "android"))]
compile_error!("the Android sheet arm targets Android only");

use std::sync::{Arc, Mutex};

use jni::objects::{JClass, JObject, JValue};
use jni::refs::Global;
use jni::sys::jlong;
use jni::{Env, jni_sig, jni_str};

use super::android_host::{
    Stream, has_resumed_activity, install_live_streaming, jni_call, sheet_resolver, take_live,
    with_presenter,
};
use super::{Detent, PresentError, Sender, SheetHost, SheetOutcome, SheetSpec, wire};

/// The live sheet's generation and detents — what
/// [`select_detent`](SheetHost::select_detent) turns a [`Detent`] into a
/// spec index with. Superseded by the next sheet and cleared when `showSheet`
/// fails; a stale entry is harmless, the generation filters it. (It stays
/// outside the live entry, which lives in the unscoped `android_host`.)
static DETENTS: Mutex<Option<(u64, Vec<Detent>)>> = Mutex::new(None);

/// The spec index of `detent` in presentation `generation`'s detents.
fn detent_index(generation: u64, detent: Detent) -> Option<usize> {
    let detents = DETENTS.lock().unwrap_or_else(|p| p.into_inner());
    match detents.as_ref() {
        Some((live, detents)) if *live == generation => detents.iter().position(|d| *d == detent),
        _ => None,
    }
}

/// Forget `generation`'s detents (a newer sheet's entry is left alone).
fn clear_detents(generation: u64) {
    let mut detents = DETENTS.lock().unwrap_or_else(|p| p.into_inner());
    if detents
        .as_ref()
        .is_some_and(|(live, _)| *live == generation)
    {
        *detents = None;
    }
}

/// The Android host: `super::SheetHost` over `BottomSheetDialog`.
pub(crate) struct Host;

impl SheetHost for Host {
    fn show_sheet(
        spec: SheetSpec,
        tx: Sender<SheetOutcome>,
        generation: u64,
    ) -> Result<(), PresentError> {
        if !with_presenter(has_resumed_activity)? {
            return Err(PresentError::NoHost);
        }
        let action_ids = spec.content.actions.iter().map(|a| a.id.clone()).collect();
        *DETENTS.lock().unwrap_or_else(|p| p.into_inner()) =
            Some((generation, spec.detents.clone()));
        let stream = spec.detent_listener().map(|listener| {
            let detents = spec.detents.clone();
            Arc::new(move |index: i32| match wire::detent_at(index, &detents) {
                Some(detent) => listener(detent),
                None => log::warn!(
                    "frust-native-widgets: sheet detent report {index} is out of range — dropped"
                ),
            }) as Stream
        });
        let resolve = sheet_resolver(tx, action_ids);
        if let Some(displaced) = install_live_streaming(generation, resolve, stream) {
            // Discarded on arrival: its receiver is already gone.
            displaced(wire::OUTCOME_DISMISSED, -1);
        }
        let json = wire::sheet_json(&spec);
        let image = spec.content.image.as_deref();
        let result =
            with_presenter(|env, class| call_show_sheet(env, class, generation, &json, image));
        if result.is_err() {
            // `showSheet` never ran: nothing will resolve the parked entry.
            drop(take_live(generation));
            clear_detents(generation);
        }
        result
    }

    fn dismiss(generation: u64) {
        let result = with_presenter(|env, class| {
            jni_call(env, "FrustNativePresenter.dismissSheet", |env| {
                env.call_static_method(
                    class,
                    jni_str!("dismissSheet"),
                    jni_sig!("(J)V"),
                    &[JValue::Long(generation as jlong)],
                )?
                .v()
            })
        });
        if let Err(err) = result {
            log::warn!(
                "frust-native-widgets: FrustNativePresenter.dismissSheet({generation}) failed: {err}"
            );
        }
    }

    fn select_detent(generation: u64, detent: Detent) {
        let Some(index) = detent_index(generation, detent).and_then(|i| i32::try_from(i).ok())
        else {
            log::warn!(
                "frust-native-widgets: select_detent({detent:?}) ignored: not one of the sheet's \
                 detents"
            );
            return;
        };
        let result = with_presenter(|env, class| {
            jni_call(env, "FrustNativePresenter.selectSheetDetent", |env| {
                env.call_static_method(
                    class,
                    jni_str!("selectSheetDetent"),
                    jni_sig!("(JI)V"),
                    &[JValue::Long(generation as jlong), JValue::Int(index)],
                )?
                .v()
            })
        });
        if let Err(err) = result {
            log::warn!(
                "frust-native-widgets: FrustNativePresenter.selectSheetDetent({generation}) \
                 failed: {err}"
            );
        }
    }
}

/// `FrustNativePresenter.showSheet(generation, specJson, image)` — see the
/// module doc's contract table. `generation` crosses as the same 64 bits
/// (`android_alert`'s `showAlert` call explains the cast).
fn call_show_sheet(
    env: &mut Env<'_>,
    class: &Global<JClass<'static>>,
    generation: u64,
    json: &str,
    image: Option<&[u8]>,
) -> Result<(), PresentError> {
    jni_call(env, "FrustNativePresenter.showSheet", |env| {
        let json = env.new_string(json)?;
        let image: JObject = match image {
            Some(bytes) => env.byte_array_from_slice(bytes)?.into(),
            None => JObject::null(),
        };
        env.call_static_method(
            class,
            jni_str!("showSheet"),
            jni_sig!("(JLjava/lang/String;[B)V"),
            &[
                JValue::Long(generation as jlong),
                JValue::Object(&json),
                JValue::Object(&image),
            ],
        )?
        .v()
    })
}
