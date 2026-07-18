//! Showcase · Notes screen (task 05) — the `examples/notes` TextInput/IME/
//! keyed-list/Image demo, ported onto a [`NotesController`] in
//! [`crate::notes_domain`].
//!
//! Hosted as its own [`Component`] (via [`forgekit::component`], the same
//! shape `routes::team_page` wraps [`crate::features::team::presentation::TeamScreen`]
//! in) rather than a plain view-building function, since — unlike
//! [`crate::screens::member_detail`] — this screen owns retained local state
//! (the controlled draft) and a controller with its own signals/failure sink.
//! [`crate::routes::build_routes`]'s `/notes` route calls [`notes_screen`] with
//! no arguments, so this file alone decides how the screen is wired.
//!
//! Feature parity with the pre-port `examples/notes` demo: a placeholder'd
//! [`forgekit::text_input`] with per-keystroke `on_change`, Enter submits into
//! a [`forgekit::keyed`] list (stable ids survive a middle-of-list delete), a
//! Delete button per row, and the embedded logo [`forgekit::Image`] decoded
//! exactly once. The one behavioral delta: a blank/whitespace submit is now
//! rejected by [`crate::notes_domain::AddNote`] as a domain-layer
//! [`NotesFailure::Validation`] (surfaced here as an error banner) rather than
//! silently dropped by the screen — see `notes_domain`'s module doc.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use clean_signals::Failure;
use clean_signals_forgekit::{use_controller, use_failure_listener};
use forgekit::{
    AnyView, Axis, Button, Column, Component, FlexView, Get, Image, ImageFit, ImageSource, Row,
    RwSignal, Set, SizedBox, any, keyed, scroll_view, text, text_input,
};

use crate::ShellState;
use crate::notes_domain::{NotesController, NotesFailure};

/// The embedded logo, decoded once at startup (see [`decode_logo`]) — a copy
/// of `examples/notes`' `assets/logo.png` under its own name
/// (`assets/notes_logo.png`) so this screen doesn't reach into another
/// example crate's asset directory.
const LOGO_PNG: &[u8] = include_bytes!("../../assets/notes_logo.png");

/// How many times the logo has actually been decoded this process — the
/// decode-once contract [`examples/notes`] established: [`ImageSource`] lives
/// in [`NotesState`] and is re-passed (a cheap `Arc` clone) into [`Image`]
/// every frame, so a single app run decodes exactly once no matter how many
/// frames render. Incremented only inside [`decode_logo`], called only from
/// [`NotesScreen::init`] — never from the per-frame [`NotesScreen::build`].
static LOGO_DECODES: AtomicUsize = AtomicUsize::new(0);

/// Decode the embedded logo, counting the decode (see [`LOGO_DECODES`]).
fn decode_logo() -> ImageSource {
    let n = LOGO_DECODES.fetch_add(1, Ordering::Relaxed) + 1;
    eprintln!("[team-demo/notes] logo decoded (decode #{n} this process)");
    ImageSource::decode(LOGO_PNG).expect("the embedded notes logo PNG is valid")
}

/// The notes screen's retained [`Component`] state.
///
/// `draft` is plain data (the notes/team draft pattern — a controlled
/// `TextInput`'s canonical value lives here so a per-frame reconcile does not
/// fight in-progress typing); `banner` is an [`RwSignal`] because the
/// background failure listener registered in [`NotesScreen::init`] writes it
/// from outside `build` (the same banner-via-signal pattern
/// [`crate::features::team::presentation::screen::TeamState::banner`]
/// established).
pub struct NotesState {
    controller: Arc<NotesController>,
    draft: String,
    banner: RwSignal<Option<String>>,
    logo: ImageSource,
}

/// The notes exhibit: a [`Component`] hosting a [`NotesController`].
#[derive(Default)]
pub struct NotesScreen;

impl Component for NotesScreen {
    type State = NotesState;

    fn init(&self) -> NotesState {
        let controller = use_controller::<NotesController, NotesFailure>(NotesController::new);

        let banner: RwSignal<Option<String>> = RwSignal::new(None);
        use_failure_listener(controller.failures(), move |f: NotesFailure| {
            banner.set(Some(f.user_message()));
        });

        NotesState {
            controller,
            draft: String::new(),
            banner,
            logo: decode_logo(),
        }
    }

    fn build(&self, state: &mut NotesState) -> AnyView<NotesState> {
        let logo = state.logo.clone();
        let draft = state.draft.clone();
        let banner_msg = state.banner.get();
        let notes = state.controller.notes.get();

        let mut children: Vec<AnyView<NotesState>> = Vec::with_capacity(5);
        // The logo sizes to its natural box (see `Image` layout) and
        // `Contain`-fits into it — a square source, so it fills exactly with
        // no letterbox.
        children.push(any(Image(logo).fit(ImageFit::Contain)));
        children.push(any(text("Notes").size(32.0)));

        // Banner arm: a validation failure message plus a Dismiss button
        // (mirrors `TeamState`'s banner arm).
        if let Some(msg) = banner_msg {
            children.push(any(Row(vec![
                any(text(msg)),
                any(SizedBox(Some(8.0), None)),
                any(Button("Dismiss", |st: &mut NotesState| {
                    st.banner.set(None);
                })),
            ])));
        }

        children.push(any(text_input(draft, |st: &mut NotesState, v: String| {
            st.draft = v;
        })
        .placeholder("Write a note, then press Enter")
        .on_submit(|st: &mut NotesState, v: String| {
            // The field always clears on submit (feature parity with the
            // pre-port demo); whether the note is actually added is decided
            // by `AddNote`'s validation inside the controller.
            st.draft.clear();
            let handle = Arc::clone(&st.controller);
            forgekit::spawn_local(async move {
                handle.add(v).await;
            });
        })));

        // The submitted notes, each a keyed row: the note text plus a Delete
        // button that removes *this* id. The stable key means deleting the
        // middle row drops the middle row's widget, not whatever now sits at
        // that index (spec §6.3).
        let rows: Vec<forgekit::FlexChild<NotesState>> = notes
            .iter()
            .map(|note| {
                let id = note.id;
                keyed(
                    id,
                    Row(vec![
                        any(text(note.text.clone()).size(20.0)),
                        any(SizedBox(Some(12.0), None)),
                        any(Button("Delete", move |st: &mut NotesState| {
                            let handle = Arc::clone(&st.controller);
                            forgekit::spawn_local(async move {
                                handle.delete(id).await;
                            });
                        })),
                    ]),
                )
            })
            .collect();
        children.push(any(scroll_view(FlexView::new(Axis::Vertical, rows))));

        any(Column(children))
    }
}

/// The notes exhibit, hosted as its own [`Component`]. `/notes` calls this
/// with no arguments (the placeholder contract, see the crate docs) — the
/// controller and its signals are entirely local to this screen's subtree.
pub fn notes_screen() -> AnyView<ShellState> {
    any(forgekit::component(NotesScreen))
}
