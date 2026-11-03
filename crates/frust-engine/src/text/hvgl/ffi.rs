//! Every CoreText and CoreGraphics call the `hvgl` provider makes, and the
//! provider's only `unsafe`: [`CtFace`], one face opened in CoreText,
//! read-only. On a target without CoreText it is uninhabited and opens
//! nothing, so every `hvgl` glyph there is counted as skipped.
//!
//! Each target's half is one `platform` module; their items are
//! `pub(in super::super)` so the re-export at the bottom hands them to the
//! `hvgl` module and no further.

#[cfg(target_vendor = "apple")]
mod platform {
    use core::ffi::c_void;
    use core::ptr::NonNull;

    use kurbo::{PathEl, Point};
    use objc2_core_foundation::{
        CFArray, CFDictionary, CFIndex, CFNumber, CFRetained, CFString, CFType,
    };
    use objc2_core_graphics::{CGPath, CGPathElement, CGPathElementType};
    use objc2_core_text::{
        CTFont, CTFontDescriptor, CTFontOrientation, CTFontTableOptions, kCTFontVariationAttribute,
        kCTFontVariationAxisDefaultValueKey, kCTFontVariationAxisIdentifierKey,
    };

    /// Why [`CtFace::open`] refuses a face here.
    pub(in super::super) const UNAVAILABLE: &str = "CoreText cannot open that name";

    /// One face opened in CoreText, read-only.
    pub(in super::super) struct CtFace {
        /// The face, at `unitsPerEm` points.
        font: CFRetained<CTFont>,
        /// How many glyphs the face holds.
        glyph_count: usize,
    }

    // SAFETY: Apple documents CoreText as thread-safe: "All individual
    // functions in Core Text are thread-safe. Font objects (CTFont,
    // CTFontDescriptor, and associated objects) can be used simultaneously by
    // multiple operations, work queues, or threads." (Core Text Programming
    // Guide, "Core Text Objects Are C-Language Opaque Types"). A `CtFace` owns
    // its `CFRetained<CTFont>` and never mutates it after `open`: every method
    // only reads through the font (`CTFontCreatePathForGlyph`,
    // `CTFontGetBoundingRectsForGlyphs`, `CTFontCopyTable`, `varied`'s
    // `CTFontCreateCopyWithAttributes`, `axis_defaults`'
    // `CTFontCopyVariationAxes`, `variation_values`' `CTFontCopyVariation`,
    // `glyphs_for`'s `CTFontGetGlyphsForCharacters` into buffers the call
    // owns, and the getters), and the `CFRetained` release on drop is
    // `CFRelease`, which is thread-safe too. `SceneCompiler`, which holds
    // these, is `Send + Sync`, and the published renderer types that hold it
    // keep both.
    unsafe impl Send for CtFace {}
    // SAFETY: as above — shared access is only ever the immutable reads named
    // there, on a font object Apple documents as usable from several threads
    // at once.
    unsafe impl Sync for CtFace {}

    impl CtFace {
        /// The face CoreText resolves `name` to, at `units_per_em` points, with
        /// the PostScript name it actually carries (which differs from `name`
        /// when CoreText substituted a face for a name it does not know).
        pub(in super::super) fn open(name: &str, units_per_em: f64) -> Option<(Self, String)> {
            let requested = CFString::from_str(name);
            // SAFETY: a null matrix is the documented "identity" argument.
            let font = unsafe { CTFont::with_name(&requested, units_per_em, core::ptr::null()) };
            // SAFETY: a getter on a live font.
            let resolved = unsafe { font.post_script_name() }.to_string();
            // SAFETY: a getter on a live font.
            let glyph_count = usize::try_from(unsafe { font.glyph_count() }).unwrap_or(0);
            Some((Self { font, glyph_count }, resolved))
        }

