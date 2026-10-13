//! The strip instance the vertex shader steps over, and the conversion into
//! it from a rasterized [`Strip`].
//!
//! One [`GpuStrip`] is one instance of a four-vertex quad: there is no
//! geometry buffer at all, the vertex shader builds the quad's corners from
//! the instance's own fields (see [`GpuStrip::vertex_range`]).

use bytemuck::{Pod, Zeroable};
use core::ops::Range;
use frust_gpu::VertexLayout;
use vello_common::strip::Strip;
use vello_common::tile::Tile;

const _: () = assert!(
    Tile::HEIGHT == 4,
    "the strip shaders require `Tile::HEIGHT` to be 4",
);

/// Bit 31 of [`GpuStrip::paint_and_rect_flag`], marking an instance as a
/// whole rectangle rather than a strip: the shader then reads
/// [`GpuStrip::dense_width_or_rect_height`] as a height and
/// [`GpuStrip::col_idx_or_rect_frac`] as a coverage fraction.
pub const RECT_STRIP_FLAG: u32 = 1 << 31;

/// Bit the colour source starts at in [`GpuStrip::paint_and_rect_flag`].
const COLOR_SOURCE_SHIFT: u32 = 29;

/// Mask of the two colour-source bits once shifted down.
const COLOR_SOURCE_MASK: u32 = 0x3;

/// Mask of the three paint-type bits once shifted down.
const PAINT_TYPE_MASK: u32 = 0x7;

/// The colour source naming a rendered layer page — what a composite instance
/// carries in bits 29-30.
const COLOR_SOURCE_LAYER: u32 = 1;

/// Bit the paint type starts at in [`GpuStrip::paint_and_rect_flag`].
const PAINT_TYPE_SHIFT: u32 = 26;

/// Mask covering the encoded-paint texel index in
/// [`GpuStrip::paint_and_rect_flag`] — everything below the paint type.
pub const PAINT_TEXTURE_INDEX_MASK: u32 = (1 << PAINT_TYPE_SHIFT) - 1;

/// The colour source saying the fragment shader reads
/// [`GpuStrip::payload`] rather than sampling a rendered layer.
///
/// The only source the engine emits: layer compositing is later work, and an
/// instance that named the layer source would sample the placeholder view
/// bound in its place.
const COLOR_SOURCE_PAYLOAD: u32 = 0;

/// How the fragment shader turns an instance's payload into colour.
///
/// The discriminants are the shader's own paint-type numbering, not an
/// arbitrary ordering — they are written into
/// [`GpuStrip::paint_and_rect_flag`] as-is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum PaintType {
    /// The payload is a premultiplied RGBA8 colour, used directly.
    Solid = 0,
    /// The payload is a sample position; the record names an atlas image.
    Image = 1,
    /// The payload is a sample position; the record names a linear gradient.
    LinearGradient = 2,
    /// The payload is a sample position; the record names a radial gradient.
    RadialGradient = 3,
    /// The payload is a sample position; the record names a sweep gradient.
    SweepGradient = 4,
    /// The payload is a sample position; the record names a blurred rounded
    /// rectangle.
    BlurredRoundedRect = 5,
}

/// The packed paint descriptor for a `paint_type` instance whose encoded-paint
/// record starts at `texel_index`.
///
/// [`PaintType::Solid`] indexes no record and passes `0`; every other type
/// carries the texel its record starts at, which is what
/// `load_encoded_paint_texel` addresses the paint texture by. The index is
/// masked rather than reported, because it is produced by this crate's own
/// serialization — a paint table large enough to overflow 26 bits is refused
/// by the paint texture's own capacity check long before it reaches here.
#[must_use]
pub const fn pack_paint_descriptor(paint_type: PaintType, texel_index: u32) -> u32 {
    (COLOR_SOURCE_PAYLOAD << COLOR_SOURCE_SHIFT)
        | ((paint_type as u32) << PAINT_TYPE_SHIFT)
        | (texel_index & PAINT_TEXTURE_INDEX_MASK)
}

