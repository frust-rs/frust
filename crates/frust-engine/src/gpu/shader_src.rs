//! The engine's WGSL sources, assembled at compile time.
//!
//! Every shader lives in `crates/frust-engine/shaders/` as its own `.wgsl`
//! file and reaches Rust through `include_str!` — never as an inline string
//! literal — so the downlevel lint that scans that directory
//! (`frust_gpu::lint::lint_wgsl_dir`, driven by `frust-gpu`'s
//! `downlevel_rules` test) sees every line the GPU ever compiles.
//!
//! ## The helper prelude
//!
//! The sources are ported from a WESL reference whose entry-point modules
//! pull shared code in with `import package::helpers::...`. WESL's own
//! resolver is not available here — its crate family needs a newer toolchain
//! than this workspace's MSRV — so the import graph is resolved two ways:
//! within [`HELPERS`] by hand-inlining, and between an entry-point module and
//! its helpers by `concat!`, exactly the shape `frust-render`'s shader-effect
//! `VERTEX_PRELUDE` uses.
//!
//! Prepending the whole helper file to every entry-point module is safe
//! because [`HELPERS`] declares no `@group`/`@binding` global: a helper that
//! reads a texture takes it as a parameter. An entry-point module's derived
//! bind-group layout is therefore exactly what its own bindings say, with or
//! without the prelude. [`FILTER_KERNELS`] is a second prelude on the same
//! terms, prepended to [`FILTER`] alone.
//!
//! Every module the engine compiles is assembled here and nowhere else:
//! [`MODULES`] lists the four fixed ones, and [`strip_class_source`] derives
//! the strip program's paint-class specialisations from [`STRIP`] — together
//! the whole list [`crate::gpu::pipelines::EngineShaders::register`] walks,
//! and the one the validation tests below walk.
//!
//! ## The paint-class pre-pass
//!
//! The strip program is compiled once per [`PaintClass`] from the ONE source,
//! by a line-oriented string pass ([`specialise`]) rather than a second copy of
//! the shader: a block between a `//#if FEATURE` line and its `//#endif` line
//! survives only in a class that enables `FEATURE` ([`class_features`]), and a
//! `//#else` line inside it opens the arm that survives exactly when the `//#if`
//! arm does not. The directives are ordinary WGSL comments, which keeps the
//! downlevel lint (which scans `shaders/`) reading the real source.
//!
//! ## The two program families
//!
//! [`IMAGE_SAMPLER`] is not a class feature but a family switch, settled per
//! renderer (`FRUST_ENGINE_IMAGE_SAMPLER`, [`crate::config::image_sampler_enabled`]):
//! with it, every program that keeps `IMAGE` — [`PaintClass::Image`],
//! [`PaintClass::ImageFiltered`] and [`PaintClass::Uber`] — reads a bilinear
//! atlas image through one hardware-filtered `textureSampleLevel`; without it
//! they keep the manual four-`textureLoad` path in the feature's `//#else` arm.
//! The sampler family is the default; the switch's `0` is the kill switch
//! that selects the manual one. A renderer compiles one family, so the switch
//! adds no pipeline variant ([`strip_program_source`]). The manual family's
//! [`PaintClass::Uber`]
//! program is [`STRIP`] with only the `IMAGE_SAMPLER` arm blanked out — line
//! for line the program the engine shipped before either axis existed.
//!
//! A removed line is replaced by an empty one rather than dropped, so a naga
//! diagnostic in a specialised module names the same line number as the source
//! file. The pass emits only lines the source already contains, and the blocks
//! are placed so that dropping any combination of them leaves well-formed WGSL
//! (each block is a whole `else if` arm), so a specialisation uses no construct
//! the Uber program does not — the property the WebGL2 arm's naga GLSL-ES
//! lowering depends on. Overrides (`override` constants) were the alternative
//! and are deliberately not used: no pipeline here sets one, and the pre-pass
//! needs no new pipeline-description axis to carry them.

use std::borrow::Cow;

use super::strips::PaintClass;

/// Binding-free helper functions shared by every entry-point module:
/// packing, quad geometry, extend modes, encoded-paint accessors, gradient
/// and blur evaluation, and atlas/external image sampling.
///
/// Not a shader in its own right — it has no entry point. It is prepended to
/// each of [`STRIP`], [`CLEAR`] and [`COPY`].
pub const HELPERS: &str = include_str!("../../shaders/helpers.wgsl");

