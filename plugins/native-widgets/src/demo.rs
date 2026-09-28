//! [`DemoCard`] — ONE composite [`NativeComponent`], shipped inside this
//! plugin behind the **non-default `demo-components` feature**: a parent view
//! with a title label and two buttons under it, published as exactly **one**
//! `platform_view` slot no matter how many children it grows. It proves the
//! [`NativeComponent`] trait works end to end — define → register → mount →
//! create → update → dispose, with a real native hierarchy under one slot —
//! with real native coverage on all three platform arms (Android, iOS,
//! macOS).
//!
//! # Why the demo lives in the plugin and not in an example app
//!
//! **An app crate cannot implement [`NativeComponent`] today.** `create` has
//! to name `jni::objects::JObject` (Android), `objc2-ui-kit`'s classes (iOS)
//! or `objc2-app-kit`'s (macOS) *in the implementing crate*, and this plugin
//! re-exports none of those FFI crates (`docs/CODE_STANDARDS.md`'s State & Reactivity Conventions put an
//! `examples/*` app on `frust` plus plugin crates only — a raw FFI dependency
//! is not among them). So the demo ships here, where both FFI crates are
//! already dependencies and all three target gates already run; a
//! consuming app merely turns the feature on, calls
//! [`register_demo_components`], and mounts the result. This proves the
//! trait works, not that a third-party app author can write one — they still
//! need raw FFI dependencies of their own; closing that gap (re-exporting a
//! curated view-construction surface, or the FFI crates themselves) is a
//! separate, unscheduled decision.
//!
//! # The subtree, and who lays it out
//!
//! | | Android | iOS | macOS | host (Linux/Windows/web) |
//! |---|---|---|---|---|
//! | parent | `android.widget.LinearLayout` (vertical) | `UIView`, children at explicit frames | layer-backed `NSView`, children at explicit frames | a recorded identity |
//! | title | `android.widget.TextView` | `UILabel` | `NSTextField` (`labelWithString:`) | a recorded identity |
//! | two buttons | `android.widget.Button` | `UIButton` (`System`) | `NSButton` (push bezel) | recorded identities |
//!
//! **The platform lays this out, and frust deliberately does not know the
//! children exist** (`crate::component`'s *A component owns its own native
//! subtree*): frust's wire carries one rect for the whole card. Android's
//! `LinearLayout` measures and stacks its own children; the two Apple arms
//! assign each child an explicit frame inside [`DEMO_CARD_WIDTH`] x
//! [`DEMO_CARD_HEIGHT`], because this crate deliberately pulls in neither a
//! stack view nor any constraint API (`crate::apple::ctx`'s and
//! `crate::appkit::ctx`'s *Frame-setting layout only*). Mount the slot at
//! exactly that size.
//!
//! # One slot, four handles, and a counted teardown
//!
//! The whole subtree is built inside ONE [`ComponentCtx::with_local_frame`]
//! (mandatory on Android, a no-op under ARC), and every handle the card keeps
//! lives in [`DemoCardState`], which the runtime drops immediately after
//! [`NativeComponent::dispose`] returns:
//!
//! - the **parent** is kept as a [`NativeChild`] on every arm — `update` is
//!   handed only the state, never the [`NativeRoot`] the runtime owns, so it
//!   needs its own way back to set the card's background, and the background
//!   is a view-level call on both Apple arms so the cross-platform handle
//!   suffices;
//! - the **three children** keep their concrete types on the Apple arms
//!   (`Retained<UILabel>`/`Retained<UIButton>` on iOS,
//!   `Retained<NSTextField>`/`Retained<NSButton>` on macOS — the shape
//!   [`ComponentCtx::retain_child`]'s own doc says to reach for first, since
//!   the caption setters are not view-level calls) and stay [`NativeChild`]s
//!   everywhere else;
//! - the primary button's **click listener** is kept as a [`ListenerHandle`]
//!   (*Event wiring*, below) — its own counted release on the host arm.
//!
//! Dropping either kind **is** the release — `DeleteGlobalRef` on Android,
//! `Retained`'s own `Drop` on iOS and macOS — so this module's own host test counts the
//! live handles back to zero rather than assuming it.
//!
//! # Event wiring: the primary button reports its clicks
//!
//! The **primary** button attaches the platform's one listener for
//! [`ListenerKinds::CLICK`] through [`ComponentCtx::attach_listener`] —
//! `FrustNativeListener` on Android, `FrustNativeControlTarget` on iOS and
//! macOS — bound by the context to this card's own slot id, which the card
//! never sees. Each click reaches [`NativeComponent::on_event`], which
//! forwards it to the app's
//! [`NativeComponentView::on_event`](crate::api::NativeComponentView::on_event)
//! hook; the app counts it and hands the count back as
//! [`DemoCardProps::presses`], which the card shows on the primary button's
//! own caption — a native tap moving a native readout through one full
//! round trip (native → Rust → signal → rebuild → props diff → native
//! setter), the same loop the six built-in controls close.
//!
//! The **secondary** button stays unwired on purpose. A listener reports its
//! slot and its event family, not which child fired: a click is
//! `(KIND_CLICK, 0)` whichever view it came from, so two children attached for
//! the same family are indistinguishable in `on_event`. A component that needs
//! to tell children apart gives each a family of its own, or mounts them as
//! separate slots. The [`ListenerHandle`] lives in [`DemoCardState`] and is
//! detached in `dispose`, mirroring the built-in `Button`'s own teardown.
//!
//! **The platform escape hatch is not how events arrive.**
//! `ComponentCtx::env`/`ComponentCtx::mtm` still hand out the raw platform,
//! but a listener a component builds itself carries no route back into
//! [`NativeComponent::on_event`] — only one attached through the context is
//! bound to the slot — and an Android `FrustNativeListener` built by hand with
//! a guessed id would, at best, be dropped by the slot's attached-family gate
//! and, at worst, deliver into whichever slot that number means.
//!
//! # The feature gate
//!
//! `demo-components` is **off by default** and enables `frust-api` (a component
//! is only mountable through the facade glue that feature carries).
//! `--no-default-features` therefore still resolves this crate to its
//! platform-plugin charter line — `frust-plugin` + FFI crates, nothing else
//! (`Cargo.toml`'s own comment; `cargo tree -p frust-native-widgets
//! --no-default-features -e normal`).

use crate::component::{ListenerHandle, NativeChild, register_component};

