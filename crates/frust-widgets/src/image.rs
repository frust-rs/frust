//! The `Image` widget: decode-once PNG/JPEG bitmaps painted
//! through the scene's [`Command::Image`](frust_scene::Command::Image).
//!
//! [`ImageSource`] is the decode-once, cheaply-clonable handle app code holds
//! (and re-passes every frame): [`ImageSource::decode`] runs the `image` crate
//! exactly once, at construction, and never again — [`ImageView::rebuild`]
//! only ever compares [`ImageSource::same`] (an `Arc` pointer check), so a
//! rebuild that re-supplies the *same* decoded source (even a fresh `.clone()`
//! of it) costs nothing beyond a refcount bump.
//!
//! [`Image`] is the declarative view-fn; [`ImageFit`] controls how the
//! widget's laid-out box relates to the image's natural pixel size at paint
//! time (`Fill` stretches, `Contain` letterboxes, `Cover` center-crops via
//! [`PaintScene::push_clip`]).

use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, View, Widget,
};
use kurbo::{Rect, Size};
use peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
use std::sync::Arc;

/// Error decoding image bytes via [`ImageSource::decode`].
///
/// Wraps the `image` crate's own decode error; callers that need to match on
/// the underlying failure can reach it through [`std::error::Error::source`].
#[derive(Debug)]
pub struct ImageError(image::ImageError);

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "failed to decode image: {}", self.0)
    }
}

impl std::error::Error for ImageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// A decode-once, cheaply-clonable handle around a decoded RGBA8 image.
///
/// Identity is the inner `Arc` pointer: [`ImageSource::same`] compares by
/// pointer, not pixel content, in O(1) regardless of image size — the signal
/// [`ImageView`]'s `rebuild` relies on to know "same image, nothing to redo"
/// without ever touching the decoder again.
#[derive(Clone, Debug)]
pub struct ImageSource {
    data: Arc<ImageData>,
}

impl ImageSource {
    /// Decode PNG/JPEG bytes into straight-alpha RGBA8 once, wrapping the
    /// result in a shareable handle. Callers own the resulting `ImageSource`
    /// (typically stashing it in app state) and re-pass it into [`Image`]
    /// every frame — this never re-decodes.
    pub fn decode(bytes: &[u8]) -> Result<Self, ImageError> {
        let decoded = image::load_from_memory(bytes).map_err(ImageError)?;
        let rgba = decoded.to_rgba8();
        let (width, height) = rgba.dimensions();
        Ok(Self::from_rgba8(rgba.into_raw(), width, height))
    }

    /// Wrap already-decoded straight-alpha RGBA8 pixel data directly (e.g. a
    /// procedurally generated image, or a test fixture) without going through
    /// the `image` crate's decoder.
    ///
    /// `pixels.len()` must equal `width * height * 4`; a mismatched length is
    /// a caller bug and is not checked here (mirrors `peniko::ImageData`'s own
    /// contract — the renderer will simply read `width * height * 4` bytes).
    pub fn from_rgba8(pixels: Vec<u8>, width: u32, height: u32) -> Self {
        Self {
            data: Arc::new(ImageData {
                data: Blob::from(pixels),
                format: ImageFormat::Rgba8,
                alpha_type: ImageAlphaType::Alpha,
                width,
                height,
            }),
        }
    }

    /// Whether `self` and `other` share the same decoded data (`Arc` pointer
    /// identity, not content equality) — never true for two independently
    /// decoded sources even if their bytes matched, since each decode
    /// allocates a fresh `Arc`.
    pub fn same(&self, other: &ImageSource) -> bool {
        Arc::ptr_eq(&self.data, &other.data)
    }

    /// The natural (decoded pixel) size.
    pub fn natural_size(&self) -> Size {
        Size::new(self.data.width as f64, self.data.height as f64)
    }

    /// Borrow the decoded `peniko::ImageData` this source wraps, for painting.
    ///
    /// This returns a borrow of already-decoded pixels and triggers no decode.
    /// The source caches pixel data once at construction via [`Self::decode`];
    /// this accessor hands back a reference to that cache. Calling this
    /// repeatedly with the same [`ImageSource`] is zero-cost and does not
    /// re-trigger the image decoder.
    pub fn image_data(&self) -> &ImageData {
        &self.data
    }
}