/// The sparse-strip rasterizer: `vs_main` + `fs_main`, six render-state
/// variants (see [`crate::gpu::pipelines`]) — and, unchanged, the
/// [`PaintClass::Uber`] program every class specialisation is cut from.
pub const STRIP: &str = concat!(
    include_str!("../../shaders/helpers.wgsl"),
    include_str!("../../shaders/strip.wgsl")
);

/// Region and fullscreen clears of an intermediate texture: `vs_main` /
/// `vs_main_fullscreen` + `fs_main`.
pub const CLEAR: &str = concat!(
    include_str!("../../shaders/helpers.wgsl"),
    include_str!("../../shaders/clear.wgsl")
);

/// Rectangular region copies between intermediate textures: `vs_main` +
/// `fs_main`.
pub const COPY: &str = concat!(
    include_str!("../../shaders/helpers.wgsl"),
    include_str!("../../shaders/copy.wgsl")
);

/// The Gaussian-blur kernels [`FILTER`]'s fragment stage dispatches into: the
/// two rescaling steps a decimated blur is built from, and the separable
/// convolution run between them.
///
/// A second prelude rather than a module of its own: like [`HELPERS`] it
/// declares no entry point and no `@group`/`@binding` global — a kernel that
/// reads a texture takes the texture and the sampler as parameters — so
/// prepending it cannot disturb [`FILTER`]'s derived bind-group layout.
pub const FILTER_KERNELS: &str = include_str!("../../shaders/filters_blur.wgsl");

/// The drop-shadow passes [`FILTER`]'s fragment stage dispatches into beyond
/// what [`FILTER_KERNELS`] already covers: the shadow's own device-space
/// shift, and the recolour from a blurred alpha mask into the shadow's
/// premultiplied colour.
///
/// A third prelude on the same terms as [`FILTER_KERNELS`]: no entry point, no
/// `@group`/`@binding` global, so prepending it cannot disturb [`FILTER`]'s
/// derived bind-group layout either. The layouts and constants it reads are
/// [`crate::filters::drop_shadow`]'s; that module's own tests pin the two
/// sides against each other.
pub const DROP_SHADOW_KERNELS: &str = include_str!("../../shaders/filters_drop_shadow.wgsl");

/// One filter pass over one destination page: `vs_main` + `fs_main`, one
/// pipeline (see [`crate::gpu::pipelines::EnginePipeline::Filter`]).
///
/// The only module assembled from more than one prelude: the binding-free
/// helpers every module gets, then [`FILTER_KERNELS`], then
/// [`DROP_SHADOW_KERNELS`], then the entry-point module that declares the
/// bindings and dispatches on the pass kind. The layouts and constants it
/// reads are [`crate::filters::blur`]'s and [`crate::filters::drop_shadow`]'s;
/// those modules' own tests pin all three sides against each other.
pub const FILTER: &str = concat!(
    include_str!("../../shaders/helpers.wgsl"),
    include_str!("../../shaders/filters_blur.wgsl"),
    include_str!("../../shaders/filters_drop_shadow.wgsl"),
    include_str!("../../shaders/filter.wgsl")
);

/// The name [`STRIP`] is registered under in the shader library.
pub const STRIP_NAME: &str = "frust-engine strip";

/// The class feature names a `//#if` directive in `strip.wgsl` may name —
/// [`IMAGE_SAMPLER`], the family switch, is the only other one.
///
/// - `IMAGE` — atlas image sampling at nearest and bilinear quality;
/// - `IMAGE_FILTERED` — nested inside `IMAGE`: bicubic atlas sampling and the
///   external texture;
/// - `GRADIENT` — the linear, radial and sweep gradients;
/// - `BLURRED_RECT` — the blurred rounded rectangle;
/// - `LAYER` — the layer colour source a composite samples its page through.
pub const STRIP_FEATURES: [&str; 5] = [
    "IMAGE",
    "IMAGE_FILTERED",
    "GRADIENT",
    "BLURRED_RECT",
    "LAYER",
];