// Named by this module's doc links only; the three per-arm `platform` modules
// below import what they actually call. Without the import the links would have
// to be spelled as full `crate::component::…` paths in every doc comment here.
#[allow(unused_imports)]
use crate::component::{ComponentCtx, ListenerKinds, NativeComponent, NativeRoot};

/// The kind string [`DemoCard`] registers under — namespaced so it can never
/// collide with the six built-in control kinds (`button`, `label`, …), which
/// this crate's backend registers first and which registration is first-wins
/// about.
pub const DEMO_CARD_KIND: &str = "frust.demo.card";

/// The slot width [`DemoCard`] is laid out for (logical px) — see the module
/// doc's *The subtree, and who lays it out*.
pub const DEMO_CARD_WIDTH: f64 = 240.0;

/// The slot height [`DemoCard`] is laid out for (logical px). Tall enough for
/// the title row plus two stacked buttons at the private constants below, on
/// both platforms.
pub const DEMO_CARD_HEIGHT: f64 = 160.0;

/// How many native views [`DemoCard`] builds **under** its own root — the
/// number that must NOT show up as extra frust slots (frust sees one).
pub const DEMO_CARD_CHILDREN: usize = 3;

// The four geometry constants below are read by the three platform arms
// (Android takes them as view padding / minimum heights, iOS and macOS as
// explicit child frames) and by this module's own size test. A host build with
// no platform arm (Linux/Windows/web) reads none of them, hence the per-item `allow` rather than a module-level one — anything
// else falling dead here still warns.
/// The card's inner padding (logical px): the Apple arms' explicit child
/// frames inset by it, Android's `LinearLayout` takes it as view padding.
#[allow(dead_code)]
const CARD_PADDING: f64 = 8.0;

/// The title row's height (logical px).
#[allow(dead_code)]
const TITLE_HEIGHT: f64 = 24.0;

/// Each button's height (logical px) — at or above the 44pt/48dp minimum touch
/// target both platforms publish, so the card is honest about being real
/// controls rather than a picture of some.
#[allow(dead_code)]
const BUTTON_HEIGHT: f64 = 44.0;

/// The vertical gap above each button (logical px).
#[allow(dead_code)]
const ROW_GAP: f64 = 8.0;

/// The JNI local-reference frame this card's whole `create` runs inside
/// (Android only; a pre-allocation hint, not a cap — see
/// [`ComponentCtx::with_local_frame`]). Four views, three strings and the class
/// references behind them fit comfortably.
const FRAME_CAPACITY: usize = 16;

/// A composite native card: a parent view owning a title label and two
/// buttons, shipped as **one** `platform_view` slot.
///
/// Register it once ([`register_demo_components`]) and mount it with
/// [`native_component`](crate::api::native_component):
///
/// ```ignore
/// frust_native_widgets::register_demo_components();       // once, at init
/// // …then, in a rebuild:
/// native_component(DEMO_CARD_KIND, DemoCard, DemoCardProps { /* … */ })
///     .size(DEMO_CARD_WIDTH, DEMO_CARD_HEIGHT)
///     .interactive()
///     .on_event(move |event| {
///         if event.is_click() {
///             presses.set(presses.get_untracked() + 1);
///         }
///     })
/// ```
///
/// A unit struct because this card carries no app callbacks of its own: its
/// [`NativeComponent::on_event`] forwards the primary button's clicks, and the
/// app's `.on_event` hook is where they are counted (the module doc's *Event
/// wiring*). A component that wanted to act on an event itself would hold its
/// closures here, on `&self`, and reach them from `on_event`.
pub struct DemoCard;

/// Everything [`DemoCard`] is told, as one Rust-diffed value.
///
/// Colours arrive as packed ARGB rather than as a `frust_theme::Theme`, because
/// a component's props are **typed Rust values the app constructs**: the app
/// reads `use_context::<Theme>()` itself and folds whatever it wants in
/// (`crate::api::mount`'s *What a component's builder does NOT carry*). That is
/// also what lets the card re-theme live through the ordinary props-diff path,
/// at zero FFI cost on an unchanged rebuild.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemoCardProps {
    /// The title label's text.
    pub title: String,
    /// The first button's caption.
    pub primary: String,
    /// The second button's caption.
    pub secondary: String,
    /// The card background, packed ARGB — the signed colour int Java uses,
    /// i.e. `i32::from_be_bytes([a, r, g, b])`.
    pub background_argb: i32,
    /// The title label's ink, packed ARGB (see [`Self::background_argb`]).
    pub title_argb: i32,
    /// How many primary-button clicks the app has counted — shown on the
    /// primary button's own caption (`"{primary} ({presses})"`, see
    /// [`Self::primary_caption`]), so a click moves a native readout once the
    /// app hands the new count back (the module doc's *Event wiring*).
    pub presses: u32,
}

impl DemoCardProps {
    /// The primary button's caption: the app's text plus the press count.
    pub fn primary_caption(&self) -> String {
        format!("{} ({})", self.primary, self.presses)
    }
}

/// [`DemoCard`]'s retained per-slot state — the module doc's *One slot, four
/// handles, and a counted teardown*.
pub struct DemoCardState {
    /// A second reference to the card's own parent view: `update` is handed
    /// only the state, never the [`NativeRoot`] the runtime holds, so the card
    /// needs its own way back in order to set the background. The same shape
    /// the six built-in controls' own states use, one tier up.
    root: NativeChild,
    /// The title label.
    label: LabelHandle,
    /// The two buttons, in declaration order (primary, secondary).
    buttons: [ButtonHandle; 2],
    /// The primary button's click listener (the module doc's *Event wiring*),
    /// detached in `dispose` and released with the state.
    listener: ListenerHandle,
}

/// The title label's handle: its concrete class on the Apple arms (`setText:`/
/// `setStringValue:` and the text colour are not `UIView`/`NSView`-level
/// calls), the cross-platform [`NativeChild`] everywhere else.
#[cfg(target_os = "ios")]
type LabelHandle = objc2::rc::Retained<objc2_ui_kit::UILabel>;
/// See the iOS arm's alias of the same name.
#[cfg(target_os = "macos")]
type LabelHandle = objc2::rc::Retained<objc2_app_kit::NSTextField>;
/// See the iOS arm's alias of the same name.
#[cfg(not(any(target_os = "ios", target_os = "macos")))]
type LabelHandle = NativeChild;

