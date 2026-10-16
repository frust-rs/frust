//! Platform-view command publication: the differ's backlog mapped into the
//! FFI-boundary shape ([`to_pv_command`]) and served as wire JSON, plus the
//! live-slot replay a surface recreate needs and the overlay range fold each
//! painted frame snapshots (`FRUST_OVERLAYS`).
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
fn to_pv_command(cmd: &ViewCommand, scale: f64) -> Option<crate::ffi_support::PlatformViewCommand> {
    fn to_pv_rect(rect: kurbo::Rect, scale: f64) -> crate::ffi_support::PvRect {
        crate::ffi_support::PvRect {
            x: rect.x0 * scale,
            y: rect.y0 * scale,
            w: rect.width() * scale,
            h: rect.height() * scale,
        }
    }
    Some(match cmd {
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
            ..
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
        // The overlay ops (`FRUST_OVERLAYS` only — plain `ingest` never
        // produces them): `FrustViewHost` stacks one overlay view directly
        // above the slot's wrapper and hands its layer back through
        // `frust_overlay_attach`. The range is informational for the host; the
        // render thread pairs ranges with frames itself.
        ViewCommand::Overlay {
            overlay_id,
            above_slot_id,
            rect,
            command_range,
        } => crate::ffi_support::PlatformViewCommand::Overlay {
            overlay_id: *overlay_id,
            above_slot_id: *above_slot_id,
            rect: to_pv_rect(*rect, scale),
            range_start: command_range.start as u64,
            range_end: command_range.end as u64,
        },
        ViewCommand::DisposeOverlay { overlay_id } => {
            crate::ffi_support::PlatformViewCommand::DisposeOverlay {
                overlay_id: *overlay_id,
            }
        }
    })
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
        // pairing goes with it. (Overlay surfaces are dropped and rebuilt by
        // the render side, which owns them, around its own surface reinstall.)
        self.platform_view_due.clear();
        self.fold_overlay_commands();
    }

    /// Fold every differ command pushed since the last fold into the overlay
    /// ranges (`Overlay` sets one, `DisposeOverlay` drops one) — run after
    /// each call that can push one and before Swift's next poll acknowledges
    /// (compacts) it. A no-op with overlays off.
    pub(crate) fn fold_overlay_commands(&mut self) {
        if !crate::ffi_glue::overlays_enabled() {
            return;
        }
        let mut ranges = crate::ffi_glue::overlay_ranges();
        let (generation, backlog) = self.platform_views.commands();
        let Some(folded) = ranges.begin_fold(generation) else {
            return;
        };
        // Everything at or below the folded generation is already in the
        // fold; the rest of the backlog is new.
        let (_, seen) = self.platform_views.commands_up_to(folded);
        for command in &backlog[seen.len()..] {
            match command {
                ViewCommand::Overlay {
                    overlay_id,
                    command_range,
                    ..
                } => ranges.set(*overlay_id, command_range.clone()),
                ViewCommand::DisposeOverlay { overlay_id } => ranges.dispose(*overlay_id),
                _ => {}
            }
        }
        ranges.finish_fold(generation);
    }

    /// Fold, then snapshot the overlays' current ranges — the cuts the frame
    /// painted just now is drawn with, carried to the render thread beside its
    /// scene. Empty with overlays off.
    pub(crate) fn overlay_frame_cuts(&mut self) -> Vec<crate::ffi_support::OverlayCutSpec> {
        if !crate::ffi_glue::overlays_enabled() {
            return Vec::new();
        }
        self.fold_overlay_commands();
        crate::ffi_glue::overlay_ranges().cuts()
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
        // Nothing the acknowledgement compacts may go unfolded.
        self.fold_overlay_commands();
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
            .filter_map(|cmd| to_pv_command(cmd, scale))
            .collect();
        crate::ffi_support::platform_view_commands_json(generation, ack_generation, &mapped)
    }
}
