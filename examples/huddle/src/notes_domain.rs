//! Notes feature domain (task 05, wave 2).
//!
//! [`NotesController`] embeds a [`ControllerCore`] by composition (see
//! `templates/AGENTS.md` controller rules) and drives [`AddNote`]/[`DeleteNote`]
//! through it — the identical clean-architecture spine
//! [`crate::features::team::presentation::controllers::TeamController`] uses,
//! not a special case for having no backend. The one deliberate difference:
//! both use cases are purely local (no repository — there is nothing to
//! persist in this demo), so `execute`'s body never actually awaits anything.
//! `UseCase::execute` is still `async fn` (the trait requires it — see
//! `clean-signals`' `UseCase`), it just never hits a real `.await` point; the
//! task's "sync is fine" allowance is about behavior, not the trait shape.
//! [`AddNote::execute`] mirrors `UpdateMember`'s validate-shape
//! (`features::team::domain::use_cases::update_member`): a blank/whitespace
//! draft is rejected with a [`NotesFailure::Validation`] before it ever reaches
//! [`NotesController::notes`], surfaced by [`crate::screens::notes`] as an
//! error banner via [`clean_signals_forgekit::use_failure_listener`].

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use clean_signals::{ControllerCore, Failure, FailureSink, RunOptions, UseCase};
use forgekit::{RwSignal, Update};

/// One submitted note: a stable id (the [`forgekit::keyed`] identity the
/// screen reconciles rows by) plus its trimmed text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub id: u64,
    pub text: String,
}

/// Every failure mode the notes feature's use cases can produce.
///
/// Matched exhaustively by presentation code (never string-matched) per
/// `docs/CODE_STANDARDS.md`'s "Generic-over-`F` failures" convention — the
/// same shape as `crate::failure::TeamFailure`, kept as its own type rather
/// than reused because the notes feature is otherwise fully independent of
/// the team slice (per-feature `Failure` types are the documented fallback
/// when a feature doesn't share the app-wide enum — see
/// `crate::failure::TeamFailure`'s module doc: "small enough for a single
/// feature slice not to need its own", i.e. huddle's other single-failure
/// features do get their own).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotesFailure {
    /// A blank/whitespace-only note was submitted. Not retryable — retrying
    /// with the same blank input would just fail again.
    Validation(String),
}

impl fmt::Display for NotesFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NotesFailure::Validation(msg) => write!(f, "validation error: {msg}"),
        }
    }
}

impl Failure for NotesFailure {
    fn user_message(&self) -> String {
        match self {
            NotesFailure::Validation(msg) => msg.clone(),
        }
    }

    fn is_retryable(&self) -> bool {
        false
    }
}

/// [`AddNote`] params: the stable id the controller reserved plus the raw
/// draft text — trimmed/validated inside `execute`, never by the caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddNoteParams {
    pub id: u64,
    pub text: String,
}

/// Adds a note. Rejects a blank/whitespace draft with
/// [`NotesFailure::Validation`] before it ever becomes a [`Note`] — mirrors
/// `UpdateMember`'s validate-shape
/// (`features::team::domain::use_cases::update_member`).
#[derive(Default)]
pub struct AddNote;

#[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
#[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
impl UseCase for AddNote {
    type Params = AddNoteParams;
    type Output = Note;
    type Failure = NotesFailure;

    async fn execute(&self, params: AddNoteParams) -> Result<Note, NotesFailure> {
        let trimmed = params.text.trim();
        if trimmed.is_empty() {
            return Err(NotesFailure::Validation(
                "Note cannot be empty.".to_string(),
            ));
        }
        Ok(Note {
            id: params.id,
            text: trimmed.to_string(),
        })
    }
}

/// [`DeleteNote`] params: the id of the note to remove.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeleteNoteParams {
    pub id: u64,
}

/// Removes a note by id. Always succeeds (deleting an id already gone is a
/// harmless no-op at [`NotesController::delete`], not a use-case-level
/// error) — kept as its own [`UseCase`] anyway so the delete path runs
/// through the same [`ControllerCore::run`] activity/failure plumbing as
/// [`AddNote`], the same shape every use case in the app follows.
#[derive(Default)]
pub struct DeleteNote;

#[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
#[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
impl UseCase for DeleteNote {
    type Params = DeleteNoteParams;
    type Output = u64;
    type Failure = NotesFailure;

    async fn execute(&self, params: DeleteNoteParams) -> Result<u64, NotesFailure> {
        Ok(params.id)
    }
}