/// A button's handle — the same split, for the same reason, as [`LabelHandle`]
/// (`setTitle:forState:` is `UIButton`'s own, `setTitle:` `NSButton`'s).
#[cfg(target_os = "ios")]
type ButtonHandle = objc2::rc::Retained<objc2_ui_kit::UIButton>;
/// See the iOS arm's alias of the same name.
#[cfg(target_os = "macos")]
type ButtonHandle = objc2::rc::Retained<objc2_app_kit::NSButton>;
/// See the iOS arm's alias of the same name.
#[cfg(not(any(target_os = "ios", target_os = "macos")))]
type ButtonHandle = NativeChild;

/// Register every component this module ships with the calling thread's
/// runtime — call once, from app init, on the platform main thread.
///
/// Returns whether [`DemoCard`] was newly registered under [`DEMO_CARD_KIND`];
/// registration is **first-wins** ([`register_component`]), so a second call
/// answers `false` and logs a warning rather than replacing anything. Guard it
/// with a flag of your own if your call site runs every rebuild.
pub fn register_demo_components() -> bool {
    register_component::<DemoCard>(DEMO_CARD_KIND)
}

// --- Android -----------------------------------------------------------------

#[cfg(target_os = "android")]
mod platform {
    //! The Android arm: a vertical `LinearLayout` with a `TextView` and two
    //! `Button`s under it.
    //!
    //! Every JNI call here goes through this module's own [`guard_jni`],
    //! because [`ComponentCtx`]'s `env()` escape hatch is what a component
    //! author actually has, and its doc is explicit that such a caller must
    //! check and clear its own pending Java exception (leaving one pending is
    //! undefined behaviour for the next JNI call). `guard_jni` itself
    //! delegates to the crate's own `crate::android::run_jni`
    //! (`android/ctx.rs`) rather than re-implementing a bare check-and-clear:
    //! that helper extracts the pending exception's **class and message**
    //! before clearing it, so a demo failure names *what* threw, not just
    //! *that* something did.

    use jni::objects::{JObject, JValue};
    use jni::strings::JNIStr;
    use jni::{jni_sig, jni_str};

    use super::{
        BUTTON_HEIGHT, CARD_PADDING, DemoCard, DemoCardProps, DemoCardState, FRAME_CAPACITY,
        ROW_GAP, TITLE_HEIGHT,
    };
    use crate::component::{ComponentCtx, ListenerKinds, NativeComponent, NativeEvent, NativeRoot};

    /// The framework classes this card builds — never a support/material one:
    /// this plugin never assumes an app dependency it did not ship
    /// (`crate::controls::button`'s same rule).
    const LINEAR_LAYOUT: &str = "android.widget.LinearLayout";
    /// See [`LINEAR_LAYOUT`].
    const TEXT_VIEW: &str = "android.widget.TextView";
    /// See [`LINEAR_LAYOUT`].
    const BUTTON: &str = "android.widget.Button";

    /// `LinearLayout.VERTICAL` — a platform constant with no binding in this
    /// crate's hand-curated JNI surface, so it is named here rather than left
    /// as a bare `1`.
    const LINEAR_LAYOUT_VERTICAL: i32 = 1;

    impl NativeComponent for DemoCard {
        type Props = DemoCardProps;
        type State = DemoCardState;

        fn create(
            &self,
            ctx: &mut ComponentCtx<'_, '_, '_>,
            props: &Self::Props,
        ) -> Option<(NativeRoot, Self::State)> {
            // ONE local frame around the whole subtree build (the module doc's
            // discipline): every view and string below is a local reference,
            // and only the global ones this returns outlive the frame.
            ctx.with_local_frame(FRAME_CAPACITY, |ctx| {
                let parent = ctx.new_view(LINEAR_LAYOUT)?;
                call_int(
                    ctx,
                    &parent,
                    jni_str!("setOrientation"),
                    LINEAR_LAYOUT_VERTICAL,
                )?;
                call_int(
                    ctx,
                    &parent,
                    jni_str!("setBackgroundColor"),
                    props.background_argb,
                )?;
                set_padding(ctx, &parent)?;

                let label = ctx.new_view(TEXT_VIEW)?;
                set_text(ctx, &label, &props.title)?;
                call_int(ctx, &label, jni_str!("setTextColor"), props.title_argb)?;
                call_int(ctx, &label, jni_str!("setMinHeight"), px(TITLE_HEIGHT))?;
                ctx.add_child(&parent, &label)?;

                let mut buttons = Vec::with_capacity(2);
                let mut listener = None;
                for (row, caption) in [props.primary_caption(), props.secondary.clone()]
                    .into_iter()
                    .enumerate()
                {
                    let button = ctx.new_view(BUTTON)?;
                    set_text(ctx, &button, &caption)?;
                    call_int(ctx, &button, jni_str!("setMinHeight"), px(BUTTON_HEIGHT))?;
                    ctx.add_child(&parent, &button)?;
                    // The primary button's click listener (module doc's *Event
                    // wiring*) — attached while its local reference is live.
                    if row == 0 {
                        listener = Some(ctx.attach_listener(&button, ListenerKinds::CLICK)?);
                    }
                    buttons.push(ctx.retain_child(&button)?);
                }
                let [primary, secondary] = <[_; 2]>::try_from(buttons).ok()?;
                let listener = listener?;

                // Deliberate double retain: `retain_child(&parent)`
                // below and `ctx.root(&parent)` two lines down each allocate
                // their own JNI global ref to the SAME Java object, so this
                // one Java view ends up with two live global refs. Not a
                // leak — `live_child_count() == 0` after teardown (this
                // module's own host test) proves both are released. Avoiding
                // it would mean either widening `NativeComponent::update`'s
                // signature to hand it the `NativeRoot` the runtime already
                // owns (a trait change), or giving `state.root` shared
                // ownership of the SAME global ref the runtime pair-deletes
                // on dispose — which breaks the one-owner, paired-delete
                // discipline every other retained handle in this crate
                // follows. Two extra refs per composite against ART's 51,200
                // live-global-ref cap is not a constraint worth either
                // tradeoff.
                let root = ctx.retain_child(&parent)?;
                let label = ctx.retain_child(&label)?;
                Some((
                    ctx.root(&parent)?,
                    DemoCardState {
                        root,
                        label,
                        buttons: [primary, secondary],
                        listener,
                    },
                ))
            })
        }

