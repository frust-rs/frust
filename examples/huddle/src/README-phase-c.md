# Huddle — Phase C hub-file contract

The skeleton (task 10) **finalized every hub file** in `examples/huddle`. Phase C
screen tasks are parallelizable *only* because each edits a disjoint set of
files. This document is the contract that keeps them disjoint.

## A Phase C screen task edits ONLY:

- its own **`src/screens/<name>.rs`** (the one screen it owns), and
- its own **`src/features/<name>/**`** (that feature's controller + use cases),
  adding a `pub mod <name>;` line to `src/features/mod.rs` **only if** its
  feature slice is new.

## A Phase C screen task must NOT touch (the finalized hub files):

| File / dir | What it owns |
|---|---|
| `src/lib.rs` | `HuddleApp`/`HuddleState`, the root `Stack` + overlay slot, context provision |
| `src/shell.rs` | the `Tab` enum + the bottom navigation bar |
| `src/routes.rs` | the full router table (every route already points at its screen fn) |
| `src/screens/mod.rs` | the screen module list + the shared `scaffold`/`placeholder_body` helpers |
| `src/mock/**` | the shared static dataset + accessor fns (read via `mock::*`, never edited) |
| `src/ui/**` | the toast/snackbar overlay service (`ToastController`) |
| `src/failure.rs` | the app-wide `HuddleFailure` enum |

If a Phase C screen genuinely needs a new route, a new mock accessor, a new
shell tab, or a new `ui` service, that is a **hub change** — it must be raised
with the conductor and land as its own task, not smuggled into a screen task.
The seams the skeleton designed for Phase C:

- **Toasts / undo:** read `use_context::<ui::toast::ToastController>()` and call
  `show` / `show_with_action`. The handle is provided under the root owner.
- **Mock data:** call the `mock::*` accessor fns (`users`, `channels`, `dms`,
  `messages_for`, `firehose_messages`, `activity`). They return owned values;
  seed your controller's signals from them.
- **Navigation:** route page builders already pass each navigating screen a
  cloned `NavigatorController<HuddleState>`; push detail pages through it.
- **Theme swap:** the `set_app_theme` single-call-site lives in
  `features::settings`; do not add a second `set_app_theme`/`clear_app_theme`
  call site anywhere.
