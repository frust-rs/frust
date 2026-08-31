//! The Gaussian-blur filter layer: the kernel it blurs with, the parameter
//! block the fragment stage reads, the passes it costs, and the rounds the
//! scheduler plans for it.
//!
//! Most of the file is device-free: which passes a σ costs and at what extents,
//! what a kernel packs into 48 bytes, which page each round writes are all
//! decisions over plain values. The last section is not — it runs the ported
//! WGSL on a real adapter and compares the texels it produces against a CPU
//! reference computed from the same `vello_common` kernels, which is the only
//! claim a host test cannot make and the only place the shader is exercised at
//! all (see *On real hardware* below).
//!
//! Four kinds of claim are made here.
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
//! - **On real hardware.** The `#[ignore]`d cases at the end run a whole blur
//!   sequence at σ 2, 8 and 32 through `FilterResources` — the same type the
//!   frame path drives, with the same pipeline, the same filter-data texture,
//!   the same sampler and the same instance packing — over two pages of the
//!   test's own, and compare the result against a CPU reference built from
//!   `vello_common::filter::gaussian_blur`'s own kernel and decimation plan.
//!
//!   Over the test's own pages rather than through
//!   [`frust_engine::EngineRenderer::encode`], and that is a limitation worth
//!   naming: `frust_scene` carries no filter command (the scene seam is a later
//!   plan), so no `Scene` can record a filter layer and no `encode` call can
//!   reach a filter round. What these cases prove is that the WGSL, the
//!   pipeline, the bind-group shapes and the wire layouts are right on a
//!   device; that the *scheduler's* rounds drive them in the right order is
//!   pinned by the host cases above, and the two meet in the frame path the day
//!   a scene command exists to enter it.

use frust_engine::filters::blur::{
    FILTER_ATLAS_PADDING, FILTER_SIZE_BYTES, FilterInstanceData, GpuFilterData, GpuGaussianBlur,
    LinearKernel, MAX_KERNEL_SIZE, MAX_TAPS_PER_SIDE, blur_passes, edge_mode, filter_type,
    pack_u16_pair,
};
use frust_engine::filters::drop_shadow::{GpuDropShadow, drop_shadow_passes};
use frust_engine::filters::{
    FilterPassKind, LayerFilter, MAX_BLUR_SIGMA, push_filter_layer, served_drop_shadow,
};
use frust_engine::gpu::pipelines::{
    EnginePipeline, EngineShaders, INTERMEDIATE_FORMAT, warm_up_descs,
};
use frust_engine::gpu::shader_src::{FILTER, FILTER_NAME};
use frust_engine::gpu::targets::{INTERMEDIATE_USAGE, IntermediateTargets};
use frust_engine::renderer::{FilterPassPlan, FilterResources};
use frust_engine::schedule::{PageConfig, PageParity, Round, RoundOp, RoundTarget, Schedule};
use frust_engine::{EngineDraw, EngineError};
use frust_gpu::{DownlevelProfile, PipelineCache, ShaderLibrary, TierCaps};
use kurbo::Affine;
use peniko::color::{AlphaColor, Srgb};
use vello_common::color::palette::css::RED;
use vello_common::filter::drop_shadow::DropShadow;
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

/// A recording of one shadowed layer under the surface, holding one draw.
fn drop_shadow_layer(
    offset: (f32, f32),
    sigma: f32,
    color: AlphaColor<Srgb>,
    opacity: f32,
) -> CommandRecorder<EngineDraw> {
    let mut recorder = recorder();
    push_filter_layer(
        &mut recorder,
        layer(opacity),
        LayerFilter::DropShadow {
            offset,
            sigma,
            color,
        },
        Affine::IDENTITY,
    )
    .expect("a finite offset and σ under the ceiling records");
    draw(&mut recorder, 64, 64, 32);
    recorder.pop_layer();
    recorder
}

/// A `vello_common` drop shadow built the way [`served_drop_shadow`] hands one
/// back: through `PreparedFilter::new`, so its decimation plan and kernel are
/// the reference's own rather than re-derived.
fn shadow(offset: (f32, f32), sigma: f32, color: AlphaColor<Srgb>) -> DropShadow {
    let filter = Filter::from_primitive(FilterPrimitive::DropShadowOnly {
        dx: offset.0,
        dy: offset.1,
        std_deviation: sigma,
        color,
        edge_mode: EdgeMode::default(),
    });
    match vello_common::filter::PreparedFilter::new(&filter, &Affine::IDENTITY) {
        vello_common::filter::PreparedFilter::DropShadow(shadow) => shadow,
        other => panic!("a `DropShadowOnly` primitive must prepare as a drop shadow: {other:?}"),
    }
}

fn schedule(recorder: &CommandRecorder<EngineDraw>) -> Vec<Round> {
    Schedule::build(recorder, &caps(), &PageConfig::default())
        .expect("a blurred layer under the surface schedules")
}