        fn update(
            &self,
            ctx: &mut ComponentCtx<'_, '_, '_>,
            state: &mut Self::State,
            old: &Self::Props,
            new: &Self::Props,
        ) {
            // Field-level diffing inside a changed props value is the
            // component's own job (`crate::component`'s lifecycle contract,
            // point 3) — only it knows which setter is cheap.
            if old.background_argb != new.background_argb {
                call_int(
                    ctx,
                    state.root.as_object(),
                    jni_str!("setBackgroundColor"),
                    new.background_argb,
                );
            }
            if old.title != new.title {
                set_text(ctx, state.label.as_object(), &new.title);
            }
            if old.title_argb != new.title_argb {
                call_int(
                    ctx,
                    state.label.as_object(),
                    jni_str!("setTextColor"),
                    new.title_argb,
                );
            }
            let captions = [
                (old.primary_caption(), new.primary_caption()),
                (old.secondary.clone(), new.secondary.clone()),
            ];
            for (button, (was, now)) in state.buttons.iter().zip(captions) {
                if was != now {
                    set_text(ctx, button.as_object(), &now);
                }
            }
        }

        fn on_event(
            &self,
            _state: &mut Self::State,
            _props: &Self::Props,
            event: NativeEvent,
        ) -> Option<NativeEvent> {
            // The primary button's click is the one event this card attached
            // for (module doc's *Event wiring*); forward it to the app's hook.
            event.is_click().then_some(event)
        }

        fn dispose(&self, ctx: &mut ComponentCtx<'_, '_, '_>, state: Self::State) {
            // `setOnClickListener(null)` first, exactly as the built-in
            // `Button`'s own teardown does. Dropping the rest of `state` as
            // this returns releases the parent's and the three children's
            // global references; the runtime pair-deletes the `NativeRoot`'s
            // immediately afterwards.
            let DemoCardState {
                listener, buttons, ..
            } = state;
            ctx.detach_listener(listener);
            log::debug!(
                "frust-native-widgets demo: card disposed, releasing {} retained handles",
                buttons.len() + 2
            );
        }
    }

    /// `view.setPadding(l, t, r, b)` — the card's inner inset, so the Android
    /// arm reads like the iOS arm's explicitly framed children.
    fn set_padding(ctx: &mut ComponentCtx<'_, '_, '_>, view: &JObject<'_>) -> Option<()> {
        let side = px(CARD_PADDING);
        let vertical = px(ROW_GAP);
        guard_jni(ctx, "setPadding", |env| {
            env.call_method(
                view,
                jni_str!("setPadding"),
                jni_sig!("(IIII)V"),
                &[
                    JValue::Int(side),
                    JValue::Int(vertical),
                    JValue::Int(side),
                    JValue::Int(vertical),
                ],
            )?
            .v()
        })
    }

    /// `view.setText(text)` — `TextView` and `Button` (a `TextView` subclass)
    /// share the one signature.
    fn set_text(ctx: &mut ComponentCtx<'_, '_, '_>, view: &JObject<'_>, text: &str) -> Option<()> {
        guard_jni(ctx, "setText", |env| {
            let value = env.new_string(text)?;
            env.call_method(
                view,
                jni_str!("setText"),
                jni_sig!("(Ljava/lang/CharSequence;)V"),
                &[JValue::Object(&value)],
            )?
            .v()
        })
    }

    /// A one-`int`-argument `void` setter — `setOrientation`,
    /// `setBackgroundColor`, `setTextColor`, `setMinHeight`.
    fn call_int(
        ctx: &mut ComponentCtx<'_, '_, '_>,
        view: &JObject<'_>,
        name: &JNIStr,
        value: i32,
    ) -> Option<()> {
        guard_jni(ctx, "int setter", |env| {
            env.call_method(view, name, jni_sig!("(I)V"), &[JValue::Int(value)])?
                .v()
        })
    }

    /// Run `f` against the live `Env` and settle its outcome: a thrown Java
    /// exception is **checked and cleared here** and latched on `ctx`, so the
    /// runtime fails the slot rather than building on a half-made view.
    ///
    /// Delegates to `crate::android::run_jni` (a re-export of
    /// `android/ctx.rs`'s already-`pub(crate)` helper of the same name,
    /// exposed here so a module outside `android`'s private `ctx`
    /// submodule can reach it) rather than a hand-rolled check-and-clear:
    /// that helper's `take_pending_exception`
    /// extracts the throwable's class name and message before clearing it,
    /// so `ctx.report_error` now reports e.g. `"native-widgets platform
    /// error: android native-widgets: setPadding:
    /// java.lang.NullPointerException: <message>"` instead of the old,
    /// silent `"frust-native-widgets demo: setPadding threw (exception
    /// cleared)"`.
    fn guard_jni<T>(
        ctx: &mut ComponentCtx<'_, '_, '_>,
        op: &str,
        f: impl FnOnce(&mut jni::Env<'_>) -> Result<T, jni::errors::Error>,
    ) -> Option<T> {
        match crate::android::run_jni(ctx.env(), op, f) {
            Ok(value) => Some(value),
            Err(error) => {
                ctx.report_error(format!("frust-native-widgets demo: {error}"));
                None
            }
        }
    }

    /// A logical-px constant as the `int` pixel count Android's own setters
    /// take.
    ///
    /// A production control would scale by `DisplayMetrics.density`; this card
    /// is a demo whose only geometric contract is *"fit inside the slot the
    /// caller declared"*, and these values are minimum heights the platform is
    /// free to exceed — so the 1:1 read keeps the two arms comparable rather
    /// than adding a density query only one of them makes.
    fn px(value: f64) -> i32 {
        value as i32
    }
}

// --- iOS ---------------------------------------------------------------------

#[cfg(target_os = "ios")]
mod platform {
    //! The Apple arm: a plain `UIView` with a `UILabel` and two `UIButton`s at
    //! explicit frames — the first caller `ComponentCtx::add_child`'s
    //! `addSubview` path has ever had.
    //!
    //! No `UIStackView`, no constraints: the host positions a slot's content
    //! view by assigning `frame` (`crate::apple::ctx`'s *Frame-setting layout
    //! only*), and a subtree that installed constraints would fight it.

    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_foundation::NSString;
    use objc2_ui_kit::{UIButton, UIButtonType, UIControlState, UILabel, UIView};

    use super::{
        BUTTON_HEIGHT, CARD_PADDING, DEMO_CARD_HEIGHT, DEMO_CARD_WIDTH, DemoCard, DemoCardProps,
        DemoCardState, FRAME_CAPACITY, ROW_GAP, TITLE_HEIGHT,
    };
    use crate::component::{ComponentCtx, ListenerKinds, NativeComponent, NativeEvent, NativeRoot};
    use crate::controls::platform::{set_background_color, set_label_text_color, ui_color};

    impl NativeComponent for DemoCard {
        type Props = DemoCardProps;
        type State = DemoCardState;

