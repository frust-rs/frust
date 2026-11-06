//! CoreText as a colour-bitmap rasterizer for `sbix` glyphs.
//!
//! Apple Color Emoji on recent OS releases stores its strikes in formats
//! (`emjc`, `dupe`, `flip` records) that `glifo`'s `skrifa` does not decode,
//! while CoreText decodes every `sbix` record type. [`rasterize_glyph`] has
//! CoreText draw one glyph into a throwaway bitmap and hands the pixels back,
//! ready to be uploaded to the glyph atlas by the caller.
//!
//! # iOS only in practice
//!
//! On macOS 27 a `CTFontDrawGlyphs` of an `sbix` emoji glyph into a bitmap
//! context faults inside ImageIO's PNG reader (a bus error in any process,
//! sandboxed or not), while the same call on iOS 27 draws correctly. macOS
//! faces store their strikes as `png ` records, which the engine decodes
//! itself, so callers must reach this rasterizer only on iOS.
//!
//! # Font identity
//!
//! iOS font fallback can hand back a hidden, dot-prefixed face such as
//! `.AppleColorEmojiUI`. CoreText's by-name lookup cannot open those; it
//! silently returns a fallback text font, and glyph ids meant for the emoji
//! face would then index into the wrong font. Every open therefore verifies
//! identity before any glyph is drawn:
//!
//! 1. the opened font's PostScript name must equal the requested name (and
//!    its glyph count the caller's `expected_glyphs`);
//! 2. otherwise, for a dot-prefixed request, the public sibling is derived
//!    (drop the leading `.` and a trailing `UI`: `.AppleColorEmojiUI` to
//!    `AppleColorEmoji`) and accepted only when its PostScript name equals the
//!    derived name and its glyph count equals the caller's `expected_glyphs`.
//!    The system collection stores the two faces with shared `sbix`, `glyf`,
//!    `cmap` and `maxp` tables, so a glyph id means the same image in both;
//! 3. otherwise the open is refused and the glyph counts as skipped.
//!
//! # Placement convention
//!
//! A [`RasterGlyph`] is positioned relative to the pen at the baseline, in
//! device pixels, y down (the engine's convention): the pixmap's top-left
//! texel lies at `(pen_x + origin_x, baseline_y - origin_y)`. `origin_x` is
//! therefore the left bearing (negative when the image starts left of the pen)
//! and `origin_y` the distance from the baseline up to the image's top edge.
//! One pixmap texel is one device pixel; the glyph is drawn with the font at
//! `ppem` points, so a face drawn at pixel size `ppem` needs no further scale.
//!
//! # Thread safety
//!
//! Apple documents every Core Text function as thread-safe and CTFont objects
//! as usable from several threads at once (the rationale `hvgl/ffi.rs` relies
//! on). Nothing is shared here regardless: each call opens its own `CTFont`
//! and draws into its own `CGBitmapContext`, so concurrent calls from any
//! thread touch no common mutable state.

use core::ffi::c_void;
use core::ptr::NonNull;

use objc2_core_foundation::{CFString, CGPoint, CGRect};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextGetData, CGColorSpace, CGImageAlphaInfo,
};
use objc2_core_text::{CTFont, CTFontOrientation};
use vello_common::peniko::color::PremulRgba8;
use vello_common::pixmap::Pixmap;

/// One glyph's colour image and where it sits relative to the pen.
pub(crate) struct RasterGlyph {
    /// Premultiplied RGBA8 texels, tightly bounded around the glyph's ink.
    pub(crate) pixmap: Pixmap,
    /// Pixels from the pen to the pixmap's left edge (right is positive).
    pub(crate) origin_x: i32,
    /// Pixels from the baseline up to the pixmap's top edge (up is positive).
    pub(crate) origin_y: i32,
}

/// The glyph CoreText's character map gives `character` in the face named
/// `postscript_name`; `None` when CoreText maps it to no glyph or does not
/// open that exact face (no sibling substitution here: see [`open`]).
pub(crate) fn glyph_for_char(postscript_name: &str, character: char) -> Option<u16> {
    let font = open(postscript_name, 16.0, None)?;
    let mut units = [0_u16; 2];
    let encoded = character.encode_utf16(&mut units).len();
    let mut glyphs = [0_u16; 2];
    // SAFETY: both buffers hold `encoded` (at most 2) elements.
    let mapped = unsafe {
        font.glyphs_for_characters(
            NonNull::from(&mut units).cast(),
            NonNull::from(&mut glyphs).cast(),
            encoded as isize,
        )
    };
    (mapped && glyphs[0] != 0).then_some(glyphs[0])
}

/// The glyph count of the face CoreText opens for exactly `postscript_name`;
/// `None` when it does not open that face.
pub(crate) fn glyph_count(postscript_name: &str) -> Option<u16> {
    let font = open(postscript_name, 16.0, None)?;
    // SAFETY: `font` is a live CTFont.
    u16::try_from(unsafe { font.glyph_count() }).ok()
}