        /// A copy of this face at the variation instance `axes` names — each
        /// `(axis tag, user value)` — at `units_per_em` points.
        ///
        /// Built from a `kCTFontVariationAttribute` dictionary (axis
        /// identifier to value, both `CFNumber`s). The copy carries no failure
        /// signal of its own, so this is always `Some`: whether CoreText
        /// applied the instance is read back from the copy
        /// ([`Self::variation_values`]).
        pub(in super::super) fn varied(
            &self,
            axes: &[(u32, f64)],
            units_per_em: f64,
        ) -> Option<Self> {
            let tags: Vec<CFRetained<CFNumber>> = axes
                .iter()
                .map(|(tag, _)| CFNumber::new_i64(i64::from(*tag)))
                .collect();
            let values: Vec<CFRetained<CFNumber>> = axes
                .iter()
                .map(|(_, value)| CFNumber::new_f64(*value))
                .collect();
            let tags: Vec<&CFNumber> = tags.iter().map(|tag| &**tag).collect();
            let values: Vec<&CFNumber> = values.iter().map(|value| &**value).collect();
            let variation = CFDictionary::<CFNumber, CFNumber>::from_slices(&tags, &values);
            // SAFETY: reading an immutable CoreText constant.
            let key: &CFString = unsafe { kCTFontVariationAttribute };
            let variation: &CFType = variation.as_ref();
            let attributes = CFDictionary::<CFString, CFType>::from_slices(&[key], &[variation]);
            // SAFETY: the attribute dictionary maps a CoreText attribute name
            // to the dictionary type that attribute documents.
            let descriptor = unsafe { CTFontDescriptor::with_attributes(attributes.as_opaque()) };
            // SAFETY: the font is live and a null matrix keeps the original's.
            let font = unsafe {
                self.font
                    .copy_with_attributes(units_per_em, core::ptr::null(), Some(&descriptor))
            };
            Some(Self {
                font,
                glyph_count: self.glyph_count,
            })
        }

        /// Whether `glyph_id` names a glyph of the face.
        pub(in super::super) fn contains(&self, glyph_id: u32) -> bool {
            u16::try_from(glyph_id).is_ok_and(|glyph| usize::from(glyph) < self.glyph_count)
        }

        /// Every axis `CTFontCopyVariationAxes` lists, as `(axis identifier,
        /// default user value)`; empty for a face that does not vary. Costly
        /// (about 130 us on macOS 27: CoreText localizes every axis name), so
        /// read once per face.
        pub(in super::super) fn axis_defaults(&self) -> Vec<(u32, f64)> {
            // SAFETY: a copy from a live font; `CFRetained` releases it.
            let Some(axes) = (unsafe { self.font.variation_axes() }) else {
                return Vec::new();
            };
            // SAFETY: every element is a CF object, which `CFType` stands for;
            // each is downcast checked before it is read as anything narrower.
            let axes: &CFArray<CFType> = unsafe { axes.cast_unchecked() };
            // SAFETY: reading immutable CoreText constants.
            let (identifier_key, default_key): (&CFType, &CFType) = unsafe {
                (
                    kCTFontVariationAxisIdentifierKey,
                    kCTFontVariationAxisDefaultValueKey,
                )
            };
            axes.iter()
                .filter_map(|axis| {
                    // SAFETY: as above, a dictionary of CF objects.
                    let axis: &CFDictionary<CFType, CFType> =
                        unsafe { axis.downcast_ref::<CFDictionary>()?.cast_unchecked() };
                    let tag = axis_tag(&*number(axis, identifier_key)?)?;
                    Some((tag, number(axis, default_key)?.as_f64()?))
                })
                .collect()
        }

        /// The values `CTFontCopyVariation` reports for the face, as `(axis
        /// identifier, user value)`: only those off the axis default.
        pub(in super::super) fn variation_values(&self) -> Vec<(u32, f64)> {
            // SAFETY: a copy from a live font; `CFRetained` releases it.
            let Some(applied) = (unsafe { self.font.variation() }) else {
                return Vec::new();
            };
            // SAFETY: every key and value is a CF object, which `CFType`
            // stands for; each is downcast checked before it is read.
            let applied: &CFDictionary<CFType, CFType> = unsafe { applied.cast_unchecked() };
            let (keys, values) = applied.to_vecs();
            keys.iter()
                .zip(&values)
                .filter_map(|(key, value)| {
                    Some((
                        axis_tag(key.downcast_ref::<CFNumber>()?)?,
                        value.downcast_ref::<CFNumber>()?.as_f64()?,
                    ))
                })
                .collect()
        }