/// The alpha *column* the strip shader addresses coverage by, from the byte
/// index a [`Strip`] carries.
///
/// The two are different units, and the conversion is the shader's own. A
/// strip's `alpha_idx` counts coverage *bytes* from the start of the frame's
/// buffer; [`GpuStrip::col_idx_or_rect_frac`] is read as a column ordinal,
/// which the fragment stage turns into a texel with `col / 4` and a channel
/// within it with `col % 4` — one channel holding one pixel column's
/// `Tile::HEIGHT` coverage bytes. So a column is `Tile::HEIGHT` bytes wide,
/// which is the same unit [`Strip::width_to`] already reports a strip's width
/// in.
///
/// Getting this wrong is invisible on fully-covered geometry and total on
/// anti-aliased geometry: every alpha-sampled span reads past its own
/// coverage, lands on unwritten texels, and resolves to zero alpha.
#[must_use]
pub const fn alpha_column(alpha_idx: u32) -> u32 {
    alpha_idx / Tile::HEIGHT as u32
}

/// One strip instance, matching the shaders' `StripInstance` byte for byte.
///
/// Three of the fields are overloaded by [`RECT_STRIP_FLAG`], which is why
/// they are named for both readings.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
pub struct GpuStrip {
    /// Left edge in pixels.
    pub x: u16,
    /// Top edge in pixels (a multiple of `Tile::HEIGHT`).
    pub y: u16,
    /// Width in pixels.
    pub width: u16,
    /// Width of the alpha-sampled (dense) part in pixels, or, for a rect
    /// instance, the rect's height in pixels.
    pub dense_width_or_rect_height: u16,
    /// Index of the instance's first alpha column in the alpha texture, or,
    /// for a rect instance, its packed coverage fraction.
    pub col_idx_or_rect_frac: u32,
    /// Paint-dependent payload — a packed color, or the coordinates the paint
    /// is sampled at.
    pub payload: u32,
    /// Packed paint descriptor, with [`RECT_STRIP_FLAG`] in bit 31.
    pub paint_and_rect_flag: u32,
    /// Painter's-order index driving early-z rejection: the backmost draw is
    /// 0 and each draw in front of it increments.
    pub depth_index: u32,
}

const _: () = assert!(
    size_of::<GpuStrip>() == 24,
    "`GpuStrip` must stay 24 bytes — the shaders' `StripInstance` layout",
);
const _: () = assert!(
    size_of::<GpuStrip>() == size_of::<u16>() * 4 + size_of::<u32>() * 4,
    "`GpuStrip` must stay padding-free — six `Uint32` vertex attributes read it",
);

/// The per-draw values every strip instance of one draw shares.
///
/// Carried separately from the geometry because a draw resolves them once
/// (its paint, its painter's-order index) and every strip it emits repeats
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StripDraw {
    /// [`GpuStrip::payload`].
    pub payload: u32,
    /// The packed paint descriptor, without [`RECT_STRIP_FLAG`].
    pub paint: u32,
    /// [`GpuStrip::depth_index`].
    pub depth_index: u32,
}

impl GpuStrip {
    /// Vertices in the quad each instance expands to.
    pub const QUAD_VERTICES: u32 = 4;

    /// The six `Uint32` vertex attributes the 24-byte instance is read
    /// through, at shader locations 0-5.
    ///
    /// The `u16` pairs are read as packed `u32`s and unpacked in the shader,
    /// so the attribute list stays uniform.
    #[must_use]
    pub fn vertex_attributes() -> [wgpu::VertexAttribute; 6] {
        wgpu::vertex_attr_array![
            0 => Uint32,
            1 => Uint32,
            2 => Uint32,
            3 => Uint32,
            4 => Uint32,
            5 => Uint32,
        ]
    }

