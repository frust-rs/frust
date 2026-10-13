//! URL launcher section: the `frust-url-launcher` plugin's device-gate page —
//! `plugins/url-launcher`'s whole vertical slice exercised from a real button
//! press, plus the `frustplay` custom-scheme return leg the round-trip device
//! gate drives back in.
//!
//! # Round trip
//!
//! "Open example.com" calls [`UrlLauncher::open_external`] with a valid
//! `https` URL — the browser-out half of the gate. "Open invalid
//! (javascript:)" calls the same function with a `javascript:` URL, proving
//! [`UrlLauncherError::InvalidUrl`] rejects it before any platform API is
//! touched (`plugins/url-launcher/src/url.rs`'s rule table). Both write the
//! call's `Debug`-formatted `Result` into a status line below the buttons.
//!
//! The deep-link-back half of the round trip is a separate mechanism this
//! page only *observes*: the `frustplay` scheme registered on Android
//! (`android/app/src/main/AndroidManifest.xml`'s second `<intent-filter>`)
//! and iOS (`ios/Runner/Info.plist`'s `CFBundleURLTypes`) routes a
//! `frustplay://…` link back into this app's `nativeOnDeepLink` handler,
//! which lands in [`frust::deep_links`]'s process-wide `latest` signal — this
//! page just reads it. The caption at the bottom of the page carries the two
//! commands (`adb`/`simctl`) the human-run device gate drives, so a
//! runner doesn't have to go find them in `docs/SHELLS_DEVELOPMENT.md`.
//!
//! # Share, QR and Maps groups
//!
//! The same page also hosts the manual-check surface for `frust-share`
//! (share text / URL / anchored to the button), `frust-qr` (a 160 px code of
//! an editable payload, defaulting to the playground's section deep link)
//! and `frust-url-launcher`'s `Maps` (https directions by coordinate and by
//! address, plus the native `geo:` URI). Every call writes its `Debug`
//! result into [`status_sig`].

use frust::authoring::Rect;
use frust::{
    ButtonStyle, Color, EdgeInsets, FlexChild, Get, GetUntracked, Padding, RwSignal, Set, SizedBox,
    Theme, View, button, column, deep_links, inflexible, row, text, text_input, use_context,
};
use frust_qr::qr_code;
use frust_share::{Share, ShareAnchor};
use frust_url_launcher::{Maps, UrlLauncher};

use crate::PlaygroundState;

/// Defines a `fn $name() -> RwSignal<$ty>` returning a screen-local signal
/// cached in a `thread_local!`, self-healing across a disposed owner — the
/// established per-module precedent (`platform_views.rs`/the demo app's
/// `composite.rs` page each carry the same macro), not shared across files.
macro_rules! local_sig {
    ($name:ident, $ty:ty, $init:expr) => {
        fn $name() -> RwSignal<$ty> {
            thread_local! {
                static SLOT: std::cell::RefCell<Option<RwSignal<$ty>>> =
                    const { std::cell::RefCell::new(None) };
            }
            SLOT.with(|cell| {
                if let Some(sig) = *cell.borrow()
                    && sig.try_get_untracked().is_some()
                {
                    return sig;
                }
                let sig = RwSignal::new($init);
                *cell.borrow_mut() = Some(sig);
                sig
            })
        }
    };
}

// The last `open_external` call's `Debug`-formatted result — empty until the
// first button press.
local_sig!(status_sig, String, String::new());

// The QR group's editable payload.
local_sig!(qr_payload_sig, String, DEEP_LINK_URL.to_string());

/// The playground's own deep link to this page (`lib.rs`'s
/// `frustplay://section/<label>` router): the QR default and the share-URL
/// payload (an http/https URL is required by `Share::url`, so that button
/// shares the project site instead).
const DEEP_LINK_URL: &str = "frustplay://section/URL";

/// A fixed point (Googleplex) for the coordinate Directions button.
const MAPS_LAT: f64 = 37.4220;
const MAPS_LNG: f64 = -122.0841;

/// The native `geo:` URI the third Maps button hands to `open_maps_native`.
const MAPS_GEO_URI: &str = "geo:37.4220,-122.0841?q=37.4220,-122.0841";

/// Group labels, asserted present in the semantics tree by the test below.
const SHARE_LABEL: &str = "Share";
const QR_LABEL: &str = "QR";
const MAPS_LABEL: &str = "Maps";

/// Where the anchored-share button sits on the page, in logical points. The
/// widget tier exposes no laid-out-rect seam (no `on_layout` hook), so this
/// is the page's known button position (16 px page padding plus the content
/// above it) rather than a measured rect; it only positions an iOS popover
/// and is ignored on every other platform.
const SHARE_ANCHOR_RECT: Rect = Rect::new(16.0, 300.0, 196.0, 332.0);

