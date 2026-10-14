//! The registry of caller-owned textures a frame's strip passes sample, and
//! the per-frame batching that binds each of them in turn.
//!
//! The engine draws two kinds of image. An atlas-backed one lives in the
//! renderer's own array texture, addressed by layer and rectangle, and every
//! draw of the frame reads it through the same binding. An *external* one is a
//! texture the host created and still owns — a video frame, a plugin's import,
//! an offscreen page some other pass rendered — which the engine never copies
//! and never packs. It is bound as a whole texture instead, and the strip
//! shader's image path reads it through a second binding chosen by the encoded
//! paint's source-kind bit (see `shaders/helpers.wgsl`).
//!
//! One binding, many textures: a frame that draws two different external
//! textures cannot bind both at once, so its instances are split into *runs* —
//! maximal spans of consecutive instances sharing one texture — and each run is
//! drawn with that texture bound. [`ExternalRuns`] assigns the slot numbers the
//! renderer's pass segments name a run by; the renderer builds one group-1 bind
//! group per slot and sets it between runs.
//!
//! Nothing here allocates a texture or copies a texel. [`ExternalTextures`] is
//! a plain map the host writes through
//! `EngineRenderer::bind_texture`/`unbind_texture`, and it is generic over the
//! stored view type — like `frust_gpu::TextureRegistry`, so its own behaviour
//! stays host-testable with a stand-in value and no GPU device in the loop.
//!
//! The *extent* half of the same registry lives on the compiler side, in
//! [`crate::compile::external::ExternalExtents`]: the walk needs it to compose
//! a paint transform, while only a pass needs the view. The one call that
//! writes both keeps them in step.
//!
//! ## The strip programs' one explicit layout
//!
//! Every strip pipeline — each render-state variant, and each
//! [paint-class](super::strips::PaintClass) specialisation of it — is built
//! against ONE explicit `wgpu::PipelineLayout`, [`strip_pipeline_layout`],
//! rather than the layout wgpu would derive from its module. A derived layout
//! is whatever bindings the module's entry points statically *use*, and a class
//! program that no longer reads, say, the atlas array would derive a layout
//! without it; the bind groups built for one class would then not fit another.
//! The explicit layout is exactly the one the whole (Uber) program derives —
//! [`strip_bind_group_layout_entries`] restates it entry for entry, and its
//! test holds that restatement to the naga reflection of `strip.wgsl` — so a
//! bind group built against any strip pipeline binds under every other one.
//!
//! There is one such layout per program family
//! ([`crate::gpu::shader_src`]'s *The two program families*): the manual
//! family's (`FRUST_ENGINE_IMAGE_SAMPLER=0`) is the one above, and the sampler
//! family's — the default — adds the one sampler its image programs filter the
//! atlas through — group 1, binding 2 — and declares the atlas binding
//! filterable. Every binding before it keeps its
//! number, so the families differ by that one entry, and the four-group
//! WebGL2 ceiling is untouched.

use std::collections::HashMap;

use frust_gpu::SceneTextureId;
use vello_common::encode::EncodedExternalTexture;

use super::atlas::{extend_mode, pack_image_offset, pack_image_params, pack_image_size, pack_tint};
use super::config::GpuConfig;
use super::paint_texture::{GpuEncodedImage, GpuEncodedPaint};

/// How many bind groups the strip programs declare — the WebGL2 ceiling of
/// four, exactly (see [`crate::gpu::pipelines`]'s *Bind groups*).
pub const STRIP_BIND_GROUP_COUNT: usize = 4;

/// One sampled, non-multisampled texture binding: what every strip texture
/// binding is, every one of them read by `textureLoad` but the sampler
/// family's atlas array.
const fn load_only_texture(
    binding: u32,
    visibility: wgpu::ShaderStages,
    sample_type: wgpu::TextureSampleType,
    view_dimension: wgpu::TextureViewDimension,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension,
            multisampled: false,
        },
        count: None,
    }
}

/// The float sample type every strip `texture_2d<f32>` binding takes but the
/// sampler family's atlas array.
///
/// Unfilterable, because no other strip binding is ever read through a
/// sampler — exactly what wgpu derives for a float texture no sampler touches,
/// and what lets the binding accept a filterable *and* an unfilterable float
/// texture.
const UNFILTERABLE: wgpu::TextureSampleType = wgpu::TextureSampleType::Float { filterable: false };

/// The float sample type of a texture a filtering sampler reads: the sampler
/// family's atlas array, which is `Rgba8Unorm` and so always filterable.
const FILTERABLE: wgpu::TextureSampleType = wgpu::TextureSampleType::Float { filterable: true };