fn escalation(recorder: &CommandRecorder<EngineDraw>) -> String {
    match Schedule::build(recorder, &caps(), &PageConfig::default()) {
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
    assert_eq!(size_of::<GpuDropShadow>(), FILTER_SIZE_BYTES);
    assert_eq!(align_of::<GpuFilterData>(), 16);
    assert_eq!(align_of::<GpuGaussianBlur>(), 16);
    assert_eq!(align_of::<GpuDropShadow>(), 16);
    assert_eq!(GpuFilterData::SIZE_TEXELS, 3);
}

/// A drop shadow's parameter block survives the round trip through the
/// type-erased form, and packs its kernel at the same offsets a blur's own
/// does — the reason its blur passes read through the exact accessors a plain
/// blur's do, unmodified.
#[test]
fn a_drop_shadows_parameter_block_round_trips_through_the_erased_form() {
    for (offset, sigma) in [((0.0, 0.0), 2.0), ((3.0, -4.0), 8.0), ((-12.0, 20.0), 32.0)] {
        let shadow_value = shadow(offset, sigma, RED);
        let packed = GpuDropShadow::from(&shadow_value);
        let erased = GpuFilterData::from(packed);

        assert_eq!(
            erased.filter_type(),
            filter_type::DROP_SHADOW,
            "{offset:?} σ {sigma}"
        );
        assert_eq!(
            erased.n_decimations(),
            shadow_value.n_decimations,
            "{offset:?} σ {sigma}"
        );
        assert_eq!(
            bytemuck::cast::<GpuFilterData, GpuDropShadow>(erased),
            packed,
            "{offset:?} σ {sigma}"
        );
        assert_eq!(packed.dx, offset.0, "{offset:?} σ {sigma}");
        assert_eq!(packed.dy, offset.1, "{offset:?} σ {sigma}");
        assert_eq!(
            packed.color,
            RED.premultiply().to_rgba8().to_u32(),
            "{offset:?} σ {sigma}"
        );

        let linear = LinearKernel::new(&shadow_value.kernel, shadow_value.kernel_size);
        assert_eq!(
            packed.center_weight, linear.center_weight,
            "{offset:?} σ {sigma}"
        );
        assert_eq!(
            (packed.header >> 11) & 0x3,
            u32::from(linear.n_taps),
            "{offset:?} σ {sigma}"
        );
    }
}

/// This engine never composites a filter layer's original content back over
/// its shadow (see `frust_engine::filters::drop_shadow`'s own doc), so a
/// shadow-only block never sets the bit a future `composite_original` shape
/// would read.
#[test]
fn a_drop_shadows_header_never_sets_the_composite_bit() {
    for sigma in [0.0, 2.0, 8.0, 32.0, MAX_BLUR_SIGMA] {
        let packed = GpuDropShadow::from(&shadow((3.0, -4.0), sigma, RED));
        assert_eq!(packed.header & (1 << 13), 0, "σ {sigma}");
    }
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
    assert_eq!(FilterPassKind::Offset.code(), 2);
    assert_eq!(FilterPassKind::Downscale.code(), 3);
    assert_eq!(FilterPassKind::BlurH.code(), 4);
    assert_eq!(FilterPassKind::BlurV.code(), 5);
    assert_eq!(FilterPassKind::Upscale.code(), 6);
    assert_eq!(FilterPassKind::Colorize.code(), 8);
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
/// then the blur kernels, then the drop-shadow kernels, then the entry-point
/// module. No prelude declares a binding, so the module's derived bind-group
/// layout is its own.
#[test]
fn the_filter_module_is_its_preludes_followed_by_its_own_source() {
    let helpers = include_str!("../shaders/helpers.wgsl");
    let kernels = include_str!("../shaders/filters_blur.wgsl");
    let shadow_kernels = include_str!("../shaders/filters_drop_shadow.wgsl");
    let entry = include_str!("../shaders/filter.wgsl");

    assert_eq!(FILTER, format!("{helpers}{kernels}{shadow_kernels}{entry}"));

    for (name, source) in [
        ("helpers", helpers),
        ("filters_blur", kernels),
        ("filters_drop_shadow", shadow_kernels),
    ] {
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
        FilterPassKind::Offset,
        FilterPassKind::Downscale,
        FilterPassKind::BlurH,
        FilterPassKind::BlurV,
        FilterPassKind::Upscale,
        FilterPassKind::Colorize,
    ] {
        let name = match kind {
            FilterPassKind::Copy => "PASS_COPY",
            FilterPassKind::Offset => "PASS_OFFSET",
            FilterPassKind::Downscale => "PASS_DOWNSCALE",
            FilterPassKind::BlurH => "PASS_BLUR_H",
            FilterPassKind::BlurV => "PASS_BLUR_V",
            FilterPassKind::Upscale => "PASS_UPSCALE",
            FilterPassKind::Colorize => "PASS_COLORIZE",
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
            FilterPassKind::Offset | FilterPassKind::Colorize => {
                unreachable!("a blur's own sequence never emits an offset or colourize pass")
            }
        }
    }
}

// -----------------------------------------------------------------------------
// The drop shadow's own passes
// -----------------------------------------------------------------------------

/// A shadow is the offset, exactly the blur sequence its own kernel plans, and
/// the colourize, in that order — the reference's own drop-shadow plan
/// narrowed to the shadow-only shape.
#[test]
fn a_drop_shadow_is_the_offset_the_blur_and_the_colorize() {
    for (offset, sigma) in [((0.0, 0.0), 2.0), ((3.0, -4.0), 8.0), ((12.0, 20.0), 32.0)] {
        let shadow_value = shadow(offset, sigma, RED);
        let blur = GaussianBlur::new(sigma, EdgeMode::default());
        let size = SizeU16::from_wh(96, 64);

        let steps = drop_shadow_passes(&shadow_value, size);
        let (first, rest) = steps
            .split_first()
            .expect("a shadow costs at least one pass");
        let (last, middle) = rest
            .split_last()
            .expect("a shadow costs at least two passes");

        assert_eq!(first.kind, FilterPassKind::Offset, "{offset:?} σ {sigma}");
        assert_eq!(first.source, size, "{offset:?} σ {sigma}");
        assert_eq!(first.dest, size, "{offset:?} σ {sigma}");

        assert_eq!(last.kind, FilterPassKind::Colorize, "{offset:?} σ {sigma}");
        assert_eq!(last.source, size, "{offset:?} σ {sigma}");
        assert_eq!(last.dest, size, "{offset:?} σ {sigma}");

        let blur_kinds: Vec<FilterPassKind> = middle.iter().map(|step| step.kind).collect();
        let expected_kinds: Vec<FilterPassKind> = blur_passes(&blur, size)
            .iter()
            .map(|step| step.kind)
            .collect();
        assert_eq!(blur_kinds, expected_kinds, "{offset:?} σ {sigma}");
    }
}

/// Offset and colourize add exactly two passes to the blur's own always-even
/// sequence, so a drop shadow's sequence never needs the closing copy
/// [`drop_shadow_passes`] would append if it ever went odd.
#[test]
fn a_drop_shadows_pass_sequence_is_always_even_and_never_needs_a_closing_copy() {
    for sigma in [0.0, 0.5, 2.0, 8.0, 32.0, 512.0, MAX_BLUR_SIGMA] {
        let shadow_value = shadow((3.0, -4.0), sigma, RED);
        let steps = drop_shadow_passes(&shadow_value, SizeU16::from_wh(96, 64));

        assert_eq!(steps.len(), 2 * shadow_value.n_decimations + 4, "σ {sigma}");
        assert!(steps.len().is_multiple_of(2), "σ {sigma}");
        assert!(
            !steps.iter().any(|step| step.kind == FilterPassKind::Copy),
            "σ {sigma} planned a closing copy it should not have needed"
        );
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

/// A shadow's bounds grow by its own offset on top of the blur's 3σ spread —
/// the reference's own filter expansion for `DropShadowOnly`
/// (`vello_common::filter_effects::FilterPrimitive::filter_expansion`), which
/// this engine takes as given rather than re-deriving (see
/// [`LayerFilter::filter_data`]).
#[test]
fn a_shadowed_layers_bounds_grow_by_the_blurs_spread_and_the_shadows_own_offset() {
    let plain = {
        let mut recorder = recorder();
        recorder.push_layer(layer(0.5), None);
        draw(&mut recorder, 64, 64, 32);
        recorder.pop_layer();
        recorder
    };
    let plain_bbox = plain.layers[0].bbox;

    let unshifted = drop_shadow_layer((0.0, 0.0), 8.0, RED, 0.5);
    let shifted = drop_shadow_layer((40.0, 0.0), 8.0, RED, 0.5);

    assert!(
        unshifted.layers[0].bbox.width() >= plain_bbox.width(),
        "a shadow's blur alone must still grow the layer: {:?}",
        unshifted.layers[0].bbox
    );
    assert!(
        shifted.layers[0].bbox.width() > unshifted.layers[0].bbox.width(),
        "an offset shadow must grow its layer past an unshifted one of the same σ: {:?} vs {:?}",
        shifted.layers[0].bbox,
        unshifted.layers[0].bbox
    );
}

/// A σ or an offset that is not finite is refused where it is recorded, on the
/// same terms [`LayerFilter::Blur`] is, and refused without opening a layer no
/// `pop_layer` would balance.
#[test]
fn a_drop_shadow_with_a_non_finite_sigma_or_offset_is_refused_at_the_recorder() {
    let cases: [(f32, f32, f32); 5] = [
        (f32::NAN, 0.0, 0.0),
        (8.0, f32::NAN, 0.0),
        (8.0, 0.0, f32::NAN),
        (8.0, f32::INFINITY, 0.0),
        (MAX_BLUR_SIGMA + 1.0, 0.0, 0.0),
    ];

    for (sigma, dx, dy) in cases {
        let mut recorder = recorder();
        let refusal = push_filter_layer(
            &mut recorder,
            layer(0.5),
            LayerFilter::DropShadow {
                offset: (dx, dy),
                sigma,
                color: RED,
            },
            Affine::IDENTITY,
        );

        assert!(
            matches!(refusal, Err(EngineError::InvalidGeometry)),
            "σ {sigma}, offset ({dx}, {dy}) recorded as {refusal:?}"
        );
        assert!(
            recorder.layers.is_empty(),
            "σ {sigma}, offset ({dx}, {dy}) opened a layer despite being refused"
        );
    }
}

// -----------------------------------------------------------------------------
// The rounds a filter layer costs
// -----------------------------------------------------------------------------

/// `served_drop_shadow` is what `Schedule::build` (and the renderer's own
/// `filter_block`) reads a recorded shadow-only drop shadow back through; it
/// has to hand back the same parameters [`LayerFilter::DropShadow`] recorded.
#[test]
fn served_drop_shadow_reads_back_what_was_recorded() {
    let recording = drop_shadow_layer((3.0, -4.0), 8.0, RED, 0.5);
    let kind = &recording.layers[0].kind;

    let served = served_drop_shadow(0, kind).expect("a recorded shadow-only drop shadow serves");
    assert_eq!(served.dx, 3.0);
    assert_eq!(served.dy, -4.0);
    assert_eq!(served.color, RED);
    assert!(
        !served.composite_original,
        "this engine never serves the compositing shape"
    );
}

/// The frame path's own entry point plans a blur rather than refusing it, and
/// it is the only entry point there is.
///
/// While the renderer had no filter pipeline, `Schedule::build` refused every
/// filter and a second `build_with_filters` planned them, so the frame path
/// could not reach a round nothing could execute. The renderer runs them now,
/// and one call plans them for every caller — this is the case that would fail
/// if the refusing arm ever came back without the pipeline going with it.
#[test]
fn the_frame_paths_own_entry_point_plans_a_blur() {
    let rounds = Schedule::build(&blurred_layer(8.0, 0.5), &caps(), &PageConfig::default())
        .expect("the one entry point plans a blur");

    assert!(rounds.iter().any(|round| round.filter_pass().is_some()));
}

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

/// The shape a shadow costs, on the same terms a blur's own does: the layer's
/// contents into one page, one round per pass of `drop_shadow_passes`
/// alternating between the two, and the surface's own round compositing
/// whatever the last pass wrote.
#[test]
fn a_shadowed_layer_costs_its_contents_its_passes_and_the_composite() {
    let shadow_value = shadow((3.0, -4.0), 8.0, RED);
    let passes = drop_shadow_passes(&shadow_value, SizeU16::from_wh(1, 1)).len();
    let rounds = schedule(&drop_shadow_layer((3.0, -4.0), 8.0, RED, 0.5));

    assert_eq!(
        rounds.len(),
        passes + 2,
        "one round for the contents, one per pass, and the surface's own: {rounds:#?}"
    );

    let contents = &rounds[0];
    assert!(contents.filter_pass().is_none());
    let contents_page = contents.page().expect("the contents render into a page");

    let mut source = contents_page.parity;
    for round in &rounds[1..rounds.len() - 1] {
        let pass = round.filter_pass().expect("a filter round runs a pass");
        let page = round.page().expect("a filter pass writes a page");
        assert_eq!(pass.source, source, "a pass reads the page before it wrote");
        assert_eq!(
            page.parity,
            source.opposite(),
            "a pass writes the group it did not read"
        );
        source = page.parity;
    }

    let root = rounds.last().expect("a frame always has a root round");
    let composite = root
        .composites()
        .next()
        .expect("the surface composites the shadowed layer");
    assert_eq!(
        composite.parity, contents_page.parity,
        "an even sequence leaves the result in the page the contents were rendered into"
    );
}

// -----------------------------------------------------------------------------
// What is refused
// -----------------------------------------------------------------------------

/// Only the blur and the shadow-only drop shadow graduate. Every other filter
/// a recording can carry is refused by name.
#[test]
fn every_filter_but_the_blur_or_drop_shadow_is_still_refused_by_name() {
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

/// The reference's other drop-shadow shape — the one that composites the
/// layer's own unfiltered content back over the shadow — is refused by name
/// too: this engine serves only `DropShadowOnly`, never `DropShadow` (see
/// `frust_engine::filters::drop_shadow`'s own doc for why).
#[test]
fn a_drop_shadow_that_composites_the_original_is_refused_by_name() {
    let mut recorder = recorder();
    let compositing = Filter::from_primitive(FilterPrimitive::DropShadow {
        dx: 3.0,
        dy: -4.0,
        std_deviation: 8.0,
        color: RED,
        edge_mode: EdgeMode::default(),
    });
    recorder.push_layer(
        layer(0.5),
        Some(vello_common::filter::FilterData::new(
            compositing,
            Affine::IDENTITY,
        )),
    );
    draw(&mut recorder, 64, 64, 32);
    recorder.pop_layer();

    let reason = escalation(&recorder);
    assert!(
        reason.contains("shadow-only"),
        "the reason has to name what this engine actually serves: {reason}"
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
///
/// A filter layer's page is sized by [`frust_engine::schedule::filter_page_size`]
/// (wired into `Schedule::build` since this test's own boundary was pinned),
/// which checks the PADDED request against two ceilings rather than one: this
/// case's bbox alone already clears the pool's configured `max_page_size`, so
/// once padded it is refused as [`EngineError::IntermediateTextureLimitReached`]
/// — the adapter could still serve it, but this pool's own budget will not —
/// rather than [`EngineError::IntermediateTextureTooLarge`], which is reserved
/// for a request past the ADAPTER's own ceiling (see the sibling test below,
/// which drives a σ large enough to cross that one too).
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
        Schedule::build(&recorder, &caps(), &config),
        Err(EngineError::IntermediateTextureLimitReached)
    ));
}

/// The refusal is a return value at every σ the recorder accepts, including the
/// largest — a blur is never a panic, whatever it is asked for.
///
/// Both refusal shapes are accepted here: `filter_page_size`'s padded request
/// can cross either ceiling depending how far the σ pushes it (see the two
/// tests above), and this sweep exists to pin "never a panic", not which of
/// the two named errors a given σ lands on.
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

        match Schedule::build(&recorder, &caps(), &config) {
            Ok(rounds) => assert!(!rounds.is_empty(), "σ {sigma}"),
            Err(
                EngineError::IntermediateTextureTooLarge
                | EngineError::IntermediateTextureLimitReached,
            ) => {}
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

// -----------------------------------------------------------------------------
// On real hardware
// -----------------------------------------------------------------------------
//
// Everything above is a decision over plain values. Below is the other half:
// the ported WGSL, the pipeline derived from it, the filter-data texture, the
// engine's one sampler and the instance packing, all exercised on a real
// adapter — the first time any of them runs at all.
//
// The pass sequence is driven through `FilterResources`, which is exactly what
// `EngineRenderer::record_frame` drives, over two pages of this file's own
// rather than two the scheduler named. See this file's header for why that
// stops short of `EngineRenderer::encode` and what it therefore does not claim.

/// Extent of both pages every hardware case renders between.
///
/// A power of two so a page read-back's `bytes_per_row` needs no padding, and
/// larger than [`FILTER_REGION`] on both axes, exactly as a pooled page is — the
/// pool quantizes a request up to its own 256-texel keys and never sizes a page
/// to the layer. The slack is not cosmetic: it is what makes a tap that reaches
/// *past* the region read a texel the round's own clear already zeroed.
const FILTER_PAGE: u32 = 512;

/// The filter layer's own extent inside those pages, at the page origin exactly
/// as the scheduler places one.
///
/// Deliberately not square, and its width is a multiple of 64 so the source
/// upload's `bytes_per_row` is already `COPY_BYTES_PER_ROW_ALIGNMENT`-aligned.
const FILTER_REGION: (u32, u32) = (448, 384);

/// The opaque rectangle the blur is measured on, inset inside
/// [`FILTER_REGION`] on every side.
///
/// The inset is what lets the comparison be exact rather than approximate, and
/// it is sized rather than picked. The one place the two tiers can disagree is
/// a tap that reaches past the region's *low* edge: the page extends past the
/// high edges, so a tap there reads a cleared texel exactly as the reference
/// reads transparent black, but a negative coordinate is clamped to the region's
/// own edge texel instead. That is harmless as long as the edge texel is itself
/// transparent — at every level of the pyramid.
///
/// A 160-texel transparent margin survives the deepest plan σ 32 produces with
/// room to spare: each halving takes it to roughly half (160 → 79 → 39 → 19 →
/// 9), the coarsest convolution's widest tap is `MAX_KERNEL_SIZE / 2` = 6, and
/// each doubling back restores it faster than the reconstruction spreads ink
/// into it. So the low edges stay transparent throughout and no border has to be
/// excluded from the comparison.
const FILTER_RECT: (u32, u32, u32, u32) = (160, 160, 128, 64);

/// Bytes one page texel occupies at `INTERMEDIATE_FORMAT`.
const PAGE_TEXEL_BYTES: u32 = 4;

/// Serializes every case in this binary that creates a GPU device, for the
/// reason `atlas_render.rs` and `encode_contract.rs` both document: each builds
/// a `wgpu::Device` of its own, and a driver that serializes device teardown on
/// a process-global mutex deadlocks when two tear down at once. Poison is
/// ignored deliberately — one case's failure must not cascade into its
/// siblings.
static RENDER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn render_lock() -> std::sync::MutexGuard<'static, ()> {
    RENDER_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Blocks on `future` by polling it to completion.
///
/// wgpu's native adapter and device requests resolve without an executor
/// driving them, so a bare poll loop is enough; this crate has no async runtime
/// of its own and these cases are the only callers.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::task::{Context, Poll, Waker};

    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

/// Pops a validation error scope, pumping the device until the pop resolves.
fn drain_error_scope(device: &wgpu::Device, scope: wgpu::ErrorScopeGuard) -> Option<wgpu::Error> {
    use std::task::{Context, Poll, Waker};

    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    let mut future = std::pin::pin!(scope.pop());
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(error) => return error,
            Poll::Pending => {
                let _ = device.poll(wgpu::PollType::wait_indefinitely());
            }
        }
    }
}

/// A device plus the capabilities probed off the adapter it came from.
fn gpu() -> (wgpu::Device, wgpu::Queue, TierCaps) {
    block_on(async {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        // The environment-aware initializer, so `WGPU_ADAPTER_NAME` picks the
        // GPU on a multi-adapter host instead of the run silently landing on
        // whichever one enumerates first.
        let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
            .await
            .expect("no compatible GPU adapter");
        println!("frust-engine filter adapter: {:?}", adapter.get_info());
        let caps = TierCaps::probe(&adapter);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("frust-engine filter test device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            })
            .await
            .expect("failed to create the device");
        (device, queue, caps)
    })
}

/// One case's device, filter pipeline, filter resources and page pair.
struct FilterHarness {
    device: wgpu::Device,
    queue: wgpu::Queue,
    caps: TierCaps,
    pipeline: wgpu::RenderPipeline,
    filters: FilterResources,
    /// The two pooled pages a filter sequence ping-pongs between, standing in
    /// for the two the scheduler's parity groups would hand out.
    pages: [Page; 2],
    /// Kept alive so the pipeline's shader module outlives the passes.
    _cache: PipelineCache,
}

/// One page: a texture with the exact usage
/// [`frust_engine::gpu::targets::INTERMEDIATE_USAGE`] gives a pooled one.
struct Page {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl FilterHarness {
    fn new() -> Self {
        let (device, queue, caps) = gpu();

        let mut library = ShaderLibrary::new();
        let shaders = EngineShaders::register(&mut library, &device);
        let mut cache = PipelineCache::new(std::sync::Arc::new(library), None);
        // The very description the warm-up list carries, so this is the same
        // pipeline object a real renderer would have compiled before its first
        // frame — the format argument is ignored by the filter variant.
        let desc = EnginePipeline::Filter.desc(&shaders, wgpu::TextureFormat::Bgra8Unorm);
        assert!(
            warm_up_descs(&shaders, wgpu::TextureFormat::Bgra8Unorm).contains(&desc),
            "the filter description must be one the warm-up list already covers"
        );
        let pipeline = cache.get_or_create(&device, &desc).clone();

        let pages = [page(&device, "a"), page(&device, "b")];
        let filters = FilterResources::new(&device);

        Self {
            device,
            queue,
            caps,
            pipeline,
            filters,
            pages,
            _cache: cache,
        }
    }

    /// Uploads `image` into page 0 at its origin, where the scheduler puts a
    /// filter layer's own contents.
    fn upload_source(&self, image: &Image) {
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.pages[0].texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &image.to_rgba8(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(image.width as u32 * PAGE_TEXEL_BYTES),
                rows_per_image: Some(image.height as u32),
            },
            wgpu::Extent3d {
                width: image.width as u32,
                height: image.height as u32,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Runs every pass of a blur's `steps` in one encoder, answering which page
    /// holds the result. A thin wrapper over [`Self::run_block`] for the
    /// blur-only callers this harness has always had.
    fn run(&mut self, blur: &GaussianBlur, steps: &[frust_engine::filters::FilterStep]) -> usize {
        self.run_block(GpuFilterData::from(GpuGaussianBlur::from(blur)), steps)
    }

    /// Runs every pass of `steps` against parameter block `block` in one
    /// encoder, answering which page holds the result.
    ///
    /// The ping-pong the scheduler plans, with page indices standing in for its
    /// parity groups: pass `i` reads the page pass `i - 1` wrote and writes the
    /// other, and the first reads the contents page. Generic over the block
    /// itself — a blur's and a drop shadow's alike are just erased 48-byte
    /// records by the time they reach here.
    fn run_block(
        &mut self,
        block: GpuFilterData,
        steps: &[frust_engine::filters::FilterStep],
    ) -> usize {
        let passes = u32::try_from(steps.len()).expect("a filter costs a handful of passes");
        self.filters
            .prepare(&self.device, &self.queue, &self.pipeline, &[block], passes);

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frust-engine filter test"),
            });

        let mut source = 0_usize;
        for (index, step) in steps.iter().enumerate() {
            let dest = 1 - source;
            let instance = u32::try_from(index).expect("a blur costs a handful of passes");
            self.filters.write_instance(
                &self.queue,
                instance,
                &FilterInstanceData::new(
                    step,
                    0,
                    (0, 0),
                    (0, 0),
                    SizeU16::from_wh(FILTER_PAGE as u16, FILTER_PAGE as u16),
                    SizeU16::from_wh(FILTER_REGION.0 as u16, FILTER_REGION.1 as u16),
                ),
            );
            self.filters.record_pass(
                &self.device,
                &mut encoder,
                &FilterPassPlan {
                    label: "frust-engine filter test pass",
                    pipeline: &self.pipeline,
                    dest: &self.pages[dest].view,
                    source: &self.pages[source].view,
                    instance,
                },
            );
            source = dest;
        }

        self.queue.submit([encoder.finish()]);
        source
    }

    /// One page's texels as an [`Image`] of [`FILTER_REGION`]'s extent.
    fn read_region(&self, page: usize) -> Image {
        let bytes_per_row = (FILTER_PAGE * PAGE_TEXEL_BYTES).next_multiple_of(256);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frust-engine filter readback"),
            size: u64::from(bytes_per_row) * u64::from(FILTER_PAGE),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frust-engine filter readback copy"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.pages[page].texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(FILTER_PAGE),
                },
            },
            wgpu::Extent3d {
                width: FILTER_PAGE,
                height: FILTER_PAGE,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);

        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("the readback poll must succeed");
        rx.recv()
            .expect("the readback channel must stay open")
            .expect("the readback buffer must map");

        let mapped = slice.get_mapped_range();
        let stride = bytes_per_row as usize;
        let mut image = Image::new(FILTER_REGION.0 as usize, FILTER_REGION.1 as usize);
        for y in 0..image.height {
            for x in 0..image.width {
                let at = y * stride + x * PAGE_TEXEL_BYTES as usize;
                image.data[y * image.width + x] = [
                    f32::from(mapped[at]),
                    f32::from(mapped[at + 1]),
                    f32::from(mapped[at + 2]),
                    f32::from(mapped[at + 3]),
                ];
            }
        }
        drop(mapped);
        buffer.unmap();
        image
    }
}

/// One page texture, with the pool's own format and usage plus `COPY_DST`.
///
/// The extra usage is the one place these pages differ from a pooled one, and
/// it is the test harness rather than the filter: a real contents page is
/// *rendered* into by a strip pass, and this file seeds it with an upload
/// instead so the source is an exact set of texels rather than a rasterization
/// the comparison would then have to model.
fn page(device: &wgpu::Device, label: &str) -> Page {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: FILTER_PAGE,
            height: FILTER_PAGE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: INTERMEDIATE_FORMAT,
        usage: INTERMEDIATE_USAGE | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Page { texture, view }
}

/// A premultiplied RGBA image in 0..=255 floats — the space both the GPU pages
/// and the CPU reference work in, so a comparison is a subtraction.
#[derive(Clone)]
struct Image {
    width: usize,
    height: usize,
    data: Vec<[f32; 4]>,
}

impl Image {
    fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![[0.0; 4]; width * height],
        }
    }

    /// Transparent everywhere but `rect`, which is opaque `color`.
    fn with_rect(width: usize, height: usize, rect: (u32, u32, u32, u32), color: [f32; 4]) -> Self {
        let mut image = Self::new(width, height);
        for y in rect.1 as usize..(rect.1 + rect.3) as usize {
            for x in rect.0 as usize..(rect.0 + rect.2) as usize {
                image.data[y * width + x] = color;
            }
        }
        image
    }

    /// The texel at `(x, y)`, or transparent black outside — `EdgeMode::None`,
    /// which is the mode every filter this engine serves is prepared with.
    fn sample(&self, x: i64, y: i64) -> [f32; 4] {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return [0.0; 4];
        }
        self.data[y as usize * self.width + x as usize]
    }

    /// Tightly packed RGBA8 bytes, for a texture upload.
    fn to_rgba8(&self) -> Vec<u8> {
        self.data
            .iter()
            .flat_map(|texel| texel.map(|c| c.clamp(0.0, 255.0).round() as u8))
            .collect()
    }

    /// Rounds every channel to the byte a pass would have stored.
    ///
    /// Applied once per *pass*, which is where this reference follows the GPU
    /// rather than the CPU rasterizer. Both store their intermediates in eight
    /// bits per channel — an `Rgba8Unorm` page here, a `PremulRgba8` pixmap
    /// there — but the rasterizer runs each rescaling step as two separable
    /// passes and rounds between them, while the shader collapses the two axes
    /// into one pass and rounds once. The kernel is identical either way (four
    /// bilinear samples reproduce `[1,3,3,1]/8` on both axes exactly); only the
    /// rounding schedule differs, and the schedule modelled here is the one the
    /// texels being compared actually went through.
    fn quantize(mut self) -> Self {
        for texel in &mut self.data {
            for channel in texel {
                *channel = channel.clamp(0.0, 255.0).round();
            }
        }
        self
    }

    /// The `(x, y)` of the largest per-channel difference from `other` inside
    /// the window `margin` texels in from every edge, and its size.
    fn worst_diff(&self, other: &Self, margin: usize) -> (f32, usize, usize) {
        let mut worst = (0.0_f32, 0_usize, 0_usize);
        for y in margin..self.height.saturating_sub(margin) {
            for x in margin..self.width.saturating_sub(margin) {
                let mine = self.data[y * self.width + x];
                let theirs = other.data[y * other.width + x];
                for channel in 0..4 {
                    let diff = (mine[channel] - theirs[channel]).abs();
                    if diff > worst.0 {
                        worst = (diff, x, y);
                    }
                }
            }
        }
        worst
    }

    /// The mean per-channel difference from `other` over the same window.
    fn mean_diff(&self, other: &Self, margin: usize) -> f32 {
        let mut total = 0.0_f64;
        let mut count = 0_u64;
        for y in margin..self.height.saturating_sub(margin) {
            for x in margin..self.width.saturating_sub(margin) {
                let mine = self.data[y * self.width + x];
                let theirs = other.data[y * other.width + x];
                for channel in 0..4 {
                    total += f64::from((mine[channel] - theirs[channel]).abs());
                    count += 1;
                }
            }
        }
        if count == 0 {
            return 0.0;
        }
        (total / count as f64) as f32
    }
}

