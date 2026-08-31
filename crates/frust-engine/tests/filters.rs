//! The Gaussian-blur filter layer: the kernel it blurs with, the parameter
//! block the fragment stage reads, the passes it costs, and the rounds the
//! scheduler plans for it.
//!
//! No GPU, device, surface or texture is involved anywhere in this file. Every
//! claim below is about a decision over plain values — which passes a σ costs
//! and at what extents, what a kernel packs into 48 bytes, which page each
//! round writes — because that is the half of the filter this tier owns; the
//! passes themselves are one instanced quad each through a pipeline the
//! renderer has yet to grow.
//!
//! Three kinds of claim are made here.
//!
//! - **Parity with the CPU rasterizer.** Both tiers blur with
//!   `vello_common::filter::gaussian_blur`'s own kernel, so σ 2, 8 and 32 plan
//!   the same decimations and the same discrete weights on either. What this
//!   tier does differently — merging adjacent taps into bilinear samples — is
//!   checked by splitting the merged taps back apart and comparing them to the
//!   discrete kernel they came from.
//! - **The layouts the shader reads.** Every filter's parameter block is 48
//!   bytes; the instance layout is byte-identical to the shader's struct; and
//!   the constants both sides duplicate (`FILTER_ATLAS_PADDING`,
//!   `FILTER_SIZE_BYTES`, `MAX_TAPS_PER_SIDE`) are read back out of the WGSL
//!   rather than trusted to a comment.
//! - **What the scheduler plans and what it refuses.** A blurred layer under
//!   the surface costs its contents' round, one round per pass, and the
//!   composite; a filter layer nested inside another layer, a filter that is
//!   not a blur, and a layer whose blur grows it past what a page can be sized
//!   to are each refused with a reason, never rendered wrong and never a panic.

use frust_engine::filters::blur::{
    FILTER_ATLAS_PADDING, FILTER_SIZE_BYTES, FilterInstanceData, GpuFilterData, GpuGaussianBlur,
    LinearKernel, MAX_KERNEL_SIZE, MAX_TAPS_PER_SIDE, blur_passes, edge_mode, filter_type,
    pack_u16_pair,
};
use frust_engine::filters::{
    FILTER, FILTER_NAME, FilterPassKind, LayerFilter, MAX_BLUR_SIGMA, push_filter_layer,
};
use frust_engine::schedule::{PageConfig, PageParity, Round, RoundOp, RoundTarget, Schedule};
use frust_engine::{EngineDraw, EngineError};
use frust_gpu::{DownlevelProfile, TierCaps};
use kurbo::Affine;
use vello_common::color::palette::css::RED;
use vello_common::filter::gaussian_blur::{
    GaussianBlur, compute_gaussian_kernel, plan_decimated_blur,
};
use vello_common::filter_effects::{EdgeMode, Filter, FilterPrimitive};
use vello_common::geometry::SizeU16;
use vello_common::paint::Paint;
use vello_common::peniko::BlendMode;
use vello_common::record::{CommandRecorder, LayerProps};
use vello_common::strip::Strip;
use vello_common::tile::Tile;
use wgpu::naga;

/// Viewport every recording is built against. Deliberately not square, so an
/// axis swapped somewhere in page or pass sizing cannot pass by symmetry.
const VIEWPORT: (u16, u16) = (256, 192);

/// The three standard deviations this file pins, spanning the decimation
/// plan's whole range: none, some, and the deepest a page-sized layer reaches.
const SIGMAS: [f32; 3] = [2.0, 8.0, 32.0];

fn caps() -> TierCaps {
    TierCaps::fake(DownlevelProfile::Full)
}

fn recorder() -> CommandRecorder<EngineDraw> {
    CommandRecorder::new(VIEWPORT.0, VIEWPORT.1)
}

/// A layer composited source-over at `opacity`, with no mask and no layer clip
/// path.
fn layer(opacity: f32) -> LayerProps {
    LayerProps {
        blend_mode: BlendMode::default(),
        opacity,
        mask: None,
        clip_path: None,
    }
}

/// Strips covering `width` pixels of the tile row holding `y`, starting at `x`.
fn strips(x: u16, y: u16, width: u16) -> Vec<Strip> {
    vec![
        Strip::new(x, y, 0, false),
        Strip::sentinel(y, u32::from(width) * u32::from(Tile::HEIGHT)),
    ]
}

/// Records one draw covering `width` pixels of the tile row holding `y`.
fn draw(recorder: &mut CommandRecorder<EngineDraw>, x: u16, y: u16, width: u16) {
    let strips = strips(x, y, width);
    let depth = u32::try_from(recorder.draws.len()).expect("a test records a handful of draws");
    recorder.push_draw(
        EngineDraw::new(Paint::from(RED), depth, 0..strips.len()),
        &strips,
    );
}

/// A recording of one blurred layer under the surface, holding one draw.
fn blurred_layer(sigma: f32, opacity: f32) -> CommandRecorder<EngineDraw> {
    let mut recorder = recorder();
    push_filter_layer(
        &mut recorder,
        layer(opacity),
        LayerFilter::Blur { sigma },
        Affine::IDENTITY,
    )
    .expect("a finite σ under the ceiling records");
    draw(&mut recorder, 64, 64, 32);
    recorder.pop_layer();
    recorder
}