/// The family feature: bilinear atlas images through a hardware sampler (see
/// the module doc's *The two program families*). Kept by the image programs
/// of the sampler family only, never by a class on its own.
pub const IMAGE_SAMPLER: &str = "IMAGE_SAMPLER";

/// The `//#if` class features `class` keeps.
///
/// Coverage, the solid paint and the vertex stage sit outside every block, so
/// they are in every class; [`PaintClass::Uber`] keeps every feature.
#[must_use]
pub const fn class_features(class: PaintClass) -> &'static [&'static str] {
    match class {
        PaintClass::SolidCoverage => &[],
        PaintClass::Gradient => &["GRADIENT"],
        PaintClass::Image => &["IMAGE"],
        PaintClass::ImageFiltered => &["IMAGE", "IMAGE_FILTERED"],
        PaintClass::BlurredRect => &["BLURRED_RECT"],
        PaintClass::LayerComposite => &["LAYER"],
        PaintClass::Uber => &STRIP_FEATURES,
    }
}

/// The name the strip program specialised for `class` is registered under in
/// the shader library — [`STRIP_NAME`] itself for [`PaintClass::Uber`].
#[must_use]
pub const fn strip_class_name(class: PaintClass) -> &'static str {
    match class {
        PaintClass::SolidCoverage => "frust-engine strip/solid",
        PaintClass::Gradient => "frust-engine strip/gradient",
        PaintClass::Image => "frust-engine strip/image",
        PaintClass::ImageFiltered => "frust-engine strip/image-filtered",
        PaintClass::BlurredRect => "frust-engine strip/blurred-rect",
        PaintClass::LayerComposite => "frust-engine strip/layer",
        PaintClass::Uber => STRIP_NAME,
    }
}

/// The strip program of the manual family specialised for `class`:
/// [`STRIP`] with every `//#if` block whose feature `class` does not keep, and
/// the [`IMAGE_SAMPLER`] arm, blanked out.
///
/// [`PaintClass::Uber`] keeps every class feature, so the kill switch
/// (`FRUST_ENGINE_UBER_PROGRAM`) compiles [`STRIP`] with only the sampler arm
/// and the directive lines blanked — line for line the program the engine
/// shipped before the classes existed.
#[must_use]
pub fn strip_class_source(class: PaintClass) -> Cow<'static, str> {
    strip_program_source(class, false)
}

/// Whether `class`'s program reads atlas images at all, and so whether the
/// sampler family specialises it with [`IMAGE_SAMPLER`].
#[must_use]
pub fn samples_images(class: PaintClass) -> bool {
    class_features(class).contains(&"IMAGE")
}

/// The features `class`'s program keeps in the family `image_sampler` names:
/// [`class_features`], plus [`IMAGE_SAMPLER`] for an image program of the
/// sampler family.
#[must_use]
pub fn strip_program_features(class: PaintClass, image_sampler: bool) -> Vec<&'static str> {
    let mut features = class_features(class).to_vec();
    if image_sampler && samples_images(class) {
        features.push(IMAGE_SAMPLER);
    }
    features
}

/// The strip program for `class` in the family `image_sampler` names: [`STRIP`]
/// with every block [`strip_program_features`] does not keep blanked out.
///
/// The two families differ only in the programs that keep `IMAGE`; every other
/// class compiles the same source in both.
#[must_use]
pub fn strip_program_source(class: PaintClass, image_sampler: bool) -> Cow<'static, str> {
    Cow::Owned(specialise(
        STRIP,
        &strip_program_features(class, image_sampler),
    ))
}

/// The shader-library name of `class`'s program in the family `image_sampler`
/// names — [`strip_class_name`] for every program the families share, with a
/// suffix on the sampler family's image programs so a library never holds two
/// different sources under one name.
#[must_use]
pub const fn strip_program_name(class: PaintClass, image_sampler: bool) -> &'static str {
    if !image_sampler {
        return strip_class_name(class);
    }
    match class {
        PaintClass::Image => "frust-engine strip/image+sampler",
        PaintClass::ImageFiltered => "frust-engine strip/image-filtered+sampler",
        PaintClass::Uber => "frust-engine strip+sampler",
        _ => strip_class_name(class),
    }
}

/// The directive opening a feature block, followed by one feature name.
const DIRECTIVE_IF: &str = "//#if ";

