//! `Image` — a real `android.widget.ImageView`, built and driven from Rust,
//! showing app-supplied encoded bytes (PNG/JPEG/WebP — whatever
//! `BitmapFactory` reads).
//!
//! Four properties: bytes, fit, tint, accessibility label. Display-only (no
//! events). Three of the four are cheap; the bytes are the set's only
//! [`Tier::Decode`](super::Tier::Decode) setter.
//!
//! # Bytes never travel through `params_json`
//!
//! Params are a flat JSON payload the differ copies per slot, per change —
//! putting an encoded image in one would mean base64-ing (and re-scanning) the
//! whole payload every rebuild. So the bytes take a side channel and the
//! params carry only a change token:
//!
//! 1. the api layer (p1-06) calls [`publish_bytes`] with the slot's
//!    `Arc<[u8]>` and gets a **revision** back — the same `Arc` published
//!    again returns the same revision, a different one bumps it;
//! 2. it writes that revision into the slot's params as [`REV`], which is what
//!    makes the params *string* change when (and only when) the bytes change,
//!    so the differ emits an `UpdateParams` at all;
//! 3. [`ImageProps::decode`] reads the bytes back out of the publish table.
//!
//! The publish table is the authority, not the params: a replayed `Create`
//! (surface recreation) carries a stale revision but must show the *current*
//! image, and reading the table gives exactly that.
//!
//! The payload is kept alive by a **hold** the control takes in `create`
//! ([`claim_bytes`]) and gives back in `dispose` ([`release_bytes`]) — a
//! counted pair that keeps a slot briefly holding two live instances (the
//! runtime creates a replacement before disposing what it replaced) from
//! having the older one delete the newer one's bytes.
//!
//! # The counted pair alone is NOT bounded (f2-03)
//!
//! An earlier version of this doc claimed the table was "bounded by the live
//! image slots." It is not, and the false claim is part of why the leak
//! shipped: paint culling can dispose a scrolled-off-screen slot
//! (`DISPOSE_AFTER_MISSING_FRAMES` in `frust-shell-common::platform_view`)
//! without ever running `View::teardown` on the still-mounted widget. The
//! next rebuild republishes into a fresh, zero-holder entry the counted pair
//! never released — so when `View::teardown` eventually does run (navigating
//! away), it retires a slot the host already disposed at the culling step,
//! and that `Dispose` command never reaches `release_bytes`. The entry then
//! leaks for the process lifetime; slot ids are never reused, so nothing ever
//! reclaims it.
//!
//! [`retire`] is the actual bound: registered via `on_cleanup` in
//! `NativeImageView`'s `Component::init` (`api::builders`,
//! `docs/CODE_STANDARDS.md`'s "Teardown disposes the component's `Owner`;
//! register cleanup via `on_cleanup`, not `Drop`"), it removes the slot's
//! entry unconditionally exactly once per mounted Component — independent of
//! however many times (if any) the native create/dispose pair actually ran in
//! between. The counted [`claim_bytes`]/[`release_bytes`] pair stays as an
//! optimization (it frees a payload the instant the last native holder goes
//! away in the ordinary case), not as the thing that bounds the table.
//!
//! # Decode once per bytes identity, never per rebuild
//!
//! [`ImageBytes`] compares by **identity** — publish revision plus
//! `Arc::ptr_eq` — never by content. An app handing the same `Arc` down every
//! rebuild therefore produces equal props, so the runtime's own diff gate
//! stops before `update`, and even a props change in another field plans no
//! [`Setter::ImageBytes`]. `BitmapFactory.decodeByteArray` runs once per
//! *actually new* payload.
//!
//! # Main-thread confinement
//!
//! The publish table is a `thread_local!`, like the runtime's own registry
//! (`crate::runtime`'s *main-thread confinement*): the api layer publishes
//! during a main-thread rebuild and the factory decodes during the
//! main-thread post-frame poll. A publish from any other thread is invisible
//! to the decode side by construction — which is the intended failure mode,
//! not a race.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;

use super::{CONTENT_DESCRIPTION, FIT, Plan, Setter, TINT, color, owned_text, slot_of};
use crate::NativeWidgetError;
use crate::registry::SlotId;
use crate::runtime::Params;