/// The reference blur: `vello_common`'s own decimation plan and discrete
/// kernel, applied exactly the way the CPU rasterizer applies them.
///
/// Not a second implementation of the *kernel* — `blur.kernel` and
/// `blur.n_decimations` come straight out of
/// `vello_common::filter::gaussian_blur`, which is the whole point of the
/// comparison. What is reproduced here is the pyramid the rasterizer walks:
/// `n` decimations by the separable `[1,3,3,1]/8` binomial filter, one
/// horizontal and one vertical convolution at the coarsest level, then `n`
/// phase-aligned doublings back, with transparent black past every edge and a
/// round to bytes after each pass.
fn cpu_blur(source: &Image, blur: &GaussianBlur) -> Image {
    let mut image = source.clone();
    let mut stack: Vec<(usize, usize)> = Vec::new();

    for _ in 0..blur.n_decimations {
        stack.push((image.width, image.height));
        image = downscale(&image);
    }

    let kernel = &blur.kernel[..usize::from(blur.kernel_size)];
    image = convolve(&image, kernel, Axis::X);
    image = convolve(&image, kernel, Axis::Y);

    while let Some(target) = stack.pop() {
        image = upscale(&image, target);
    }

    image
}

/// Which axis a separable pass runs along.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Axis {
    X,
    Y,
}