    /// The instance-stepped vertex buffer layout over [`GpuStrip`].
    #[must_use]
    pub fn vertex_layout() -> VertexLayout {
        VertexLayout {
            array_stride: size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: Self::vertex_attributes().to_vec(),
        }
    }

    /// The vertex range of a strip draw: one quad, built in the shader from
    /// the vertex index, with no vertex buffer behind it.
    #[must_use]
    pub fn vertex_range() -> Range<u32> {
        0..Self::QUAD_VERTICES
    }

    /// The instance range of a strip draw covering `count` instances from
    /// `first_instance` — paired with [`Self::vertex_range`] for one
    /// `draw` call.
    #[must_use]
    pub fn instance_range(first_instance: u32, count: u32) -> Range<u32> {
        first_instance..first_instance.saturating_add(count)
    }

    /// The alpha-sampled instance for `strip`, `width` pixels wide.
    ///
    /// The strip's alpha index becomes the instance's first alpha *column*,
    /// and the instance is dense across its whole width: every column samples
    /// coverage.
    #[must_use]
    pub fn from_strip(strip: &Strip, width: u16, draw: StripDraw) -> Self {
        Self {
            x: strip.x,
            y: strip.y,
            width,
            dense_width_or_rect_height: width,
            col_idx_or_rect_frac: alpha_column(strip.alpha_idx()),
            payload: draw.payload,
            paint_and_rect_flag: draw.paint,
            depth_index: draw.depth_index,
        }
    }

    /// The alpha-sampled instance for `strip`, taking its width from `next` —
    /// the strip immediately after it in the same run, which is where a
    /// strip's extent is recorded.
    #[must_use]
    pub fn from_strip_pair(strip: &Strip, next: &Strip, draw: StripDraw) -> Self {
        Self::from_strip(strip, strip.width_to(next), draw)
    }

    /// The solid instance filling the gap between `strip` and `next`, or
    /// `None` when there is no gap to fill.
    ///
    /// A gap is filled only when `next` carries the fill-gap flag and sits on
    /// the same strip row: the flag means "the winding between me and the
    /// previous strip is non-zero", which says nothing about a strip on
    /// another row. The instance samples no coverage, so its dense width and
    /// column index are both zero.
    #[must_use]
    pub fn gap_fill(strip: &Strip, next: &Strip, draw: StripDraw) -> Option<Self> {
        if !next.fill_gap() || next.y != strip.y {
            return None;
        }
        let gap_x = strip.x.saturating_add(strip.width_to(next));
        let gap_width = next.x.saturating_sub(gap_x);
        if gap_width == 0 {
            return None;
        }
        Some(Self::solid_fill(gap_x, strip.y, gap_width, draw))
    }

    /// A solid instance covering one strip row, sampling no coverage.
    #[must_use]
    pub fn solid_fill(x: u16, y: u16, width: u16, draw: StripDraw) -> Self {
        Self {
            x,
            y,
            width,
            dense_width_or_rect_height: 0,
            col_idx_or_rect_frac: 0,
            payload: draw.payload,
            paint_and_rect_flag: draw.paint,
            depth_index: draw.depth_index,
        }
    }

    /// A rectangle instance of `width` x `height` pixels with packed coverage
    /// fraction `frac`, flagged with [`RECT_STRIP_FLAG`].
    ///
    /// Unlike a strip, a rect instance is not bound to one strip row: its
    /// height is carried in the instance itself.
    #[must_use]
    pub fn from_rect(x: u16, y: u16, width: u16, height: u16, frac: u32, draw: StripDraw) -> Self {
        Self {
            x,
            y,
            width,
            dense_width_or_rect_height: height,
            col_idx_or_rect_frac: frac,
            payload: draw.payload,
            paint_and_rect_flag: draw.paint | RECT_STRIP_FLAG,
            depth_index: draw.depth_index,
        }
    }

