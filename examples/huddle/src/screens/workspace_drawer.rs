//! Workspace switcher (`/workspace-switcher`) — a top drawer.
//!
//! **Stub** (Phase C task: Workspace switcher — the list of workspaces the top
//! drawer reveals, with the slide-down top-drawer transition).
//!
//! # Transition note (skeleton)
//!
//! The plan calls for a transparent push with a **slide-down top-drawer
//! transition — the `SlideUp` preset's inverse, app-tuned**. The framework
//! ships `PageTransition::SlideUp` but no `SlideDown` preset, and adding a
//! framework transition preset is out of this examples-only task's scope, so
//! the skeleton routes `/workspace-switcher` through the shell's normal
//! navigator transition. Phase C (or a small framework task) adds the tuned
//! slide-down; the route + stub page are in place today.

use forgekit::AnyView;

use crate::HuddleState;
use crate::screens::{placeholder_body, scaffold};

/// The workspace-switcher stub page.
pub fn workspace_drawer_screen() -> AnyView<HuddleState> {
    scaffold(
        "Workspaces",
        placeholder_body("Workspace switcher: a slide-down top drawer of workspaces (Phase C)."),
    )
}
