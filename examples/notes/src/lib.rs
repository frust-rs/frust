//! The Phase 4B exit-criterion demo: a notes app (spec §14).
//!
//! Proves the whole 4B stack live — a [`text_input`] with real IME on every
//! platform, submit-to-keyed-list (each note carries a stable id so
//! reconciliation preserves the *right* rows across a delete), an [`Image`]
//! decoded exactly once, and the 4A layout/scroll/button vocabulary — in one
//! screen the user can type into. The identical UI is what `forgekit create`
//! now scaffolds (`templates/app/src/lib.rs.tmpl`), so this example and a fresh
//! project render the same thing.
//!
//! `AppState`/`app_logic` are `pub` so the headless interaction test
//! (`tests/interaction.rs`) can drive them through the framework's `RenderRoot`
//! with synthetic keyboard/IME input — the same event seam the desktop shell
//! feeds and the mobile shells reach through `AppTree::event`.

use std::sync::atomic::{AtomicUsize, Ordering};

use forgekit::{
    AnyView, Axis, Button, Column, FlexView, Image, ImageFit, ImageSource, Row, SizedBox, View,
    any, keyed, scroll_view, text, text_input,
};

/// The embedded logo, decoded once at startup (see [`decode_logo`]). Bundling
/// the bytes with `include_bytes!` keeps the demo self-contained — no runtime
/// asset path to resolve on Android/iOS.
const LOGO_PNG: &[u8] = include_bytes!("../assets/logo.png");

/// How many times the logo has actually been decoded this process. The demo's
/// contract is **decode-once per run**: [`ImageSource`] lives in [`AppState`]
/// and is re-passed (a cheap `Arc` clone) into [`Image`] every frame, so a
/// single app run decodes exactly once no matter how many frames render — this
/// counter increments only inside [`AppState::new`], never in `app_logic`.
/// Surfaced as a runtime tripwire for the e2e gate (acceptance criterion 3):
/// the headless test asserts it does not grow across frames, and the mobile
/// run greps the one-shot log line below out of `logcat`/the console.
static LOGO_DECODES: AtomicUsize = AtomicUsize::new(0);

/// Decode the embedded logo, counting and logging the decode. Called exactly
/// once per app run, from [`AppState::new`] — never from the per-frame
/// `app_logic`, which only clones the resulting `Arc` handle.
fn decode_logo() -> ImageSource {
    let n = LOGO_DECODES.fetch_add(1, Ordering::Relaxed) + 1;
    // Log-once-per-run evidence for the e2e gate (visible in `logcat`/console).
    // A second line here in one run would mean the ImageSource is being rebuilt.
    eprintln!("[notes] logo decoded (decode #{n} this process)");
    ImageSource::decode(LOGO_PNG).expect("the embedded logo PNG is valid")
}

/// The number of times the logo has been decoded this process — read by the
/// headless test to assert decode-once *across frames* (the count must not grow
/// while `app_logic` runs, only when a new [`AppState`] is constructed).
pub fn logo_decode_count() -> usize {
    LOGO_DECODES.load(Ordering::Relaxed)
}

/// Notes demo state (spec §5 `app_logic` model).
///
/// The view is a pure function of these fields. `draft` is the controlled
/// [`text_input`]'s current text; `notes` is the submitted list, each entry a
/// `(stable id, text)` pair so the keyed rows survive a middle-of-list delete
/// (spec §6.3); `next_id` hands out the ids.
pub struct AppState {
    /// The in-progress note text bound to the field (controlled component).
    pub draft: String,
    /// Submitted notes: `(stable id, text)`. The id is the [`keyed`] identity.
    pub notes: Vec<(u64, String)>,
    /// The next id to assign — monotonic, never reused, so keys stay unique.
    pub next_id: u64,
    /// The decode-once logo handle, re-passed into [`Image`] every frame.
    pub logo: ImageSource,
}

impl AppState {
    /// Build the initial state, decoding the embedded logo once.
    pub fn new() -> Self {
        Self {
            draft: String::new(),
            notes: Vec::new(),
            next_id: 0,
            logo: decode_logo(),
        }
    }

    /// Submit the current `draft` as a new keyed note and clear the field.
    ///
    /// Empty/whitespace-only drafts are dropped (still clearing the field), so a
    /// stray Enter never appends a blank row. Factored out of the `on_submit`
    /// closure so the headless test can assert it directly.
    pub fn submit_draft(&mut self, text: String) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            let id = self.next_id;
            self.next_id += 1;
            self.notes.push((id, trimmed.to_string()));
        }
        self.draft.clear();
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// Pure view function: renders `AppState` into the notes screen (spec §5).
/// Re-run every frame, so it is cheap by construction — the only non-trivial
/// resource, the decoded logo, is an `Arc` clone, never a re-decode.
///
/// `+ use<>` opts the return type out of edition-2024's implicit lifetime
/// capture — the view borrows nothing from `state` (views are `'static`).
pub fn app_logic(state: &mut AppState) -> impl View<AppState> + use<> {
    let logo = state.logo.clone();
    let draft = state.draft.clone();

    // The submitted notes, each a keyed row: the note text plus a Delete button
    // that removes *this* id. The stable key means deleting the middle row drops
    // the middle row's widget, not whatever now sits at that index (spec §6.3).
    let rows: Vec<forgekit::FlexChild<AppState>> = state
        .notes
        .iter()
        .map(|(id, note)| {
            let id = *id;
            keyed(
                id,
                Row(vec![
                    any(text(note.clone()).size(20.0)),
                    any(SizedBox(Some(12.0), None)),
                    any(Button("Delete", move |s: &mut AppState| {
                        s.notes.retain(|(nid, _)| *nid != id);
                    })),
                ]),
            )
        })
        .collect();

    let mut children: Vec<AnyView<AppState>> = Vec::with_capacity(4);
    // The logo sizes to its natural box (see [`Image`] layout) and `Contain`-fits
    // into it — a square source, so it fills exactly with no letterbox.
    children.push(any(Image(logo).fit(ImageFit::Contain)));
    children.push(any(text("Notes").size(32.0)));
    children.push(any(text_input(draft, |s: &mut AppState, v: String| {
        s.draft = v;
    })
    .placeholder("Write a note, then press Enter")
    .on_submit(|s: &mut AppState, v: String| {
        s.submit_draft(v);
    })));
    children.push(any(scroll_view(FlexView::new(Axis::Vertical, rows))));

    Column(children)
}

// Android entry point: stamps out the `Java_dev_forgekit_ForgeKitSurfaceView_native*`
// JNI exports bound to this app's `AppState`/`app_logic` (spec §10.1).
#[cfg(target_os = "android")]
forgekit::android_app!(AppState, app_logic);

// iOS entry point: stamps out the fixed `forgekit_*` C-ABI exports the generated
// Swift app calls (spec §10.2), bound to this app's `AppState`/`app_logic`.
forgekit::ios_app!(AppState, app_logic);
