//! [`video_view`]: the one-line `frust::platform_view` composition over a
//! [`VideoPlayerHandle`] — the app-facing counterpart to the crate's own
//! [`crate::PlayerSession::view_type`]/[`crate::PlayerSession::params_json`]
//! pair, so an app writes a video the way it writes any other widget rather
//! than wiring those two calls by hand (mirrors
//! `examples/playground/src/pages/camera.rs`'s `preview_block`, this
//! crate's closest in-tree precedent for the same composition).

use frust::kurbo::Size;
use frust::{Get, PlatformViewView, TextureFit, TextureView, platform_view, texture_view};

use crate::VideoFit;

use super::handle::VideoPlayerHandle;

/// Build the `platform_view` slot hosting `handle`'s picture, filling its
/// container by default (`PlatformViewView`'s own `Expand` default — call
/// `.size(..)` on the result for a fixed-size embed instead).
///
/// Equivalent to
/// `platform_view(handle.session().view_type()).params_json(handle.session().params_json(fit))`
/// — no `.interactive()` call: this crate's picture is display-only in v1,
/// and `PlatformViewView`'s own default (non-interactive) is exactly the
/// contract this builder wants, so it is never called here (see
/// `crates/frust-widgets/src/platform_view.rs`'s *No input contract (v1)*
/// section).
///
/// # Composition: controls belong below the slot, not on top of it
///
/// On every platform this works — stack the slot and a row of transport
/// controls in a [`frust::Column`]:
///
/// ```no_run
/// use frust::{View, column, text};
/// use frust_video_player::api::{VideoPlayerHandle, video_view};
/// use frust_video_player::{PlayerOptions, VideoFit, VideoPlayer, VideoSource};
///
/// struct PlayerPage;
///
/// impl PlayerPage {
///     fn body(handle: &VideoPlayerHandle) -> impl View<()> {
///         column()
///             .child(video_view(handle, VideoFit::Contain))
///             .child(text("Play / Pause / Seek go here"))
///     }
/// }
///
/// let session = VideoPlayer::open(
///     VideoSource::Url("https://example.invalid/clip.mp4".to_owned()),
///     PlayerOptions::default(),
/// )
/// .expect("open");
/// let handle = VideoPlayerHandle::new(session);
/// let _page = PlayerPage::body(&handle);
/// ```
///
/// Overlaying controls **inside a [`frust::Stack`]**, on top of this slot,
/// is a **mobile Mode-B affordance only**. This crate's own doc explains why
/// macOS in particular cannot take it: the hosted view there is an *opaque
/// native sibling* under platform-view **Mode A** — the frust slot paints
/// nothing at all, and the OS composites the player's layer above the whole
/// frust surface (see the crate root doc's *The picture is a platform view,
/// not a widget* section), so any frust content a `Stack` places over this
/// slot is physically hidden underneath the native player on desktop. Even
/// where Mode B is available (mobile), `platform_view`'s own module doc's
/// *Z-shields* section is the contract a caller stacking chrome over this
/// slot must still follow, so a control drawn over the picture keeps
/// receiving its taps instead of losing them to the native view underneath.
/// See `docs/CODE_STANDARDS.md`'s Platform-View Conventions for the
/// Mode A/B rules this builder itself has no opt-in for.
pub fn video_view(handle: &VideoPlayerHandle, fit: VideoFit) -> PlatformViewView {
    let session = handle.session();
    platform_view(session.view_type()).params_json(session.params_json(fit))
}

/// Build the in-scene picture of a [`crate::VideoOutput::Texture`] session:
/// a `frust::texture_view` over [`VideoPlayerHandle::texture_output`]'s scene
/// texture, `fit` mapped onto [`TextureFit`], filling its container by
/// default (`.size(..)`, `.corner_radius(..)` and the other `TextureView`
/// builders apply as usual). `None` when the session has no texture output —
/// a native-view session, or a backend without a frame producer on this
/// target.
///
/// The picture's aspect comes from the frame currently shown
/// ([`frust::texture::TextureRegistration::frame_size`]), or before the first
/// frame from the size the platform reported ([`VideoPlayerHandle::video_size`],
/// a tracked read, so a `build` calling this re-runs once it is known); with
/// neither the texture fills the box.
///
/// Unlike [`video_view`]'s native slot, this is ordinary frust paint: it
/// clips to its box and to a `corner_radius`, scrolls and transforms with its
/// parent, and frust content stacked over it is drawn over it on every
/// platform — `VideoFit::Cover` crops inside the box rather than spilling.
pub fn video_texture_view(handle: &VideoPlayerHandle, fit: VideoFit) -> Option<TextureView> {
    let registration = handle.texture_output()?;
    let fit = match fit {
        VideoFit::Contain => TextureFit::Contain,
        VideoFit::Cover => TextureFit::Cover,
    };
    let view = texture_view(registration.scene_texture_id()).fit(fit);
    let natural = registration
        .frame_size()
        .or_else(|| handle.video_size.get());
    Some(match natural {
        Some((width, height)) => view.source_size(Size::new(f64::from(width), f64::from(height))),
        None => view,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PlayerOptions, VideoOutput, VideoPlayer, VideoSource};

    fn open() -> VideoPlayerHandle {
        let session = VideoPlayer::open(
            VideoSource::Url("https://example.invalid/clip.mp4".to_owned()),
            PlayerOptions::default(),
        )
        .expect("the mock backend opens an ordinary source");
        VideoPlayerHandle::new(session)
    }

    /// `PlatformViewView` exposes no accessor to introspect the `view_type`/
    /// `params_json` it was built with (by design — those are
    /// paint-time-only fields no test elsewhere in the tree reads back
    /// either), so this asserts the composed *inputs* `video_view` feeds it
    /// — the same target-gated spellings `crate::conformance` already pins
    /// — rather than the opaque built value; `video_view` itself is a
    /// direct, one-line forward of the two.
    #[test]
    fn video_view_feeds_platform_view_the_target_gated_view_type_and_params() {
        let handle = open();

        // Just proves the call composes without panicking.
        let _slot = video_view(&handle, VideoFit::Cover);

        assert_eq!(handle.session().view_type(), crate::VIEW_TYPE);
        assert_eq!(
            handle.session().params_json(VideoFit::Cover),
            crate::params_json_with(
                crate::SESSION_KEY,
                handle.session().mock_host().id(),
                VideoFit::Cover,
            )
        );
    }

    #[test]
    fn video_texture_view_needs_a_texture_session() {
        let native = open();
        assert!(video_texture_view(&native, VideoFit::Contain).is_none());

        let session = VideoPlayer::open(
            VideoSource::Url("https://example.invalid/clip.mp4".to_owned()),
            PlayerOptions {
                output: VideoOutput::Texture,
                ..PlayerOptions::default()
            },
        )
        .expect("the mock backend opens an ordinary source");
        let handle = VideoPlayerHandle::new(session);
        assert!(video_texture_view(&handle, VideoFit::Cover).is_some());
        // The reported size feeds the aspect before any frame was shown.
        handle.session().mock_host().emit_video_size(1_280, 720);
        assert!(video_texture_view(&handle, VideoFit::Contain).is_some());
    }
}