/// The registered kind string the api layer injects as `__frustControl`.
pub(crate) const KIND: &str = "image";

/// `"imageRev"` — the publish revision that makes a bytes change visible in
/// the params string (module doc). Never the bytes themselves.
pub(crate) const REV: &str = "imageRev";

/// The marker type registered under [`KIND`]; its
/// [`NativeWidget`](crate::runtime::NativeWidget) impl is the Android half
/// below.
pub(crate) struct Image;

/// How the image is scaled into the slot's box — an `ImageView.ScaleType`
/// subset chosen for the four behaviours apps actually ask for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Fit {
    /// Whole image, aspect kept, centred — `ScaleType.FIT_CENTER`. The
    /// platform's own default, and hence this enum's.
    #[default]
    Contain,
    /// Fills the box, aspect kept, cropped — `ScaleType.CENTER_CROP`.
    Cover,
    /// Fills the box, aspect ignored — `ScaleType.FIT_XY`.
    Fill,
    /// No scaling at all, centred — `ScaleType.CENTER`.
    Center,
}

impl Fit {
    /// The wire spelling the api layer writes into [`FIT`].
    fn from_params(raw: &str) -> Option<Self> {
        match raw {
            "contain" => Some(Self::Contain),
            "cover" => Some(Self::Cover),
            "fill" => Some(Self::Fill),
            "center" => Some(Self::Center),
            _ => None,
        }
    }

    /// The `ImageView$ScaleType` constant this fit selects.
    #[cfg(target_os = "android")]
    pub(crate) fn scale_type_constant(self) -> &'static jni::strings::JNIStr {
        use jni::jni_str;
        match self {
            Self::Contain => jni_str!("FIT_CENTER"),
            Self::Cover => jni_str!("CENTER_CROP"),
            Self::Fill => jni_str!("FIT_XY"),
            Self::Center => jni_str!("CENTER"),
        }
    }
}

/// The encoded bytes a slot should show, compared by **identity** (module
/// doc's *decode once*), never by content.
#[derive(Clone, Default)]
pub(crate) struct ImageBytes {
    /// The publish revision, or `0` when nothing is published for the slot.
    rev: u64,
    /// The published payload, if any.
    bytes: Option<Arc<[u8]>>,
}

impl ImageBytes {
    /// No image — an `ImageView` showing nothing (and what clears a live one).
    pub(crate) fn empty() -> Self {
        Self::default()
    }

    /// The payload, if a slot has one published.
    pub(crate) fn as_slice(&self) -> Option<&[u8]> {
        self.bytes.as_deref()
    }

    /// The payload's length in bytes (`0` when empty) — diagnostics only.
    pub(crate) fn len(&self) -> usize {
        self.bytes.as_ref().map_or(0, |bytes| bytes.len())
    }

    /// The publish revision this value was read at (`0` when empty).
    pub(crate) fn rev(&self) -> u64 {
        self.rev
    }
}

impl PartialEq for ImageBytes {
    /// Identity, not content: same revision **and** same allocation. Two
    /// distinct `Arc`s holding identical bytes deliberately compare unequal —
    /// a re-decode is the cost of an app that rebuilds its byte buffer, and
    /// hashing megabytes per rebuild to avoid it would cost more.
    fn eq(&self, other: &Self) -> bool {
        self.rev == other.rev
            && match (&self.bytes, &other.bytes) {
                (Some(ours), Some(theirs)) => Arc::ptr_eq(ours, theirs),
                (None, None) => true,
                _ => false,
            }
    }
}

impl std::fmt::Debug for ImageBytes {
    /// Never prints the payload — an image in a test failure is noise.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageBytes")
            .field("rev", &self.rev)
            .field("len", &self.len())
            .finish()
    }
}

