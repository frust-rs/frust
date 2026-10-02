//! The Android alert arm: an `android.app.AlertDialog` built, shown and
//! resolved by `FrustNativePresenter.kt`, over `super::android_host`'s
//! shared JNI plumbing (`with_presenter`, the generation-keyed live-entry
//! guard, `nativeOnOutcome`).
//!
//! # The contract
//!
//! Two more members alongside `android_host`'s module doc's table, both
//! called Rust → Kotlin — plain `@JvmStatic fun`s, not `external fun`s, so
//! neither adds to the frozen export set `tests/kotlin_conformance.rs`
//! pins:
//!
//! | Kotlin | Contract |
//! |---|---|
//! | `@JvmStatic fun showAlert(generation, title, message, labels, roles, cancelable)` | Builds and shows the dialog; `labels`/`roles` are parallel, spec-ordered arrays (`roles`' wire codes below) |
//! | `@JvmStatic fun dismiss(generation)` | Takes the live dialog down, if it is still `generation`'s |
//!
//! # Mapping
//!
//! | [`AlertSpec`] | `android.app.AlertDialog` |
//! |---|---|
//! | `title` / `message` | `setTitle` / `setMessage` — always set, even empty: like `NSAlert`'s setters (`appkit_alert`'s module doc), `AlertDialog.Builder`'s take a plain `CharSequence`, never `null` |
//! | up to [`super::MAX_ALERT_ACTIONS`] `actions` | one of `setPositiveButton`/`setNegativeButton`/`setNeutralButton` each — see *Button mapping* |
//! | `cancelable` | `setCancelable` — gates both the back key and an outside tap |
//! | `style` / `anchor` | ignored — `android.app.AlertDialog` has no action-sheet idiom (framework-only, no Material/appcompat), so [`super::AlertStyle::ActionSheet`] presents the same dialog as [`super::AlertStyle::Alert`] and [`AlertSpec::anchor`] has nothing to point at, mirroring `appkit_alert`'s macOS arm |
//!
//! # Button mapping
//!
//! `AlertDialog` has exactly three button slots — positive, negative,
//! neutral — matching [`super::MAX_ALERT_ACTIONS`]. Each action claims its
//! preferred slot in spec order, falling back down its preference list when
//! that slot is already taken (Kotlin's `assignSlots`):
//!
//! - [`ActionRole::Cancel`] prefers negative, then neutral, then positive.
//! - [`ActionRole::Default`]/[`ActionRole::Destructive`] both prefer
//!   positive, then neutral, then negative — the framework dialog has no
//!   severity styling for `Destructive` (no color role to reach for, unlike
//!   `appkit_alert`'s `hasDestructiveAction`), so the two roles differ only
//!   in which slot they end up sharing when a spec mixes them.
//!
//! With at most one `Cancel` action ([`AlertSpec::validate`]) and at most
//! [`super::MAX_ALERT_ACTIONS`] actions total, every action always finds a
//! free slot: a one-action alert always lands on positive, and a
//! `[Default, Cancel]` pair lands positive/negative — the platform's own
//! "OK" / "Cancel" convention.
//!
//! # Resolution
//!
//! Every path resolves through the ONE callback
//! `android_host::Java_dev_frust_nativewidgets_FrustNativePresenter_nativeOnOutcome`,
//! guarded exactly once **on the Kotlin side** (`FrustNativePresenter`'s own
//! `liveDialog`-null check — independent of, and in addition to,
//! `android_host`'s `LIVE` generation-keyed parking on the Rust side):
//!
//! - **An action** (button tap) → `DialogInterface.OnClickListener` →
//!   [`wire::OUTCOME_ACTION`] with the button's spec index.
//! - **Cancel** (back key, outside tap — only when `cancelable`) →
//!   `setOnCancelListener` → [`wire::OUTCOME_CANCELLED`]. A `Dialog`
//!   window receives the back key before the hosting Activity's
//!   `OnBackPressedDispatcher`, so no dispatcher wiring is needed.
//! - **[`super::dismiss`]** — `FrustNativePresenter.dismiss(generation)`
//!   reports [`wire::OUTCOME_DISMISSED`] itself, then closes the dialog.
//! - **No resumed Activity** — checked twice: synchronously in
//!   [`Host::show_alert`] via `android_host::has_resumed_activity` (skips
//!   marshaling and the JNI round-trip into `showAlert` entirely, the same
//!   check the pre-arm stub this module replaces already made), and again
//!   inside `showAlert` itself for the narrow race window between the two
//!   calls (the resumed Activity finishing in between) → the latter reports
//!   [`wire::OUTCOME_NO_HOST`] (maps to [`PresentError::NoHost`]); the
//!   former returns the same error synchronously, without ever touching
//!   `LIVE`.
//! - **An exception** building or showing the dialog →
//!   [`wire::OUTCOME_FAILED`].
//! - **The hosting Activity destroyed** while showing (a configuration
//!   change, e.g. rotation, or the user leaving the app) →
//!   `Application.ActivityLifecycleCallbacks.onActivityDestroyed` →
//!   [`wire::OUTCOME_HOST_LOST`], and the dialog is dismissed with it
//!   (module doc's *Retention*).
//!
//! # A displaced presentation
//!
//! A caller that drops its [`super::Presentation`] frees the process-wide
//! Busy slot but not the dialog on screen, so a newer `show_alert` can find
//! the older one still parked. Its Rust-side resolver is resolved at once
//! as `android_host::install_live` displaces it — discarded, since its
//! receiver is already gone (the module doc's own contract: dropping the
//! future frees only Rust-side bookkeeping). The stale `AlertDialog` itself
//! is taken down on Kotlin's side, inside `showAlert`, before the new one is
//! built, so the two never show stacked.
//!
//! # Retention
//!
//! Unlike the Apple arms, which retain the platform objects themselves
//! (`Retained<UIAlertController>`, a delegate) in Rust-owned storage
//! (`apple_host`'s `LIVE`), the `AlertDialog` and its listeners are retained
//! entirely on the JVM side: `FrustNativePresenter`'s own `liveDialog`/
//! `liveActivity` fields, reachable from the `object` singleton (a GC root)
//! for as long as the presentation is live. `android_host`'s `LIVE` holds
//! only what Rust itself needs to resolve the presentation later — the
//! boxed [`Sender`] and the spec's action ids
//! (`android_host::alert_resolver`).
//!
//! # Main-thread dispatch
//!
//! Unlike the Apple arms, which always bounce onto the main queue
//! (`apple_host::on_main`) because a request can arrive from any thread,
//! this arm calls `FrustNativePresenter.showAlert`/`dismiss` **directly**,
//! synchronously, with no Handler post: the whole frust Android runtime is
//! thread-confined to the platform main `Looper` (`android/mod.rs`'s
//! *Threading*; `docs/PLUGINS_CODE_STANDARDS.md`'s events-as-signals
//! convention makes the same claim for a native listener callback), so a
//! [`show_alert`](AlertHost::show_alert)/[`dismiss`](AlertHost::dismiss)
//! request is itself already running there — the same assumption
//! `android_host::has_resumed_activity` already made before this arm
//! existed. There is no debug-only main-thread assertion here (unlike
//! `android/mod.rs`'s exports): that helper is private to `crate::android`,
//! and duplicating it was judged not worth the extra surface for one more
//! call site sharing the same, already-documented assumption.
//!
//! # `unsafe`
//!
//! None — every JNI call here goes through the `jni` crate's safe wrapper
//! API. The raw FFI boundary is `android_host`'s
//! `#[unsafe(no_mangle)]`-exported `nativeOnOutcome`.