fn schedule(recorder: &CommandRecorder<EngineDraw>) -> Vec<Round> {
    Schedule::build_with_filters(recorder, &caps(), &PageConfig::default())
        .expect("a blurred layer under the surface schedules")
}

fn escalation(recorder: &CommandRecorder<EngineDraw>) -> String {
    match Schedule::build_with_filters(recorder, &caps(), &PageConfig::default()) {
        Err(EngineError::SchedulerEscalation { reason }) => reason,
        other => panic!("expected an escalation, got {other:?}"),
    }
}

// -----------------------------------------------------------------------------
// The kernel, and its bilinear form
// -----------------------------------------------------------------------------

/// The decimation plan is the CPU rasterizer's, not a second one: the blur this
/// tier renders at a given σ halves the same number of times and convolves with
/// the same discrete weights, which is the whole reason the two tiers can be
/// compared pixel for pixel at all.
#[test]
fn every_sigma_plans_the_shared_kernel_rather_than_one_of_this_tiers_own() {
    for sigma in SIGMAS {
        let blur = GaussianBlur::new(sigma, EdgeMode::default());
        let (n_decimations, kernel, kernel_size) = plan_decimated_blur(sigma);

        assert_eq!(blur.n_decimations, n_decimations, "σ {sigma}");
        assert_eq!(blur.kernel_size, kernel_size, "σ {sigma}");
        assert_eq!(blur.kernel, kernel, "σ {sigma}");
        assert!(
            usize::from(kernel_size) <= MAX_KERNEL_SIZE,
            "σ {sigma} planned a kernel of {kernel_size}, past the {MAX_KERNEL_SIZE} the \
             decimation plan is supposed to bound it to"
        );
    }
}

/// Decimation is what keeps the kernel bounded, so a larger σ has to buy its
/// blur with halvings rather than with taps.
#[test]
fn a_larger_sigma_costs_decimations_rather_than_a_wider_kernel() {
    let decimations: Vec<usize> = SIGMAS
        .iter()
        .map(|sigma| plan_decimated_blur(*sigma).0)
        .collect();

    assert_eq!(decimations, vec![0, 2, 4], "σ 2 / 8 / 32");
}

/// The merged bilinear taps have to describe the same Gaussian the discrete
/// kernel does. Each merged tap stands for two adjacent discrete ones, and its
/// fractional offset is what says how its weight divides between them — so
/// splitting it back apart must reproduce the kernel it came from.
#[test]
fn the_bilinear_kernel_splits_back_into_the_discrete_one() {
    for sigma in SIGMAS {
        let (kernel, kernel_size) = compute_gaussian_kernel(plan_remaining_sigma(sigma));
        let linear = LinearKernel::new(&kernel, kernel_size);
        let radius = usize::from(kernel_size) / 2;

        assert_eq!(
            linear.center_weight, kernel[radius],
            "σ {sigma}: the centre tap is not merged with anything"
        );

        for tap in 0..usize::from(linear.n_taps) {
            let merged = linear.weights[tap];
            let offset = linear.offsets[tap];
            let low = 2 * tap + 1;

            // A tap merging two neighbours divides its weight by how far its
            // sample sits between them; a leftover single tap sits on a whole
            // texel and takes all of it.
            let high_share = merged * (offset - low as f32);
            let low_share = merged - high_share;

            assert!(
                (low_share - kernel[radius + low]).abs() < 1e-6,
                "σ {sigma}, tap {tap}: low share {low_share} != {}",
                kernel[radius + low]
            );
            let high = kernel.get(radius + low + 1).copied().unwrap_or(0.0);
            assert!(
                (high_share - high).abs() < 1e-6,
                "σ {sigma}, tap {tap}: high share {high_share} != {high}"
            );
        }
    }
}

/// A convolution that did not sum to one would darken or brighten what it
/// blurred, which is the failure the normalization exists to prevent — and the
/// merge has to preserve it, since every tap is applied on both sides.
#[test]
fn the_bilinear_kernel_still_sums_to_one() {
    for sigma in SIGMAS {
        let blur = GaussianBlur::new(sigma, EdgeMode::default());
        let linear = LinearKernel::new(&blur.kernel, blur.kernel_size);
        let sum: f32 = linear.center_weight
            + 2.0
                * linear
                    .weights
                    .iter()
                    .take(usize::from(linear.n_taps))
                    .sum::<f32>();

        assert!((sum - 1.0).abs() < 1e-5, "σ {sigma} summed to {sum}");
    }
}

/// The shader reads the tap weights and offsets as one `vec3<f32>` each, which
/// is only enough because the decimation plan bounds the kernel.
#[test]
fn no_kernel_the_plan_produces_needs_more_taps_than_the_shader_reads() {
    for sigma in [0.0, 0.1, 0.5, 1.0, 2.0, 8.0, 32.0, 1024.0, MAX_BLUR_SIGMA] {
        let blur = GaussianBlur::new(sigma, EdgeMode::default());
        let linear = LinearKernel::new(&blur.kernel, blur.kernel_size);

        assert!(
            usize::from(linear.n_taps) <= MAX_TAPS_PER_SIDE,
            "σ {sigma} merged into {} taps, past the {MAX_TAPS_PER_SIDE} the shader reads",
            linear.n_taps
        );
    }
}

