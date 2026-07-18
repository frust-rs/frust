//! About page (`/you/settings/about`).
//!
//! **Placeholder** (Phase C task: About — app version, credits, and licenses).
//! Today it renders a short about blurb.

use forgekit::AnyView;

use crate::HuddleState;
use crate::screens::{placeholder_body, scaffold};

/// The about page.
pub fn about_screen() -> AnyView<HuddleState> {
    scaffold(
        "About",
        placeholder_body(
            "Huddle — the ForgeKit showcase app. Version, credits, licenses (Phase C).",
        ),
    )
}