#[cfg(not(target_os = "android"))]
compile_error!("the Android alert arm targets Android only");

use jni::objects::{JClass, JIntArray, JObject, JValue};
use jni::refs::Global;
use jni::sys::jlong;
use jni::{Env, jni_sig, jni_str};

use super::android_host::{
    alert_resolver, has_resumed_activity, install_live, take_live, with_presenter,
};
use super::{
    ActionRole, AlertAction, AlertHost, AlertOutcome, AlertSpec, PresentError, Sender, wire,
};

/// The button-role wire codes `showAlert`'s `roles` parameter carries, in
/// [`ActionRole`]'s own declaration order — mirrored verbatim by
/// `FrustNativePresenter.kt`'s `ROLE_*` constants (edited together, the same
/// discipline `super::wire`'s `OUTCOME_*` table documents for the callback
/// direction). No automated drift test pins this table the way
/// `super`'s own tests pin `OUTCOME_*`: this whole module is
/// `#[cfg(target_os = "android")]`, so a test living here never runs under
/// the host-targeted `cargo test` the existing Kotlin-conformance checks
/// rely on to catch drift without an Android toolchain.
const ROLE_DEFAULT: i32 = 0;
const ROLE_CANCEL: i32 = 1;
const ROLE_DESTRUCTIVE: i32 = 2;

/// [`ROLE_DEFAULT`]/[`ROLE_CANCEL`]/[`ROLE_DESTRUCTIVE`] for `role`.
fn role_code(role: ActionRole) -> i32 {
    match role {
        ActionRole::Default => ROLE_DEFAULT,
        ActionRole::Cancel => ROLE_CANCEL,
        ActionRole::Destructive => ROLE_DESTRUCTIVE,
    }
}

/// The Android host: `super::AlertHost` over `android.app.AlertDialog`.
pub(crate) struct Host;