/// A σ of zero is a valid identity blur rather than a refusal or a panic: the
/// plan hands back a one-tap kernel, and one tap at full weight copies.
#[test]
fn a_zero_sigma_is_an_identity_kernel_rather_than_a_refusal() {
    let blur = GaussianBlur::new(0.0, EdgeMode::default());
    let linear = LinearKernel::new(&blur.kernel, blur.kernel_size);

    assert_eq!(blur.kernel_size, 1);
    assert_eq!(linear.n_taps, 0);
    assert!((linear.center_weight - 1.0).abs() < 1e-6);
}

/// The σ that reaches the kernel after the decimations have taken their share.
fn plan_remaining_sigma(sigma: f32) -> f32 {
    let mut variance = sigma * sigma;
    while variance > 4.0 {
        variance = (variance - 1.5) * 0.25;
    }
    variance.sqrt()
}

// -----------------------------------------------------------------------------
// The layouts the shader reads
// -----------------------------------------------------------------------------

/// The law the type erasure rests on: every filter's parameter block is one
/// size, so a filter's own block is addressed by a plain texel multiple.
#[test]
fn every_filter_parameter_block_is_one_size() {
    assert_eq!(FILTER_SIZE_BYTES, 48);
    assert_eq!(size_of::<GpuFilterData>(), FILTER_SIZE_BYTES);
    assert_eq!(size_of::<GpuGaussianBlur>(), FILTER_SIZE_BYTES);
    assert_eq!(align_of::<GpuFilterData>(), 16);
    assert_eq!(align_of::<GpuGaussianBlur>(), 16);
    assert_eq!(GpuFilterData::SIZE_TEXELS, 3);
}

/// The padding a filter layer reserves is half a kernel, because that is how
/// far a tap can reach past the region it filters — the bound that lets every
/// kernel skip its bounds checks.
#[test]
fn the_filter_padding_is_half_the_widest_kernel() {
    assert_eq!(FILTER_ATLAS_PADDING, (MAX_KERNEL_SIZE / 2) as u16);
    assert_eq!(FILTER_ATLAS_PADDING, 6);
}

/// A blur's parameter block survives the round trip through the type-erased
/// form, and the header it packs is what the fragment stage unpacks.
#[test]
fn a_blurs_parameter_block_round_trips_through_the_erased_form() {
    for sigma in SIGMAS {
        let blur = GaussianBlur::new(sigma, EdgeMode::default());
        let packed = GpuGaussianBlur::from(&blur);
        let erased = GpuFilterData::from(packed);

        assert_eq!(
            erased.filter_type(),
            filter_type::GAUSSIAN_BLUR,
            "σ {sigma}"
        );
        assert_eq!(erased.n_decimations(), blur.n_decimations, "σ {sigma}");
        assert_eq!(
            bytemuck::cast::<GpuFilterData, GpuGaussianBlur>(erased),
            packed,
            "σ {sigma}"
        );

        let linear = LinearKernel::new(&blur.kernel, blur.kernel_size);
        assert_eq!((packed.header >> 5) & 0x3, edge_mode::NONE, "σ {sigma}");
        assert_eq!(
            (packed.header >> 11) & 0x3,
            u32::from(linear.n_taps),
            "σ {sigma}"
        );
    }
}

/// Bit 13 belongs to the drop shadow that lands next; the blur's own fields
/// must not have grown into it at any σ the plan produces.
#[test]
fn a_blurs_header_leaves_the_drop_shadows_bit_alone() {
    for sigma in [0.0, 2.0, 8.0, 32.0, MAX_BLUR_SIGMA] {
        let packed = GpuGaussianBlur::from(&GaussianBlur::new(sigma, EdgeMode::default()));
        assert_eq!(packed.header & (1 << 13), 0, "σ {sigma}");
    }
}

/// The instance layout the vertex stage reads is the one the host packs.
#[test]
fn the_filter_instance_matches_its_shader_layout() {
    let module = validate(FILTER_NAME, FILTER);

    assert_eq!(
        struct_span(&module, "FilterInstanceData") as usize,
        size_of::<FilterInstanceData>(),
        "the shader's `FilterInstanceData` must stay byte-identical to the Rust one"
    );
    assert_eq!(size_of::<FilterInstanceData>(), 32);
}

/// An instance carries the step's own extents, the page it writes into, and the
/// layer's unscaled extent — which is what bounds the transparent border a
/// decimated pass overdraws.
#[test]
fn an_instance_carries_its_steps_extents_and_the_pages_it_addresses() {
    let blur = GaussianBlur::new(8.0, EdgeMode::default());
    let original = SizeU16::from_wh(96, 64);
    let steps = blur_passes(&blur, original);
    let first = steps.first().expect("a blur costs at least one pass");

    let instance = FilterInstanceData::new(first, 3, (0, 0), (0, 0), SizeU16::new(512), original);

    assert_eq!(instance.filter_data_offset, 3);
    assert_eq!(instance.filter_pass_kind, first.kind.code());
    assert_eq!(
        instance.source_size,
        pack_u16_pair(first.source.width(), first.source.height())
    );
    assert_eq!(
        instance.dest_size,
        pack_u16_pair(first.dest.width(), first.dest.height())
    );
    assert_eq!(instance.dest_texture_size, pack_u16_pair(512, 512));
    assert_eq!(instance.original_size, pack_u16_pair(96, 64));
}

