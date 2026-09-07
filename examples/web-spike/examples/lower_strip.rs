//! w0-07 step 2: lower `frust-engine`'s `strip.wgsl` to GLSL ES 3.00 the way
//! wgpu's GL backend does, and print it.
//!
//! Native, dev-only (a cargo example, so it is outside `cargo build`'s graph
//! and the wasm32 artifact this spike ships is byte-identical with and
//! without it). It exists so RESULTS.md § 17 can quote the *actual* lowered
//! GLSL for the two reads the WebGL2 glyph defect is suspected in — the
//! `alphas_texture` `textureLoad` (`usampler2D`/`texelFetch`) and the glyph
//! atlas's `atlas_texture_array` `textureLoad` (`sampler2DArray`) — instead
//! of describing them from the WGSL.
//!
//! # Why the options below are copied, not chosen
//!
//! Every knob is transcribed from `wgpu-hal` 30.0.1's own GLES backend
//! (`src/gles/device.rs`, read-only reference), so the output is the source
//! ANGLE would be handed rather than naga's defaults:
//!
//! * `Version::Embedded { version: 300, is_webgl: true }` — WebGL2's shading
//!   language, the `shading_language_version` the GLES backend resolves on a
//!   WebGL2 context.
//! * `WriterFlags::ADJUST_COORDINATE_SPACE | FORCE_POINT_SIZE` — the two the
//!   backend always sets. `TEXTURE_SHADOW_LOD` and `DRAW_PARAMETERS` are
//!   private-capability gated and off on WebGL2.
//! * `BoundsCheckPolicies { index: Unchecked, buffer: Unchecked, image_load:
//!   Unchecked, binding_array: Unchecked }` — `image_load` is
//!   `ReadZeroSkipWrite` only on non-embedded GL >= 4.3, so a WebGL2 context
//!   takes `Unchecked`. This is a real behavioural fork between the host's
//!   desktop-GL arm and the browser's WebGL2 arm and is called out in
//!   RESULTS.md.
//! * `binding_map` — the backend flattens `(group, binding)` into one
//!   per-resource-class counter that runs across groups in group order. The
//!   strip pipeline's four groups declare six sampled textures and one
//!   uniform buffer and no samplers at all (every texture read in
//!   `strip.wgsl`/`helpers.wgsl` is a `textureLoad`), so the map below is
//!   that walk written out.
//!
//! Run with:
//!
//! ```sh
//! cargo run --example lower_strip -- fs_main
//! ```

use naga::back::glsl;
use naga::proc::{BoundsCheckPolicies, BoundsCheckPolicy};
use naga::valid::{Capabilities, ValidationFlags, Validator};

fn main() {
    let mut args = std::env::args().skip(1);
    let entry = args.next().unwrap_or_else(|| "fs_main".to_string());
    let stage = match entry.as_str() {
        "vs_main" => naga::ShaderStage::Vertex,
        _ => naga::ShaderStage::Fragment,
    };

    let source = frust_engine::gpu::shader_src::STRIP;
    let module = match naga::front::wgsl::parse_str(source) {
        Ok(module) => module,
        Err(err) => {
            eprintln!("wgsl-in FAILED:\n{}", err.emit_to_string(source));
            std::process::exit(1);
        }
    };

    // `Capabilities::empty()` is deliberate: it is the floor a downlevel
    // target validates at, so a shader that only validates with extra
    // capabilities would fail here rather than lower silently.
    let mut validator = Validator::new(ValidationFlags::all(), Capabilities::empty());
    let info = match validator.validate(&module) {
        Ok(info) => info,
        Err(err) => {
            eprintln!("validation FAILED: {err:?}");
            std::process::exit(1);
        }
    };

    let mut binding_map = glsl::BindingMap::default();
    // group 0: alphas_texture (texture 0), config (uniform 0),
    // layer_input_texture (texture 1)
    binding_map.insert(res(0, 0), 0);
    binding_map.insert(res(0, 1), 0);
    binding_map.insert(res(0, 2), 1);
    // group 1: atlas_texture_array (texture 2), external_texture (texture 3)
    binding_map.insert(res(1, 0), 2);
    binding_map.insert(res(1, 1), 3);
    // group 2: encoded_paints_texture (texture 4)
    binding_map.insert(res(2, 0), 4);
    // group 3: gradient_texture (texture 5)
    binding_map.insert(res(3, 0), 5);

    let options = glsl::Options {
        version: glsl::Version::Embedded {
            version: 300,
            is_webgl: true,
        },
        writer_flags: glsl::WriterFlags::ADJUST_COORDINATE_SPACE
            | glsl::WriterFlags::FORCE_POINT_SIZE,
        binding_map,
        zero_initialize_workgroup_memory: true,
    };
    let pipeline_options = glsl::PipelineOptions {
        shader_stage: stage,
        entry_point: entry.clone(),
        multiview: None,
    };
    let policies = BoundsCheckPolicies {
        index: BoundsCheckPolicy::Unchecked,
        buffer: BoundsCheckPolicy::Unchecked,
        image_load: BoundsCheckPolicy::Unchecked,
        binding_array: BoundsCheckPolicy::Unchecked,
    };

    let mut out = String::new();
    let mut writer = match glsl::Writer::new(
        &mut out,
        &module,
        &info,
        &options,
        &pipeline_options,
        policies,
    ) {
        Ok(writer) => writer,
        Err(err) => {
            eprintln!("glsl-out writer FAILED for {entry}: {err:?}");
            std::process::exit(1);
        }
    };
    match writer.write() {
        Ok(_reflection) => println!("{out}"),
        Err(err) => {
            eprintln!("glsl-out write FAILED for {entry}: {err:?}");
            std::process::exit(1);
        }
    }
}

fn res(group: u32, binding: u32) -> naga::ResourceBinding {
    naga::ResourceBinding { group, binding }
}