impl AlertHost for Host {
    fn show_alert(
        spec: AlertSpec,
        tx: Sender<AlertOutcome>,
        generation: u64,
    ) -> Result<(), PresentError> {
        // The fast, synchronous refusal — module doc's *Resolution*: skips
        // marshaling the spec and the JNI round-trip into `showAlert`
        // entirely when there is plainly no host yet.
        if !with_presenter(has_resumed_activity)? {
            return Err(PresentError::NoHost);
        }
        let action_ids: Vec<String> = spec
            .actions
            .iter()
            .map(|action| action.id.clone())
            .collect();
        let resolve = alert_resolver(tx, action_ids);
        if let Some(displaced) = install_live(generation, resolve) {
            // Module doc's *A displaced presentation*: resolved here for
            // Rust's own bookkeeping, discarded on arrival (its receiver is
            // already gone) — Kotlin's `showAlert` takes the stale dialog
            // itself down.
            displaced(wire::OUTCOME_DISMISSED, -1);
        }
        let result = with_presenter(|env, class| call_show_alert(env, class, generation, &spec));
        if result.is_err() {
            // `showAlert` never ran (or the call to reach it never
            // returned), so nothing will ever call this presentation's
            // resolver. Reclaim and drop it rather than leaving it parked
            // forever: the oneshot module's drop-without-outcome fallback
            // resolves the channel, which `submit`'s `Err` branch then
            // discards in favor of the more specific error returned below.
            drop(take_live(generation));
        }
        result
    }

    fn dismiss(generation: u64) {
        if let Err(err) = with_presenter(|env, class| call_dismiss(env, class, generation)) {
            log::warn!(
                "frust-native-widgets: FrustNativePresenter.dismiss({generation}) failed: {err}"
            );
        }
    }
}

/// `FrustNativePresenter.showAlert(generation, title, message, labels,
/// roles, cancelable)` — see the module doc's contract table.
fn call_show_alert(
    env: &mut Env<'_>,
    class: &Global<JClass<'static>>,
    generation: u64,
    spec: &AlertSpec,
) -> Result<(), PresentError> {
    jni_call(env, "FrustNativePresenter.showAlert", |env| {
        let title = env.new_string(&spec.title)?;
        let message = env.new_string(&spec.message)?;
        let labels = label_array(env, &spec.actions)?;
        let roles = role_array(env, &spec.actions)?;
        env.call_static_method(
            class,
            jni_str!("showAlert"),
            jni_sig!("(JLjava/lang/String;Ljava/lang/String;[Ljava/lang/String;[IZ)V"),
            &[
                // `u64` → `jlong` is a reinterpretation, not a truncation —
                // the presenter treats it as opaque and echoes the same 64
                // bits back through `nativeOnOutcome` (`android_host`'s own
                // doc comment on the inverse cast).
                JValue::Long(generation as jlong),
                JValue::Object(&title),
                JValue::Object(&message),
                JValue::Object(&labels),
                JValue::Object(&roles),
                JValue::Bool(spec.cancelable),
            ],
        )?
        .v()
    })
}

/// `FrustNativePresenter.dismiss(generation)`.
fn call_dismiss(
    env: &mut Env<'_>,
    class: &Global<JClass<'static>>,
    generation: u64,
) -> Result<(), PresentError> {
    jni_call(env, "FrustNativePresenter.dismiss", |env| {
        env.call_static_method(
            class,
            jni_str!("dismiss"),
            jni_sig!("(J)V"),
            &[JValue::Long(generation as jlong)],
        )?
        .v()
    })
}

/// Build a `String[]` of each action's label, in spec order — `showAlert`'s
/// `labels` parameter.
fn label_array<'local>(
    env: &mut Env<'local>,
    actions: &[AlertAction],
) -> Result<JObject<'local>, jni::errors::Error> {
    let empty = JObject::null();
    let array = env.new_object_array(actions.len() as i32, jni_str!("java/lang/String"), &empty)?;
    for (index, action) in actions.iter().enumerate() {
        let label = env.new_string(&action.label)?;
        array.set_element(env, index, &label)?;
    }
    Ok(array.into())
}

/// Build an `int[]` of each action's [`role_code`], in spec order —
/// `showAlert`'s `roles` parameter.
fn role_array<'local>(
    env: &mut Env<'local>,
    actions: &[AlertAction],
) -> Result<JObject<'local>, jni::errors::Error> {
    let codes: Vec<i32> = actions
        .iter()
        .map(|action| role_code(action.role))
        .collect();
    let array = JIntArray::new(env, codes.len())?;
    array.set_region(env, 0, &codes)?;
    Ok(array.into())
}

/// Run JNI calls through the crate's exception-clearing helper
/// (`crate::android::run_jni`), as a [`PresentError::Platform`] — this
/// module's own copy of `android_host`'s private helper of the same shape.
/// Not shared: that one stays private to `android_host`, and widening it was
/// outside this module's own two-file write scope.
fn jni_call<'local, T>(
    env: &mut Env<'local>,
    op: &str,
    f: impl FnOnce(&mut Env<'local>) -> Result<T, jni::errors::Error>,
) -> Result<T, PresentError> {
    crate::android::run_jni(env, op, f).map_err(|e| PresentError::Platform(e.to_string()))
}