    /// Whether this instance is a rectangle rather than a strip.
    #[must_use]
    pub const fn is_rect(&self) -> bool {
        self.paint_and_rect_flag & RECT_STRIP_FLAG != 0
    }

    /// The packed paint descriptor with [`RECT_STRIP_FLAG`] masked off.
    #[must_use]
    pub const fn paint(&self) -> u32 {
        self.paint_and_rect_flag & !RECT_STRIP_FLAG
    }
}

/// Which specialised strip program an instance can be drawn with.
///
/// The strip fragment stage dispatches on the instance's colour source and
/// paint type, and a mobile GPU charges every fragment for the branches that
/// dispatch *could* take, not the one it does (the S1 ablation: a program
/// keeping only coverage, solid and gradients ran S1's fragments at less than
/// half the cost of the whole one). So the one `strip.wgsl` source is compiled
/// once per class, each keeping only its own branches
/// ([`crate::gpu::shader_src::strip_class_source`]), and a pass's instances are
/// drawn in runs of one class each ([`split_class_runs`]).
///
/// Coverage (strip and rect) and the solid paint are in every class, which is
/// what lets a short solid run be drawn by its non-solid neighbour's program
/// (see [`Self::draws`]). [`Self::Uber`] is the whole program — the source
/// unchanged — and draws everything.
///
/// Derived from the instance alone ([`Self::of_flag`]): bits 29-30 of
/// [`GpuStrip::paint_and_rect_flag`] name the colour source and bits 26-28 the
/// paint type, so the class needs no side table and cannot disagree with what
/// the shader will read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PaintClass {
    /// Coverage plus the inline solid colour — and nothing else.
    SolidCoverage,
    /// Adds the linear, radial and sweep gradients.
    Gradient,
    /// Adds atlas and external image sampling.
    Image,
    /// Adds the blurred rounded rectangle.
    BlurredRect,
    /// Adds the layer colour source a composite reads its finished page
    /// through.
    LayerComposite,
    /// The whole program, every branch: what a run too short to be worth its
    /// own pipeline switch, and every run under `FRUST_ENGINE_UBER_PROGRAM`,
    /// is drawn with.
    Uber,
}

impl PaintClass {
    /// Every class, in index order.
    pub const ALL: [Self; 6] = [
        Self::SolidCoverage,
        Self::Gradient,
        Self::Image,
        Self::BlurredRect,
        Self::LayerComposite,
        Self::Uber,
    ];

    /// The classes compiled from a specialisation of the source — every class
    /// but [`Self::Uber`], which is the source itself.
    pub const SPECIALISED: [Self; 5] = [
        Self::SolidCoverage,
        Self::Gradient,
        Self::Image,
        Self::BlurredRect,
        Self::LayerComposite,
    ];

    /// The class of an instance whose packed descriptor is
    /// `paint_and_rect_flag` ([`RECT_STRIP_FLAG`] may be set or not).
    ///
    /// A colour source or paint type the shader does not name maps to
    /// [`Self::Uber`] rather than to a guess: the whole program reads it
    /// exactly as it always did.
    #[must_use]
    pub const fn of_flag(paint_and_rect_flag: u32) -> Self {
        let color_source = (paint_and_rect_flag >> COLOR_SOURCE_SHIFT) & COLOR_SOURCE_MASK;
        if color_source == COLOR_SOURCE_LAYER {
            return Self::LayerComposite;
        }
        if color_source != COLOR_SOURCE_PAYLOAD {
            return Self::Uber;
        }
        match (paint_and_rect_flag >> PAINT_TYPE_SHIFT) & PAINT_TYPE_MASK {
            0 => Self::SolidCoverage,
            1 => Self::Image,
            2..=4 => Self::Gradient,
            5 => Self::BlurredRect,
            _ => Self::Uber,
        }
    }

    /// The class of `strip`.
    #[must_use]
    pub const fn of_instance(strip: &GpuStrip) -> Self {
        Self::of_flag(strip.paint_and_rect_flag)
    }