/// The sampler family's sampler binding: group 1, after the two image
/// textures.
pub const ATLAS_SAMPLER_BINDING: u32 = 2;

/// The strip programs' bind-group layout entries, group by group, exactly as
/// wgpu derives them from the whole (Uber) program of the family
/// `image_sampler` names — binding, type and visibility.
///
/// | Group | Binding | WGSL | Read by |
/// |---|---|---|---|
/// | 0 | 0 | `alphas_texture: texture_2d<u32>` | fragment |
/// | 0 | 1 | `config: Config` (uniform, [`GpuConfig::SIZE`] bytes) | vertex, fragment |
/// | 0 | 2 | `layer_input_texture: texture_2d<f32>` | fragment |
/// | 1 | 0 | `atlas_texture_array: texture_2d_array<f32>` (filterable in the sampler family) | fragment |
/// | 1 | 1 | `external_texture: texture_2d<f32>` | fragment |
/// | 1 | 2 | `atlas_sampler: sampler` (filtering; the sampler family only) | fragment |
/// | 2 | 0 | `encoded_paints_texture: texture_2d<u32>` | vertex, fragment |
/// | 3 | 0 | `gradient_texture: texture_2d<f32>` | fragment |
#[must_use]
pub fn strip_bind_group_layout_entries(
    image_sampler: bool,
) -> [Vec<wgpu::BindGroupLayoutEntry>; STRIP_BIND_GROUP_COUNT] {
    use wgpu::ShaderStages as S;
    use wgpu::TextureSampleType as T;
    use wgpu::TextureViewDimension as D;
    [
        vec![
            load_only_texture(0, S::FRAGMENT, T::Uint, D::D2),
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: S::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(GpuConfig::SIZE),
                },
                count: None,
            },
            load_only_texture(2, S::FRAGMENT, UNFILTERABLE, D::D2),
        ],
        if image_sampler {
            vec![
                load_only_texture(0, S::FRAGMENT, FILTERABLE, D::D2Array),
                load_only_texture(1, S::FRAGMENT, UNFILTERABLE, D::D2),
                wgpu::BindGroupLayoutEntry {
                    binding: ATLAS_SAMPLER_BINDING,
                    visibility: S::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ]
        } else {
            vec![
                load_only_texture(0, S::FRAGMENT, UNFILTERABLE, D::D2Array),
                load_only_texture(1, S::FRAGMENT, UNFILTERABLE, D::D2),
            ]
        },
        vec![load_only_texture(0, S::VERTEX_FRAGMENT, T::Uint, D::D2)],
        vec![load_only_texture(0, S::FRAGMENT, UNFILTERABLE, D::D2)],
    ]
}

/// The one explicit pipeline layout every strip pipeline of the family
/// `image_sampler` names is built against.
///
/// Created once per renderer, at shader registration
/// ([`crate::gpu::pipelines::EngineShaders::register`]). A pipeline built
/// against it hands out these same group layouts from
/// `get_bind_group_layout`, so the renderer's existing bind-group construction
/// is unchanged and its groups bind under every strip variant and class.
#[must_use]
pub fn strip_pipeline_layout(device: &wgpu::Device, image_sampler: bool) -> wgpu::PipelineLayout {
    let entries = strip_bind_group_layout_entries(image_sampler);
    let groups = entries.each_ref().map(|entries| {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frust-engine strip group"),
            entries,
        })
    });
    let refs = groups.each_ref().map(Some);
    device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("frust-engine strip layout"),
        bind_group_layouts: &refs,
        immediate_size: 0,
    })
}

/// Every externally owned texture currently bound, keyed by the
/// [`SceneTextureId`] its owner minted.
///
/// The stored view must be a non-array 2D view of a float-sampleable texture
/// whose texture carries `wgpu::TextureUsages::TEXTURE_BINDING`; only mip level
/// 0 is ever read. `wgpu` rejects anything else when the bind group is built,
/// which is the frame the mistake surfaces on.
///
/// Generic over the view type (default `wgpu::TextureView`, the real engine's)
/// so the map's own insert/replace/remove behaviour is testable without a
/// device.
#[derive(Debug)]
pub struct ExternalTextures<V = wgpu::TextureView> {
    /// Keyed by the id's opaque value rather than by [`SceneTextureId`]
    /// itself: a display list names an external texture by that same plain
    /// `u64` (`frust_scene::Command::SceneTexture`), and a lookup from a
    /// compiled frame has nothing but that number to ask with.
    views: HashMap<u64, V>,
}