/// One separable convolution pass along `axis`.
fn convolve(source: &Image, kernel: &[f32], axis: Axis) -> Image {
    let radius = (kernel.len() / 2) as i64;
    let mut out = Image::new(source.width, source.height);

    for y in 0..source.height {
        for x in 0..source.width {
            let mut rgba = [0.0_f32; 4];
            for (j, weight) in kernel.iter().enumerate() {
                let offset = j as i64 - radius;
                let texel = match axis {
                    Axis::X => source.sample(x as i64 + offset, y as i64),
                    Axis::Y => source.sample(x as i64, y as i64 + offset),
                };
                for channel in 0..4 {
                    rgba[channel] += texel[channel] * weight;
                }
            }
            out.data[y * out.width + x] = rgba;
        }
    }

    out.quantize()
}

/// One 2x decimation: the `[1,3,3,1]/8` binomial filter along each axis, over
/// the taps `[2k - 1, 2k, 2k + 1, 2k + 2]`.
///
/// Both axes inside one [`Image::quantize`], because the shader's four bilinear
/// samples do the same — see that method for why the reference follows the
/// shader's rounding schedule rather than the rasterizer's.
fn downscale(source: &Image) -> Image {
    let dst_width = source.width.div_ceil(2);
    let dst_height = source.height.div_ceil(2);

    let mut horizontal = Image::new(dst_width, source.height);
    for y in 0..source.height {
        for x in 0..dst_width {
            let base = 2 * x as i64;
            horizontal.data[y * dst_width + x] = binomial(
                source.sample(base - 1, y as i64),
                source.sample(base, y as i64),
                source.sample(base + 1, y as i64),
                source.sample(base + 2, y as i64),
            );
        }
    }

    let mut out = Image::new(dst_width, dst_height);
    for x in 0..dst_width {
        for y in 0..dst_height {
            let base = 2 * y as i64;
            out.data[y * dst_width + x] = binomial(
                horizontal.sample(x as i64, base - 1),
                horizontal.sample(x as i64, base),
                horizontal.sample(x as i64, base + 1),
                horizontal.sample(x as i64, base + 2),
            );
        }
    }
    out.quantize()
}