/// View model for the notes screen. Embeds a [`ControllerCore`] by
/// composition, mirroring [`crate::features::team::presentation::controllers::TeamController`].
pub struct NotesController {
    core: ControllerCore<NotesFailure>,
    add_note: AddNote,
    delete_note: DeleteNote,
    /// The submitted notes, in submission order. [`Note::id`] is the `keyed`
    /// identity the screen reconciles rows by across a middle-of-list delete.
    pub notes: RwSignal<Vec<Note>>,
    /// The next id to hand out — monotonic, never reused, so keys stay
    /// unique (mirrors the pre-port `examples/notes` demo's `next_id`).
    next_id: AtomicU64,
}

impl NotesController {
    pub fn new() -> Self {
        Self {
            core: ControllerCore::new(),
            add_note: AddNote,
            delete_note: DeleteNote,
            notes: RwSignal::new(Vec::new()),
            next_id: AtomicU64::new(0),
        }
    }

    /// Submits `text` as a new note. A blank/whitespace draft surfaces a
    /// [`NotesFailure::Validation`] on [`Self::failures`] and is dropped —
    /// [`Self::notes`] is left untouched (spec parity with the pre-port demo's
    /// "blank submit dropped" contract, now enforced in the domain layer
    /// instead of the screen).
    pub async fn add(&self, text: String) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let result = self
            .core
            .run(
                &self.add_note,
                AddNoteParams { id, text },
                RunOptions::default(),
            )
            .await;
        if let Ok(note) = result {
            self.notes.try_update(|notes| notes.push(note));
        }
    }

    /// Removes the note with `id`.
    pub async fn delete(&self, id: u64) {
        let result = self
            .core
            .run(
                &self.delete_note,
                DeleteNoteParams { id },
                RunOptions::default(),
            )
            .await;
        if let Ok(deleted_id) = result {
            self.notes
                .try_update(|notes| notes.retain(|n| n.id != deleted_id));
        }
    }

    /// The controller's failure event sink. The screen subscribes once, near
    /// the feature's UI root, to surface a validation failure as an error
    /// banner (see `clean_signals_forgekit::use_failure_listener`).
    pub fn failures(&self) -> &FailureSink<NotesFailure> {
        self.core.failures()
    }
}

impl Default for NotesController {
    fn default() -> Self {
        Self::new()
    }
}

impl AsRef<ControllerCore<NotesFailure>> for NotesController {
    fn as_ref(&self) -> &ControllerCore<NotesFailure> {
        &self.core
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forgekit::GetUntracked;

    #[tokio::test]
    async fn add_appends_a_trimmed_note_with_a_fresh_id() {
        let controller = NotesController::new();
        controller.add("  hello world  ".to_string()).await;

        let notes = controller.notes.get_untracked();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].id, 0);
        assert_eq!(notes[0].text, "hello world");
        controller.core.dispose();
    }

    #[tokio::test]
    async fn add_assigns_monotonic_unique_ids_across_calls() {
        let controller = NotesController::new();
        controller.add("first".to_string()).await;
        controller.add("second".to_string()).await;

        let notes = controller.notes.get_untracked();
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].id, 0);
        assert_eq!(notes[1].id, 1);
        controller.core.dispose();
    }

    #[tokio::test]
    async fn add_blank_draft_is_dropped_and_surfaces_a_validation_failure() {
        let controller = NotesController::new();

        let failures = std::sync::Arc::new(std::sync::Mutex::new(Vec::<NotesFailure>::new()));
        let sink = std::sync::Arc::clone(&failures);
        controller
            .failures()
            .subscribe(move |f: &NotesFailure| sink.lock().unwrap().push(f.clone()))
            .forget();

        controller.add("   ".to_string()).await;

        assert!(controller.notes.get_untracked().is_empty());
        let captured = failures.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(
            captured[0],
            NotesFailure::Validation("Note cannot be empty.".to_string())
        );
        assert!(!captured[0].is_retryable());
        controller.core.dispose();
    }

    #[tokio::test]
    async fn delete_removes_only_the_matching_id() {
        let controller = NotesController::new();
        controller.add("keep me".to_string()).await;
        controller.add("delete me".to_string()).await;
        controller.add("keep me too".to_string()).await;

        let target = controller.notes.get_untracked()[1].id;
        controller.delete(target).await;

        let notes = controller.notes.get_untracked();
        assert_eq!(notes.len(), 2);
        assert!(notes.iter().all(|n| n.id != target));
        controller.core.dispose();
    }

    #[tokio::test]
    async fn delete_unknown_id_is_a_harmless_no_op() {
        let controller = NotesController::new();
        controller.add("only note".to_string()).await;

        controller.delete(9999).await;

        assert_eq!(controller.notes.get_untracked().len(), 1);
        controller.core.dispose();
    }
}