/// One slot's published payload.
struct Published {
    /// The revision handed back to the publisher.
    rev: u64,
    /// The payload itself, kept alive by this table for as long as the slot
    /// is live.
    bytes: Arc<[u8]>,
    /// How many live control instances claimed this slot ([`claim_bytes`]).
    ///
    /// Normally one — but the runtime creates a replacement **before**
    /// disposing the instance it replaces (`crate::runtime`'s `create`), so a
    /// slot briefly has two. Counting is what keeps the replaced instance's
    /// dispose from deleting the replacement's bytes; a rev check would not,
    /// because both instances hold the *same* revision when the payload did
    /// not change.
    holders: u32,
}

// `missing_const_for_thread_local` fires on the table below and is wrong
// here: `HashMap::new` is not a `const fn` (it builds a `RandomState`), so a
// `const` block around it does not compile. The counter beside it *is* const.
#[allow(clippy::missing_const_for_thread_local)]
mod tables {
    use super::{Cell, HashMap, Published, RefCell, SlotId};

    thread_local! {
        /// The publish table (module doc). Bounded by the number of live
        /// image slots: publishing replaces a slot's entry, and the last
        /// holder's `dispose` removes it.
        pub(super) static PUBLISHED: RefCell<HashMap<SlotId, Published>> =
            RefCell::new(HashMap::new());

        /// The revision counter. Process-wide (not per slot) so a revision is
        /// never re-used after a slot id is recycled.
        pub(super) static NEXT_REV: Cell<u64> = const { Cell::new(1) };
    }
}

use tables::{NEXT_REV, PUBLISHED};

/// Publish the bytes `slot` should show, returning the revision to write into
/// that slot's params as [`REV`].
///
/// Publishing the **same** `Arc` again returns the same revision and leaves
/// the table untouched, so an app that hands its buffer down every rebuild
/// produces no params change and no decode (module doc). Replacing a slot's
/// payload keeps whatever [`claim_bytes`] count it already had — the holder
/// is the *slot's control*, not the individual payload.
pub(crate) fn publish_bytes(slot: SlotId, bytes: Arc<[u8]>) -> u64 {
    PUBLISHED.with(|table| {
        let mut table = table.borrow_mut();
        let holders = match table.get(&slot) {
            Some(existing) if Arc::ptr_eq(&existing.bytes, &bytes) => return existing.rev,
            Some(existing) => existing.holders,
            None => 0,
        };
        let rev = NEXT_REV.with(|next| {
            let rev = next.get();
            next.set(rev.wrapping_add(1));
            rev
        });
        table.insert(
            slot,
            Published {
                rev,
                bytes,
                holders,
            },
        );
        rev
    })
}

/// Take a hold on `slot`'s published bytes — the control's `create` does this
/// once it has successfully built its view.
///
/// An entry that was published but never claimed (a slot whose `Create`
/// command never ran — the app unmounted it inside a single frame) stays
/// until the same slot publishes again, or the Component tears down —
/// [`retire`] (the module doc's f2-03 fix) reaps this case too, not only the
/// counted holders-reach-zero path.
pub(crate) fn claim_bytes(slot: SlotId) {
    PUBLISHED.with(|table| {
        if let Some(entry) = table.borrow_mut().get_mut(&slot) {
            entry.holders = entry.holders.saturating_add(1);
        }
    });
}

/// Release a hold taken by [`claim_bytes`], dropping the payload once the last
/// holder is gone — the control's `dispose`.
pub(crate) fn release_bytes(slot: SlotId) {
    PUBLISHED.with(|table| {
        let mut table = table.borrow_mut();
        let Some(entry) = table.get_mut(&slot) else {
            return;
        };
        entry.holders = entry.holders.saturating_sub(1);
        if entry.holders == 0 {
            table.remove(&slot);
        }
    });
}

/// Remove `slot`'s published entry unconditionally, whatever its holder count
/// — the Component-teardown reaper (module doc's f2-03 fix). Registered via
/// `on_cleanup` in `NativeImageView`'s `Component::init` (`api::builders`),
/// so it runs exactly once per mounted Component regardless of how many
/// times (if any) the native `create`/`dispose` pair ran on the platform side
/// in between — a culled slot that never ran `View::teardown`, then quietly
/// republished, is exactly the case the counted [`claim_bytes`]/
/// [`release_bytes`] pair alone cannot see.
///
/// Idempotent: a slot the counted path already emptied is a silent no-op
/// (`HashMap::remove` on a missing key), and a [`claim_bytes`] arriving late
/// — after this already ran — finds nothing to claim and no-ops too (its own
/// `get_mut` on a missing entry, `crate::controls::image`'s existing
/// contract). No ordering machinery needed between the two paths.
pub(crate) fn retire(slot: SlotId) {
    PUBLISHED.with(|table| {
        table.borrow_mut().remove(&slot);
    });
}

