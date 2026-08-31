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
//! Every module the engine compiles is assembled here and nowhere else, so
//! [`MODULES`] is the whole list — the one
//! [`crate::gpu::pipelines::EngineShaders::register`] walks, and the one the
//! validation tests below walk.

/// Binding-free helper functions shared by every entry-point module:
/// packing, quad geometry, extend modes, encoded-paint accessors, gradient
/// and blur evaluation, and atlas/external image sampling.
///
/// Not a shader in its own right — it has no entry point. It is prepended to
/// each of [`STRIP`], [`CLEAR`] and [`COPY`].
pub const HELPERS: &str = include_str!("../../shaders/helpers.wgsl");

/// The sparse-strip rasterizer: `vs_main` + `fs_main`, four pipeline variants
/// (see [`crate::gpu::pipelines`]).
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

/// The name [`CLEAR`] is registered under in the shader library.
pub const CLEAR_NAME: &str = "frust-engine clear";

/// The name [`COPY`] is registered under in the shader library.
pub const COPY_NAME: &str = "frust-engine copy";

/// The name [`FILTER`] is registered under in the shader library.
pub const FILTER_NAME: &str = "frust-engine filter";

/// Every module the engine compiles, as `(name, source)` pairs — the list
/// [`crate::gpu::pipelines::EngineShaders::register`] walks, and the list the
/// validation tests walk.
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