    /// This class's position in [`Self::ALL`].
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// A short name for logs, pipeline labels and shader-library names.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::SolidCoverage => "solid",
            Self::Gradient => "gradient",
            Self::Image => "image",
            Self::BlurredRect => "blurred-rect",
            Self::LayerComposite => "layer",
            Self::Uber => "uber",
        }
    }

    /// Whether this class's program draws an instance of class `other`
    /// exactly as [`Self::Uber`] would.
    ///
    /// A class draws itself and the solid paint (coverage and solid are in
    /// every program); [`Self::Uber`] draws everything.
    #[must_use]
    pub const fn draws(self, other: Self) -> bool {
        matches!(self, Self::Uber)
            || matches!(other, Self::SolidCoverage)
            || self as usize == other as usize
    }
}

/// A set of [`PaintClass`]es, as a bitmask — which class programs one pass
/// needs, so a frame resolves exactly those pipelines and no others.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ClassSet(u8);

impl ClassSet {
    /// The empty set.
    pub const EMPTY: Self = Self(0);

    /// The set holding `class` alone.
    #[must_use]
    pub const fn only(class: PaintClass) -> Self {
        Self(1 << class as u8)
    }

    /// Adds `class`.
    pub fn insert(&mut self, class: PaintClass) {
        self.0 |= 1 << class as u8;
    }

    /// Whether `class` is in the set.
    #[must_use]
    pub const fn contains(self, class: PaintClass) -> bool {
        self.0 & (1 << class as u8) != 0
    }

    /// Whether the set is empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The classes in the set, in [`PaintClass::ALL`] order.
    pub fn iter(self) -> impl Iterator<Item = PaintClass> {
        PaintClass::ALL
            .into_iter()
            .filter(move |class| self.contains(*class))
    }
}

/// One run of consecutive instances drawn with one class's program, as an
/// offset into the slice [`split_class_runs`] was handed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassRun {
    /// Index of the run's first instance within the split slice.
    pub first: u32,
    /// How many instances the run holds; never zero.
    pub count: u32,
    /// The program the run is drawn with.
    pub class: PaintClass,
}

/// The default [`split_class_runs`] threshold: a run of fewer instances than
/// this is not worth a pipeline switch of its own and is drawn with
/// [`PaintClass::Uber`] instead.
///
/// Chosen by measurement on a Pixel 4a (Adreno 618) S1 capture at 16, 64 and
/// 256 — see the task record. Overridable at build or launch time with
/// `FRUST_ENGINE_RUN_SPLIT_MIN` ([`crate::config::run_split_min`]).
pub const RUN_SPLIT_MIN: u32 = 16;