/// One 2x reconstruction, cropped back to the extent the matching decimation
/// consumed: the phase-aligned `[0.75, 0.25]` interpolation the decimation's
/// half-texel offset calls for.
///
/// Both axes inside one [`Image::quantize`], for the same reason
/// [`downscale`] is: the shader reconstructs with a single bilinear sample.
fn upscale(source: &Image, target: (usize, usize)) -> Image {
    let mut horizontal = Image::new(source.width * 2, source.height);
    for y in 0..source.height {
        for x in 0..source.width {
            let centre = source.sample(x as i64, y as i64);
            let low = interpolate(source.sample(x as i64 - 1, y as i64), centre);
            let high = interpolate(source.sample(x as i64 + 1, y as i64), centre);
            horizontal.data[y * horizontal.width + 2 * x] = low;
            horizontal.data[y * horizontal.width + 2 * x + 1] = high;
        }
    }

    let mut doubled = Image::new(horizontal.width, source.height * 2);
    for x in 0..doubled.width {
        for y in 0..source.height {
            let centre = horizontal.sample(x as i64, y as i64);
            let low = interpolate(horizontal.sample(x as i64, y as i64 - 1), centre);
            let high = interpolate(horizontal.sample(x as i64, y as i64 + 1), centre);
            doubled.data[2 * y * doubled.width + x] = low;
            doubled.data[(2 * y + 1) * doubled.width + x] = high;
        }
    }
    let doubled = doubled.quantize();

    let mut out = Image::new(target.0, target.1);
    for y in 0..target.1 {
        for x in 0..target.0 {
            out.data[y * target.0 + x] = doubled.sample(x as i64, y as i64);
        }
    }
    out
}

/// `(a + 3b + 3c + d) / 8`.
fn binomial(a: [f32; 4], b: [f32; 4], c: [f32; 4], d: [f32; 4]) -> [f32; 4] {
    let mut out = [0.0_f32; 4];
    for channel in 0..4 {
        out[channel] = (a[channel] + 3.0 * b[channel] + 3.0 * c[channel] + d[channel]) / 8.0;
    }
    out
}

/// `0.25 * neighbour + 0.75 * centre`.
fn interpolate(neighbour: [f32; 4], centre: [f32; 4]) -> [f32; 4] {
    let mut out = [0.0_f32; 4];
    for channel in 0..4 {
        out[channel] = 0.25 * neighbour[channel] + 0.75 * centre[channel];
    }
    out
}