        fn create(
            &self,
            ctx: &mut ComponentCtx<'_, '_, '_>,
            props: &Self::Props,
        ) -> Option<(NativeRoot, Self::State)> {
            // The frame wrapper does nothing under ARC (`crate::apple::ctx`);
            // it is kept so this arm reads exactly like the Android one.
            ctx.with_local_frame(FRAME_CAPACITY, |ctx| {
                let mtm = ctx.mtm();
                let parent = UIView::new(mtm);
                parent.setFrame(CGRect::new(
                    CGPoint::ZERO,
                    CGSize::new(DEMO_CARD_WIDTH, DEMO_CARD_HEIGHT),
                ));
                set_background_color(&parent, props.background_argb);

                let label = UILabel::new(mtm);
                label.setText(Some(&NSString::from_str(&props.title)));
                set_label_text_color(&label, &ui_color(props.title_argb));
                label.setFrame(row_frame(0));
                ctx.add_child(&parent, &label)?;

                let mut buttons = Vec::with_capacity(2);
                for (row, caption) in [props.primary_caption(), props.secondary.clone()]
                    .into_iter()
                    .enumerate()
                {
                    let button = UIButton::buttonWithType(UIButtonType::System, mtm);
                    button.setTitle_forState(
                        Some(&NSString::from_str(&caption)),
                        UIControlState::Normal,
                    );
                    button.setFrame(row_frame(row + 1));
                    ctx.add_child(&parent, &button)?;
                    buttons.push(button);
                }
                let [primary, secondary] = <[_; 2]>::try_from(buttons).ok()?;
                // The primary button's click target (module doc's *Event
                // wiring*): `TouchUpInside`, retained by the handle, since
                // UIKit holds a control's targets weakly.
                let listener = ctx.attach_listener(&primary, ListenerKinds::CLICK)?;

                // The parent is the one handle kept in the cross-platform
                // shape: it only ever needs `UIView`-level setters, where the
                // three children need their own classes' (module doc).
                let root = ctx.retain_child(&parent)?;
                Some((
                    ctx.root(parent)?,
                    DemoCardState {
                        root,
                        label,
                        buttons: [primary, secondary],
                        listener,
                    },
                ))
            })
        }

        fn update(
            &self,
            ctx: &mut ComponentCtx<'_, '_, '_>,
            state: &mut Self::State,
            old: &Self::Props,
            new: &Self::Props,
        ) {
            let mtm = ctx.mtm();
            if old.background_argb != new.background_argb {
                set_background_color(state.root.view(mtm), new.background_argb);
            }
            if old.title != new.title {
                state.label.setText(Some(&NSString::from_str(&new.title)));
            }
            if old.title_argb != new.title_argb {
                set_label_text_color(&state.label, &ui_color(new.title_argb));
            }
            let captions = [
                (old.primary_caption(), new.primary_caption()),
                (old.secondary.clone(), new.secondary.clone()),
            ];
            for (button, (was, now)) in state.buttons.iter().zip(captions) {
                if was != now {
                    button
                        .setTitle_forState(Some(&NSString::from_str(&now)), UIControlState::Normal);
                }
            }
        }

        fn on_event(
            &self,
            _state: &mut Self::State,
            _props: &Self::Props,
            event: NativeEvent,
        ) -> Option<NativeEvent> {
            // See the Android arm: the primary's click, forwarded.
            event.is_click().then_some(event)
        }

        fn dispose(&self, ctx: &mut ComponentCtx<'_, '_, '_>, state: Self::State) {
            // The primary's target-action pair is removed first (the handle's
            // detach — the built-in `Button`'s own teardown order); ARC then
            // releases the parent's `NativeChild` and the three typed children
            // as the rest of `state` drops on return, and the runtime releases
            // the `NativeRoot` immediately afterwards.
            let DemoCardState {
                listener, buttons, ..
            } = state;
            ctx.detach_listener(listener);
            log::debug!(
                "frust-native-widgets demo: card disposed, releasing {} retained handles",
                buttons.len() + 2
            );
        }
    }

    /// The frame of stacked row `row` (0 = the title, 1/2 = the buttons) inside
    /// the card's declared size — the whole of this arm's layout.
    fn row_frame(row: usize) -> CGRect {
        let width = DEMO_CARD_WIDTH - 2.0 * CARD_PADDING;
        let (y, height) = match row {
            0 => (CARD_PADDING, TITLE_HEIGHT),
            n => (
                CARD_PADDING
                    + TITLE_HEIGHT
                    + (n as f64) * ROW_GAP
                    + ((n - 1) as f64) * BUTTON_HEIGHT,
                BUTTON_HEIGHT,
            ),
        };
        CGRect::new(CGPoint::new(CARD_PADDING, y), CGSize::new(width, height))
    }
}

// --- macOS -------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod platform {
    //! The AppKit arm: a plain layer-backed `NSView` with an `NSTextField`
    //! title and two `NSButton`s at explicit frames — the iOS arm's card,
    //! control for control, built through the same [`ComponentCtx`] calls.
    //!
    //! No stack view, no constraints: the desktop host positions a slot's view
    //! by assigning `frame` (`crate::appkit::ctx`'s *Frame-setting layout
    //! only*), and a subtree that installed constraints would fight it.
    //!
    //! # Bottom-left origin, pinned to the top
    //!
    //! A plain `NSView` is **not** flipped (its origin is bottom-left, where
    //! `UIView`'s is top-left), and flipping it would mean a subclass — a new
    //! class this arm deliberately does not add. So [`row_frame`] computes the
    //! iOS arm's top-down rows and converts each to AppKit's bottom-up `y`
    //! inside [`DEMO_CARD_HEIGHT`], and every child carries a flexible
    //! *bottom* margin (`NSViewMinYMargin`, a springs-and-struts autoresizing
    //! mask — frame arithmetic AppKit does on resize, not an Auto Layout
    //! constraint), so a host `setFrame:` that changes the card's height (a
    //! clip shrinking the frame) keeps the rows anchored to the top edge, as
    //! the iOS arm's top-left frames are by construction.
    //!
    //! # Colours ride the controls' own shared setters
    //!
    //! The background and the title's ink go through
    //! `crate::controls::platform`'s macOS `set_background_color`/
    //! `set_text_color` — the same theme-ladder L2 setters the six controls'
    //! own macOS arms call, a real `CALayer.backgroundColor`/`NSTextField.
    //! textColor` write, not AppKit's stock colours — so the card re-themes
    //! live exactly as they do. The parent is made layer-backed here so that
    //! L2 write has a layer to land on.

    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSBezelStyle, NSButton, NSButtonType, NSTextField, NSView,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