        /// CoreText's nominal glyph for each of `characters`
        /// (`CTFontGetGlyphsForCharacters`, the face's own character map), 0
        /// for one the face does not map.
        pub(in super::super) fn glyphs_for(&self, characters: &[char]) -> Vec<u16> {
            let mut units: Vec<u16> = Vec::with_capacity(characters.len() * 2);
            let mut firsts = Vec::with_capacity(characters.len());
            for character in characters {
                firsts.push(units.len());
                units.extend_from_slice(character.encode_utf16(&mut [0; 2]));
            }
            let mut glyphs = vec![0_u16; units.len()];
            let (Some(units_at), Some(glyphs_at), Ok(count)) = (
                NonNull::new(units.as_mut_ptr()),
                NonNull::new(glyphs.as_mut_ptr()),
                CFIndex::try_from(units.len()),
            ) else {
                return Vec::new();
            };
            // SAFETY: both buffers hold `count` elements. A non-BMP character
            // takes two units, its glyph in the first slot.
            unsafe { self.font.glyphs_for_characters(units_at, glyphs_at, count) };
            firsts.into_iter().map(|first| glyphs[first]).collect()
        }

        /// How many glyphs the face holds (`CTFontGetGlyphCount`).
        pub(in super::super) fn glyph_count(&self) -> usize {
            self.glyph_count
        }

        /// The face's design units per em (`CTFontGetUnitsPerEm`).
        pub(in super::super) fn units_per_em(&self) -> u32 {
            // SAFETY: a getter on a live font.
            unsafe { self.font.units_per_em() }
        }

        /// A copy of the face's table `tag` (`CTFontCopyTable`); `None` when
        /// the face has no such table.
        pub(in super::super) fn table(&self, tag: [u8; 4]) -> Option<Vec<u8>> {
            // SAFETY: a copy from a live font; the returned data follows the
            // Create rule and `CFRetained` releases it.
            let data = unsafe {
                self.font
                    .table(u32::from_be_bytes(tag), CTFontTableOptions::NoOptions)
            }?;
            Some(data.to_vec())
        }

        /// `glyph_id`'s outline in font units, y up, as path elements; `None`
        /// when CoreText returns no path for it.
        pub(in super::super) fn path(&self, glyph_id: u32) -> Option<Vec<PathEl>> {
            let glyph = u16::try_from(glyph_id).ok()?;
            // SAFETY: the font is live and a null matrix is the documented
            // "identity" argument.
            let path = unsafe { self.font.path_for_glyph(glyph, core::ptr::null()) }?;
            let mut elements: Vec<PathEl> = Vec::new();
            // SAFETY: `push_element` writes only through `info`, which points
            // at `elements` for the duration of this call and nothing else
            // borrows it meanwhile.
            unsafe {
                CGPath::apply(
                    Some(&path),
                    (&raw mut elements).cast::<c_void>(),
                    Some(push_element),
                );
            }
            Some(elements)
        }

        /// Whether `glyph_id` has a non-empty bounding box.
        pub(in super::super) fn has_ink(&self, glyph_id: u32) -> bool {
            let Ok(mut glyph) = u16::try_from(glyph_id) else {
                return false;
            };
            // SAFETY: `glyphs` points at one valid `CGGlyph` and `count` is 1;
            // a null `bounding_rects` asks for the overall box alone.
            let bounds = unsafe {
                self.font.bounding_rects_for_glyphs(
                    CTFontOrientation::Default,
                    NonNull::from(&mut glyph),
                    core::ptr::null_mut(),
                    1,
                )
            };
            bounds.size.width > 0.0 && bounds.size.height > 0.0
        }
    }

    /// The axis tag a `CFNumber` axis identifier carries.
    fn axis_tag(identifier: &CFNumber) -> Option<u32> {
        u32::try_from(identifier.as_i64()?).ok()
    }

    /// `dictionary`'s value for `key`, when it is a `CFNumber`.
    fn number(
        dictionary: &CFDictionary<CFType, CFType>,
        key: &CFType,
    ) -> Option<CFRetained<CFNumber>> {
        dictionary.get(key)?.downcast::<CFNumber>().ok()
    }