impl<V> Default for ExternalTextures<V> {
    fn default() -> Self {
        Self {
            views: HashMap::new(),
        }
    }
}

impl<V> ExternalTextures<V> {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `view` under `id`, returning whatever was registered before.
    pub fn bind(&mut self, id: SceneTextureId, view: V) -> Option<V> {
        self.views.insert(id.get(), view)
    }

    /// Removes and returns the view registered under `id`.
    pub fn unbind(&mut self, id: SceneTextureId) -> Option<V> {
        self.views.remove(&id.get())
    }

    /// The view registered under `id`.
    #[must_use]
    pub fn get(&self, id: SceneTextureId) -> Option<&V> {
        self.views.get(&id.get())
    }

    /// The view registered under the opaque id a display list names, which is
    /// the only form a compiled frame carries.
    #[must_use]
    pub fn view(&self, key: u64) -> Option<&V> {
        self.views.get(&key)
    }

    /// How many textures are bound.
    #[must_use]
    pub fn len(&self) -> usize {
        self.views.len()
    }

    /// Whether nothing is bound.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.views.is_empty()
    }
}

/// The distinct external textures one frame draws with, in the order it first
/// names them.
///
/// A frame's instances are drawn in painter order, and the single external
/// binding has to be re-set whenever that order crosses from one texture to
/// another. Rather than carry the id on every instance, each encoded paint
/// takes a *slot* — its index here — and the renderer's pass segments carry
/// that slot, so a run of instances sharing a texture is one segment and one
/// bind-group set.
///
/// Filled while the frame's paints are lowered and read while its passes are
/// recorded; cleared and refilled per frame, never growing past the number of
/// external textures a single frame draws.
#[derive(Debug, Default)]
pub struct ExternalRuns {
    keys: Vec<u64>,
}

impl ExternalRuns {
    /// An empty set of runs.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets the previous frame's textures, keeping the allocation.
    pub fn clear(&mut self) {
        self.keys.clear();
    }

    /// The slot `key` is drawn under, assigning one if this frame has not
    /// named it yet.
    ///
    /// A linear scan rather than a map: a frame draws a handful of external
    /// textures at most — the binding is re-set between runs, so a scene that
    /// interleaved hundreds would be paying far more for the pass breaks than
    /// for the lookup.
    ///
    /// `None` once the slot numbering would leave `u32`, which no real frame
    /// reaches; the paint is then left unresolved and its draws skipped, which
    /// is the same answer an unbound texture gets.
    pub fn slot_of(&mut self, key: u64) -> Option<u32> {
        if let Some(index) = self.keys.iter().position(|held| *held == key) {
            return u32::try_from(index).ok();
        }
        let slot = u32::try_from(self.keys.len()).ok()?;
        self.keys.push(key);
        Some(slot)
    }

    /// The texture each slot names, indexed by slot number.
    #[must_use]
    pub fn keys(&self) -> &[u64] {
        &self.keys
    }

    /// The texture drawn under `slot`.
    #[must_use]
    pub fn key_at(&self, slot: u32) -> Option<u64> {
        self.keys.get(slot as usize).copied()
    }