/// Splits `instances` — one pass segment, in painter order — into consecutive
/// runs, each drawn with one class's program, appending them to `out`.
///
/// Painter order is preserved by construction: the runs are contiguous,
/// non-overlapping and cover the slice front to back, so drawing them in the
/// order returned draws exactly the instances the unsplit segment did, in the
/// same order. Only the program each instance is shaded by changes, and every
/// program shades the instances it is handed exactly as the whole one would.
///
/// Three steps, each linear:
///
/// 1. maximal runs of equal [`PaintClass::of_instance`];
/// 2. a solid run shorter than `min_run` is folded into a neighbouring
///    non-solid run (the previous one, else the next) — every program draws
///    solid, so this costs that neighbour nothing and saves a switch; runs of
///    one class left adjacent by a fold are joined;
/// 3. a remaining non-solid run shorter than `min_run` is drawn with
///    [`PaintClass::Uber`], and adjacent Uber runs are joined — the short run
///    is merged into its neighbours rather than paying for a pipeline switch
///    of its own.
///
/// A solid run is never promoted to Uber: it only survives step 2 short when
/// it has no non-solid neighbour at all, and then the solid program is the
/// cheapest one that draws it. `min_run` 0 or 1 disables both merges.
///
/// `uber_only` is the kill switch (`FRUST_ENGINE_UBER_PROGRAM`): the whole
/// slice becomes ONE Uber run, which is the unsplit segment exactly.
pub fn split_class_runs(
    instances: &[GpuStrip],
    min_run: u32,
    uber_only: bool,
    out: &mut Vec<ClassRun>,
) {
    let start = out.len();
    let total = u32::try_from(instances.len()).unwrap_or(u32::MAX);
    if total == 0 {
        return;
    }
    if uber_only {
        out.push(ClassRun {
            first: 0,
            count: total,
            class: PaintClass::Uber,
        });
        return;
    }

    // Steps 1 and 2 together, over a stack of closed runs: each maximal run is
    // pushed, folding a short solid run into a non-solid neighbour as soon as
    // both are known.
    let mut index = 0_u32;
    while index < total {
        let class = PaintClass::of_instance(&instances[index as usize]);
        let mut end = index + 1;
        while end < total && PaintClass::of_instance(&instances[end as usize]) == class {
            end += 1;
        }
        push_folding(
            out,
            start,
            ClassRun {
                first: index,
                count: end - index,
                class,
            },
            min_run,
        );
        index = end;
    }

    // Step 3, in place: a short non-solid run is drawn with Uber, and Uber
    // runs left adjacent are joined.
    let mut write = start;
    for read in start..out.len() {
        let mut run = out[read];
        if run.class != PaintClass::SolidCoverage && run.count < min_run {
            run.class = PaintClass::Uber;
        }
        if write > start && out[write - 1].class == run.class {
            out[write - 1].count += run.count;
        } else {
            out[write] = run;
            write += 1;
        }
    }
    out.truncate(write);
}