/// Draws `glyph` of the face named `postscript_name` at `ppem` pixels per em
/// and returns its image, or `None` when the face's identity cannot be
/// verified against `expected_glyphs` (see the module docs), the glyph has no ink, either
/// dimension of its bounds exceeds `max_dim` pixels, or CoreText cannot
/// produce the bitmap. The pixmap is premultiplied RGBA8: the bitmap context
/// is created `kCGImageAlphaPremultipliedLast` at the default byte order, so
/// CoreGraphics itself writes R, G, B, A in memory order and no swizzle is
/// needed. A non-colour face yields its outline as coverage in the context's
/// fill colour (black), not a refusal; the caller decides by face which route
/// to use.
pub(crate) fn rasterize_glyph(
    postscript_name: &str,
    glyph: u16,
    expected_glyphs: u16,
    ppem: u16,
    max_dim: u32,
) -> Option<RasterGlyph> {
    if ppem == 0 {
        return None;
    }
    let font = open(postscript_name, f64::from(ppem), Some(expected_glyphs))?;
    let mut glyph = glyph;
    // SAFETY: `glyph` is one valid `CGGlyph` and the count is 1; a null
    // `bounding_rects` asks for the overall box alone.
    let bounds: CGRect = unsafe {
        font.bounding_rects_for_glyphs(
            CTFontOrientation::Default,
            NonNull::from(&mut glyph),
            core::ptr::null_mut(),
            1,
        )
    };
    if !(bounds.size.width > 0.0 && bounds.size.height > 0.0) {
        return None;
    }
    let left = bounds.origin.x.floor();
    let bottom = bounds.origin.y.floor();
    let right = (bounds.origin.x + bounds.size.width).ceil();
    let top = (bounds.origin.y + bounds.size.height).ceil();
    let (width, height) = (right - left, top - bottom);
    let limit = f64::from(max_dim.min(u32::from(u16::MAX)));
    if !(width >= 1.0 && height >= 1.0 && width <= limit && height <= limit) {
        return None;
    }
    // Exact: both are whole numbers within `u16`.
    let (w, h) = (width as usize, height as usize);

    let space = CGColorSpace::new_device_rgb()?;
    let bytes_per_row = w * 4;
    // SAFETY: a null `data` makes CoreGraphics allocate (zeroed, suitably
    // aligned) backing store of `h` rows of `bytes_per_row` bytes that lives as
    // long as `context`; 8 bits per component with four components per pixel
    // matches `bytes_per_row`.
    let context = unsafe {
        CGBitmapContextCreate(
            core::ptr::null_mut::<c_void>(),
            w,
            h,
            8,
            bytes_per_row,
            Some(&space),
            CGImageAlphaInfo::PremultipliedLast.0,
        )
    }?;
    let position = CGPoint::new(-left, -bottom);
    // SAFETY: one glyph and one position, valid for the call; `context` is a
    // live bitmap context.
    unsafe {
        font.draw_glyphs(
            NonNull::from(&mut glyph),
            NonNull::from(&position),
            1,
            &context,
        );
    }
    let data = CGBitmapContextGetData(Some(&context)).cast::<u8>();
    if data.is_null() {
        return None;
    }
    // SAFETY: the context owns `h * bytes_per_row` initialised bytes at
    // `data` until it is dropped, after the last read below.
    let bytes = unsafe { core::slice::from_raw_parts(data, bytes_per_row * h) };

    let texels: Vec<PremulRgba8> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|px| PremulRgba8 {
            r: px[0],
            g: px[1],
            b: px[2],
            a: px[3],
        })
        .collect();
    if texels.iter().all(|texel| texel.a == 0) {
        return None;
    }
    Some(RasterGlyph {
        pixmap: Pixmap::from_parts(texels, w as u16, h as u16),
        origin_x: left as i32,
        origin_y: top as i32,
    })
}

/// The face CoreText resolves `name` to at `size` points, verified to be that
/// face (module docs, "Font identity"). `expected_glyphs` enables the
/// public-sibling route for a dot-prefixed `name` and is checked against the
/// glyph count of whichever face opens; `None` accepts an exact name match
/// with no count check.
fn open(
    name: &str,
    size: f64,
    expected_glyphs: Option<u16>,
) -> Option<objc2_core_foundation::CFRetained<CTFont>> {
    let font = open_named(name, size);
    if postscript_name(&font) == name {
        // SAFETY: `font` is a live CTFont.
        let count = unsafe { font.glyph_count() };
        return expected_glyphs
            .is_none_or(|expected| u16::try_from(count) == Ok(expected))
            .then_some(font);
    }
    let expected = expected_glyphs?;
    let sibling = public_sibling(name)?;
    let font = open_named(&sibling, size);
    // SAFETY: `font` is a live CTFont.
    let count = unsafe { font.glyph_count() };
    (postscript_name(&font) == sibling && u16::try_from(count) == Ok(expected)).then_some(font)
}

/// `.AppleColorEmojiUI` to `AppleColorEmoji`: the hidden face's public name;
/// `None` for a name that is not dot-prefixed or leaves nothing.
fn public_sibling(name: &str) -> Option<String> {
    let rest = name.strip_prefix('.')?;
    let rest = rest.strip_suffix("UI").unwrap_or(rest);
    (!rest.is_empty()).then(|| rest.to_owned())
}

fn open_named(name: &str, size: f64) -> objc2_core_foundation::CFRetained<CTFont> {
    let name = CFString::from_str(name);
    // SAFETY: a null matrix is the documented "identity" argument.
    unsafe { CTFont::with_name(&name, size, core::ptr::null()) }
}

fn postscript_name(font: &CTFont) -> String {
    // SAFETY: `font` is a live CTFont.
    unsafe { font.post_script_name() }.to_string()
}