/// The pass numbering is the reference renderer's, kept so the kinds this
/// engine does not serve can be added without renumbering the wire format.
#[test]
fn the_pass_kinds_keep_the_reference_numbering() {
    assert_eq!(FilterPassKind::Copy.code(), 0);
    assert_eq!(FilterPassKind::Downscale.code(), 3);
    assert_eq!(FilterPassKind::BlurH.code(), 4);
    assert_eq!(FilterPassKind::BlurV.code(), 5);
    assert_eq!(FilterPassKind::Upscale.code(), 6);
}

// -----------------------------------------------------------------------------
// The shader sources
// -----------------------------------------------------------------------------

/// The filter program compiles the way a `wgpu::Device` would compile it — no
/// GPU, no adapter, and a failure surfaces here rather than as a
/// pipeline-creation error at run time.
#[test]
fn the_filter_module_parses_and_validates() {
    validate(FILTER_NAME, FILTER);
}

/// Assembled the way every engine module is: the binding-free helper prelude,
/// then the blur kernels, then the entry-point module. Neither prelude declares
/// a binding, so the module's derived bind-group layout is its own.
#[test]
fn the_filter_module_is_its_preludes_followed_by_its_own_source() {
    let helpers = include_str!("../shaders/helpers.wgsl");
    let kernels = include_str!("../shaders/filters_blur.wgsl");
    let entry = include_str!("../shaders/filter.wgsl");

    assert_eq!(FILTER, format!("{helpers}{kernels}{entry}"));

    for (name, source) in [("helpers", helpers), ("filters_blur", kernels)] {
        let declaration = source
            .lines()
            .find(|line| !line.trim_start().starts_with("//") && line.contains("@group"));
        assert!(
            declaration.is_none(),
            "{name} is prepended to every module that uses it, so a binding of its own would \
             change their derived bind-group layouts: {declaration:?}"
        );
    }
}

/// The constants the host and the shader both spell out have to agree; a
/// comment saying "keep in sync" is not a mechanism.
#[test]
fn the_shader_constants_match_the_host_ones() {
    let entry = include_str!("../shaders/filter.wgsl");
    let kernels = include_str!("../shaders/filters_blur.wgsl");

    assert_shader_const(
        entry,
        "FILTER_ATLAS_PADDING",
        u32::from(FILTER_ATLAS_PADDING),
    );
    assert_shader_const(entry, "FILTER_SIZE_BYTES", FILTER_SIZE_BYTES as u32);
    assert_shader_const(kernels, "MAX_TAPS_PER_SIDE", MAX_TAPS_PER_SIDE as u32);

    for kind in [
        FilterPassKind::Copy,
        FilterPassKind::Downscale,
        FilterPassKind::BlurH,
        FilterPassKind::BlurV,
        FilterPassKind::Upscale,
    ] {
        let name = match kind {
            FilterPassKind::Copy => "PASS_COPY",
            FilterPassKind::Downscale => "PASS_DOWNSCALE",
            FilterPassKind::BlurH => "PASS_BLUR_H",
            FilterPassKind::BlurV => "PASS_BLUR_V",
            FilterPassKind::Upscale => "PASS_UPSCALE",
        };
        assert_shader_const(entry, name, kind.code());
    }
}

/// The value the WGSL declares for `name`, as a `u32`.
fn assert_shader_const(source: &str, name: &str, expected: u32) {
    let needle = format!("const {name}: u32 = ");
    let line = source
        .lines()
        .find(|line| line.trim_start().starts_with(&needle))
        .unwrap_or_else(|| panic!("the shader declares no `{name}`"));
    let value = line
        .trim_start()
        .trim_start_matches(&needle)
        .trim_end()
        .trim_end_matches(';')
        .trim_end_matches('u');

    assert_eq!(
        value.parse::<u32>().ok(),
        Some(expected),
        "the shader's `{name}` is `{value}`, the host's is `{expected}`"
    );
}

/// Parses and validates `src` the way a `wgpu::Device` would.
fn validate(name: &str, src: &str) -> naga::Module {
    let module = naga::front::wgsl::parse_str(src)
        .unwrap_or_else(|err| panic!("{name} failed to parse: {err:?}"));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap_or_else(|err| panic!("{name} failed validation: {err:?}"));
    module
}