/// Pushes `run` onto `out[start..]`, folding a short solid run into a
/// non-solid neighbour and joining equal neighbours (step 2 of
/// [`split_class_runs`]).
fn push_folding(out: &mut Vec<ClassRun>, start: usize, mut run: ClassRun, min_run: u32) {
    let short_solid =
        |run: &ClassRun| run.class == PaintClass::SolidCoverage && run.count < min_run;
    if let Some(top) = out.get_mut(start..).and_then(<[ClassRun]>::last_mut) {
        if top.class == run.class || (short_solid(&run) && top.class != PaintClass::SolidCoverage) {
            top.count += run.count;
            return;
        }
        if short_solid(top) && run.class != PaintClass::SolidCoverage {
            // The solid run before this one had no non-solid run to its left
            // to fold into; it folds into this one instead.
            run.first = top.first;
            run.count += top.count;
            out.pop();
            // The fold can leave this run beside one of its own class.
            if let Some(below) = out.get_mut(start..).and_then(<[ClassRun]>::last_mut)
                && below.class == run.class
            {
                below.count += run.count;
                return;
            }
        }
    }
    out.push(run);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(paint: u32) -> GpuStrip {
        GpuStrip::solid_fill(
            0,
            0,
            1,
            StripDraw {
                payload: 0,
                paint,
                depth_index: 0,
            },
        )
    }

    fn of(classes: &[(PaintType, u32)]) -> Vec<GpuStrip> {
        classes
            .iter()
            .flat_map(|(paint, count)| {
                std::iter::repeat_n(strip(pack_paint_descriptor(*paint, 7)), *count as usize)
            })
            .collect()
    }

    fn runs(instances: &[GpuStrip], min_run: u32) -> Vec<(u32, u32, PaintClass)> {
        let mut out = Vec::new();
        split_class_runs(instances, min_run, false, &mut out);
        out.iter().map(|r| (r.first, r.count, r.class)).collect()
    }

    #[test]
    fn every_paint_type_and_source_maps_to_its_class() {
        use PaintClass as C;
        assert_eq!(
            C::of_flag(pack_paint_descriptor(PaintType::Solid, 0)),
            C::SolidCoverage
        );
        assert_eq!(
            C::of_flag(pack_paint_descriptor(PaintType::Image, 3)),
            C::Image
        );
        for gradient in [
            PaintType::LinearGradient,
            PaintType::RadialGradient,
            PaintType::SweepGradient,
        ] {
            assert_eq!(C::of_flag(pack_paint_descriptor(gradient, 9)), C::Gradient);
        }
        assert_eq!(
            C::of_flag(pack_paint_descriptor(PaintType::BlurredRoundedRect, 1)),
            C::BlurredRect
        );
        // A composite: layer source, opacity in the low byte, rect flag set.
        assert_eq!(
            C::of_flag((1 << 29) | 0x80 | RECT_STRIP_FLAG),
            C::LayerComposite
        );
        // The rect flag never changes a class.
        assert_eq!(
            C::of_flag(pack_paint_descriptor(PaintType::Image, 3) | RECT_STRIP_FLAG),
            C::Image
        );
        // Reserved sources and paint types fall back to the whole program.
        assert_eq!(C::of_flag(2 << 29), C::Uber);
        assert_eq!(C::of_flag(3 << 29), C::Uber);
        assert_eq!(C::of_flag(6 << 26), C::Uber);
        assert_eq!(C::of_flag(7 << 26), C::Uber);
    }

    #[test]
    fn the_class_lattice_is_solid_below_everything_and_uber_above() {
        for class in PaintClass::ALL {
            assert!(class.draws(PaintClass::SolidCoverage));
            assert!(class.draws(class));
            assert!(PaintClass::Uber.draws(class));
        }
        assert!(!PaintClass::Gradient.draws(PaintClass::Image));
        assert!(!PaintClass::SolidCoverage.draws(PaintClass::Gradient));
        assert!(!PaintClass::Image.draws(PaintClass::Uber));
        for (index, class) in PaintClass::ALL.iter().enumerate() {
            assert_eq!(class.index(), index);
        }
        assert_eq!(PaintClass::SPECIALISED.len() + 1, PaintClass::ALL.len());
        assert!(!PaintClass::SPECIALISED.contains(&PaintClass::Uber));
    }

    #[test]
    fn a_class_set_round_trips() {
        let mut set = ClassSet::EMPTY;
        assert!(set.is_empty());
        set.insert(PaintClass::Image);
        set.insert(PaintClass::Uber);
        set.insert(PaintClass::Image);
        assert!(set.contains(PaintClass::Image));
        assert!(!set.contains(PaintClass::Gradient));
        assert_eq!(
            set.iter().collect::<Vec<_>>(),
            vec![PaintClass::Image, PaintClass::Uber]
        );
        assert_eq!(ClassSet::only(PaintClass::Gradient).iter().count(), 1);
    }

    #[test]
    fn long_runs_keep_their_class_and_cover_the_slice_in_order() {
        use PaintType as P;
        let instances = of(&[
            (P::LinearGradient, 20),
            (P::Image, 30),
            (P::RadialGradient, 17),
        ]);
        assert_eq!(
            runs(&instances, 16),
            vec![
                (0, 20, PaintClass::Gradient),
                (20, 30, PaintClass::Image),
                (50, 17, PaintClass::Gradient),
            ]
        );
    }

    #[test]
    fn a_short_solid_run_folds_into_its_non_solid_neighbour() {
        use PaintType as P;
        // Behind a gradient: folds backwards.
        let instances = of(&[
            (P::LinearGradient, 20),
            (P::Solid, 3),
            (P::LinearGradient, 20),
        ]);
        assert_eq!(runs(&instances, 16), vec![(0, 43, PaintClass::Gradient)]);
        // At the front: folds forwards into the first non-solid run.
        let instances = of(&[(P::Solid, 3), (P::Image, 20)]);
        assert_eq!(runs(&instances, 16), vec![(0, 23, PaintClass::Image)]);
        // A long solid run stays a run of its own.
        let instances = of(&[(P::Image, 20), (P::Solid, 40), (P::Image, 20)]);
        assert_eq!(
            runs(&instances, 16),
            vec![
                (0, 20, PaintClass::Image),
                (20, 40, PaintClass::SolidCoverage),
                (60, 20, PaintClass::Image),
            ]
        );
        // An all-solid segment is one solid run however short.
        assert_eq!(
            runs(&of(&[(P::Solid, 2)]), 16),
            vec![(0, 2, PaintClass::SolidCoverage)]
        );
    }

    #[test]
    fn a_run_below_the_threshold_is_merged_and_drawn_with_uber() {
        use PaintType as P;
        // S1's shape: a long gradient run, then a short text (image) run.
        let instances = of(&[
            (P::RadialGradient, 40),
            (P::Image, 5),
            (P::BlurredRoundedRect, 3),
            (P::RadialGradient, 40),
        ]);
        assert_eq!(
            runs(&instances, 16),
            vec![
                (0, 40, PaintClass::Gradient),
                (40, 8, PaintClass::Uber),
                (48, 40, PaintClass::Gradient),
            ]
        );
        // The same slice under a higher threshold: the gradient runs are now
        // short too, and the whole segment collapses into one Uber run.
        assert_eq!(runs(&instances, 64), vec![(0, 88, PaintClass::Uber)]);
        // Threshold 0 disables merging altogether.
        assert_eq!(runs(&instances, 0).len(), 4);
    }

    #[test]
    fn the_kill_switch_yields_one_uber_run() {
        use PaintType as P;
        let instances = of(&[(P::RadialGradient, 40), (P::Image, 40), (P::Solid, 40)]);
        let mut out = Vec::new();
        split_class_runs(&instances, 16, true, &mut out);
        assert_eq!(
            out,
            vec![ClassRun {
                first: 0,
                count: 120,
                class: PaintClass::Uber
            }]
        );
        out.clear();
        split_class_runs(&[], 16, false, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn appending_to_a_non_empty_vec_never_merges_across_calls() {
        use PaintType as P;
        let mut out = Vec::new();
        split_class_runs(&of(&[(P::Image, 20)]), 16, false, &mut out);
        split_class_runs(&of(&[(P::Image, 20)]), 16, false, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].first, 0);
    }

    /// Exhaustive over short sequences: the runs always tile the slice, never
    /// hand an instance to a program that cannot draw it, and never leave two
    /// adjacent runs of one class.
    #[test]
    fn the_split_always_tiles_and_every_run_draws_its_instances() {
        let kinds = [
            PaintType::Solid,
            PaintType::Image,
            PaintType::LinearGradient,
            PaintType::BlurredRoundedRect,
        ];
        for len in 1..=6_u32 {
            for code in 0..(kinds.len() as u32).pow(len) {
                let mut instances = Vec::new();
                let mut c = code;
                for _ in 0..len {
                    let kind = kinds[(c % kinds.len() as u32) as usize];
                    c /= kinds.len() as u32;
                    instances.push(strip(pack_paint_descriptor(kind, 1)));
                }
                for min_run in [0, 1, 2, 3, 16] {
                    let mut out = Vec::new();
                    split_class_runs(&instances, min_run, false, &mut out);
                    let mut next = 0;
                    for (i, run) in out.iter().enumerate() {
                        assert_eq!(run.first, next, "{out:?}");
                        assert!(run.count > 0);
                        for instance in
                            &instances[run.first as usize..(run.first + run.count) as usize]
                        {
                            assert!(
                                run.class.draws(PaintClass::of_instance(instance)),
                                "{out:?}"
                            );
                        }
                        if i > 0 {
                            assert_ne!(out[i - 1].class, run.class, "{out:?}");
                        }
                        next += run.count;
                    }
                    assert_eq!(next, len);
                }
            }
        }
    }
}