    use super::{
        BUTTON_HEIGHT, CARD_PADDING, DEMO_CARD_HEIGHT, DEMO_CARD_WIDTH, DemoCard, DemoCardProps,
        DemoCardState, FRAME_CAPACITY, ROW_GAP, TITLE_HEIGHT,
    };
    use crate::component::{ComponentCtx, ListenerKinds, NativeComponent, NativeEvent, NativeRoot};
    use crate::controls::platform::{set_background_color, set_text_color};

    impl NativeComponent for DemoCard {
        type Props = DemoCardProps;
        type State = DemoCardState;

        fn create(
            &self,
            ctx: &mut ComponentCtx<'_, '_, '_>,
            props: &Self::Props,
        ) -> Option<(NativeRoot, Self::State)> {
            // The frame wrapper does nothing under ARC (`crate::appkit::ctx`);
            // it is kept so this arm reads exactly like the other two.
            ctx.with_local_frame(FRAME_CAPACITY, |ctx| {
                let mtm = ctx.mtm();
                let parent = NSView::new(mtm);
                parent.setFrame(NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(DEMO_CARD_WIDTH, DEMO_CARD_HEIGHT),
                ));
                parent.setWantsLayer(true);
                set_background_color(&parent, props.background_argb);

                let label = NSTextField::labelWithString(&NSString::from_str(&props.title), mtm);
                set_text_color(&label, props.title_argb);
                place(&label, 0);
                ctx.add_child(&parent, &label)?;

                let mut buttons = Vec::with_capacity(2);
                for (row, caption) in [props.primary_caption(), props.secondary.clone()]
                    .into_iter()
                    .enumerate()
                {
                    // `crate::controls::button`'s macOS construction: a
                    // momentary push button with the standard push bezel.
                    let button = NSButton::new(mtm);
                    button.setButtonType(NSButtonType::MomentaryPushIn);
                    button.setBezelStyle(NSBezelStyle::Push);
                    button.setTitle(&NSString::from_str(&caption));
                    place(&button, row + 1);
                    ctx.add_child(&parent, &button)?;
                    buttons.push(button);
                }
                let [primary, secondary] = <[_; 2]>::try_from(buttons).ok()?;
                // The primary button's click target (module doc's *Event
                // wiring*): the `NSButton`'s one target/action pair, retained
                // by the handle, since `NSControl.target` is weak.
                let listener = ctx.attach_listener(&primary, ListenerKinds::CLICK)?;

                // The parent is the one handle kept in the cross-platform
                // shape: it only ever needs `NSView`-level setters, where the
                // three children need their own classes' (module doc).
                let root = ctx.retain_child(&parent)?;
                Some((
                    ctx.root(parent)?,
                    DemoCardState {
                        root,
                        label,
                        buttons: [primary, secondary],
                        listener,
                    },
                ))
            })
        }

        fn update(
            &self,
            ctx: &mut ComponentCtx<'_, '_, '_>,
            state: &mut Self::State,
            old: &Self::Props,
            new: &Self::Props,
        ) {
            let mtm = ctx.mtm();
            if old.background_argb != new.background_argb {
                set_background_color(state.root.view(mtm), new.background_argb);
            }
            if old.title != new.title {
                state.label.setStringValue(&NSString::from_str(&new.title));
            }
            if old.title_argb != new.title_argb {
                set_text_color(&state.label, new.title_argb);
            }
            let captions = [
                (old.primary_caption(), new.primary_caption()),
                (old.secondary.clone(), new.secondary.clone()),
            ];
            for (button, (was, now)) in state.buttons.iter().zip(captions) {
                if was != now {
                    button.setTitle(&NSString::from_str(&now));
                }
            }
        }

        fn on_event(
            &self,
            _state: &mut Self::State,
            _props: &Self::Props,
            event: NativeEvent,
        ) -> Option<NativeEvent> {
            // See the Android arm: the primary's click, forwarded.
            event.is_click().then_some(event)
        }

        fn dispose(&self, ctx: &mut ComponentCtx<'_, '_, '_>, state: Self::State) {
            // The primary's target/action is cleared first (the handle's
            // detach — the built-in `Button`'s own teardown order); ARC then
            // releases the parent's `NativeChild` and the three typed children
            // as the rest of `state` drops on return, and the runtime releases
            // the `NativeRoot` immediately afterwards.
            let DemoCardState {
                listener, buttons, ..
            } = state;
            ctx.detach_listener(listener);
            log::debug!(
                "frust-native-widgets demo: card disposed, releasing {} retained handles",
                buttons.len() + 2
            );
        }
    }

    /// Give `view` stacked row `row`'s frame and pin it to the card's top
    /// edge (module doc's *Bottom-left origin, pinned to the top*).
    fn place(view: &NSView, row: usize) {
        view.setFrame(row_frame(row));
        view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
    }

    /// The frame of stacked row `row` (0 = the title, 1/2 = the buttons) inside
    /// the card's declared size — the iOS arm's top-down rows, converted to
    /// AppKit's bottom-left origin. The whole of this arm's layout.
    fn row_frame(row: usize) -> NSRect {
        let width = DEMO_CARD_WIDTH - 2.0 * CARD_PADDING;
        let (top, height) = match row {
            0 => (CARD_PADDING, TITLE_HEIGHT),
            n => (
                CARD_PADDING
                    + TITLE_HEIGHT
                    + (n as f64) * ROW_GAP
                    + ((n - 1) as f64) * BUTTON_HEIGHT,
                BUTTON_HEIGHT,
            ),
        };
        let y = DEMO_CARD_HEIGHT - top - height;
        NSRect::new(NSPoint::new(CARD_PADDING, y), NSSize::new(width, height))
    }
}

// --- the no-platform host stand-in -------------------------------------------

#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
mod platform {
    //! The host arm — Linux/Windows/web, the targets with no native-widgets
    //! platform arm (macOS has the real AppKit arm above): the same
    //! create/update/dispose *plan*, recorded rather than executed, so an
    //! ordinary `cargo test` on a machine with no JNI and no Objective-C
    //! runtime can assert both the plan and the paired release
    //! (`crate::component`'s host `ComponentCtx`).

    use super::{DemoCard, DemoCardProps, DemoCardState, FRAME_CAPACITY};
    use crate::component::{ComponentCtx, ListenerKinds, NativeComponent, NativeEvent, NativeRoot};