/// The directive closing the innermost open feature block.
const DIRECTIVE_ENDIF: &str = "//#endif";

/// The directive switching the innermost open feature block to its other arm.
const DIRECTIVE_ELSE: &str = "//#else";

/// Runs the feature pre-pass over `source`, keeping the blocks whose feature
/// is in `enabled`.
///
/// Line-oriented and total: a directive is a line whose trimmed text starts
/// with `//#if ` (one feature name after it) or is exactly `//#else` or
/// `//#endif`; blocks nest, a `//#else` inverts its block for the lines after
/// it, and a line is kept only while every enclosing block is live. Each
/// directive line, and each line of a dead arm, becomes an empty line, so the
/// output has exactly the source's line count. An `//#else` or `//#endif` with
/// no open block is ignored and an unclosed block runs to the end of the
/// source; neither occurs in the shipped shaders, which the tests below pin.
#[must_use]
pub fn specialise(source: &str, enabled: &[&str]) -> String {
    let mut out = String::with_capacity(source.len());
    // Whether each open block is live, innermost last; a line is kept only
    // when every entry is.
    let mut open: Vec<bool> = Vec::new();
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim();
        let newline = if line.ends_with('\n') { "\n" } else { "" };
        if let Some(feature) = trimmed.strip_prefix(DIRECTIVE_IF) {
            let feature = feature.trim();
            open.push(enabled.contains(&feature));
            out.push_str(newline);
        } else if trimmed == DIRECTIVE_ELSE {
            if let Some(live) = open.last_mut() {
                *live = !*live;
            }
            out.push_str(newline);
        } else if trimmed == DIRECTIVE_ENDIF {
            open.pop();
            out.push_str(newline);
        } else if open.iter().all(|live| *live) {
            out.push_str(line);
        } else {
            out.push_str(newline);
        }
    }
    out
}

/// The name [`CLEAR`] is registered under in the shader library.
pub const CLEAR_NAME: &str = "frust-engine clear";

/// The name [`COPY`] is registered under in the shader library.
pub const COPY_NAME: &str = "frust-engine copy";

/// The name [`FILTER`] is registered under in the shader library.
pub const FILTER_NAME: &str = "frust-engine filter";