/// The Android return-leg command — cold or warm, `singleTop` (see the
/// manifest's `android:launchMode`) routes either one to `onNewIntent`. The
/// applicationId is `examples/playground/android/app/build.gradle.kts`'s
/// real value, not a placeholder.
const ADB_RETURN_COMMAND: &str =
    "adb shell am start -a android.intent.action.VIEW -d 'frustplay://back?ok=1' it.f0x.playground";

/// The iOS Simulator return-leg command — no physical-device CLI trigger
/// exists (`docs/SHELLS_DEVELOPMENT.md`'s Deep-link manual test), so a real
/// device instead taps a registered `frustplay://` link.
const SIMCTL_RETURN_COMMAND: &str = "xcrun simctl openurl booted 'frustplay://back?ok=1'";

/// Live-theme accent-text role (`primary`), falling back to the Material
/// baseline pre-context — the same pattern every other section page uses.
fn accent() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(frust_material::baseline)
        .scheme()
        .primary
}

/// A muted caption ink.
fn muted() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(frust_material::baseline)
        .scheme()
        .on_surface_variant
}

/// A fixed-width horizontal spacer between the two buttons.
fn gap_h(w: f64) -> FlexChild<PlaygroundState> {
    inflexible(SizedBox(Some(w), None))
}

/// Namespace for the page's group builders (associated fns, not free view fns).
struct UrlLauncherPage;

impl UrlLauncherPage {
    /// A small button that records `label -> result` into [`status_sig`].
    fn action(
        label: &'static str,
        style: ButtonStyle,
        call: impl Fn() -> String + 'static,
    ) -> impl View<PlaygroundState> {
        button(label, move |_state: &mut PlaygroundState| {
            status_sig().set(call());
        })
        .style(style)
        .small()
    }

    fn share_group() -> impl View<PlaygroundState> {
        column()
            .child(text(SHARE_LABEL).size(13.0).color(accent()))
            .child(
                row()
                    .child(Self::action("Share text", ButtonStyle::Secondary, || {
                        format!(
                            "Share::text -> {:?}",
                            Share::text("Frust playground share check", Some("Frust"))
                        )
                    }))
                    .push(gap_h(8.0))
                    .child(Self::action("Share URL", ButtonStyle::Secondary, || {
                        format!(
                            "Share::url -> {:?}",
                            Share::url("https://frust.dev", Some("Frust"))
                        )
                    }))
                    .push(gap_h(8.0))
                    .child(Self::action(
                        "Share anchored",
                        ButtonStyle::Secondary,
                        || {
                            let anchor = ShareAnchor {
                                rect: SHARE_ANCHOR_RECT,
                            };
                            format!(
                                "Share::text_anchored -> {:?}",
                                Share::text_anchored("Anchored share check", None, Some(&anchor))
                            )
                        },
                    )),
            )
    }

    fn qr_group() -> impl View<PlaygroundState> {
        let payload = qr_payload_sig().get();
        column()
            .child(text(QR_LABEL).size(13.0).color(accent()))
            .child(text_input(
                payload.clone(),
                |_state: &mut PlaygroundState, value| {
                    qr_payload_sig().set(value);
                },
            ))
            .child(SizedBox(None, Some(8.0)))
            .child(
                SizedBox(Some(160.0), Some(160.0)).child(
                    qr_code::<PlaygroundState>(payload)
                        .size(160.0)
                        .semantic_label("Deep-link QR code"),
                ),
            )
    }

    fn maps_group() -> impl View<PlaygroundState> {
        column()
            .child(text(MAPS_LABEL).size(13.0).color(accent()))
            .child(
                row()
                    .child(Self::action(
                        "Directions (lat/lng)",
                        ButtonStyle::Secondary,
                        || {
                            format!(
                                "Maps::directions -> {:?}",
                                Maps::directions(MAPS_LAT, MAPS_LNG)
                            )
                        },
                    ))
                    .push(gap_h(8.0))
                    .child(Self::action(
                        "Directions (address)",
                        ButtonStyle::Secondary,
                        || {
                            format!(
                                "Maps::directions_to -> {:?}",
                                Maps::directions_to("1600 Amphitheatre Parkway, Mountain View")
                            )
                        },
                    )),
            )
            .child(SizedBox(None, Some(8.0)))
            .child(Self::action(
                "Directions (native geo:)",
                ButtonStyle::Secondary,
                || {
                    format!(
                        "open_maps_native -> {:?}",
                        UrlLauncher::open_maps_native(MAPS_GEO_URI)
                    )
                },
            ))
    }
}