    impl NativeComponent for DemoCard {
        type Props = DemoCardProps;
        type State = DemoCardState;

        fn create(
            &self,
            ctx: &mut ComponentCtx<'_, '_, '_>,
            props: &Self::Props,
        ) -> Option<(NativeRoot, Self::State)> {
            ctx.with_local_frame(FRAME_CAPACITY, |ctx| {
                let parent = next_identity();
                ctx.record(format!("card parent {parent} bg {}", props.background_argb));

                let label = next_identity();
                ctx.record(format!(
                    "card title {label} '{}' ink {}",
                    props.title, props.title_argb
                ));
                ctx.add_child(parent, label)?;

                let mut buttons = Vec::with_capacity(2);
                for caption in [props.primary_caption(), props.secondary.clone()] {
                    let button = next_identity();
                    ctx.record(format!("card button {button} '{caption}'"));
                    ctx.add_child(parent, button)?;
                    buttons.push(ctx.retain_child(button)?);
                }
                let [primary, secondary] = <[_; 2]>::try_from(buttons).ok()?;
                let listener = ctx.attach_listener(primary.identity(), ListenerKinds::CLICK)?;

                let root = ctx.retain_child(parent)?;
                let label = ctx.retain_child(label)?;
                Some((
                    ctx.root(parent)?,
                    DemoCardState {
                        root,
                        label,
                        buttons: [primary, secondary],
                        listener,
                    },
                ))
            })
        }

        fn update(
            &self,
            ctx: &mut ComponentCtx<'_, '_, '_>,
            state: &mut Self::State,
            old: &Self::Props,
            new: &Self::Props,
        ) {
            if old.background_argb != new.background_argb {
                ctx.record(format!(
                    "card bg {} -> {}",
                    state.root.identity(),
                    new.background_argb
                ));
            }
            if old.title != new.title {
                ctx.record(format!(
                    "card title {} -> '{}'",
                    state.label.identity(),
                    new.title
                ));
            }
            if old.title_argb != new.title_argb {
                ctx.record(format!(
                    "card ink {} -> {}",
                    state.label.identity(),
                    new.title_argb
                ));
            }
            let captions = [
                (old.primary_caption(), new.primary_caption()),
                (old.secondary.clone(), new.secondary.clone()),
            ];
            for (button, (was, now)) in state.buttons.iter().zip(captions) {
                if was != now {
                    ctx.record(format!("card caption {} -> '{now}'", button.identity()));
                }
            }
        }

        fn on_event(
            &self,
            _state: &mut Self::State,
            _props: &Self::Props,
            event: NativeEvent,
        ) -> Option<NativeEvent> {
            event.is_click().then_some(event)
        }

        fn dispose(&self, ctx: &mut ComponentCtx<'_, '_, '_>, state: Self::State) {
            let DemoCardState {
                root,
                buttons,
                listener,
                ..
            } = state;
            ctx.detach_listener(listener);
            ctx.record(format!(
                "card dispose {} with {} retained handles",
                root.identity(),
                buttons.len() + 2
            ));
        }
    }

    /// Monotonic stand-in for *"the platform handed us a fresh view"* — the
    /// host arm's only invention, mirroring `crate::component`'s own test
    /// helper of the same name.
    fn next_identity() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }
}

// Gated on the host arm, not merely on `test` (the gate every other test module
// in this crate carries): these drive the card through the real runtime using
// the host stand-in context, which a platform build — macOS included, since it
// has the real AppKit arm — replaces with the platform-only types. So on a Mac
// they are skipped, and they run on Linux/Windows hosts exactly as before; the
// platform-neutral size check lives in `layout_tests` below and runs everywhere.
#[cfg(all(
    test,
    not(any(target_os = "android", target_os = "ios", target_os = "macos"))
))]
mod tests {
    use std::rc::Rc;

    use super::*;
    use crate::component::{
        component_params, forget, live_child_count, live_listener_count, publish, staged_count,
    };
    use crate::registry::SlotId;
    use crate::runtime::{DisposeOutcome, NativeCtx as PlatformCtx, UpdateOutcome, with_runtime};

    /// Every handle [`DemoCardState`] retains on the host arm: the parent, the
    /// title, and the two buttons.
    const HANDLES: usize = DEMO_CARD_CHILDREN + 1;

    fn props(title: &str, primary: &str) -> DemoCardProps {
        DemoCardProps {
            title: title.to_string(),
            primary: primary.to_string(),
            secondary: "Dismiss".to_string(),
            background_argb: 0x1122_3344,
            title_argb: -1,
            presses: 0,
        }
    }

    /// Publish `props` for `slot` and return the `params_json` its
    /// `platform_view` would carry — exactly what
    /// `crate::api::native_component` runs on every rebuild. Brightness is
    /// out of scope for this module's tests (`crate::api::mount`'s and
    /// `crate::component`'s own tests cover the `dark` bit), so it is pinned
    /// to `false`.
    fn mount(slot: SlotId, props: DemoCardProps) -> String {
        let generation = publish(slot, Rc::new(DemoCard), props);
        component_params(DEMO_CARD_KIND, slot, generation, false)
    }