/// Every module source the engine assembles at compile time, as
/// `(name, source)` pairs — the list the validation tests walk.
///
/// [`STRIP`] is listed as written, both [`IMAGE_SAMPLER`] arms present, which
/// is valid WGSL in its own right; what a renderer compiles from it are the
/// specialisations [`strip_program_source`] derives at start-up, the Uber
/// program among them.
pub const MODULES: [(&str, &str); 4] = [
    (STRIP_NAME, STRIP),
    (CLEAR_NAME, CLEAR),
    (COPY_NAME, COPY),
    (FILTER_NAME, FILTER),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::{GpuConfig, GpuStrip};
    use wgpu::naga;

    /// Parses and validates `src` the way a `wgpu::Device` would, returning
    /// the validated module.
    ///
    /// `wgpu` re-exports the same `naga` its own WGSL front end uses, so this
    /// is the device's validation path minus the device: no GPU, no adapter,
    /// and a failure surfaces here as a test failure instead of as a
    /// pipeline-creation error at run time. The device-backed counterpart
    /// (`gpu::pipelines`' real-pipeline test) additionally proves the
    /// *pipelines* are accepted, but it needs hardware and is `#[ignore]`d.
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
                (Some(ty_name), naga::TypeInner::Struct { span, .. }) if ty_name == name => {
                    Some(*span)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("the module declares no struct named {name}"))
    }

    #[test]
    fn every_module_parses_and_validates() {
        for (name, src) in MODULES {
            validate(name, src);
        }
        for image_sampler in [false, true] {
            for class in PaintClass::ALL {
                validate(
                    strip_program_name(class, image_sampler),
                    &strip_program_source(class, image_sampler),
                );
            }
        }
    }

    /// Whether `line` is a pre-pass directive.
    fn is_directive(line: &str) -> bool {
        let trimmed = line.trim();
        trimmed.starts_with(DIRECTIVE_IF) || trimmed == DIRECTIVE_ELSE || trimmed == DIRECTIVE_ENDIF
    }

    /// The kill switch's whole promise: the manual family's Uber class
    /// compiles the shipped source, not a reconstruction of it — every line of
    /// [`STRIP`] kept in place, save the directives and the sampler family's
    /// own arm, which are blanked rather than dropped.
    #[test]
    fn the_manual_uber_program_is_the_strip_source_without_the_sampler_arm() {
        let uber = strip_class_source(PaintClass::Uber);
        let sampled = strip_program_source(PaintClass::Uber, true);
        assert_eq!(strip_class_name(PaintClass::Uber), STRIP_NAME);
        assert_eq!(uber.lines().count(), STRIP.lines().count());
        let mut blanked = 0;
        for ((source, kept), sampled) in STRIP.lines().zip(uber.lines()).zip(sampled.lines()) {
            if kept == source {
                continue;
            }
            assert!(kept.is_empty(), "a kept line was rewritten: `{source}`");
            if is_directive(source) || source.trim().is_empty() {
                continue;
            }
            assert_eq!(
                sampled, source,
                "the manual Uber program blanked `{source}`, which is not the sampler arm"
            );
            blanked += 1;
        }
        assert!(blanked > 0, "the sampler arm is in the source");
        assert!(!uber.contains("atlas_sampler"));
        assert!(!uber.contains("textureSampleLevel("));
        assert!(sampled.contains("var atlas_sampler: sampler;"));
    }

    /// The families differ exactly where the switch says: in the programs
    /// that read atlas images, and nowhere else.
    #[test]
    fn the_families_differ_only_in_the_image_programs() {
        for class in PaintClass::ALL {
            let default = strip_program_source(class, false);
            let sampled = strip_program_source(class, true);
            assert_eq!(
                default == sampled,
                !samples_images(class),
                "{}",
                class.label()
            );
            assert_eq!(
                strip_program_name(class, false) == strip_program_name(class, true),
                !samples_images(class),
                "{}",
                class.label()
            );
            assert_eq!(strip_program_name(class, false), strip_class_name(class));
            assert_eq!(default, strip_class_source(class));
        }
        assert!(samples_images(PaintClass::Image));
        assert!(samples_images(PaintClass::ImageFiltered));
        assert!(samples_images(PaintClass::Uber));
        // The sampler arm replaces the manual one: neither family keeps both.
        for class in [
            PaintClass::Image,
            PaintClass::ImageFiltered,
            PaintClass::Uber,
        ] {
            let default = strip_program_source(class, false);
            let sampled = strip_program_source(class, true);
            let fs =
                |src: &str| src[src.find("fn fs_main(").expect("a fragment stage")..].to_string();
            assert!(fs(&default).contains("sample_color = bilinear_sample("));
            assert!(!fs(&default).contains("sampled_bilinear("));
            assert!(fs(&sampled).contains("sample_color = sampled_bilinear("));
            assert!(!fs(&sampled).contains("bilinear_sample("));
        }
    }

    #[test]
    fn every_directive_names_a_known_feature_and_every_block_closes() {
        let mut depth = 0_usize;
        let mut seen = Vec::new();
        for (number, line) in STRIP.lines().enumerate() {
            let trimmed = line.trim();
            if let Some(feature) = trimmed.strip_prefix(DIRECTIVE_IF) {
                assert!(
                    STRIP_FEATURES.contains(&feature.trim()) || feature.trim() == IMAGE_SAMPLER,
                    "line {number} names an unknown feature `{feature}`"
                );
                seen.push(feature.trim().to_string());
                depth += 1;
            } else if trimmed == DIRECTIVE_ELSE {
                assert!(
                    depth > 0,
                    "line {number} switches a block that was never opened"
                );
            } else if trimmed == DIRECTIVE_ENDIF {
                assert!(
                    depth > 0,
                    "line {number} closes a block that was never opened"
                );
                depth -= 1;
            } else {
                assert!(
                    !trimmed.starts_with("//#"),
                    "line {number} looks like a directive but is not one: `{trimmed}`"
                );
            }
        }
        assert_eq!(
            depth, 0,
            "a feature block is left open at the end of the source"
        );
        for feature in STRIP_FEATURES.into_iter().chain([IMAGE_SAMPLER]) {
            assert!(
                seen.iter().any(|f| f == feature),
                "feature {feature} gates nothing — a class enabling it would equal one that does not"
            );
        }
    }

    #[test]
    fn the_pre_pass_keeps_line_numbers_and_only_drops_disabled_blocks() {
        let src = "a\n//#if X\nb\n//#if Y\nc\n//#endif\n//#endif\nd";
        assert_eq!(specialise(src, &["X", "Y"]), "a\n\nb\n\nc\n\n\nd");
        assert_eq!(specialise(src, &["X"]), "a\n\nb\n\n\n\n\nd");
        assert_eq!(specialise(src, &["Y"]), "a\n\n\n\n\n\n\nd");
        assert_eq!(specialise(src, &[]), "a\n\n\n\n\n\n\nd");
        // An `//#else` arm is live exactly when its `//#if` arm is not, and
        // only while every enclosing block is.
        let src = "a\n//#if X\nb\n//#if Y\nc\n//#else\ne\n//#endif\n//#endif\nd";
        assert_eq!(specialise(src, &["X", "Y"]), "a\n\nb\n\nc\n\n\n\n\nd");
        assert_eq!(specialise(src, &["X"]), "a\n\nb\n\n\n\ne\n\n\nd");
        assert_eq!(specialise(src, &["Y"]), "a\n\n\n\n\n\n\n\n\nd");
        for image_sampler in [false, true] {
            for class in PaintClass::ALL {
                assert_eq!(
                    strip_program_source(class, image_sampler).lines().count(),
                    STRIP.lines().count(),
                    "{} must keep the source's line numbering",
                    class.label()
                );
            }
        }
    }

    /// Every class keeps exactly its own colour-source branches: the fragment
    /// entry point of each specialisation names only the paint types the
    /// class owns, and the solid paint and both coverage paths in all of them.
    #[test]
    fn each_class_keeps_only_its_own_branches() {
        let branch = |src: &str, needle: &str| src.contains(needle);
        let image = "paint_type == PAINT_TYPE_IMAGE {";
        let bicubic = "sample_color = bicubic_sample(";
        let external = "sample_color = sample_external_image(";
        let bilinear = "sample_color = bilinear_sample(";
        let linear = "paint_type == PAINT_TYPE_LINEAR_GRADIENT {";
        let radial = "paint_type == PAINT_TYPE_RADIAL_GRADIENT {";
        let sweep = "paint_type == PAINT_TYPE_SWEEP_GRADIENT {";
        let blurred = "paint_type == PAINT_TYPE_BLURRED_ROUNDED_RECT {";
        let layer = "} else if color_source == COLOR_SOURCE_LAYER {\n        let layer_opacity";
        for class in PaintClass::ALL {
            let src = strip_class_source(class);
            let fs = &src[src.find("fn fs_main(").expect("a fragment entry point")..];
            let keeps = |feature: &str| class_features(class).contains(&feature);
            assert!(
                branch(fs, "paint_type == PAINT_TYPE_SOLID {"),
                "{}",
                class.label()
            );
            assert!(
                branch(fs, "if is_rect && rect_frac != 0u {"),
                "{}",
                class.label()
            );
            assert_eq!(branch(fs, image), keeps("IMAGE"), "{}", class.label());
            assert_eq!(branch(fs, bilinear), keeps("IMAGE"), "{}", class.label());
            for filtered in [bicubic, external] {
                assert_eq!(
                    branch(fs, filtered),
                    keeps("IMAGE_FILTERED"),
                    "{}",
                    class.label()
                );
            }
            for gradient in [linear, radial, sweep] {
                assert_eq!(branch(fs, gradient), keeps("GRADIENT"), "{}", class.label());
            }
            assert_eq!(
                branch(fs, blurred),
                keeps("BLURRED_RECT"),
                "{}",
                class.label()
            );
            assert_eq!(branch(&src, layer), keeps("LAYER"), "{}", class.label());
        }
    }

    /// candB: the strip program addresses its alpha and LUT textures by the
    /// `Config` shifts alone — no per-fragment `textureDimensions` query and no
    /// integer `%`/`/` against a queried width (see `gpu::config`'s guard).
    /// The one query in the source is the sampler family's normalisation of an
    /// atlas sample position, which no manual-family program keeps.
    #[test]
    fn the_strip_program_addresses_its_resource_textures_by_config_shifts() {
        assert_eq!(STRIP.matches("textureDimensions(").count(), 1);
        assert!(STRIP.contains("vec2<f32>(textureDimensions(tex))"));
        for class in PaintClass::ALL {
            assert!(!strip_class_source(class).contains("textureDimensions("));
            let sampled = strip_program_source(class, true);
            assert_eq!(
                sampled.contains("textureDimensions("),
                samples_images(class),
                "{}",
                class.label()
            );
        }
        assert!(STRIP.contains("(1u << config.alphas_tex_width_bits) - 1u"));
        assert_eq!(
            STRIP
                .matches(
                    "get_gradient_texture_width(gradient_texel0),\n                \
                     config.encoded_paints_tex_width_bits\n"
                )
                .count(),
            3,
            "each gradient kind passes the LUT width shift"
        );
        assert!(HELPERS.contains("flat_coord & ((1u << lut_width_bits) - 1u)"));
        assert!(HELPERS.contains("flat_coord >> lut_width_bits"));
    }

    #[test]
    fn the_helper_prelude_validates_on_its_own() {
        // It has no entry point, so nothing else would catch a syntax error
        // in a helper that no current entry point happens to call.
        validate("helpers", HELPERS);
    }

    #[test]
    fn every_module_is_the_helper_prelude_followed_by_its_own_source() {
        for (name, src) in MODULES {
            assert!(
                src.starts_with(HELPERS),
                "{name} must be assembled as the helper prelude followed by its own source"
            );
            assert!(
                src.len() > HELPERS.len(),
                "{name} must contribute source of its own"
            );
        }
    }

    #[test]
    fn the_helper_prelude_declares_no_bindings() {
        // The whole reason every entry-point module can be prefixed with it
        // without disturbing its derived bind-group layout. Comment lines are
        // skipped: this file's own header describes the rule. The blur
        // kernels are held to the same rule for the same reason — they are a
        // prelude too, just one only the filter module gets.
        for (name, prelude) in [
            ("helpers", HELPERS),
            ("filter kernels", FILTER_KERNELS),
            ("drop shadow kernels", DROP_SHADOW_KERNELS),
        ] {
            let declaration = prelude
                .lines()
                .find(|line| !line.trim_start().starts_with("//") && line.contains("@group"));
            assert!(
                declaration.is_none(),
                "a prelude that declared a binding would change the bind-group layout of every \
                 module it is prepended to ({name}): {declaration:?}"
            );
        }
    }

    #[test]
    fn the_filter_module_is_every_prelude_followed_by_its_own_source() {
        // `every_module_is_the_helper_prelude_followed_by_its_own_source`
        // above only pins the first prelude; the filter program is the one
        // module assembled from three, and each has to sit between the helpers
        // it calls and the entry point that calls it.
        assert!(FILTER.starts_with(HELPERS));
        assert!(FILTER[HELPERS.len()..].starts_with(FILTER_KERNELS));
        assert!(FILTER[HELPERS.len() + FILTER_KERNELS.len()..].starts_with(DROP_SHADOW_KERNELS));
        assert!(FILTER.len() > HELPERS.len() + FILTER_KERNELS.len() + DROP_SHADOW_KERNELS.len());
    }

    #[test]
    fn the_filter_instance_matches_its_rust_layout() {
        let module = validate(FILTER_NAME, FILTER);
        assert_eq!(
            struct_span(&module, "FilterInstanceData") as usize,
            size_of::<crate::filters::blur::FilterInstanceData>(),
            "the shader's `FilterInstanceData` must stay byte-identical to the Rust one"
        );
    }

    #[test]
    fn the_strip_config_uniform_matches_its_rust_layout() {
        let module = validate(STRIP_NAME, STRIP);
        assert_eq!(
            u64::from(struct_span(&module, "Config")),
            GpuConfig::SIZE,
            "the shader's `Config` must stay byte-identical to `GpuConfig`"
        );
    }

    #[test]
    fn the_strip_instance_matches_its_rust_layout() {
        let module = validate(STRIP_NAME, STRIP);
        assert_eq!(
            struct_span(&module, "StripInstance") as usize,
            size_of::<GpuStrip>(),
            "the shader's `StripInstance` must stay byte-identical to `GpuStrip`"
        );
    }
}