/// See the page-fn contract in [`crate::pages`]. Reads no
/// [`PlaygroundState`] signal — the call status lives in [`status_sig`] and
/// the deep-link readout comes straight from [`frust::deep_links`].
pub fn page(_state: &PlaygroundState) -> impl View<PlaygroundState> {
    let status = status_sig().get();
    // Tracked read: `Get::get()` subscribes this rebuild to `latest`, so a
    // warm `frustplay://` link pushed while this page is on screen repaints
    // it live — the same "track the signal you read" contract every other
    // section observes (see e.g. `pages/i18n.rs`'s `active`/`format_locale`
    // reads).
    let latest = deep_links().latest.get();
    let latest_line = match latest {
        Some(link) => format!("latest: {}", link.url),
        None => "latest: none".to_string(),
    };
    let status_line = if status.is_empty() {
        "status: (no attempt yet)".to_string()
    } else {
        format!("status: {status}")
    };

    let body = column()
        .child(text("URL launcher").size(13.0).color(accent()))
        .child(
            text(
                "frust-url-launcher's open_external, exercised from a real button press \u{2014} \
                 the browser-out half of the round-trip device gate. The frustplay:// deep-link-\
                 back half is read below and driven by the commands in the caption.",
            )
            .size(11.0)
            .color(muted()),
        )
        .child(SizedBox(None, Some(12.0)))
        .child(
            row()
                .child(
                    button("Open example.com", |_state: &mut PlaygroundState| {
                        let result = UrlLauncher::open_external("https://example.com");
                        status_sig().set(format!(
                            "open_external(\"https://example.com\") -> {result:?}"
                        ));
                    })
                    .style(ButtonStyle::Primary)
                    .small(),
                )
                .push(gap_h(8.0))
                .child(
                    button(
                        "Open invalid (javascript:)",
                        |_state: &mut PlaygroundState| {
                            let result = UrlLauncher::open_external("javascript:alert(1)");
                            status_sig().set(format!(
                                "open_external(\"javascript:alert(1)\") -> {result:?}"
                            ));
                        },
                    )
                    .style(ButtonStyle::Secondary)
                    .small(),
                ),
        )
        .child(SizedBox(None, Some(8.0)))
        .child(text(status_line).size(12.0))
        .child(SizedBox(None, Some(12.0)))
        .child(text(latest_line).size(12.0).color(accent()))
        .child(SizedBox(None, Some(12.0)))
        .child(UrlLauncherPage::share_group())
        .child(SizedBox(None, Some(12.0)))
        .child(UrlLauncherPage::qr_group())
        .child(SizedBox(None, Some(12.0)))
        .child(UrlLauncherPage::maps_group())
        .child(SizedBox(None, Some(12.0)))
        .child(
            text(format!("Return leg (Android): {ADB_RETURN_COMMAND}"))
                .size(10.0)
                .color(muted()),
        )
        .child(
            text(format!(
                "Return leg (iOS Simulator): {SIMCTL_RETURN_COMMAND}"
            ))
            .size(10.0)
            .color(muted()),
        );

    Padding(EdgeInsets::all(16.0), body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::{AnyView, any};
    use frust_core::RenderRoot;
    use frust_reactive::ReactiveRuntime;
    use frust_text::TextContext;
    use kurbo::Size;
    use reactive_graph::owner::Owner;
    use std::any::Any;

    /// Each group's labelled members reach the semantics tree.
    #[test]
    fn group_labels_are_in_the_semantics_tree() {
        let _ = ReactiveRuntime::init(std::sync::Arc::new(|| {}));
        let owner = Owner::new();
        owner.set();
        let mut tcx = TextContext::new();
        let mut root: RenderRoot<PlaygroundState, AnyView<PlaygroundState>> = RenderRoot::new();
        let mut state = PlaygroundState::new();
        let mut logic = |s: &mut PlaygroundState| any(page(s));
        root.rebuild(&mut logic, &mut state);
        let tcx_any: &mut dyn Any = &mut tcx;
        root.layout_with_text(Size::new(390.0, 1400.0), tcx_any);
        let update = root.semantics();
        let labels: Vec<&str> = update.nodes.iter().filter_map(|(_, n)| n.label()).collect();
        // Plain `text` carries no semantics node; each group is detected by
        // its interactive/labelled members (buttons, the QR image role).
        for want in [
            "Share text",
            "Share URL",
            "Share anchored",
            "Deep-link QR code",
            "Directions (lat/lng)",
            "Directions (address)",
            "Directions (native geo:)",
        ] {
            assert!(labels.contains(&want), "missing {want:?} in {labels:?}");
        }
    }
}
