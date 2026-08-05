//! Platform-view command publication: the differ's backlog mapped into the
//! FFI-boundary shape ([`to_pv_command`]) and served as wire JSON, plus the
//! live-slot replay a surface recreate needs.
//!
//! The present-sync release gate this consults lives in
//! [`super::present_sync`]; the differ itself is `frust_shell_common`'s.

use frust_shell_common::platform_view::ViewCommand;
use frust_shell_common::sanitize_scale;

use super::IosAppHandle;

/// Map one differ [`ViewCommand`] onto the host-testable
/// [`crate::ffi_support::PlatformViewCommand`] shape, converting its logical,
/// absolute-window rect/clip into physical px (`* scale`) — the one place
/// this crate crosses from `frust_shell_common`'s `kurbo::Rect`-typed
/// vocabulary into the FFI-boundary-safe plain-`f64` one (see
/// [`crate::ffi_support::PvRect`]'s doc comment for why `kurbo` can't appear
/// in `ffi_support` itself).
fn to_pv_command(cmd: &ViewCommand, scale: f64) -> crate::ffi_support::PlatformViewCommand {
    fn to_pv_rect(rect: kurbo::Rect, scale: f64) -> crate::ffi_support::PvRect {
        crate::ffi_support::PvRect {
            x: rect.x0 * scale,
            y: rect.y0 * scale,
            w: rect.width() * scale,
            h: rect.height() * scale,
        }
    }
    match cmd {
        ViewCommand::Create {
            slot_id,
            view_type,
            params_json,
            interactive,
        } => crate::ffi_support::PlatformViewCommand::Create {
            slot_id: *slot_id,
            view_type: view_type.clone(),
            params_json: params_json.clone(),
            interactive: *interactive,
        },
        ViewCommand::Update {
            slot_id,
            rect,
            clip,
            visible,
            shields,
        } => crate::ffi_support::PlatformViewCommand::Update {
            slot_id: *slot_id,
            rect: to_pv_rect(*rect, scale),
            clip: clip.map(|c| to_pv_rect(c, scale)),
            visible: *visible,
            shields: shields.iter().map(|s| to_pv_rect(*s, scale)).collect(),
        },
        ViewCommand::UpdateParams {
            slot_id,
            params_json,
        } => crate::ffi_support::PlatformViewCommand::UpdateParams {
            slot_id: *slot_id,
            params_json: params_json.clone(),
        },
        ViewCommand::Dispose { slot_id } => {
            crate::ffi_support::PlatformViewCommand::Dispose { slot_id: *slot_id }
        }
    }
}

impl IosAppHandle {
    /// Re-emit `Create`+`Update` for every currently-live platform-view slot,
    /// for the inline path's successful surface
    /// recreate ([`crate::ffi_glue::recover_surface`]) to call. Delegates to
    /// [`PlatformViewState::reset_for_surface_recreate`](frust_shell_common::PlatformViewState::reset_for_surface_recreate).
    pub(crate) fn reset_platform_views_for_surface_recreate(&mut self) {
        self.platform_views.reset_for_surface_recreate();
        // The replay supersedes every held batch, and the frames those batches
        // were paired with belong to the surface that just went away — so the
        // pairing goes with it.
        self.platform_view_due.clear();
    }

    /// `frust_platform_view_commands_json`'s core: acknowledge `ack_generation`
    /// (compacting the differ's backlog), then
    /// serialize whatever remains into the wire JSON both mobile shells'
    /// peek getters return verbatim (`frust-shell-android`'s
    /// `nativePlatformViewCommands` shares the byte-identical
    /// schema). `None` on the no-change fast path.
    ///
    /// Converts each command's logical, absolute-window rect/clip into
    /// **physical** px (`* scale`) at this FFI boundary — the
    /// physical-at-FFI/logical-inside rule, applied outbound (matches every
    /// other outbound-geometry seam in this shell).
    pub(crate) fn platform_view_commands_json(&mut self, ack_generation: u64) -> Option<String> {
        self.platform_views.acknowledge(ack_generation);
        // Keep the gate's bookkeeping in step with the differ's: a batch
        // the host has applied needs no pairing.
        self.platform_view_due.acknowledge(ack_generation);
        // Under present-sync, serve only the prefix whose producing frame is on
        // screen — with the present issued from this thread, "on screen" means
        // "presented in this very transaction" (see `platform_view_due`).
        // Otherwise serve the whole backlog, byte-for-byte as before.
        let (generation, commands) = if self.present_sync {
            let releasable = self
                .platform_view_due
                .releasable_generation(self.presented_frame_id, self.executor.submitted_frame_id());
            self.platform_views.commands_up_to(releasable)
        } else {
            self.platform_views.commands()
        };
        let scale = sanitize_scale(self.scale) as f64;
        let mapped: Vec<crate::ffi_support::PlatformViewCommand> = commands
            .iter()
            .map(|cmd| to_pv_command(cmd, scale))
            .collect();
        crate::ffi_support::platform_view_commands_json(generation, ack_generation, &mapped)
    }
}
