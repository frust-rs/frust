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

use frust::{
    AnyView, Axis, ButtonStyle, Color, EdgeInsets, FlexChild, FlexView, Get, GetUntracked, Padding,
    RwSignal, Set, SizedBox, Theme, any, button, deep_links, inflexible, row, text, use_context,
};
use frust_url_launcher::UrlLauncher;

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

/// See the page-fn contract in [`crate::pages`]. Reads no
/// [`PlaygroundState`] signal — the call status lives in [`status_sig`] and
/// the deep-link readout comes straight from [`frust::deep_links`].
pub fn page(_state: &PlaygroundState) -> AnyView<PlaygroundState> {
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

    let children: Vec<AnyView<PlaygroundState>> = vec![
        any(text("URL launcher").size(13.0).color(accent())),
        any(text(
            "frust-url-launcher's open_external, exercised from a real button press \u{2014} \
                 the browser-out half of the round-trip device gate. The frustplay:// deep-link-\
                 back half is read below and driven by the commands in the caption.",
        )
        .size(11.0)
        .color(muted())),
        any(SizedBox(None, Some(12.0))),
        any(row()
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
            )),
        any(SizedBox(None, Some(8.0))),
        any(text(status_line).size(12.0)),
        any(SizedBox(None, Some(12.0))),
        any(text(latest_line).size(12.0).color(accent())),
        any(SizedBox(None, Some(12.0))),
        any(text(format!("Return leg (Android): {ADB_RETURN_COMMAND}"))
            .size(10.0)
            .color(muted())),
        any(text(format!(
            "Return leg (iOS Simulator): {SIMCTL_RETURN_COMMAND}"
        ))
        .size(10.0)
        .color(muted())),
    ];

    any(Padding(
        EdgeInsets::all(16.0),
        FlexView::new(
            Axis::Vertical,
            children.into_iter().map(inflexible).collect(),
        ),
    ))
}