/// The reference drop shadow: [`cpu_blur`]'s own pyramid, run over a shifted
/// copy of the source, then recoloured into the shadow's own premultiplied
/// colour — the host-side mirror of the sequence
/// `drop_shadow_passes` plans (offset, the blur, colourize), built from the
/// same `vello_common::filter::drop_shadow::DropShadow` kernel and decimation
/// plan every other reference in this file uses.
fn cpu_drop_shadow(source: &Image, shadow: &DropShadow) -> Image {
    let shifted = shift_image(source, shadow.dx, shadow.dy);
    let blur = GaussianBlur {
        std_deviation: shadow.std_deviation,
        n_decimations: shadow.n_decimations,
        kernel: shadow.kernel,
        kernel_size: shadow.kernel_size,
        edge_mode: shadow.edge_mode,
    };
    let blurred = cpu_blur(&shifted, &blur);
    colorize(&blurred, shadow.color)
}

/// Shift `source` by `(dx, dy)`, rounded to the nearest whole texel the same
/// way the WGSL offset pass does (`floor(x + 0.5)`, ties away from zero,
/// rather than `round()`'s ties-to-even) — transparent black past every edge,
/// exactly [`Image::sample`]'s own `EdgeMode::None`.
fn shift_image(source: &Image, dx: f32, dy: f32) -> Image {
    let shift_x = (dx + 0.5).floor() as i64;
    let shift_y = (dy + 0.5).floor() as i64;
    let mut out = Image::new(source.width, source.height);
    for y in 0..source.height {
        for x in 0..source.width {
            out.data[y * source.width + x] = source.sample(x as i64 - shift_x, y as i64 - shift_y);
        }
    }
    out
}

/// Recolour every texel of `source` into `color`'s own premultiplied value,
/// scaled by the texel's own alpha — the mask's own colour is discarded, as
/// `colorize_drop_shadow` discards it on the GPU. Quantized once, the same
/// per-pass 8-bit rounding budget every other pass in this pyramid pays.
fn colorize(source: &Image, color: AlphaColor<Srgb>) -> Image {
    let premultiplied = color.premultiply().to_rgba8();
    let channels = [
        f32::from(premultiplied.r),
        f32::from(premultiplied.g),
        f32::from(premultiplied.b),
        f32::from(premultiplied.a),
    ];

    let mut out = Image::new(source.width, source.height);
    for (dest, texel) in out.data.iter_mut().zip(source.data.iter()) {
        let alpha = texel[3] / 255.0;
        for channel in 0..4 {
            dest[channel] = channels[channel] * alpha;
        }
    }
    out.quantize()
}

/// The claim the whole card exists for: at σ 2, 8 and 32 the ported WGSL
/// produces the same blur on a real GPU that `vello_common`'s own kernel and
/// decimation plan produce on the CPU.
///
/// Three σ so all three shapes of the plan are covered — no decimation at all,
/// two levels, and the four the plan tops out at — which is also every pass kind
/// the shader implements bar the closing copy a blur never needs.
///
/// The tolerance is a *quantization* budget, not a correctness one. Both tiers
/// round to eight bits per channel after every pass of the pyramid and a deep
/// plan is ten passes, so the two walk apart by an LSB at a time; what would
/// show up as a real disagreement — a kernel read at the wrong offset, an axis
/// swapped, a tap merged wrong, a pass reading the page it wrote — moves texels
/// by tens or empties the region entirely, and neither is inside any budget.
#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test filters -- --ignored`"]
fn a_blur_on_the_gpu_matches_the_shared_kernel_on_the_cpu() {
    let _guard = render_lock();
    let mut harness = FilterHarness::new();
    let scope = harness
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);

    let source = Image::with_rect(
        FILTER_REGION.0 as usize,
        FILTER_REGION.1 as usize,
        FILTER_RECT,
        // Premultiplied opaque red: every channel is exercised, and an opaque
        // source is what makes an alpha that is not blurred with the colour
        // show up as a difference rather than as a coincidence.
        [255.0, 0.0, 0.0, 255.0],
    );

    for sigma in SIGMAS {
        let blur = GaussianBlur::new(sigma, EdgeMode::default());
        let steps = blur_passes(
            &blur,
            SizeU16::from_wh(FILTER_REGION.0 as u16, FILTER_REGION.1 as u16),
        );

        harness.upload_source(&source);
        let result = harness.run(&blur, &steps);
        let gpu = harness.read_region(result);
        let cpu = cpu_blur(&source, &blur);

        // The whole region, with no border excluded — see [`FILTER_RECT`] for
        // why the inset makes that sound.
        let (worst, x, y) = gpu.worst_diff(&cpu, 0);
        let mean = gpu.mean_diff(&cpu, 0);
        println!(
            "σ {sigma}: {} passes, worst per-channel diff {worst} at ({x}, {y}), mean {mean}",
            steps.len()
        );

        assert!(
            worst <= GPU_CPU_TOLERANCE,
            "σ {sigma}: the GPU blur differs from the shared kernel's own by {worst} at ({x}, {y}), \
             past the {GPU_CPU_TOLERANCE} eight-bit-rounding budget"
        );
        assert!(
            mean <= GPU_CPU_MEAN_TOLERANCE,
            "σ {sigma}: mean per-channel difference {mean} past {GPU_CPU_MEAN_TOLERANCE}"
        );

        // A blur that produced nothing at all would satisfy any tolerance
        // against a reference that also produced nothing, so three properties
        // of the output are asserted directly, none of them derived from the
        // reference.
        //
        // It paints outside the rectangle that produced it…
        let outside = gpu.sample(
            i64::from(FILTER_RECT.0) - 2,
            i64::from(FILTER_RECT.1 + FILTER_RECT.3 / 2),
        );
        assert!(
            outside[3] > 0.0,
            "σ {sigma}: nothing was painted outside the source rectangle — the pass produced no \
             texels at all: {outside:?}"
        );

        // …its middle is still the brightest thing in the region, and still
        // substantially inked. Not "nearly opaque": at σ 32 the 3σ spread is
        // wider than the rectangle is tall, so a correct blur really does thin
        // its own middle out — which is the point of running the deepest plan.
        let centre = gpu.sample(
            i64::from(FILTER_RECT.0 + FILTER_RECT.2 / 2),
            i64::from(FILTER_RECT.1 + FILTER_RECT.3 / 2),
        );
        assert!(
            centre[3] > 64.0,
            "σ {sigma}: the rectangle's own middle should stay the region's brightest texel by a \
             wide margin: {centre:?}"
        );
        assert!(
            gpu.data.iter().all(|texel| texel[3] <= centre[3]),
            "σ {sigma}: some texel outranks the rectangle's own middle, so the blur is not \
             centred where its source was"
        );

        // …and it blurs every channel alike. The source is premultiplied opaque
        // red, so red equals alpha at every texel a correct convolution
        // produces, and green and blue stay zero — a kernel applied per channel
        // with a mismatched weight, or an alpha left unfiltered, shows up here
        // whatever the reference says.
        assert!(
            gpu.data
                .iter()
                .all(|texel| texel[0] == texel[3] && texel[1] == 0.0 && texel[2] == 0.0),
            "σ {sigma}: premultiplied opaque red must stay red-equals-alpha through every pass"
        );

        // Coverage is conserved: a normalized kernel neither creates nor
        // destroys alpha, and the region is wide enough to hold the whole 3σ
        // spread, so the blurred total must match the rectangle's own to within
        // the pyramid's eight-bit rounding.
        let painted: f64 = gpu.data.iter().map(|texel| f64::from(texel[3])).sum();
        let source_total: f64 = source.data.iter().map(|texel| f64::from(texel[3])).sum();
        let drift = (painted - source_total).abs() / source_total;
        println!("σ {sigma}: coverage drift {drift}");
        assert!(
            drift <= COVERAGE_DRIFT_TOLERANCE,
            "σ {sigma}: the blur moved {drift} of the layer's total coverage, past the \
             {COVERAGE_DRIFT_TOLERANCE} eight-bit-rounding budget — a normalized kernel conserves it"
        );
    }

    let error = drain_error_scope(&harness.device, scope);
    assert!(error.is_none(), "the filter passes raised {error:?}");
}

/// The largest per-channel difference the eight-bit rounding of a ten-pass
/// pyramid is allowed to accumulate.
///
/// Measured on the pinned T400 runner (NVIDIA 610.43.03, Vulkan), not guessed:
/// worst 1 at σ 2, 2 at σ 8 and 2 at σ 32. Two LSBs of headroom over the
/// deepest plan's measured worst, which is room for another driver's own
/// bilinear-weight precision without room for a wrong kernel.
const GPU_CPU_TOLERANCE: f32 = 4.0;