/// The bytes currently published for `slot`, if any.
fn published_bytes(slot: SlotId) -> ImageBytes {
    PUBLISHED.with(|table| {
        table
            .borrow()
            .get(&slot)
            .map(|entry| ImageBytes {
                rev: entry.rev,
                bytes: Some(Arc::clone(&entry.bytes)),
            })
            .unwrap_or_default()
    })
}

/// Everything an `Image` slot can be told, as one Rust-diffed value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ImageProps {
    /// The differ's slot id — **not a property**, but load-bearing: it is the
    /// publish table's key.
    pub(crate) slot: SlotId,
    /// The encoded payload, by identity (module doc).
    pub(crate) bytes: ImageBytes,
    /// How it is scaled into the slot's box.
    pub(crate) fit: Fit,
    /// Packed ARGB tint, or `None` to draw the image untinted.
    pub(crate) tint: Option<i32>,
    /// The TalkBack label. An unlabelled image is invisible to a screen
    /// reader, so an app is expected to set it (or say so explicitly with an
    /// empty string).
    pub(crate) content_description: Option<String>,
}

impl ImageProps {
    /// The state a freshly constructed `new ImageView(context)` is already in:
    /// no image, `FIT_CENTER`, no tint.
    pub(crate) fn platform_default(slot: SlotId) -> Self {
        Self {
            slot,
            bytes: ImageBytes::empty(),
            fit: Fit::Contain,
            tint: None,
            content_description: None,
        }
    }

    /// Decode an `Image` slot's params, resolving its bytes through the
    /// publish table (module doc).
    ///
    /// A slot with nothing published decodes to [`ImageBytes::empty`] and
    /// renders an empty view rather than failing — a `Create` replay whose
    /// bytes the app has since replaced must stay alive long enough for the
    /// `UpdateParams` behind it to fix it.
    ///
    /// # Errors
    /// [`NativeWidgetError::Params`] when the reserved identity keys are
    /// missing.
    pub(crate) fn decode(params: &Params<'_>) -> Result<Self, NativeWidgetError> {
        let slot = slot_of(params)?;
        let bytes = published_bytes(slot);
        if let Some(claimed) = params.int(REV)
            && claimed as u64 != bytes.rev()
        {
            log::debug!(
                "frust-native-widgets: image slot {slot} params claim revision {claimed} but the \
                 publish table holds {} — showing the published bytes",
                bytes.rev()
            );
        }
        Ok(Self {
            slot,
            bytes,
            fit: params
                .string(FIT)
                .and_then(|raw| Fit::from_params(&raw))
                .unwrap_or_default(),
            tint: color(params, TINT),
            content_description: owned_text(params, CONTENT_DESCRIPTION),
        })
    }

    /// The setter-call plan for `old` → `new`, in declaration order.
    ///
    /// The bytes setter is planned **only** on an identity change, which is
    /// what keeps the decode off the rebuild path entirely.
    pub(crate) fn plan<'a>(old: &Self, new: &'a Self) -> Plan<'a> {
        let mut plan = Plan::new();
        if old.bytes != new.bytes {
            plan.push(Setter::ImageBytes(&new.bytes));
        }
        if old.fit != new.fit {
            plan.push(Setter::ScaleType(new.fit));
        }
        if old.tint != new.tint {
            plan.push(Setter::ImageTint(new.tint));
        }
        if old.content_description != new.content_description {
            plan.push(Setter::ContentDescription(
                new.content_description.as_deref(),
            ));
        }
        plan
    }
}

#[cfg(target_os = "android")]
pub(crate) mod platform {
    //! The Android half: build the `ImageView` and hand its planned setters to
    //! [`crate::controls::platform`].

    use jni::objects::JObject;
    use jni::refs::Global;