    /// `CGPathApply`'s callback: append one element to the `Vec<PathEl>`
    /// `info` points at.
    ///
    /// # Safety
    ///
    /// `info` must point at a live, otherwise unborrowed `Vec<PathEl>`, and
    /// `element` must be a valid element whose `points` hold as many points as
    /// its type carries — which `CGPathApply` guarantees for the call.
    unsafe extern "C-unwind" fn push_element(info: *mut c_void, element: NonNull<CGPathElement>) {
        // SAFETY: `info` is the `&mut Vec<PathEl>` `CtFace::path` passed in.
        let out = unsafe { &mut *info.cast::<Vec<PathEl>>() };
        // SAFETY: CoreGraphics hands a valid element for the call's duration.
        let element = unsafe { element.as_ref() };
        let points = element.points.as_ptr();
        let point = |index: usize| {
            // SAFETY: every element type read below carries at least
            // `index + 1` points.
            let p = unsafe { *points.add(index) };
            Point::new(p.x, p.y)
        };
        let el = match element.r#type {
            CGPathElementType::MoveToPoint => PathEl::MoveTo(point(0)),
            CGPathElementType::AddLineToPoint => PathEl::LineTo(point(0)),
            CGPathElementType::AddQuadCurveToPoint => PathEl::QuadTo(point(0), point(1)),
            CGPathElementType::AddCurveToPoint => PathEl::CurveTo(point(0), point(1), point(2)),
            CGPathElementType::CloseSubpath => PathEl::ClosePath,
            _ => return,
        };
        out.push(el);
    }
}

#[cfg(not(target_vendor = "apple"))]
mod platform {
    use kurbo::PathEl;

    /// Why [`CtFace::open`] refuses a face here.
    pub(in super::super) const UNAVAILABLE: &str =
        "this target has no CoreText to read hvgl outlines with";

    /// No face can be opened without CoreText.
    pub(in super::super) enum CtFace {}

    impl CtFace {
        /// Always `None`: there is no CoreText here.
        pub(in super::super) fn open(_name: &str, _units_per_em: f64) -> Option<(Self, String)> {
            None
        }

        /// Unreachable — no `CtFace` exists on this target.
        pub(in super::super) fn varied(
            &self,
            _axes: &[(u32, f64)],
            _units_per_em: f64,
        ) -> Option<Self> {
            match *self {}
        }

        /// Unreachable — no `CtFace` exists on this target.
        pub(in super::super) fn contains(&self, _glyph_id: u32) -> bool {
            match *self {}
        }

        /// Unreachable — no `CtFace` exists on this target.
        pub(in super::super) fn axis_defaults(&self) -> Vec<(u32, f64)> {
            match *self {}
        }

        /// Unreachable — no `CtFace` exists on this target.
        pub(in super::super) fn variation_values(&self) -> Vec<(u32, f64)> {
            match *self {}
        }

        /// Unreachable — no `CtFace` exists on this target.
        pub(in super::super) fn glyphs_for(&self, _characters: &[char]) -> Vec<u16> {
            match *self {}
        }

        /// Unreachable — no `CtFace` exists on this target.
        pub(in super::super) fn glyph_count(&self) -> usize {
            match *self {}
        }

        /// Unreachable — no `CtFace` exists on this target.
        pub(in super::super) fn units_per_em(&self) -> u32 {
            match *self {}
        }

        /// Unreachable — no `CtFace` exists on this target.
        pub(in super::super) fn table(&self, _tag: [u8; 4]) -> Option<Vec<u8>> {
            match *self {}
        }

        /// Unreachable — no `CtFace` exists on this target.
        pub(in super::super) fn path(&self, _glyph_id: u32) -> Option<Vec<PathEl>> {
            match *self {}
        }

        /// Unreachable — no `CtFace` exists on this target.
        pub(in super::super) fn has_ink(&self, _glyph_id: u32) -> bool {
            match *self {}
        }
    }
}

pub(super) use platform::{CtFace, UNAVAILABLE};