/// The mean per-channel difference over the same region.
///
/// Measured on the same runner: 0.00006 at σ 2, 0.033 at σ 8, 0.098 at σ 32 —
/// three orders of magnitude under the worst case, because a rounding
/// disagreement is a rare texel rather than a bias.
const GPU_CPU_MEAN_TOLERANCE: f32 = 0.3;

/// The fraction of the layer's total coverage a blur is allowed to gain or
/// lose.
///
/// A normalized kernel conserves it exactly; eight-bit rounding does not, and
/// each level of the pyramid rounds again. Measured on the same runner:
/// 0.00001 at σ 2, 0.0035 at σ 8, 0.0137 at σ 32 — the deepest plan loses the
/// most because it rounds ten times.
const COVERAGE_DRIFT_TOLERANCE: f64 = 0.02;

/// A σ of zero is an identity kernel, so the sequence has to hand the layer
/// back unchanged rather than blank, doubled or shifted.
///
/// The negative control for the case above: it is the one input whose expected
/// output is known without a reference implementation at all, so a shader that
/// sampled at a constant offset, or a pass that read the page it was writing,
/// fails here with nothing to hide behind.
#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test filters -- --ignored`"]
fn a_zero_sigma_blur_returns_the_layer_unchanged_on_the_gpu() {
    let _guard = render_lock();
    let mut harness = FilterHarness::new();
    let scope = harness
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);

    let source = Image::with_rect(
        FILTER_REGION.0 as usize,
        FILTER_REGION.1 as usize,
        FILTER_RECT,
        [255.0, 0.0, 0.0, 255.0],
    );
    let blur = GaussianBlur::new(0.0, EdgeMode::default());
    let steps = blur_passes(
        &blur,
        SizeU16::from_wh(FILTER_REGION.0 as u16, FILTER_REGION.1 as u16),
    );
    assert_eq!(steps.len(), 2, "an undecimated blur is one pass per axis");

    harness.upload_source(&source);
    let result = harness.run(&blur, &steps);
    let gpu = harness.read_region(result);

    let (worst, x, y) = gpu.worst_diff(&source, 0);
    println!("σ 0: worst per-channel diff {worst} at ({x}, {y})");
    assert!(
        worst <= 1.0,
        "an identity kernel must return the layer it was handed, but ({x}, {y}) moved by {worst}"
    );

    let error = drain_error_scope(&harness.device, scope);
    assert!(error.is_none(), "the filter passes raised {error:?}");
}

/// Every pass of the sequence writes real texels, not just the last one.
///
/// A sequence whose middle passes silently did nothing would still end with a
/// plausible image at σ 2 (two passes, both of them the ones that matter), so
/// the decimated plan is the one to check: after the two halvings of σ 8 the
/// region really is a quarter of the texels on each axis, and the padding
/// border around it really is transparent — which is the invariant the kernels'
/// bounds-check-free sampling rests on.
#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test filters -- --ignored`"]
fn a_decimated_pass_writes_its_own_extent_and_clears_the_border_around_it() {
    let _guard = render_lock();
    let mut harness = FilterHarness::new();
    let scope = harness
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);

    let source = Image::with_rect(
        FILTER_REGION.0 as usize,
        FILTER_REGION.1 as usize,
        FILTER_RECT,
        [255.0, 0.0, 0.0, 255.0],
    );
    let blur = GaussianBlur::new(8.0, EdgeMode::default());
    let all = blur_passes(
        &blur,
        SizeU16::from_wh(FILTER_REGION.0 as u16, FILTER_REGION.1 as u16),
    );
    // Just the two decimations, so the page that is read back is the one the
    // second halving wrote rather than the one the way back up restored.
    let downscales: Vec<frust_engine::filters::FilterStep> = all
        .iter()
        .take_while(|step| step.kind == FilterPassKind::Downscale)
        .copied()
        .collect();
    assert_eq!(downscales.len(), 2, "σ 8 halves twice");

    harness.upload_source(&source);
    let result = harness.run(&blur, &downscales);
    let page = harness.read_region(result);

    let decimated = downscales[1].dest;
    assert_eq!(
        (decimated.width(), decimated.height()),
        (112, 96),
        "448 x 384 halved twice"
    );

    // Ink inside the decimated region…
    let inked = page.sample(
        i64::from(decimated.width()) / 2,
        i64::from(decimated.height()) / 2,
    );
    assert!(
        inked[3] > 200.0,
        "the decimated region's own middle should still be very nearly opaque: {inked:?}"
    );

    // …and nothing at all past the padding border the quad overdraws. A pass
    // that wrote its *source* extent instead of its destination one would leave
    // the previous level's ink out here.
    let padding = i64::from(FILTER_ATLAS_PADDING);
    for offset in 0..8 {
        let beyond = i64::from(decimated.width()) + padding + offset;
        let texel = page.sample(beyond, i64::from(decimated.height()) / 2);
        assert_eq!(
            texel, [0.0; 4],
            "texel ({beyond}, mid) is past the region and its padding, so it must be transparent"
        );
    }

    let error = drain_error_scope(&harness.device, scope);
    assert!(error.is_none(), "the filter passes raised {error:?}");
}

/// The oversized-layer case on the real device path: a blur that grows its
/// layer past what this adapter's own pool will allocate is refused as a value,
/// and nothing is allocated and no device error is raised on the way.
///
/// The host cases above make the same claim against `TierCaps::fake`; this one
/// makes it against the adapter's real `max_texture_dimension_2d` and a real
/// `wgpu::Device`, which is where "refused" and "the driver refused it for us"
/// would finally read differently.
#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test filters -- --ignored`"]
fn an_oversized_filter_layer_is_refused_by_the_real_pool_rather_than_the_driver() {
    let _guard = render_lock();
    let harness = FilterHarness::new();
    let scope = harness
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);

    let config = PageConfig::default();
    // The DEVICE ceiling, not the pool's own configured budget: a filter
    // layer's page is sized by `filter_page_size` since this test's own
    // boundary was pinned, which checks the padded request against both
    // ceilings separately (see that function's doc) — this test's claim is
    // specifically about the adapter's own `max_texture_dimension_2d`, so the
    // σ below has to cross THAT one, not merely the pool's configured
    // `max_page_size` (a σ that only crosses the latter is `pages.rs`'s own
    // host-level `a_filter_layer_inside_the_device_ceiling_but_past_the_pool_
    // budget_is_limit_reached`, not this device-level claim).
    let device_ceiling = frust_engine::gpu::targets::max_texture_size(&harness.caps);

    // A σ whose 3σ spread on every side is past the adapter's own device
    // ceiling, over contents small enough that only the blur can have done
    // it.
    let sigma = (device_ceiling as f32) / 2.0;
    let mut recorder = recorder();
    push_filter_layer(
        &mut recorder,
        layer(0.5),
        LayerFilter::Blur { sigma },
        Affine::IDENTITY,
    )
    .expect("a σ under the ceiling records");
    draw(&mut recorder, 64, 64, 32);
    recorder.pop_layer();

    assert!(
        u32::from(recorder.layers[0].bbox.width()) > device_ceiling,
        "the blur has to be what pushes the layer past this adapter's device ceiling: {:?}",
        recorder.layers[0].bbox
    );
    assert!(
        matches!(
            Schedule::build(&recorder, &harness.caps, &config),
            Err(EngineError::IntermediateTextureTooLarge)
        ),
        "a filter layer past the adapter's own device ceiling is refused rather than clipped \
         onto it"
    );

    // And the pool the frame path would have asked answers the same way,
    // without touching the device.
    let mut targets = IntermediateTargets::new(&harness.caps);
    let max = targets.max_texture_size();
    let before = targets.stats().created;
    let refused = targets.acquire(&harness.device, max + 1, 64, "oversized filter page");
    assert!(refused.is_too_large());
    assert_eq!(
        targets.stats().created,
        before,
        "a refused request must allocate nothing on a real device either"
    );

    let error = drain_error_scope(&harness.device, scope);
    assert!(
        error.is_none(),
        "refusing an oversized filter layer raised {error:?}"
    );
}