    use super::{Image, ImageProps, claim_bytes, release_bytes};
    use crate::NativeWidgetError;
    use crate::android::{NativeCtx, NativeView};
    use crate::controls::platform::{FRAME_CAPACITY, apply_all};
    use crate::registry::SlotId;
    use crate::runtime::{NativeWidget, Params};

    /// `android.widget.ImageView` — the framework class.
    const CLASS: &str = "android.widget.ImageView";

    /// A live image's retained state.
    pub(crate) struct ImageState {
        /// The view's own global reference (the second one — see
        /// `button.rs`).
        view: Global<JObject<'static>>,
        /// Which slot's publish-table hold this instance took in `create`
        /// ([`claim_bytes`]) and gives back in `dispose` ([`release_bytes`]).
        slot: SlotId,
    }

    impl NativeWidget for Image {
        type Props = ImageProps;
        type State = ImageState;

        fn decode_props(params: &Params<'_>) -> Result<Self::Props, NativeWidgetError> {
            ImageProps::decode(params)
        }

        fn create(
            ctx: &mut NativeCtx<'_, '_>,
            props: &Self::Props,
        ) -> Result<(NativeView, Self::State), NativeWidgetError> {
            let view = ctx.new_view(CLASS)?;
            let plan = ImageProps::plan(&ImageProps::platform_default(props.slot), props);
            ctx.with_frame(FRAME_CAPACITY, |ctx| apply_all(ctx, &view, &plan))?;
            let handle = ctx.retain(&view)?;
            let retained = ctx.retain(&view)?;
            // Claimed last: an earlier failure returns before taking a hold
            // nothing would ever release.
            claim_bytes(props.slot);
            Ok((
                NativeView::new(handle),
                ImageState {
                    view: retained,
                    slot: props.slot,
                },
            ))
        }

        fn update(
            ctx: &mut NativeCtx<'_, '_>,
            state: &mut Self::State,
            old: &Self::Props,
            new: &Self::Props,
        ) -> Result<(), NativeWidgetError> {
            let plan = ImageProps::plan(old, new);
            ctx.with_frame(FRAME_CAPACITY, |ctx| apply_all(ctx, &state.view, &plan))
        }