/// How an [`Image`]'s laid-out box relates to its source's natural pixel size
/// at paint time.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ImageFit {
    /// Stretch the image to exactly fill the widget's box, ignoring aspect
    /// ratio.
    Fill,
    /// Scale the image (preserving aspect ratio) to fit entirely within the
    /// widget's box; the non-fitting axis is letterboxed (centered, with
    /// empty space on either side).
    #[default]
    Contain,
    /// Scale the image (preserving aspect ratio) to cover the widget's box
    /// entirely; the overflowing axis is center-cropped via a clip.
    Cover,
}

/// Compute the rect (relative to the widget's own origin) the image, scaled
/// per `fit`, should be drawn into.
///
/// For [`ImageFit::Fill`] this exactly equals `(0, 0, dest.width,
/// dest.height)`. For [`ImageFit::Contain`] it is centered and no larger than
/// `dest` on either axis (possibly smaller on one — the letterbox). For
/// [`ImageFit::Cover`] it is centered and no smaller than `dest` on either
/// axis (possibly larger on one — the caller must clip to `dest` around it).
///
/// A degenerate (zero-area) `natural` or `dest` collapses to `dest` itself
/// (nothing sensible to scale).
fn fit_rect(natural: Size, dest: Size, fit: ImageFit) -> Rect {
    if natural.width <= 0.0 || natural.height <= 0.0 || dest.width <= 0.0 || dest.height <= 0.0 {
        return Rect::new(0.0, 0.0, dest.width, dest.height);
    }
    match fit {
        ImageFit::Fill => Rect::new(0.0, 0.0, dest.width, dest.height),
        ImageFit::Contain => centered_scaled_rect(
            natural,
            dest,
            (dest.width / natural.width).min(dest.height / natural.height),
        ),
        ImageFit::Cover => centered_scaled_rect(
            natural,
            dest,
            (dest.width / natural.width).max(dest.height / natural.height),
        ),
    }
}

/// Scale `natural` by `scale` and center the result within `dest`.
fn centered_scaled_rect(natural: Size, dest: Size, scale: f64) -> Rect {
    let w = natural.width * scale;
    let h = natural.height * scale;
    let x = (dest.width - w) / 2.0;
    let y = (dest.height - h) / 2.0;
    Rect::new(x, y, x + w, y + h)
}

/// A declarative description of an image.
///
/// Construct with [`Image`]; set the fit mode with [`ImageView::fit`]
/// (defaults to [`ImageFit::Contain`]).
pub struct ImageView {
    source: ImageSource,
    fit: ImageFit,
}

/// Create an image view over a decoded [`ImageSource`], defaulting to
/// [`ImageFit::Contain`].
#[allow(non_snake_case)]
pub fn Image(source: ImageSource) -> ImageView {
    ImageView {
        source,
        fit: ImageFit::default(),
    }
}

impl ImageView {
    /// Set how the image's natural size relates to its laid-out box at paint
    /// time.
    pub fn fit(mut self, fit: ImageFit) -> Self {
        self.fit = fit;
        self
    }
}