    /// How many distinct external textures the frame draws.
    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether the frame draws none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

/// Lower one encoded external texture into the record the strip shader samples.
///
/// The external counterpart of [`super::atlas::lower_encoded_image`], and the
/// same record type: the shader reads both through one `GpuEncodedImage`
/// layout and picks the binding from the source-kind bit
/// [`pack_image_params`] sets here. What differs is where the numbers come
/// from — the source region is the caller's own texel rectangle rather than an
/// atlas allocation, there is no atlas layer to name (hence the zero index,
/// which the shader never reads on this path) and no padding, since nothing
/// was packed beside it.
///
/// The transform is already the inverse mapping — device space into texel
/// space — because that is the direction the shader applies it in; narrowing
/// it to `f32` here is what the record and the WGSL both read it at.
#[must_use]
pub fn lower_encoded_external(entry: &EncodedExternalTexture) -> GpuEncodedPaint {
    let region = entry.source_region;
    let (tint, tint_mode) = pack_tint(entry.tint);

    GpuEncodedPaint::Image(GpuEncodedImage {
        image_params: pack_image_params(
            entry.sampler.quality as u32,
            extend_mode(entry.sampler.x_extend),
            extend_mode(entry.sampler.y_extend),
            0,
            true,
        ),
        image_size: pack_image_size(
            region.x1.saturating_sub(region.x0),
            region.y1.saturating_sub(region.y0),
        ),
        image_offset: pack_image_offset(region.x0, region.y0),
        transform: entry.transform.as_coeffs().map(|coeff| coeff as f32),
        tint,
        tint_mode,
        image_padding: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use peniko::{ImageQuality, ImageSampler};
    use vello_common::TextureId;
    use vello_common::geometry::RectU16;
    use vello_common::kurbo::Affine;

    /// A registry over plain values, which is all the map's own behaviour
    /// needs — minting a real `wgpu::TextureView` would require a device.
    fn registry() -> ExternalTextures<&'static str> {
        ExternalTextures::new()
    }

    fn entry(region: RectU16) -> EncodedExternalTexture {
        EncodedExternalTexture {
            texture_id: TextureId(1),
            source_region: region,
            sampler: ImageSampler {
                quality: ImageQuality::Medium,
                ..ImageSampler::default()
            },
            may_have_transparency: true,
            transform: Affine::IDENTITY,
            tint: None,
        }
    }

    #[test]
    fn a_bound_view_is_reachable_by_id_and_by_the_scenes_own_number() {
        let mut textures = registry();
        let id = fresh_id();

        assert!(textures.bind(id, "view").is_none());

        assert_eq!(textures.get(id), Some(&"view"));
        assert_eq!(textures.view(id.get()), Some(&"view"));
        assert_eq!(textures.len(), 1);
    }

    #[test]
    fn rebinding_returns_the_previous_view() {
        let mut textures = registry();
        let id = fresh_id();
        textures.bind(id, "first");

        assert_eq!(textures.bind(id, "second"), Some("first"));
        assert_eq!(textures.get(id), Some(&"second"));
    }

    #[test]
    fn unbinding_returns_the_view_and_empties_the_registry() {
        let mut textures = registry();
        let id = fresh_id();
        textures.bind(id, "view");

        assert_eq!(textures.unbind(id), Some("view"));
        assert!(textures.is_empty());
        assert_eq!(textures.view(id.get()), None);
    }

    #[test]
    fn an_unbound_id_resolves_to_nothing() {
        let textures = registry();

        assert_eq!(textures.view(4_242), None);
    }

    #[test]
    fn a_slot_is_assigned_once_per_texture_and_reused_after() {
        let mut runs = ExternalRuns::new();

        assert_eq!(runs.slot_of(7), Some(0));
        assert_eq!(runs.slot_of(9), Some(1));
        assert_eq!(runs.slot_of(7), Some(0), "the same texture keeps its slot");
        assert_eq!(runs.keys(), &[7, 9]);
        assert_eq!(runs.key_at(1), Some(9));
        assert_eq!(runs.key_at(2), None);
    }

    #[test]
    fn clearing_forgets_the_previous_frames_textures() {
        let mut runs = ExternalRuns::new();
        runs.slot_of(7);

        runs.clear();

        assert!(runs.is_empty());
        assert_eq!(runs.slot_of(9), Some(0), "slots restart each frame");
    }

    #[test]
    fn a_lowered_external_record_names_the_external_source_and_no_atlas_layer() {
        let record = lower_encoded_external(&entry(RectU16 {
            x0: 0,
            y0: 0,
            x1: 64,
            y1: 32,
        }));

        let GpuEncodedPaint::Image(image) = record else {
            panic!("an external texture lowers to the image record");
        };
        assert_eq!(
            (image.image_params >> 14) & 0x1,
            1,
            "the source-kind bit selects the external binding"
        );
        assert_eq!((image.image_params >> 6) & 0xFF, 0, "no atlas layer");
        assert_eq!(image.image_size, pack_image_size(64, 32));
        assert_eq!(image.image_offset, pack_image_offset(0, 0));
        assert_eq!(image.image_padding, 0, "nothing was packed beside it");
    }

    #[test]
    fn a_sub_rectangle_lowers_as_its_own_offset_and_extent() {
        let record = lower_encoded_external(&entry(RectU16 {
            x0: 8,
            y0: 4,
            x1: 24,
            y1: 20,
        }));

        let GpuEncodedPaint::Image(image) = record else {
            panic!("an external texture lowers to the image record");
        };
        assert_eq!(image.image_offset, pack_image_offset(8, 4));
        assert_eq!(image.image_size, pack_image_size(16, 16));
    }

    /// The explicit strip layout of each family is the layout wgpu would
    /// derive from that family's whole strip program, entry for entry: every
    /// binding the program declares is present with the type naga reflects for
    /// it — a float texture filterable exactly when a sampler reads it —
    /// visible to exactly the stages whose entry point uses it, and nothing
    /// else is declared. That is what makes switching the Uber program from a
    /// derived to this explicit layout a no-op, and what lets every class
    /// program of the family — a subset of the Uber one's uses — validate
    /// against it.
    #[test]
    fn the_explicit_strip_layout_is_the_uber_programs_derived_layout() {
        for image_sampler in [false, true] {
            explicit_layout_matches_derived(image_sampler);
        }
    }

    fn explicit_layout_matches_derived(image_sampler: bool) {
        use crate::gpu::shader_src::strip_program_source;
        use crate::gpu::strips::PaintClass;
        use wgpu::naga;
        let src = strip_program_source(PaintClass::Uber, image_sampler);
        let module = naga::front::wgsl::parse_str(&src).expect("the strip source parses");
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .expect("the strip source validates");
        let sampled = |handle| {
            (0..module.entry_points.len()).any(|index| {
                info.get_entry_point(index)
                    .sampling_set
                    .iter()
                    .any(|key| key.image == handle)
            })
        };

        let entries = strip_bind_group_layout_entries(image_sampler);
        let mut declared = 0;
        for (handle, global) in module.global_variables.iter() {
            let Some(binding) = &global.binding else {
                continue;
            };
            declared += 1;
            let entry = entries
                .get(binding.group as usize)
                .and_then(|group| group.iter().find(|e| e.binding == binding.binding))
                .unwrap_or_else(|| panic!("no layout entry for {binding:?}"));

            let mut stages = wgpu::ShaderStages::NONE;
            for (index, entry_point) in module.entry_points.iter().enumerate() {
                if !info.get_entry_point(index)[handle].is_empty() {
                    stages |= match entry_point.stage {
                        naga::ShaderStage::Vertex => wgpu::ShaderStages::VERTEX,
                        naga::ShaderStage::Fragment => wgpu::ShaderStages::FRAGMENT,
                        _ => wgpu::ShaderStages::NONE,
                    };
                }
            }
            assert_eq!(entry.visibility, stages, "{:?} visibility", global.name);

            match (&module.types[global.ty].inner, entry.ty) {
                (
                    naga::TypeInner::Image {
                        dim: naga::ImageDimension::D2,
                        arrayed,
                        class: naga::ImageClass::Sampled { kind, multi: false },
                    },
                    wgpu::BindingType::Texture {
                        sample_type,
                        view_dimension,
                        multisampled: false,
                    },
                ) => {
                    let expected_dim = if *arrayed {
                        wgpu::TextureViewDimension::D2Array
                    } else {
                        wgpu::TextureViewDimension::D2
                    };
                    assert_eq!(view_dimension, expected_dim, "{:?}", global.name);
                    let expected_sample = match kind {
                        naga::ScalarKind::Uint => wgpu::TextureSampleType::Uint,
                        naga::ScalarKind::Float if sampled(handle) => FILTERABLE,
                        naga::ScalarKind::Float => UNFILTERABLE,
                        other => panic!("unexpected sample kind {other:?}"),
                    };
                    assert_eq!(sample_type, expected_sample, "{:?}", global.name);
                }
                (
                    naga::TypeInner::Sampler { comparison: false },
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ) => {
                    assert_eq!(binding.group, 1);
                    assert_eq!(binding.binding, ATLAS_SAMPLER_BINDING);
                }
                (
                    naga::TypeInner::Struct { span, .. },
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size,
                    },
                ) => {
                    assert_eq!(global.space, naga::AddressSpace::Uniform);
                    assert_eq!(
                        min_binding_size.map(|size| size.get()),
                        Some(u64::from(*span))
                    );
                }
                (shader, layout) => panic!("{:?}: {shader:?} vs {layout:?}", global.name),
            }
        }
        assert_eq!(
            declared,
            entries.iter().map(Vec::len).sum::<usize>(),
            "the layout declares a binding the program does not"
        );
        assert_eq!(
            entries[1].len(),
            if image_sampler { 3 } else { 2 },
            "only the sampler family declares the sampler"
        );
    }

    /// A fresh [`SceneTextureId`], minted the only way this crate can: off a
    /// texture handle built over stand-in values.
    fn fresh_id() -> SceneTextureId {
        frust_gpu::Texture::new(
            (),
            (),
            frust_gpu::TextureDesc {
                width: 1,
                height: 1,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                label: None,
            },
        )
        .as_scene_texture()
    }
}