// -----------------------------------------------------------------------------
// On real hardware: the drop shadow
// -----------------------------------------------------------------------------
//
// The same claim the blur cases above make, run over `drop_shadow_passes`'
// sequence instead: the offset and colourize this file's own
// `filters_drop_shadow.wgsl` prelude adds, plus the blur passes a shadow reuses
// unmodified, driven through the identical `FilterHarness` and compared
// against a CPU reference built from the same `vello_common::filter::gaussian_blur`
// kernel `cpu_blur` already uses, wrapped in [`shift_image`]/[`colorize`].

/// The offset every drop-shadow GPU case shifts its shadow by.
///
/// Small enough that even σ 32's own 3σ ≈ 96px spread, plus this shift, stays
/// inside [`FILTER_RECT`]'s ~160px margin from [`FILTER_REGION`]'s edge on
/// every side (see those constants' own docs) — the same margin the plain
/// blur cases rely on to keep every tap reading transparent padding rather
/// than a stale texel.
const DROP_SHADOW_OFFSET: (f32, f32) = (16.0, -12.0);

/// The shadow's own colour: opaque blue, deliberately not [`FILTER_RECT`]'s
/// opaque red — a colorize pass that let the source's own colour leak
/// through, or one that ran before the blur rather than after, shows up as a
/// colour disagreement rather than only a shape one.
const DROP_SHADOW_COLOR: AlphaColor<Srgb> = AlphaColor::new([0.0, 0.0, 1.0, 1.0]);

/// The claim this half of the card exists for: at σ 2, 8 and 32 the ported
/// offset and colourize passes, run around the same blur pyramid the plain
/// blur cases already proved, produce the same shadow on a real GPU that
/// `vello_common`'s own kernel, decimation plan and drop-shadow colour packing
/// produce on the CPU.
///
/// The tolerance is the blur's own quantization budget plus the one extra
/// 8-bit rounding the colourize pass pays (see [`DROP_SHADOW_GPU_CPU_TOLERANCE`]).
#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test filters -- --ignored`"]
fn a_drop_shadow_on_the_gpu_matches_the_shared_kernel_on_the_cpu() {
    let _guard = render_lock();
    let mut harness = FilterHarness::new();
    let scope = harness
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);

    let source = Image::with_rect(
        FILTER_REGION.0 as usize,
        FILTER_REGION.1 as usize,
        FILTER_RECT,
        [255.0, 0.0, 0.0, 255.0],
    );

    for sigma in SIGMAS {
        let shadow_value = shadow(DROP_SHADOW_OFFSET, sigma, DROP_SHADOW_COLOR);
        let steps = drop_shadow_passes(
            &shadow_value,
            SizeU16::from_wh(FILTER_REGION.0 as u16, FILTER_REGION.1 as u16),
        );

        harness.upload_source(&source);
        let block = GpuFilterData::from(GpuDropShadow::from(&shadow_value));
        let result = harness.run_block(block, &steps);
        let gpu = harness.read_region(result);
        let cpu = cpu_drop_shadow(&source, &shadow_value);

        let (worst, x, y) = gpu.worst_diff(&cpu, 0);
        let mean = gpu.mean_diff(&cpu, 0);
        println!(
            "drop shadow σ {sigma}: {} passes, worst per-channel diff {worst} at ({x}, {y}), mean {mean}",
            steps.len()
        );

        assert!(
            worst <= DROP_SHADOW_GPU_CPU_TOLERANCE,
            "σ {sigma}: the GPU drop shadow differs from the shared kernel's own by {worst} at \
             ({x}, {y}), past the {DROP_SHADOW_GPU_CPU_TOLERANCE} eight-bit-rounding budget"
        );
        assert!(
            mean <= DROP_SHADOW_GPU_CPU_MEAN_TOLERANCE,
            "σ {sigma}: mean per-channel difference {mean} past {DROP_SHADOW_GPU_CPU_MEAN_TOLERANCE}"
        );

        // The shadow is inked at its own shifted centre...
        let shifted_x = i64::from(FILTER_RECT.0 as u16 + FILTER_RECT.2 as u16 / 2)
            + DROP_SHADOW_OFFSET.0.round() as i64;
        let shifted_y = i64::from(FILTER_RECT.1 as u16 + FILTER_RECT.3 as u16 / 2)
            + DROP_SHADOW_OFFSET.1.round() as i64;
        let centre = gpu.sample(shifted_x, shifted_y);
        assert!(
            centre[3] > 64.0,
            "σ {sigma}: the shadow's own shifted middle should be substantially inked: {centre:?}"
        );

        // ...and every texel of it is blue, never red: the source's own
        // colour must never reach a colorized shadow, wherever the blur or
        // the offset placed it.
        assert!(
            gpu.data.iter().all(|texel| texel[0] <= 1.0),
            "σ {sigma}: a shadow texel carries red, so the source's own colour leaked through \
             the colourize pass"
        );
    }

    let error = drain_error_scope(&harness.device, scope);
    assert!(error.is_none(), "the drop-shadow passes raised {error:?}");
}

/// The largest per-channel difference the eight-bit rounding of a drop
/// shadow's pyramid is allowed to accumulate.
///
/// Measured on the pinned T400 runner (NVIDIA 610.43.03, Vulkan), not
/// guessed: worst 1 at σ 2, 2 at σ 8 and 2 at σ 32 — identical to
/// [`GPU_CPU_TOLERANCE`]'s own blur-only measurement, so the offset and
/// colourize passes this file adds cost the pyramid no extra rounding this
/// budget has to account for. Kept as its own named constant rather than
/// reusing [`GPU_CPU_TOLERANCE`] directly: the two are equal by measurement,
/// not by any shared reasoning that would make a future drift in one an
/// error in the other.
const DROP_SHADOW_GPU_CPU_TOLERANCE: f32 = 4.0;

/// The mean per-channel difference over the same region.
///
/// Measured on the same runner: 0.00006 at σ 2, 0.033 at σ 8, 0.082 at σ 32 —
/// on the same order as [`GPU_CPU_MEAN_TOLERANCE`]'s own blur-only
/// measurement.
const DROP_SHADOW_GPU_CPU_MEAN_TOLERANCE: f32 = 0.3;

/// σ 0 at offset 0 is an identity blur and a no-op shift, so the sequence has
/// to hand back the source's own shape, recoloured — neither blank, shifted
/// nor still the source's own colour.
///
/// The negative control for the case above, on the same terms the blur's own
/// zero-σ case is: the one input whose expected output is knowable without a
/// pyramid at all, so a shader that shifted at a constant offset regardless of
/// the parameter block, or a colourize pass that ran before the blur instead
/// of after, fails here with nothing to hide behind.
#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test filters -- --ignored`"]
fn a_zero_sigma_zero_offset_drop_shadow_recolors_the_sources_own_shape_on_the_gpu() {
    let _guard = render_lock();
    let mut harness = FilterHarness::new();
    let scope = harness
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);

    let source = Image::with_rect(
        FILTER_REGION.0 as usize,
        FILTER_REGION.1 as usize,
        FILTER_RECT,
        [255.0, 0.0, 0.0, 255.0],
    );
    let shadow_value = shadow((0.0, 0.0), 0.0, DROP_SHADOW_COLOR);
    let steps = drop_shadow_passes(
        &shadow_value,
        SizeU16::from_wh(FILTER_REGION.0 as u16, FILTER_REGION.1 as u16),
    );
    assert_eq!(
        steps.len(),
        4,
        "an unshifted, undecimated shadow is one offset, one pass per blur axis, and one colourize"
    );

    harness.upload_source(&source);
    let block = GpuFilterData::from(GpuDropShadow::from(&shadow_value));
    let result = harness.run_block(block, &steps);
    let gpu = harness.read_region(result);
    let expected = colorize(&source, DROP_SHADOW_COLOR);

    let (worst, x, y) = gpu.worst_diff(&expected, 0);
    println!("drop shadow σ 0, offset 0: worst per-channel diff {worst} at ({x}, {y})");
    assert!(
        worst <= 1.0,
        "an identity blur at zero offset must recolour the source's own shape unchanged, but \
         ({x}, {y}) moved by {worst}"
    );

    let error = drain_error_scope(&harness.device, scope);
    assert!(error.is_none(), "the drop-shadow passes raised {error:?}");
}