        fn dispose(
            _ctx: &mut NativeCtx<'_, '_>,
            state: Self::State,
        ) -> Result<(), NativeWidgetError> {
            // Give back this instance's hold on the slot's bytes; the payload
            // is dropped only when the last holder is gone (the runtime
            // creates a replacement *before* disposing what it replaced).
            // Dropping the state then releases its global reference.
            release_bytes(state.slot);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::with_identity;

    /// Every test uses its own slot id: the publish table is a thread-local
    /// shared by the whole test binary's main thread.
    fn decode(slot: SlotId, body: &str) -> ImageProps {
        let raw = with_identity(KIND, slot, body);
        ImageProps::decode(&Params::new(&raw)).expect("decodes")
    }

    fn payload(byte: u8) -> Arc<[u8]> {
        Arc::from(vec![byte; 8].into_boxed_slice())
    }

    #[test]
    fn a_slot_with_nothing_published_decodes_to_an_empty_image() {
        let props = decode(900, "");
        assert_eq!(props, ImageProps::platform_default(900));
        assert!(props.bytes.as_slice().is_none());
        assert!(ImageProps::plan(&ImageProps::platform_default(900), &props).is_empty());
    }

    #[test]
    fn publishing_the_same_arc_twice_keeps_one_revision_and_plans_no_decode() {
        let slot = 901;
        let bytes = payload(1);
        let first = publish_bytes(slot, Arc::clone(&bytes));
        let second = publish_bytes(slot, Arc::clone(&bytes));
        assert_eq!(first, second, "identity, not content, drives the revision");

        let old = decode(slot, &format!("\"imageRev\":{first}"));
        let new = decode(slot, &format!("\"imageRev\":{second}"));
        assert_eq!(old, new);
        assert!(
            ImageProps::plan(&old, &new).is_empty(),
            "an unchanged Arc never re-decodes"
        );
        release_bytes(slot);
    }

    #[test]
    fn a_different_arc_bumps_the_revision_and_plans_exactly_one_decode() {
        let slot = 902;
        let first_rev = publish_bytes(slot, payload(1));
        let old = decode(slot, &format!("\"imageRev\":{first_rev}"));
        let second_rev = publish_bytes(slot, payload(2));
        assert_ne!(first_rev, second_rev);
        let new = decode(slot, &format!("\"imageRev\":{second_rev}"));

        let plan = ImageProps::plan(&old, &new);
        assert_eq!(plan, vec![Setter::ImageBytes(&new.bytes)]);
        assert_eq!(plan[0].tier(), super::super::Tier::Decode);
        release_bytes(slot);
    }

    #[test]
    fn identical_content_in_a_fresh_arc_is_a_new_identity_by_design() {
        let slot = 903;
        let first = publish_bytes(slot, payload(7));
        let old = decode(slot, "");
        let second = publish_bytes(slot, payload(7));
        let new = decode(slot, "");
        assert_ne!(first, second);
        assert_eq!(ImageProps::plan(&old, &new).len(), 1);
        release_bytes(slot);
    }

    #[test]
    fn the_create_plan_sets_exactly_the_non_default_fields() {
        let slot = 904;
        publish_bytes(slot, payload(3));
        let props = decode(
            slot,
            "\"fit\":\"cover\",\"tint\":128,\"contentDescription\":\"avatar\"",
        );
        assert_eq!(
            ImageProps::plan(&ImageProps::platform_default(slot), &props),
            vec![
                Setter::ImageBytes(&props.bytes),
                Setter::ScaleType(Fit::Cover),
                Setter::ImageTint(Some(128)),
                Setter::ContentDescription(Some("avatar")),
            ]
        );
        release_bytes(slot);
    }

    #[test]
    fn an_unknown_fit_spelling_degrades_to_the_platform_default() {
        let props = decode(905, "\"fit\":\"stretch-ish\"");
        assert_eq!(props.fit, Fit::Contain);
        assert_eq!(decode(905, "\"fit\":\"center\"").fit, Fit::Center);
    }

    #[test]
    fn a_replaced_instances_dispose_never_deletes_the_replacements_bytes() {
        // The runtime creates a replacement *before* disposing what it
        // replaced (`crate::runtime`'s `create`), so a slot briefly has two
        // holders — and both hold the SAME revision when the payload did not
        // change, which is exactly why the count, not the revision, decides.
        let slot = 906;
        let rev = publish_bytes(slot, payload(1));
        claim_bytes(slot); // the original instance
        claim_bytes(slot); // its replacement

        release_bytes(slot); // the original's late dispose
        assert_eq!(
            decode(slot, "").bytes.rev(),
            rev,
            "the replacement's bytes survive"
        );

        release_bytes(slot); // the replacement's own dispose
        assert!(decode(slot, "").bytes.as_slice().is_none());
    }

    #[test]
    fn replacing_a_payload_keeps_the_slots_existing_hold() {
        let slot = 907;
        publish_bytes(slot, payload(1));
        claim_bytes(slot);
        let replaced = publish_bytes(slot, payload(2));
        assert_eq!(decode(slot, "").bytes.rev(), replaced);

        // One hold was taken, so one release empties the slot — the hold
        // belongs to the control, not to the payload it happened to show.
        release_bytes(slot);
        assert!(decode(slot, "").bytes.as_slice().is_none());
    }

    #[test]
    fn releasing_a_slot_nobody_published_is_a_silent_no_op() {
        release_bytes(908);
        assert!(decode(908, "").bytes.as_slice().is_none());
    }

    // --- f2-03 regression: the culled-dispose-then-republish leak ----------
    //
    // The counted `claim_bytes`/`release_bytes` pair alone cannot see this:
    // paint culling can dispose a scrolled-off slot without the widget ever
    // running `View::teardown`, so a still-mounted widget republishes into a
    // fresh, zero-holder entry the counted pair never releases. `retire`
    // (registered via `on_cleanup` in `NativeImageView::init`,
    // `api::builders`) is the actual fix — a Component-teardown reaper
    // independent of the native create/dispose lifecycle.

    #[test]
    fn the_culled_dispose_then_republish_leak_is_reaped_on_component_teardown() {
        // Step 1: mount — publish rev R1, `Create` lands, `claim_bytes` takes
        // the hold (holders=1).
        let slot = 950;
        let r1 = publish_bytes(slot, payload(1));
        claim_bytes(slot);
        assert_eq!(decode(slot, "").bytes.rev(), r1, "R1 is live after claim");

        // Step 2: scrolled out of view — paint culling suppresses the frame
        // publish, and after `DISPOSE_AFTER_MISSING_FRAMES` the differ emits
        // `Dispose` -> `nativeDisposeControl` -> `release_bytes` ->
        // holders=0 -> entry removed. The widget stays mounted (a culled
        // slot never runs `View::teardown`) — nothing here calls `retire`.
        release_bytes(slot);
        assert!(
            decode(slot, "").bytes.as_slice().is_none(),
            "the culled dispose emptied the table via the ordinary counted path"
        );

        // Step 3: still mounted, the next rebuild republishes — a fresh
        // entry at rev R2 with holders: 0 (the `None => 0` fallback in
        // `publish_bytes`). Nothing native has claimed R2.
        let r2 = publish_bytes(slot, payload(2));
        assert_ne!(r1, r2);
        assert_eq!(
            decode(slot, "").bytes.rev(),
            r2,
            "R2 is published but has zero holders — nothing native ever \
             claimed it after the culled dispose"
        );

        // Steps 4/5: navigate away. `View::teardown` disposes the
        // Component's owner, running the `on_cleanup` `NativeImageView`
        // registered in `init` — which calls `retire(slot)` unconditionally,
        // regardless of the (already-zero) holder count.
        retire(slot);
        assert!(
            decode(slot, "").bytes.as_slice().is_none(),
            "retire reaps the R2 entry on Component teardown even though \
             nothing native ever claimed it — without `retire`, R2 would \
             leak for the process lifetime (the shipped f2-03 defect)"
        );
    }

    #[test]
    fn the_ordinary_counted_path_still_works_with_no_premature_removal_while_a_claim_is_live() {
        // A ordinary, non-culled mount: publish + claim, no `retire` in
        // sight (the widget is still mounted, so `on_cleanup` has not run).
        // Adding `retire` alongside the counted pair must not change this
        // path's own behaviour at all.
        let slot = 951;
        let rev = publish_bytes(slot, payload(3));
        claim_bytes(slot);
        assert_eq!(
            decode(slot, "").bytes.rev(),
            rev,
            "a live native claim must not be prematurely removed"
        );
        release_bytes(slot);
        assert!(
            decode(slot, "").bytes.as_slice().is_none(),
            "the ordinary counted release path still empties the table on its own"
        );
    }

    #[test]
    fn retire_is_unconditional_and_reaps_a_live_claim_too() {
        // `retire` is not the counted path — it removes the entry regardless
        // of the holder count, because Component teardown must reap the
        // bytes even if a native instance still holds a claim (e.g. a
        // component torn down mid-navigation, before its own `Dispose`
        // command ever lands).
        let slot = 952;
        publish_bytes(slot, payload(4));
        claim_bytes(slot);
        retire(slot);
        assert!(decode(slot, "").bytes.as_slice().is_none());

        // A late `release_bytes` for the claim taken before `retire` ran is a
        // silent no-op — double-removal is harmless (both paths tolerate a
        // missing entry).
        release_bytes(slot);
        assert!(decode(slot, "").bytes.as_slice().is_none());
    }

    #[test]
    fn retire_on_an_already_empty_slot_is_a_silent_no_op() {
        retire(953);
        assert!(decode(953, "").bytes.as_slice().is_none());
    }

    #[test]
    fn a_late_claim_after_retire_no_ops_rather_than_resurrecting_the_entry() {
        let slot = 954;
        publish_bytes(slot, payload(5));
        retire(slot);
        // A late `claim_bytes` arriving after teardown already no-ops on a
        // missing entry — no machinery needed to order the two paths.
        claim_bytes(slot);
        assert!(decode(slot, "").bytes.as_slice().is_none());
    }
}