    #[test]
    fn the_demo_composite_ships_one_slot_and_releases_every_child() {
        // The reason this test exists at all: this
        // feature has already shipped two leaks that looked fine until
        // something counted.
        assert_eq!(live_child_count(), 0, "this test thread starts clean");
        assert!(register_demo_components());

        let params = mount(70, props("Now playing", "Play"));
        let mut calls = Vec::new();
        {
            let mut ctx = PlatformCtx::new(&mut calls);
            with_runtime(|runtime| {
                assert_eq!(runtime.create(&mut ctx, &params).unwrap(), 70);
                assert_eq!(
                    runtime.live_count(),
                    1,
                    "a parent and {DEMO_CARD_CHILDREN} children ship as ONE frust slot — \
                     that is the whole point of the composite"
                );
                assert_eq!(
                    live_child_count(),
                    HANDLES,
                    "every handle is retained while the card is live"
                );

                // A props change drives the live subtree through the ordinary
                // diff gate — one `update`, no remount…
                let changed = mount(70, props("Up next", "Play"));
                assert_eq!(
                    runtime.update_params(&mut ctx, &changed).unwrap(),
                    UpdateOutcome::Applied
                );
                // …and an unchanged rebuild crosses nothing at all.
                let same = mount(70, props("Up next", "Play"));
                assert_eq!(
                    runtime.update_params(&mut ctx, &same).unwrap(),
                    UpdateOutcome::Unchanged
                );

                assert_eq!(runtime.dispose_slot(&mut ctx, 70), DisposeOutcome::Disposed);
                assert_eq!(runtime.live_count(), 0);
            })
            .expect("the thread's runtime");
        }
        forget(70);

        // The teardown bar, counted rather than assumed.
        assert_eq!(
            live_child_count(),
            0,
            "every retained handle was released with the card"
        );
        assert_eq!(
            live_listener_count(),
            0,
            "the primary's click listener was released with the card"
        );
        assert_eq!(staged_count(), 0);

        // …and the plan the card actually executed.
        assert_eq!(calls[0], format!("pushLocalFrame {FRAME_CAPACITY}"));
        let parent = calls[1]
            .split(' ')
            .nth(2)
            .expect("the parent identity")
            .to_string();
        let attached: Vec<&String> = calls
            .iter()
            .filter(|call| call.starts_with("addChild "))
            .collect();
        assert_eq!(
            attached.len(),
            DEMO_CARD_CHILDREN,
            "one addChild per child: {calls:?}"
        );
        assert!(
            attached
                .iter()
                .all(|call| call.starts_with(&format!("addChild {parent} <- "))),
            "every child went under the card's own parent: {attached:?}"
        );
        assert_eq!(
            calls
                .iter()
                .filter(|call| call.starts_with("retainChild "))
                .count(),
            HANDLES
        );

        let last_add = calls
            .iter()
            .rposition(|call| call.starts_with("addChild "))
            .expect("an addChild");
        let popped = calls
            .iter()
            .position(|call| call == "popLocalFrame")
            .expect("the frame pops");
        assert!(
            last_add < popped,
            "the whole subtree build ran inside ONE local frame: {calls:?}"
        );
        assert!(
            calls.iter().any(|call| call.contains("-> 'Up next'")),
            "the changed title reached the retained label: {calls:?}"
        );
        assert!(
            calls.last().unwrap().starts_with("card dispose"),
            "{calls:?}"
        );
    }

    #[test]
    fn back_to_back_card_cycles_strand_nothing() {
        // The churn shape the gate harness's own cycler exercises on device:
        // two independent mount/dispose cycles must each return the thread's
        // handle count to zero, so nothing from the first lingers into the
        // second.
        assert_eq!(live_child_count(), 0);
        register_demo_components();

        for slot in [71, 72] {
            let params = mount(slot, props("Cycle", "Go"));
            let mut calls = Vec::new();
            let mut ctx = PlatformCtx::new(&mut calls);
            with_runtime(|runtime| {
                runtime.create(&mut ctx, &params).unwrap();
                assert_eq!(runtime.live_count(), 1, "one slot per card, every cycle");
                assert_eq!(live_child_count(), HANDLES);
                assert_eq!(
                    runtime.dispose_slot(&mut ctx, slot),
                    DisposeOutcome::Disposed
                );
            })
            .expect("the thread's runtime");
            forget(slot);
            assert_eq!(live_child_count(), 0, "cycle {slot} released everything");
            assert_eq!(
                live_listener_count(),
                0,
                "cycle {slot} released its listener"
            );
        }
    }

    #[test]
    fn a_primary_click_reaches_the_hook_and_the_count_moves_the_caption() {
        // The card's whole event loop, below the frust rebuild: the primary
        // button's attached listener fires → the card forwards the click →
        // the slot callback (what `.on_event` registers) hears it → the app's
        // next rebuild hands the count back → the primary caption moves.
        use std::sync::{Arc, Mutex};

        use crate::events::EventPayload;
        use crate::runtime::NativeEvent as WireEvent;

        register_demo_components();
        let heard: Arc<Mutex<Vec<EventPayload>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&heard);

        let params = mount(73, props("Tap", "Play"));
        let mut calls = Vec::new();
        {
            let mut ctx = PlatformCtx::new(&mut calls);
            with_runtime(|runtime| {
                runtime.set_callback(
                    73,
                    Arc::new(move |payload| recorder.lock().unwrap().push(payload)),
                );
                runtime.create(&mut ctx, &params).unwrap();
                assert_eq!(live_listener_count(), 1, "the primary listens");

                runtime.on_event(73, WireEvent { kind: 1, detail: 0 });

                let counted = mount(
                    73,
                    DemoCardProps {
                        presses: 1,
                        ..props("Tap", "Play")
                    },
                );
                assert_eq!(
                    runtime.update_params(&mut ctx, &counted).unwrap(),
                    UpdateOutcome::Applied
                );
                assert_eq!(runtime.dispose_slot(&mut ctx, 73), DisposeOutcome::Disposed);
            })
            .expect("the thread's runtime");
        }
        forget(73);

        assert_eq!(*heard.lock().unwrap(), vec![EventPayload::Click]);
        let primary = calls
            .iter()
            .find(|call| call.starts_with("card button ") && call.ends_with("'Play (0)'"))
            .and_then(|call| call.split(' ').nth(2))
            .expect("the primary button, captioned with its count")
            .to_string();
        assert!(
            calls.contains(&format!("attachListener {primary} click")),
            "the click listener went on the PRIMARY button: {calls:?}"
        );
        assert!(
            calls.contains(&format!("card caption {primary} -> 'Play (1)'")),
            "the new count reached the primary's caption: {calls:?}"
        );
        assert!(
            calls.contains(&format!("detachListener {primary} click")),
            "dispose detached it: {calls:?}"
        );
        assert_eq!(live_listener_count(), 0);
    }
}

// Platform-neutral: this checks only the Rust-side geometry constants every
// arm lays out against, so it runs on every host — macOS included, where the
// runtime-driving `tests` above are skipped.
#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn the_card_declares_a_slot_size_its_own_rows_fit_inside() {
        // The Apple arms lay their children out at explicit frames, so a caller
        // mounting the slot at the published size must actually have room for
        // them — the checkable half of "look at the phone".
        let stacked = CARD_PADDING
            + TITLE_HEIGHT
            + ROW_GAP
            + BUTTON_HEIGHT
            + ROW_GAP
            + BUTTON_HEIGHT
            + CARD_PADDING;
        assert!(
            stacked <= DEMO_CARD_HEIGHT,
            "the card's own rows ({stacked}) must fit inside DEMO_CARD_HEIGHT"
        );
        let content_width = DEMO_CARD_WIDTH - 2.0 * CARD_PADDING;
        assert!(
            content_width > 0.0,
            "the padded content width ({content_width}) must be positive"
        );
    }
}