impl<State: 'static> View<State> for ImageView {
    type Element = ImageWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ImageWidget {
        ImageWidget {
            source: self.source.clone(),
            fit: self.fit,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ImageWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        // `same()` is an Arc-pointer check, never a decode: swapping to a
        // genuinely different source is the only path that touches the
        // decoder, and only inside `ImageSource::decode` at construction —
        // never here.
        if !prev.source.same(&self.source) {
            element.source = self.source.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.fit != self.fit {
            element.fit = self.fit;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

/// The retained widget for an [`ImageView`].
pub struct ImageWidget {
    source: ImageSource,
    fit: ImageFit,
}

impl Widget for ImageWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // A tight constraint (e.g. this image sits inside a `SizedBox`) wins
        // outright; otherwise the image prefers its natural size, clamped
        // into the incoming bounds — mirrors `TextWidget`'s intrinsic-size
        // pattern. `fit` only affects how that box's content is scaled at
        // paint time, never the box itself.
        if bc.is_tight() {
            return bc.max();
        }
        bc.constrain(self.source.natural_size())
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let natural = self.source.natural_size();
        let dest_size = ctx.size();
        let local = fit_rect(natural, dest_size, self.fit);
        let origin = ctx.origin();
        let absolute = local + origin.to_vec2();

        let needs_clip = self.fit == ImageFit::Cover
            && (local.width() > dest_size.width || local.height() > dest_size.height);
        if needs_clip {
            scene.push_clip(origin, dest_size);
        }
        scene.draw_image(self.source.image_data(), absolute);
        if needs_clip {
            scene.pop_clip();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::BuildCtx;
    use kurbo::Point;

    fn rgba(width: u32, height: u32) -> ImageSource {
        ImageSource::from_rgba8(vec![0u8; (width * height * 4) as usize], width, height)
    }

    fn build(view: &ImageView) -> ImageWidget {
        let mut counter = 0u64;
        <ImageView as View<()>>::build(view, &mut BuildCtx::new(&mut counter))
    }

    // -- ImageSource ---------------------------------------------------

    #[test]
    fn from_rgba8_reports_natural_size() {
        let source = rgba(4, 8);
        assert_eq!(source.natural_size(), Size::new(4.0, 8.0));
    }

    #[test]
    fn same_is_true_only_for_a_shared_arc() {
        let a = rgba(2, 2);
        let b = a.clone();
        let c = rgba(2, 2); // independently constructed, same pixels, different Arc.
        assert!(a.same(&b), "a clone shares the Arc");
        assert!(!a.same(&c), "an independently built source never matches");
    }

    #[test]
    fn decode_error_reports_the_underlying_image_crate_failure() {
        let err = ImageSource::decode(b"not an image").unwrap_err();
        // The `Display` impl should at least name what went wrong, and
        // `source()` must recover the underlying `image::ImageError`.
        assert!(err.to_string().contains("failed to decode image"));
        assert!(std::error::Error::source(&err).is_some());
    }

    #[test]
    fn decode_a_two_by_two_png_round_trips_dimensions_and_alpha_type() {
        // A minimal, hand-encoded 2x2 RGBA PNG (generated once and embedded as
        // bytes — no filesystem/asset dependency for the test).
        let png = two_by_two_png();
        let source = ImageSource::decode(&png).expect("valid PNG decodes");
        assert_eq!(source.natural_size(), Size::new(2.0, 2.0));
        assert_eq!(source.image_data().alpha_type, ImageAlphaType::Alpha);
        assert_eq!(source.image_data().format, ImageFormat::Rgba8);
        assert_eq!(source.image_data().data.len(), 2 * 2 * 4);
    }

    /// Encode a tiny 2x2 RGBA PNG in-process via the `image` crate itself, so
    /// the decode-path test above has no external fixture file to keep in
    /// sync.
    fn two_by_two_png() -> Vec<u8> {
        use image::{ImageEncoder, codecs::png::PngEncoder};
        let pixels: [u8; 2 * 2 * 4] = [
            255, 0, 0, 255, // red
            0, 255, 0, 255, // green
            0, 0, 255, 255, // blue
            255, 255, 0, 255, // yellow
        ];
        let mut out = Vec::new();
        PngEncoder::new(&mut out)
            .write_image(&pixels, 2, 2, image::ExtendedColorType::Rgba8)
            .expect("encoding a tiny in-memory PNG must not fail");
        out
    }

    // -- fit math --------------------------------------------------------

    #[test]
    fn fill_always_matches_dest_regardless_of_aspect_ratio() {
        let rect = fit_rect(
            Size::new(10.0, 20.0),
            Size::new(100.0, 50.0),
            ImageFit::Fill,
        );
        assert_eq!(rect, Rect::new(0.0, 0.0, 100.0, 50.0));
    }

    #[test]
    fn contain_letterboxes_a_wider_dest_around_a_taller_natural_image() {
        // Natural 1:2 (portrait) into a 200x100 (landscape) dest: height-bound
        // scale (100/2 = 50), width shrinks to 50 and is centered.
        let rect = fit_rect(
            Size::new(100.0, 200.0),
            Size::new(200.0, 100.0),
            ImageFit::Contain,
        );
        assert_eq!(rect.width(), 50.0);
        assert_eq!(rect.height(), 100.0);
        assert_eq!(rect.x0, 75.0); // (200 - 50) / 2
        assert_eq!(rect.y0, 0.0);
    }

    #[test]
    fn contain_letterboxes_a_taller_dest_around_a_wider_natural_image() {
        // Natural 2:1 (landscape) into a 100x200 (portrait) dest: width-bound
        // scale (100/2 = 50), height shrinks to 50 and is centered.
        let rect = fit_rect(
            Size::new(200.0, 100.0),
            Size::new(100.0, 200.0),
            ImageFit::Contain,
        );
        assert_eq!(rect.width(), 100.0);
        assert_eq!(rect.height(), 50.0);
        assert_eq!(rect.x0, 0.0);
        assert_eq!(rect.y0, 75.0); // (200 - 50) / 2
    }

    #[test]
    fn contain_preserves_aspect_ratio_at_a_non_trivial_ratio() {
        // A 3:4 natural image into a 300x300 square dest: width-bound scale
        // (300/3 = 100) since 3:4 is portrait-ish; check aspect is preserved.
        let rect = fit_rect(
            Size::new(300.0, 400.0),
            Size::new(300.0, 300.0),
            ImageFit::Contain,
        );
        let natural_aspect = 300.0 / 400.0;
        let rect_aspect = rect.width() / rect.height();
        assert!(
            (natural_aspect - rect_aspect).abs() < 1e-9,
            "contain must preserve the natural aspect ratio"
        );
        assert!(rect.width() <= 300.0 && rect.height() <= 300.0);
    }

    #[test]
    fn cover_crops_a_wider_dest_around_a_taller_natural_image() {
        // Natural 1:2 (portrait) into a 200x100 (landscape) dest: width-bound
        // scale (200/1 = 200) so the image overflows vertically and is
        // center-cropped.
        let rect = fit_rect(
            Size::new(100.0, 200.0),
            Size::new(200.0, 100.0),
            ImageFit::Cover,
        );
        assert_eq!(rect.width(), 200.0);
        assert_eq!(rect.height(), 400.0);
        assert_eq!(rect.x0, 0.0);
        assert_eq!(rect.y0, -150.0); // (100 - 400) / 2
    }

    #[test]
    fn cover_at_least_covers_dest_on_both_axes() {
        let dest = Size::new(150.0, 90.0);
        let rect = fit_rect(Size::new(37.0, 51.0), dest, ImageFit::Cover);
        assert!(rect.width() >= dest.width - 1e-9);
        assert!(rect.height() >= dest.height - 1e-9);
    }

    #[test]
    fn fit_rect_collapses_to_dest_for_degenerate_natural_size() {
        let dest = Size::new(40.0, 20.0);
        for fit in [ImageFit::Fill, ImageFit::Contain, ImageFit::Cover] {
            let rect = fit_rect(Size::ZERO, dest, fit);
            assert_eq!(rect, Rect::new(0.0, 0.0, dest.width, dest.height));
        }
    }

    // -- layout ------------------------------------------------------------

    #[test]
    fn loose_constraints_choose_the_natural_size() {
        let view = Image(rgba(40, 30));
        let mut w: ImageWidget = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        assert_eq!(size, Size::new(40.0, 30.0));
    }

    #[test]
    fn tight_constraints_force_the_exact_box_regardless_of_natural_size() {
        let view = Image(rgba(40, 30));
        let mut w: ImageWidget = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::tight(Size::new(200.0, 10.0)));
        assert_eq!(size, Size::new(200.0, 10.0));
    }

    #[test]
    fn loose_constraints_clamp_a_natural_size_larger_than_the_bound() {
        let view = Image(rgba(1000, 1000));
        let mut w: ImageWidget = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(50.0, 50.0)));
        assert_eq!(size, Size::new(50.0, 50.0));
    }

    // -- paint / cache identity --------------------------------------------

    /// A minimal recording [`PaintScene`] capturing only what `Image::paint`
    /// touches: clip push/pop and the drawn image's dest rect + dimensions.
    #[derive(Default)]
    struct RecordingScene {
        clips: Vec<(Point, Size)>,
        pops: u32,
        images: Vec<(u32, u32, Rect)>,
    }

    impl PaintScene for RecordingScene {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: peniko::Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn push_clip(&mut self, origin: Point, size: Size) {
            self.clips.push((origin, size));
        }
        fn pop_clip(&mut self) {
            self.pops += 1;
        }
        fn draw_image(&mut self, data: &ImageData, dest: Rect) {
            self.images.push((data.width, data.height, dest));
        }
    }

    fn paint_at(w: &mut ImageWidget, origin: Point, size: Size) -> RecordingScene {
        let mut ctx = PaintCtx::new(origin, size);
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        scene
    }

    #[test]
    fn fill_paints_the_full_dest_rect_with_no_clip() {
        let view = Image(rgba(10, 10)).fit(ImageFit::Fill);
        let mut w: ImageWidget = build(&view);
        let scene = paint_at(&mut w, Point::new(0.0, 0.0), Size::new(50.0, 20.0));
        assert!(scene.clips.is_empty(), "Fill never clips");
        assert_eq!(scene.pops, 0);
        assert_eq!(scene.images.len(), 1);
        let (w_px, h_px, dest) = scene.images[0];
        assert_eq!((w_px, h_px), (10, 10));
        assert_eq!(dest, Rect::new(0.0, 0.0, 50.0, 20.0));
    }

    #[test]
    fn contain_paints_a_letterboxed_rect_with_no_clip() {
        let view = Image(rgba(100, 200)).fit(ImageFit::Contain);
        let mut w: ImageWidget = build(&view);
        let scene = paint_at(&mut w, Point::new(5.0, 5.0), Size::new(200.0, 100.0));
        assert!(scene.clips.is_empty(), "Contain never overflows dest");
        let (_, _, dest) = scene.images[0];
        // Same shape as the pure fit_rect test, offset by the paint origin
        // (5, 5).
        assert_eq!(dest.width(), 50.0);
        assert_eq!(dest.x0, 5.0 + 75.0);
        assert_eq!(dest.y0, 5.0);
    }

    #[test]
    fn cover_clips_to_dest_around_an_overflowing_scaled_rect() {
        let view = Image(rgba(100, 200)).fit(ImageFit::Cover);
        let mut w: ImageWidget = build(&view);
        let origin = Point::new(2.0, 3.0);
        let dest_size = Size::new(200.0, 100.0);
        let scene = paint_at(&mut w, origin, dest_size);
        assert_eq!(
            scene.clips,
            vec![(origin, dest_size)],
            "Cover clips to the widget's own box"
        );
        assert_eq!(scene.pops, 1, "the clip must be popped");
        let (_, _, dest) = scene.images[0];
        // The drawn rect overflows the widget's dest on the vertical axis
        // (matches the pure cover fit-math test, offset by `origin`).
        assert!(dest.height() > dest_size.height);
    }

    #[test]
    fn rebuild_with_a_cloned_source_keeps_the_widgets_arc_identity() {
        let source = rgba(8, 8);
        let view: ImageView = Image(source.clone());
        let mut w: ImageWidget = build(&view);
        let original_source = w.source.clone();

        // The next view re-passes a *clone* of the same source (the ordinary
        // "app_logic re-runs every frame" shape) — never a fresh decode.
        let next: ImageView = Image(source.clone());
        let mut counter = 0u64;
        <ImageView as View<()>>::rebuild(&next, &view, &mut w, &mut BuildCtx::new(&mut counter));

        assert!(
            w.source.same(&original_source),
            "a cloned-source rebuild must keep the same decoded Arc, never re-decode"
        );
    }

    #[test]
    fn rebuild_with_a_different_source_replaces_the_widgets_source() {
        let first = rgba(8, 8);
        let second = rgba(16, 16);
        let view: ImageView = Image(first.clone());
        let mut w: ImageWidget = build(&view);

        let next: ImageView = Image(second.clone());
        let mut counter = 0u64;
        let flags = <ImageView as View<()>>::rebuild(
            &next,
            &view,
            &mut w,
            &mut BuildCtx::new(&mut counter),
        );

        assert!(w.source.same(&second));
        assert!(!w.source.same(&first));
        assert!(flags.contains(ChangeFlags::LAYOUT));
        assert!(flags.contains(ChangeFlags::PAINT));
    }

    #[test]
    fn rebuild_with_a_different_fit_flags_paint_only() {
        let source = rgba(8, 8);
        let view: ImageView = Image(source.clone()).fit(ImageFit::Contain);
        let mut w: ImageWidget = build(&view);

        let next: ImageView = Image(source).fit(ImageFit::Cover);
        let mut counter = 0u64;
        let flags = <ImageView as View<()>>::rebuild(
            &next,
            &view,
            &mut w,
            &mut BuildCtx::new(&mut counter),
        );

        assert_eq!(w.fit, ImageFit::Cover);
        assert!(flags.contains(ChangeFlags::PAINT));
        assert!(!flags.contains(ChangeFlags::LAYOUT));
    }
}