/// The byte size naga computed for the struct named `name`.
fn struct_span(module: &naga::Module, name: &str) -> u32 {
    module
        .types
        .iter()
        .find_map(|(_, ty)| match (&ty.name, &ty.inner) {
            (Some(ty_name), naga::TypeInner::Struct { span, .. }) if ty_name == name => Some(*span),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the module declares no struct named {name}"))
}

// -----------------------------------------------------------------------------
// The pass sequence
// -----------------------------------------------------------------------------

/// A blur is `n` halvings, one horizontal and one vertical convolution at the
/// decimated resolution, then `n` doublings back — the reference's own
/// sequence, and the reason a large σ costs passes rather than taps.
#[test]
fn a_blur_is_its_decimations_a_convolution_and_the_way_back() {
    let expectations: [(f32, Vec<FilterPassKind>); 3] = [
        (2.0, vec![FilterPassKind::BlurH, FilterPassKind::BlurV]),
        (
            8.0,
            vec![
                FilterPassKind::Downscale,
                FilterPassKind::Downscale,
                FilterPassKind::BlurH,
                FilterPassKind::BlurV,
                FilterPassKind::Upscale,
                FilterPassKind::Upscale,
            ],
        ),
        (
            32.0,
            vec![
                FilterPassKind::Downscale,
                FilterPassKind::Downscale,
                FilterPassKind::Downscale,
                FilterPassKind::Downscale,
                FilterPassKind::BlurH,
                FilterPassKind::BlurV,
                FilterPassKind::Upscale,
                FilterPassKind::Upscale,
                FilterPassKind::Upscale,
                FilterPassKind::Upscale,
            ],
        ),
    ];

    for (sigma, expected) in expectations {
        let blur = GaussianBlur::new(sigma, EdgeMode::default());
        let kinds: Vec<FilterPassKind> = blur_passes(&blur, SizeU16::from_wh(96, 64))
            .iter()
            .map(|step| step.kind)
            .collect();

        assert_eq!(kinds, expected, "σ {sigma}");
    }
}

/// Each pass writes the page it did not read, so an odd-length sequence would
/// leave the result in the scratch page rather than the one the parent
/// composites. A blur's is always even, which is why it never needs the copy
/// the reference's `ensure_result_in_original` would append.
#[test]
fn a_blurs_pass_sequence_is_always_even_and_never_needs_a_closing_copy() {
    for sigma in [0.0, 0.5, 2.0, 8.0, 32.0, 512.0, MAX_BLUR_SIGMA] {
        let blur = GaussianBlur::new(sigma, EdgeMode::default());
        let steps = blur_passes(&blur, SizeU16::from_wh(96, 64));

        assert_eq!(steps.len(), 2 * blur.n_decimations + 2, "σ {sigma}");
        assert!(steps.len().is_multiple_of(2), "σ {sigma}");
        assert!(
            !steps.iter().any(|step| step.kind == FilterPassKind::Copy),
            "σ {sigma} planned a closing copy it should not have needed"
        );
    }
}

/// A step's destination is the next step's source, and the extents halve and
/// double back to exactly the layer's own — an off-by-one on an odd axis would
/// composite a filtered image one texel short of the page it was rendered at.
#[test]
fn the_extents_halve_and_double_back_to_the_layers_own() {
    // Odd on both axes, which is where the halving's rounding shows.
    let size = SizeU16::from_wh(97, 63);
    let blur = GaussianBlur::new(8.0, EdgeMode::default());
    let steps = blur_passes(&blur, size);

    assert_eq!(
        steps.first().map(|step| step.source),
        Some(size),
        "the first pass reads the layer at its own extent"
    );
    assert_eq!(
        steps.last().map(|step| step.dest),
        Some(size),
        "the last pass writes the layer back at its own extent"
    );

    for pair in steps.windows(2) {
        assert_eq!(
            pair[0].dest, pair[1].source,
            "a pass reads exactly what the pass before it wrote: {pair:?}"
        );
    }

    let decimated = SizeU16::from_wh(25, 16);
    for step in &steps {
        if step.kind == FilterPassKind::BlurH || step.kind == FilterPassKind::BlurV {
            assert_eq!(step.source, decimated);
            assert_eq!(step.dest, decimated);
        }
    }
}

/// A convolution neither grows nor shrinks what it reads; only the rescaling
/// passes change extent.
#[test]
fn only_the_rescaling_passes_change_extent() {
    let blur = GaussianBlur::new(32.0, EdgeMode::default());

    for step in blur_passes(&blur, SizeU16::from_wh(96, 64)) {
        match step.kind {
            FilterPassKind::Downscale => {
                assert_eq!(step.dest.width(), step.source.width().div_ceil(2));
                assert_eq!(step.dest.height(), step.source.height().div_ceil(2));
            }
            FilterPassKind::Upscale => {
                assert!(step.dest.width() > step.source.width());
                assert!(step.dest.height() > step.source.height());
            }
            FilterPassKind::BlurH | FilterPassKind::BlurV | FilterPassKind::Copy => {
                assert_eq!(step.source, step.dest);
            }
        }
    }
}

// -----------------------------------------------------------------------------
// Recording a filter layer
// -----------------------------------------------------------------------------

/// The blur's whole point at the recording level: the layer's bounds grow by
/// the distance the blur paints past its contents, so the page it is rendered
/// into is large enough to hold the halo rather than clipping it.
#[test]
fn a_blurred_layers_bounds_grow_by_the_blurs_own_spread() {
    let plain = {
        let mut recorder = recorder();
        recorder.push_layer(layer(0.5), None);
        draw(&mut recorder, 64, 64, 32);
        recorder.pop_layer();
        recorder
    };
    let plain_bbox = plain.layers[0].bbox;

    for sigma in SIGMAS {
        let blurred = blurred_layer(sigma, 0.5);
        let bbox = blurred.layers[0].bbox;
        // 3σ each side, snapped up to the tile grid; the low edges are clamped
        // at the viewport origin rather than going negative.
        let spread = (3.0 * sigma).ceil() as u16;

        assert!(
            bbox.width() >= plain_bbox.width() + spread,
            "σ {sigma}: {bbox:?} did not grow past {plain_bbox:?} by 3σ"
        );
        assert!(
            bbox.height() >= plain_bbox.height() + spread,
            "σ {sigma}: {bbox:?} did not grow past {plain_bbox:?} by 3σ"
        );
    }
}

/// A σ that is not a standard deviation is refused where it is recorded, and
/// refused without opening a layer no `pop_layer` would balance.
#[test]
fn a_sigma_that_is_not_a_standard_deviation_is_refused_at_the_recorder() {
    for sigma in [f32::NAN, f32::INFINITY, -1.0, MAX_BLUR_SIGMA + 1.0] {
        let mut recorder = recorder();
        let refusal = push_filter_layer(
            &mut recorder,
            layer(0.5),
            LayerFilter::Blur { sigma },
            Affine::IDENTITY,
        );

        assert!(
            matches!(refusal, Err(EngineError::InvalidGeometry)),
            "σ {sigma} recorded as {refusal:?}"
        );
        assert!(
            recorder.layers.is_empty(),
            "σ {sigma} opened a layer despite being refused"
        );
    }
}

/// The transform is what scales σ into device space, so a transform that is not
/// finite is refused on the same terms the compiler already refuses one on.
#[test]
fn a_non_finite_transform_is_refused_at_the_recorder() {
    let mut recorder = recorder();
    let refusal = push_filter_layer(
        &mut recorder,
        layer(0.5),
        LayerFilter::Blur { sigma: 8.0 },
        Affine::translate((f64::NAN, 0.0)),
    );

    assert!(matches!(refusal, Err(EngineError::InvalidTransform)));
    assert!(recorder.layers.is_empty());
}

/// σ is a user-space quantity; the transform in force when the layer is opened
/// is what turns it into the device-space spread the page is sized from.
#[test]
fn the_transform_scales_the_blur_into_device_space() {
    let mut scaled = recorder();
    push_filter_layer(
        &mut scaled,
        layer(0.5),
        LayerFilter::Blur { sigma: 8.0 },
        Affine::scale(2.0),
    )
    .expect("a finite transform records");
    draw(&mut scaled, 64, 64, 32);
    scaled.pop_layer();

    let unscaled = blurred_layer(8.0, 0.5);

    assert!(
        scaled.layers[0].bbox.width() > unscaled.layers[0].bbox.width(),
        "a 2x transform must widen the blurred layer's bounds"
    );
}

// -----------------------------------------------------------------------------
// The rounds a filter layer costs
// -----------------------------------------------------------------------------

/// The shape the whole feature is for: the layer's contents into one page, one
/// round per filter pass alternating between the two, and the surface's own
/// round compositing whatever the last pass wrote.
#[test]
fn a_blurred_layer_costs_its_contents_its_passes_and_the_composite() {
    let blur = GaussianBlur::new(8.0, EdgeMode::default());
    let passes = blur_passes(&blur, SizeU16::from_wh(1, 1)).len();
    let rounds = schedule(&blurred_layer(8.0, 0.5));

    assert_eq!(
        rounds.len(),
        passes + 2,
        "one round for the contents, one per pass, and the surface's own: {rounds:#?}"
    );

    let contents = &rounds[0];
    assert!(contents.filter_pass().is_none());
    assert_eq!(contents.draw_count(), 1);
    let contents_page = contents.page().expect("the contents render into a page");
    assert!(!contents_page.continued, "the layer's first round clears");

    let mut source = contents_page.parity;
    for round in &rounds[1..rounds.len() - 1] {
        let pass = round.filter_pass().expect("a filter round runs a pass");
        let page = round.page().expect("a filter pass writes a page");

        assert!(round.ops.is_empty(), "a filter round draws nothing");
        assert_eq!(pass.layer, 0);
        assert_eq!(pass.source, source, "a pass reads the page before it wrote");
        assert_eq!(
            page.parity,
            source.opposite(),
            "a pass writes the group it did not read"
        );
        assert!(
            !page.continued,
            "a filter round clears, so what surrounds the region it writes is transparent"
        );
        assert_eq!(page.size, contents_page.size, "both pages are one size");
        assert_eq!(page.bounds, contents_page.bounds);

        source = page.parity;
    }

    let root = rounds.last().expect("a frame always has a root round");
    assert!(matches!(root.target, RoundTarget::Root));
    let composite = root
        .composites()
        .next()
        .expect("the surface composites the filtered layer");
    assert_eq!(
        composite.parity, contents_page.parity,
        "an even sequence leaves the result in the page the contents were rendered into"
    );
    assert_eq!(composite.opacity, 0.5);
}

/// Both groups are the filter layer's for the length of its sequence, and the
/// scratch one goes back to the pool as the last pass ends rather than being
/// held to the composite.
#[test]
fn the_scratch_page_goes_back_at_the_last_pass_and_the_result_at_the_composite() {
    let rounds = schedule(&blurred_layer(8.0, 0.5));
    let result = rounds[0]
        .page()
        .expect("the contents render into a page")
        .parity;

    let filter_rounds: Vec<&Round> = rounds
        .iter()
        .filter(|round| round.filter_pass().is_some())
        .collect();
    let (last, earlier) = filter_rounds
        .split_last()
        .expect("a blur costs at least one pass");

    for round in earlier {
        assert!(
            round.released.is_empty(),
            "both pages are live until the last pass: {round:#?}"
        );
    }
    assert_eq!(
        last.released,
        vec![result.opposite()],
        "the last pass hands the scratch group back"
    );

    let root = rounds.last().expect("a frame always has a root round");
    assert_eq!(
        root.released,
        vec![result],
        "the composite hands the result's group back"
    );
}

/// A blurred layer at full opacity still costs its passes: filtering a layer's
/// contents is not what drawing them into the parent does, so the inlining a
/// plain opaque layer gets does not apply.
#[test]
fn a_blurred_layer_at_full_opacity_is_still_isolated() {
    let rounds = schedule(&blurred_layer(8.0, 1.0));

    assert!(rounds.len() > 1, "an opaque blurred layer was inlined away");
    assert!(rounds.iter().any(|round| round.filter_pass().is_some()));
}

/// A layer at zero opacity contributes nothing whatever it filters, and a
/// filtered layer covering no pixel is not worth a page or a pass either.
#[test]
fn a_blurred_layer_that_contributes_nothing_costs_no_pass() {
    let transparent = schedule(&blurred_layer(8.0, 0.0));
    assert_eq!(transparent.len(), 1, "{transparent:#?}");
    assert!(
        transparent
            .iter()
            .all(|round| round.filter_pass().is_none())
    );

    let mut empty = recorder();
    push_filter_layer(
        &mut empty,
        layer(0.5),
        LayerFilter::Blur { sigma: 8.0 },
        Affine::IDENTITY,
    )
    .expect("a finite σ records");
    empty.pop_layer();
    let rounds = schedule(&empty);

    assert_eq!(rounds.len(), 1, "{rounds:#?}");
    assert!(rounds.iter().all(|round| round.filter_pass().is_none()));
}

/// A larger σ costs more passes and so more rounds, which is the price
/// decimation trades a bounded kernel for.
#[test]
fn a_larger_sigma_costs_more_rounds() {
    let counts: Vec<usize> = SIGMAS
        .iter()
        .map(|sigma| schedule(&blurred_layer(*sigma, 0.5)).len())
        .collect();

    assert_eq!(counts, vec![4, 8, 12], "σ 2 / 8 / 32");
}

/// A filter layer takes the group its own depth names for its contents, exactly
/// as an opacity layer does, and the other for its passes.
#[test]
fn a_filter_layer_takes_its_depths_group_for_its_contents() {
    let rounds = schedule(&blurred_layer(2.0, 0.5));
    let contents = rounds[0].page().expect("the contents render into a page");

    assert_eq!(contents.depth, 1);
    assert_eq!(contents.parity, PageParity::from_depth(1));
}

/// A blurred layer beside an opacity layer is served: the sibling's page comes
/// back when the surface's round is cut, and the filter takes both groups.
#[test]
fn a_blurred_layer_beside_an_opacity_layer_is_served_by_cutting_the_surfaces_round() {
    let mut recorder = recorder();
    recorder.push_layer(layer(0.5), None);
    draw(&mut recorder, 16, 16, 16);
    recorder.pop_layer();
    push_filter_layer(
        &mut recorder,
        layer(0.5),
        LayerFilter::Blur { sigma: 2.0 },
        Affine::IDENTITY,
    )
    .expect("a finite σ records");
    draw(&mut recorder, 96, 96, 16);
    recorder.pop_layer();

    let rounds = schedule(&recorder);
    let filter_rounds = rounds
        .iter()
        .filter(|round| round.filter_pass().is_some())
        .count();

    assert_eq!(filter_rounds, 2, "{rounds:#?}");
    // The sibling's composite was emitted early, in a cut round of the
    // surface's own, so its page was back in the pool before the filter's
    // passes needed it.
    assert!(
        rounds
            .iter()
            .filter(|round| matches!(round.target, RoundTarget::Root))
            .count()
            > 1,
        "the surface's round had to be cut to hand the second group back: {rounds:#?}"
    );
    assert_eq!(
        rounds
            .iter()
            .flat_map(|round| round.ops.iter())
            .filter(|op| matches!(op, RoundOp::Composite(_)))
            .count(),
        2
    );
}

// -----------------------------------------------------------------------------
// What is refused
// -----------------------------------------------------------------------------

/// The frame path's own entry point still refuses every filter layer, because
/// the renderer has no pipeline to run a filter pass through: a filter round
/// draws nothing and composites nothing, so a caller that ignored one would
/// clear the page and composite the hole.
#[test]
fn the_frame_paths_entry_point_still_refuses_a_filter_layer() {
    let recorder = blurred_layer(8.0, 0.5);
    let refusal = Schedule::build(&recorder, &caps(), &PageConfig::default());

    let EngineError::SchedulerEscalation { reason } = refusal.expect_err("build refuses filters")
    else {
        panic!("expected an escalation");
    };
    assert!(
        reason.contains("Gaussian blur") && reason.contains("build_with_filters"),
        "the reason has to name both what was found and where it is served: {reason}"
    );
}

/// Only the blur graduates. Every other filter a recording can carry is refused
/// by name — including on the entry point that serves the blur.
#[test]
fn every_filter_but_the_blur_is_still_refused_by_name() {
    let mut recorder = recorder();
    let flood = Filter::from_primitive(FilterPrimitive::Flood { color: RED });
    recorder.push_layer(
        layer(0.5),
        Some(vello_common::filter::FilterData::new(
            flood,
            Affine::IDENTITY,
        )),
    );
    draw(&mut recorder, 64, 64, 32);
    recorder.pop_layer();

    let reason = escalation(&recorder);
    assert!(
        reason.contains("Gaussian blur"),
        "the reason has to say what the engine does serve: {reason}"
    );
}

/// A filter layer inside another layer is refused rather than rendered from
/// bounds that would clip it: `vello_common` places one in its parent by
/// undoing a source shift the reference renderer applies to a filter layer's
/// contents and this engine's compiler does not.
#[test]
fn a_filter_layer_inside_another_layer_is_refused() {
    for outer in [0.5, 1.0] {
        let mut recorder = recorder();
        recorder.push_layer(layer(outer), None);
        push_filter_layer(
            &mut recorder,
            layer(0.5),
            LayerFilter::Blur { sigma: 8.0 },
            Affine::IDENTITY,
        )
        .expect("a finite σ records");
        draw(&mut recorder, 64, 64, 32);
        recorder.pop_layer();
        recorder.pop_layer();

        let reason = escalation(&recorder);
        assert!(
            reason.contains("recorded inside another layer"),
            "outer opacity {outer}: {reason}"
        );
    }
}

/// The padding overflow the reference guards: a blur grows its layer past what
/// a page can be sized to, and the frame is refused rather than clipped to the
/// ceiling — and never panicked (E17), however far past the ceiling it lands.
#[test]
fn a_blur_that_grows_its_layer_past_a_page_is_refused_rather_than_clipped() {
    let config = PageConfig {
        min_page_size: 512,
        max_page_size: 1024,
    };

    // Small enough contents that only the blur's own spread pushes the layer
    // past the ceiling.
    let mut recorder = recorder();
    push_filter_layer(
        &mut recorder,
        layer(0.5),
        LayerFilter::Blur { sigma: 400.0 },
        Affine::IDENTITY,
    )
    .expect("a finite σ under the ceiling records");
    draw(&mut recorder, 64, 64, 32);
    recorder.pop_layer();

    assert!(
        recorder.layers[0].bbox.width() > 1024,
        "the blur has to be what pushes the layer past the ceiling: {:?}",
        recorder.layers[0].bbox
    );
    assert!(matches!(
        Schedule::build_with_filters(&recorder, &caps(), &config),
        Err(EngineError::IntermediateTextureTooLarge)
    ));
}

/// The refusal is a return value at every σ the recorder accepts, including the
/// largest — a blur is never a panic, whatever it is asked for.
#[test]
fn no_sigma_the_recorder_accepts_panics_the_scheduler() {
    let config = PageConfig {
        min_page_size: 512,
        max_page_size: 1024,
    };

    for sigma in [0.0, 0.5, 2.0, 8.0, 32.0, 400.0, 4000.0, MAX_BLUR_SIGMA] {
        let mut recorder = recorder();
        push_filter_layer(
            &mut recorder,
            layer(0.5),
            LayerFilter::Blur { sigma },
            Affine::IDENTITY,
        )
        .expect("every σ here is finite and inside the ceiling");
        draw(&mut recorder, 64, 64, 32);
        recorder.pop_layer();

        match Schedule::build_with_filters(&recorder, &caps(), &config) {
            Ok(rounds) => assert!(!rounds.is_empty(), "σ {sigma}"),
            Err(EngineError::IntermediateTextureTooLarge) => {}
            Err(other) => panic!("σ {sigma} refused unexpectedly: {other:?}"),
        }
    }
}

/// A blurred layer with a non-default blend, a mask or a layer clip path is
/// refused on the properties themselves, before the filter is ever considered —
/// the filter round changes nothing about what a layer's own properties can be.
#[test]
fn a_blurred_layer_with_a_blend_mode_is_refused_on_the_blend() {
    use vello_common::peniko::{Compose, Mix};

    let mut recorder = recorder();
    let props = LayerProps {
        blend_mode: BlendMode::new(Mix::Multiply, Compose::SrcOver),
        opacity: 0.5,
        mask: None,
        clip_path: None,
    };
    push_filter_layer(
        &mut recorder,
        props,
        LayerFilter::Blur { sigma: 8.0 },
        Affine::IDENTITY,
    )
    .expect("a finite σ records");
    draw(&mut recorder, 64, 64, 32);
    recorder.pop_layer();

    let reason = escalation(&recorder);
    assert!(reason.contains("blend mode"), "{reason}");
}
