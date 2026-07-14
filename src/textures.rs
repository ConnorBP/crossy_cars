//! Procedural textures generated entirely in code (no asset files).
//!
//! `TexturesPlugin` runs a `Startup` system that builds RGBA pixel data for
//! grass, road, sidewalk, foliage, hay, and car paint, wraps each image in a
//! repeating sampler, and stores ready-to-use `StandardMaterial` handles in
//! the `TextureAssets` resource so other systems only clone cached handles.
//!
//! Color textures use WebGL2-safe `Rgba8UnormSrgb`; normal maps use linear
//! `Rgba8Unorm` (lighting breaks if vectors are stored sRGB). Procedural color
//! coordinates wrap explicitly and every sampler repeats, so all patterns tile
//! exactly without external assets.

use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::math::Affine2;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

/// Texture edge length in pixels.
const TEX_SIZE: u32 = 64;

/// Normal-map edge length (can differ from color textures; 64 is plenty for
/// a subtle surface perturbation that tiles).
const NORMAL_SIZE: u32 = 64;

pub const FOLIAGE_VARIANTS: usize = 3;
pub const HAY_VARIANTS: usize = 2;

// A 64px tile contains only two long slabs across. Rows are staggered by half
// a slab, and all dimensions divide TEX_SIZE to preserve the exact period.
const SIDEWALK_SLAB_WIDTH: u32 = 32;
const SIDEWALK_SLAB_HEIGHT: u32 = 16;

// Base sRGB values matching crate::palette constants (see palette.rs).
const GRASS_SRGB: [f32; 3] = [0.30, 0.60, 0.30]; // palette::GRASS_LIGHT
const ASPHALT_SRGB: [f32; 3] = [0.13, 0.13, 0.14]; // palette::ASPHALT
const CONCRETE_SRGB: [f32; 3] = [0.72, 0.71, 0.68]; // palette::CONCRETE

/// Ready-to-use textured `StandardMaterial` handles, inserted as a resource.
/// Organic variant arrays are public integration API, hence their public
/// length constants.
///
/// Field names (for orchestrator wiring):
/// - `grass`      → grass ground plane
/// - `road`       → asphalt road strip
/// - `sidewalk`   → concrete sidewalk curbs
/// - `foliage`    → three deterministic leaf palettes/species
/// - `hay`        → `[field_straw, bale_straw]`
/// - `car_paint`  → car body paint
#[derive(Resource)]
pub struct TextureAssets {
    pub grass: Handle<StandardMaterial>,
    pub road: Handle<StandardMaterial>,
    pub sidewalk: Handle<StandardMaterial>,
    // These caches are consumed by the foliage/field integration separately;
    // keeping them here guarantees streaming code never allocates per block.
    #[allow(dead_code)]
    pub foliage: [Handle<StandardMaterial>; FOLIAGE_VARIANTS],
    #[allow(dead_code)]
    pub hay: [Handle<StandardMaterial>; HAY_VARIANTS],
    pub car_paint: Handle<StandardMaterial>,
}

pub struct TexturesPlugin;

impl Plugin for TexturesPlugin {
    fn build(&self, app: &mut App) {
        // Built via `FromWorld` so the handles exist before any `Startup`
        // spawn system (e.g. world/car) tries to use `Res<TextureAssets>`.
        app.init_resource::<TextureAssets>();
    }
}

impl FromWorld for TextureAssets {
    fn from_world(world: &mut World) -> Self {
        world.resource_scope::<Assets<Image>, _>(|world, mut images| {
            let (grass, road, sidewalk, foliage, hay, car_paint) = {
                let mut materials = world.resource_mut::<Assets<StandardMaterial>>();

                // --- Procedural normal maps ---
                // Road/asphalt gets a gravelly normal map (stronger) so the surface
                // catches light per-pixel instead of looking like flat paint.
                let road_normal = images.add(asphalt_normal_map());
                // Grass gets gentle blade bumps for natural light scatter.
                let grass_normal = images.add(grass_normal_map());
                // Sidewalk gets a bumpy normal for concrete texture.
                let sidewalk_normal = images.add(concrete_normal_map());

                // GRASS — subtle deterministic mottle, mowing bands, and blade
                // strokes; tile 16x with a matte blade-bump normal map.
                let grass = materials.add(StandardMaterial {
                    base_color: Color::WHITE,
                    base_color_texture: Some(images.add(grass_texture())),
                    normal_map_texture: Some(grass_normal),
                    perceptual_roughness: 1.0,
                    metallic: 0.0,
                    uv_transform: Affine2::from_scale(Vec2::splat(16.0)),
                    ..default()
                });

                // ROAD — dark asphalt with richer gravel/noise; tile 8×. T15: more
                // gravel specks + multi-frequency noise, near-fully rough with a
                // gravelly normal map for per-pixel surface detail.
                let road = materials.add(StandardMaterial {
                    base_color: Color::WHITE,
                    base_color_texture: Some(images.add(road_texture())),
                    normal_map_texture: Some(road_normal),
                    perceptual_roughness: 0.92,
                    metallic: 0.0,
                    uv_transform: Affine2::from_scale(Vec2::splat(8.0)),
                    ..default()
                });

                // SIDEWALK — long staggered concrete slabs, restrained slab-tone
                // variation, expansion joints, and a rough concrete normal map.
                let sidewalk = materials.add(StandardMaterial {
                    base_color: Color::WHITE,
                    base_color_texture: Some(images.add(sidewalk_texture())),
                    normal_map_texture: Some(sidewalk_normal),
                    perceptual_roughness: 0.88,
                    metallic: 0.0,
                    // Fewer repeats make the larger, staggered slabs legible.
                    uv_transform: Affine2::from_scale(Vec2::splat(3.0)),
                    ..default()
                });

                // Three cached species/palette variants. Textures add leaf-sized
                // highlights and veins without transparent cutouts (they are used
                // by closed foliage meshes).
                let foliage = std::array::from_fn(|variant| {
                    materials.add(StandardMaterial {
                        base_color: Color::WHITE,
                        base_color_texture: Some(images.add(foliage_texture(variant))),
                        perceptual_roughness: 0.9,
                        metallic: 0.0,
                        uv_transform: Affine2::from_scale(Vec2::splat(2.0)),
                        ..default()
                    })
                });

                // Cached field/bale straw variants. Both carry directional fibres;
                // their different band direction and palette suit the two uses.
                let hay = std::array::from_fn(|variant| {
                    materials.add(StandardMaterial {
                        base_color: Color::WHITE,
                        base_color_texture: Some(images.add(hay_texture(variant))),
                        perceptual_roughness: 0.96,
                        metallic: 0.0,
                        uv_transform: Affine2::from_scale(Vec2::splat(3.0)),
                        ..default()
                    })
                });

                // Real red metallic paint. The rounded body supplies smooth
                // normals; Bevy's PBR shader samples the camera's prefiltered
                // diffuse/GGX environment maps. Clearcoat adds a glossy lacquer
                // lobe over the red metal rather than faking it in custom WGSL.
                let car_paint = materials.add(StandardMaterial {
                    base_color: Color::srgb(0.62, 0.025, 0.02),
                    metallic: 0.9,
                    perceptual_roughness: 0.16,
                    clearcoat: 1.0,
                    clearcoat_perceptual_roughness: 0.10,
                    ..default()
                });

                (grass, road, sidewalk, foliage, hay, car_paint)
            };

            TextureAssets {
                grass,
                road,
                sidewalk,
                foliage,
                hay,
                car_paint,
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Noise helpers
// ---------------------------------------------------------------------------

/// Hash-based pseudo-random noise → u32.
fn noise(x: u32, y: u32) -> u32 {
    x.wrapping_mul(374761393)
        .wrapping_add(y.wrapping_mul(668265263))
        ^ 0x9e3779b9
}

/// Signed noise in range -128..127.
fn signed_noise(x: u32, y: u32) -> i32 {
    ((noise(x, y) >> 8) & 0xFF) as i32 - 128
}

/// A second, uncorrelated hash (different constants) so multi-octave noise
/// doesn't alias with the primary `noise`/`signed_noise` functions.
fn noise2(x: u32, y: u32) -> u32 {
    x.wrapping_mul(2246822519)
        .wrapping_add(y.wrapping_mul(3266489917))
        ^ 0x61c8864d
}

/// Signed noise (second hash) in range -128..127.
fn signed_noise2(x: u32, y: u32) -> i32 {
    ((noise2(x, y) >> 8) & 0xFF) as i32 - 128
}

/// Clamp an i32 color channel to a u8.
fn clamp_byte(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// Convert an sRGB float triple to i32 byte base values.
fn srgb_base(c: [f32; 3]) -> [i32; 3] {
    [
        (c[0].clamp(0.0, 1.0) * 255.0).round() as i32,
        (c[1].clamp(0.0, 1.0) * 255.0).round() as i32,
        (c[2].clamp(0.0, 1.0) * 255.0).round() as i32,
    ]
}

/// Set the image sampler to repeat in both U and V for tiling.
fn set_repeat(img: &mut Image) {
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        ..default()
    });
}

/// Build a `TEX_SIZE²` RGBA image from a per-pixel closure, with a repeating
/// sampler. Color textures use `Rgba8UnormSrgb`.
fn make_image<F>(fill: F) -> Image
where
    F: Fn(u32, u32) -> [u8; 4],
{
    let size = TEX_SIZE;
    let mut data = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let px = fill(x, y);
            let i = ((y * size + x) * 4) as usize;
            data[i] = px[0];
            data[i + 1] = px[1];
            data[i + 2] = px[2];
            data[i + 3] = px[3];
        }
    }
    let mut img = Image::new(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    set_repeat(&mut img);
    img
}

// ---------------------------------------------------------------------------
// Per-texture pixel builders
// ---------------------------------------------------------------------------

/// Grass: restrained tileable mottle plus tiny deterministic blade strokes.
/// The blade cells and every band divide 64, and `grass_pixel` wraps incoming
/// coordinates before hashing so callers see an exact 64px period.
fn grass_texture() -> Image {
    make_image(grass_pixel)
}

fn grass_pixel(x: u32, y: u32) -> [u8; 4] {
    let x = x % TEX_SIZE;
    let y = y % TEX_SIZE;
    let b = srgb_base(GRASS_SRGB);

    // Eight-by-eight clumps produce broad, quiet mottle rather than obvious
    // per-texel static. Fine grain prevents the patches looking airbrushed.
    let mottle = signed_noise2(x / 8, y / 8) * 13 / 128;
    let fine = signed_noise(x, y) * 7 / 128;
    let stripe = if (y / 16) % 2 == 0 { 3 } else { -3 };

    // One short blade per 4x4 cell. The hash picks its origin and lean; testing
    // the preceding row draws a subtle two-pixel stroke instead of bright dots.
    let cell_x = x / 4;
    let cell_y = y / 4;
    let blade_hash = noise2(cell_x, cell_y);
    let origin_x = blade_hash & 3;
    let origin_y = (blade_hash >> 2) & 3;
    let local_x = x & 3;
    let local_y = y & 3;
    let blade = local_y == origin_y && local_x == origin_x
        || local_y == (origin_y + 1) % 4 && local_x == (origin_x + ((blade_hash >> 4) & 1)) % 4;
    let blade_light = if blade { 11 } else { 0 };

    [
        clamp_byte(b[0] + mottle / 2 + fine / 2 + stripe / 2 + blade_light / 3),
        clamp_byte(b[1] + mottle + fine + stripe + blade_light),
        clamp_byte(b[2] + mottle / 3 + fine / 2 + stripe / 3 + blade_light / 4),
        255,
    ]
}

/// Road: dark asphalt with richer multi-frequency gravel/noise and more
/// frequent lighter gravel specks of varying brightness.
fn road_texture() -> Image {
    let b = srgb_base(ASPHALT_SRGB);
    make_image(move |x, y| {
        // Base fine grain + a coarser low-frequency modulation.
        let v = signed_noise(x, y) * 12 / 128;
        let coarse = signed_noise2(x / 3, y / 3) * 14 / 128;

        // Gravel specks at three densities/brightnesses for richer grain.
        let n = noise(x, y);
        let n2 = noise2(x * 2, y * 2);
        let speck = if (n & 0x3F) == 0 {
            15
        } else if (n2 & 0x1F) == 0 {
            22
        } else if (n & 0x0F) == 0 {
            8
        } else {
            0
        };

        let r = clamp_byte(b[0] + v + coarse + speck);
        let g = clamp_byte(b[1] + v + coarse + speck);
        let bl = clamp_byte(b[2] + v + coarse + speck);
        [r, g, bl, 255]
    })
}

/// Sidewalk: long 32x16 concrete slabs with staggered vertical seams. It has
/// one quarter as many columns as the old 4x4 checker and avoids alternating
/// every slab's brightness, substantially reducing visible repetition.
fn sidewalk_texture() -> Image {
    make_image(sidewalk_pixel)
}

fn sidewalk_is_joint(x: u32, y: u32) -> bool {
    let x = x % TEX_SIZE;
    let y = y % TEX_SIZE;
    let row = y / SIDEWALK_SLAB_HEIGHT;
    let stagger = if row % 2 == 0 {
        0
    } else {
        SIDEWALK_SLAB_WIDTH / 2
    };
    y % SIDEWALK_SLAB_HEIGHT == 0 || (x + stagger) % SIDEWALK_SLAB_WIDTH == 0
}

fn sidewalk_pixel(x: u32, y: u32) -> [u8; 4] {
    let x = x % TEX_SIZE;
    let y = y % TEX_SIZE;
    let b = srgb_base(CONCRETE_SRGB);
    let row = y / SIDEWALK_SLAB_HEIGHT;
    let stagger = if row % 2 == 0 {
        0
    } else {
        SIDEWALK_SLAB_WIDTH / 2
    };
    // Modulo aliases the two pieces of the odd-row slab that crosses the
    // texture boundary, so its tone remains continuous across U repeat.
    let slabs_per_row = TEX_SIZE / SIDEWALK_SLAB_WIDTH;
    let slab_x = ((x + stagger) / SIDEWALK_SLAB_WIDTH) % slabs_per_row;
    let slab_tone = signed_noise2(slab_x, row) * 7 / 128;
    let grain = signed_noise(x, y) * 8 / 128;
    let brushed = if (x + 2 * y) % 11 == 0 { 2 } else { 0 };
    let joint = if sidewalk_is_joint(x, y) { -17 } else { 0 };
    let value = slab_tone + grain + brushed + joint;

    [
        clamp_byte(b[0] + value),
        clamp_byte(b[1] + value),
        clamp_byte(b[2] + value),
        255,
    ]
}

/// Dense, opaque leaf surfaces for the three cached foliage variants. Palette,
/// clump scale, vein direction, and highlights differ by variant while every
/// coordinate is reduced into the 64px tile before hashing.
fn foliage_texture(variant: usize) -> Image {
    assert!(variant < FOLIAGE_VARIANTS);
    make_image(move |x, y| foliage_pixel(variant, x, y))
}

fn foliage_pixel(variant: usize, x: u32, y: u32) -> [u8; 4] {
    let x = x % TEX_SIZE;
    let y = y % TEX_SIZE;
    let bases = [[45, 108, 39], [57, 123, 48], [35, 91, 53]];
    let b = bases[variant];
    let scale = [8, 4, 8][variant];
    let clump = signed_noise2(x / scale, y / scale) * [18, 14, 20][variant] / 128;
    let grain = signed_noise(x + variant as u32 * 19, y) * 7 / 128;

    // Broken diagonal veins and sparse warm leaf tips avoid a stamped grid.
    let vein = match variant {
        0 => (x + 2 * y) % 13 == 0,
        1 => (2 * x + y) % 11 == 0,
        _ => (x + TEX_SIZE - y) % 17 == 0,
    };
    let vein_light = if vein { 10 } else { 0 };
    let tip = if noise2(x + 41, y + variant as u32 * 23) & 0x7f == 0 {
        15
    } else {
        0
    };

    [
        clamp_byte(b[0] + clump / 2 + grain / 3 + vein_light / 3 + tip),
        clamp_byte(b[1] + clump + grain + vein_light + tip / 2),
        clamp_byte(b[2] + clump / 3 + grain / 2 + vein_light / 4),
        255,
    ]
}

/// Directional straw suitable for broad fields (0) and wrapped bales (1).
/// Fibres are narrow broken lines; 16px bands add structure without obvious
/// high-frequency repetition.
fn hay_texture(variant: usize) -> Image {
    assert!(variant < HAY_VARIANTS);
    make_image(move |x, y| hay_pixel(variant, x, y))
}

fn hay_pixel(variant: usize, x: u32, y: u32) -> [u8; 4] {
    let x = x % TEX_SIZE;
    let y = y % TEX_SIZE;
    let bases = [[184, 139, 43], [211, 166, 57]];
    let b = bases[variant];
    let along = if variant == 0 { x } else { y };
    let across = if variant == 0 { y } else { x };
    let band = if (across / 16) % 2 == 0 { 7 } else { -7 };
    let grain = signed_noise(x, y) * 8 / 128;

    // Broken 1px fibres, offset per row/column. A second sparse dark strand
    // gives bales and fields depth while remaining fully opaque and matte.
    let phase = noise2(across / 2, variant as u32 * 29) & 15;
    let fibre = across % 4 == (phase & 3) && (along + phase) % 16 < 11;
    let dark_fibre = across % 9 == (phase % 9) && (along + phase * 3) % 32 < 17;
    let fibre_light = if fibre { 18 } else { 0 };
    let fibre_dark = if dark_fibre { -12 } else { 0 };

    [
        clamp_byte(b[0] + band + grain + fibre_light + fibre_dark),
        clamp_byte(b[1] + band + grain + fibre_light * 2 / 3 + fibre_dark),
        clamp_byte(b[2] + band / 2 + grain / 2 + fibre_light / 5 + fibre_dark / 2),
        255,
    ]
}

// ---------------------------------------------------------------------------
// Procedural normal maps
// ---------------------------------------------------------------------------
//
// A normal map encodes perturbed surface normals in tangent space as RGB:
// (128, 128, 255) ≈ flat (normal pointing +Z). We derive the normal from a
// noise height field via finite differences: n = normalize(-dh/dx, -dh/dy, 1)
// scaled by a `strength` factor. The result is an RGBA image in **linear**
// space (`Rgba8Unorm`, NOT sRGB) — lighting breaks if normal maps are stored
// sRGB. Kept subtle so surfaces get per-pixel light scatter without looking
// bumpy/low-poly. All height fields use wrapping (tileable) indices.

/// Build a `NORMAL_SIZE²` RGBA **normal map** image from a height-field
/// closure. `strength` controls how much the height derivatives perturb the
/// normal (smaller = subtler). The sampler repeats for tiling.
fn make_normal_map<F>(height: F, strength: f32) -> Image
where
    F: Fn(u32, u32) -> f32,
{
    let size = NORMAL_SIZE;
    // Precompute the height field so finite differences can sample neighbours.
    let mut h = vec![0.0f32; (size * size) as usize];
    for y in 0..size {
        for x in 0..size {
            h[(y * size + x) as usize] = height(x, y);
        }
    }
    let at = |x: i32, y: i32| -> f32 {
        // Wrap (tileable) indices.
        let x = x.rem_euclid(size as i32) as u32;
        let y = y.rem_euclid(size as i32) as u32;
        h[(y * size + x) as usize]
    };

    let mut data = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let xi = x as i32;
            let yi = y as i32;
            // Central differences (one texel in each direction).
            let dx = (at(xi + 1, yi) - at(xi - 1, yi)) * strength;
            let dy = (at(xi, yi + 1) - at(xi, yi - 1)) * strength;
            // Tangent-space normal: (-dx, -dy, 1) normalized.
            let nx = -dx;
            let ny = -dy;
            let nz = 1.0;
            let len = (nx * nx + ny * ny + nz * nz).sqrt().max(1e-6);
            let r = ((nx / len) * 0.5 + 0.5).clamp(0.0, 1.0);
            let g = ((ny / len) * 0.5 + 0.5).clamp(0.0, 1.0);
            let b = ((nz / len) * 0.5 + 0.5).clamp(0.0, 1.0);
            let i = ((y * size + x) * 4) as usize;
            data[i] = (r * 255.0).round() as u8;
            data[i + 1] = (g * 255.0).round() as u8;
            data[i + 2] = (b * 255.0).round() as u8;
            data[i + 3] = 255;
        }
    }
    let mut img = Image::new(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        // Linear, NOT sRGB — normal maps store vectors, not colours.
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::default(),
    );
    set_repeat(&mut img);
    img
}

/// Grass normal map: gentle blade bumps — slightly stronger and with a hint
/// of directional bias so light scatters along blade-like ridges. T15:
/// updated to a two-octave height field for richer micro-relief.
fn grass_normal_map() -> Image {
    make_normal_map(
        |x, y| {
            // Low-frequency bumps for clumps + finer high-frequency for blades.
            let clump = signed_noise(x, y) as f32 / 128.0 * 0.6;
            let blade = signed_noise2(x * 3, y * 3) as f32 / 128.0 * 0.4;
            clump + blade
        },
        0.7,
    )
}

/// Asphalt/road normal map: a mix of low-frequency bumps + sharper gravel
/// specks so the road catches light with a gravelly grain. T15: richer with
/// a third high-frequency octave for finer gravel.
fn asphalt_normal_map() -> Image {
    make_normal_map(
        |x, y| {
            let base = signed_noise(x, y) as f32 / 128.0;
            // Sharper speckles at a different frequency for gravel grain.
            let speck = signed_noise(x * 3, y * 3) as f32 / 128.0;
            // Finer high-frequency gravel dust.
            let dust = signed_noise2(x * 5, y * 5) as f32 / 128.0;
            base * 0.5 + speck * 0.35 + dust * 0.15
        },
        0.8,
    )
}

/// Concrete/sidewalk normal map: bumpy with a slightly stronger, coarser
/// grain than `smooth_normal_map` to read as brushed concrete. T15: split
/// into its own generator so the sidewalk can be tuned independently from
/// the generic smooth noise used previously.
fn concrete_normal_map() -> Image {
    make_normal_map(
        |x, y| {
            // Coarse concrete grain + a finer component.
            let coarse = signed_noise(x, y) as f32 / 128.0 * 0.7;
            let fine = signed_noise2(x * 2, y * 2) as f32 / 128.0 * 0.3;
            coarse + fine
        },
        0.6,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixels(image: &Image) -> &[u8] {
        image.data.as_deref().expect("procedural image has data")
    }

    fn assert_repeat_srgb(image: &Image) {
        assert_eq!(image.texture_descriptor.size.width, TEX_SIZE);
        assert_eq!(image.texture_descriptor.size.height, TEX_SIZE);
        assert_eq!(
            image.texture_descriptor.format,
            TextureFormat::Rgba8UnormSrgb
        );
        assert_eq!(pixels(image).len(), (TEX_SIZE * TEX_SIZE * 4) as usize);
        assert!(pixels(image).chunks_exact(4).all(|pixel| pixel[3] == 255));
        let ImageSampler::Descriptor(sampler) = &image.sampler else {
            panic!("procedural texture must have an explicit sampler")
        };
        assert_eq!(sampler.address_mode_u, ImageAddressMode::Repeat);
        assert_eq!(sampler.address_mode_v, ImageAddressMode::Repeat);
    }

    #[test]
    fn organic_texture_generation_is_deterministic() {
        assert_eq!(pixels(&grass_texture()), pixels(&grass_texture()));
        for variant in 0..FOLIAGE_VARIANTS {
            assert_eq!(
                pixels(&foliage_texture(variant)),
                pixels(&foliage_texture(variant))
            );
        }
        for variant in 0..HAY_VARIANTS {
            assert_eq!(pixels(&hay_texture(variant)), pixels(&hay_texture(variant)));
        }
    }

    #[test]
    fn new_color_textures_are_webgl2_repeat_rgba8_srgb() {
        assert_repeat_srgb(&grass_texture());
        assert_repeat_srgb(&sidewalk_texture());
        for variant in 0..FOLIAGE_VARIANTS {
            assert_repeat_srgb(&foliage_texture(variant));
        }
        for variant in 0..HAY_VARIANTS {
            assert_repeat_srgb(&hay_texture(variant));
        }
    }

    #[test]
    fn cached_organic_variants_are_visibly_distinct() {
        let foliage = std::array::from_fn::<_, FOLIAGE_VARIANTS, _>(|i| foliage_texture(i));
        for a in 0..FOLIAGE_VARIANTS {
            for b in (a + 1)..FOLIAGE_VARIANTS {
                assert_ne!(pixels(&foliage[a]), pixels(&foliage[b]));
            }
        }
        assert_ne!(pixels(&hay_texture(0)), pixels(&hay_texture(1)));
    }

    #[test]
    fn procedural_pixel_functions_have_exact_tile_periods() {
        for y in 0..TEX_SIZE {
            for x in 0..TEX_SIZE {
                let grass = grass_pixel(x, y);
                assert_eq!(grass, grass_pixel(x + TEX_SIZE, y));
                assert_eq!(grass, grass_pixel(x, y + TEX_SIZE));

                let sidewalk = sidewalk_pixel(x, y);
                assert_eq!(sidewalk, sidewalk_pixel(x + TEX_SIZE, y));
                assert_eq!(sidewalk, sidewalk_pixel(x, y + TEX_SIZE));

                for variant in 0..FOLIAGE_VARIANTS {
                    let leaf = foliage_pixel(variant, x, y);
                    assert_eq!(leaf, foliage_pixel(variant, x + TEX_SIZE, y));
                    assert_eq!(leaf, foliage_pixel(variant, x, y + TEX_SIZE));
                }
                for variant in 0..HAY_VARIANTS {
                    let straw = hay_pixel(variant, x, y);
                    assert_eq!(straw, hay_pixel(variant, x + TEX_SIZE, y));
                    assert_eq!(straw, hay_pixel(variant, x, y + TEX_SIZE));
                }
            }
        }
    }

    #[test]
    fn texture_resource_caches_five_distinct_organic_materials() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<TextureAssets>();

        let textures = app.world().resource::<TextureAssets>();
        let handles = textures
            .foliage
            .iter()
            .chain(textures.hay.iter())
            .collect::<Vec<_>>();
        for a in 0..handles.len() {
            for b in (a + 1)..handles.len() {
                assert_ne!(handles[a].id(), handles[b].id());
            }
        }

        let materials = app.world().resource::<Assets<StandardMaterial>>();
        let image_ids = handles
            .iter()
            .map(|handle| {
                materials
                    .get(*handle)
                    .and_then(|material| material.base_color_texture.as_ref())
                    .expect("cached organic material has a color texture")
                    .id()
            })
            .collect::<Vec<_>>();
        for a in 0..image_ids.len() {
            for b in (a + 1)..image_ids.len() {
                assert_ne!(image_ids[a], image_ids[b]);
            }
        }
    }

    #[test]
    fn sidewalk_uses_long_staggered_slab_period() {
        assert!(SIDEWALK_SLAB_WIDTH > 16);
        assert!(SIDEWALK_SLAB_HEIGHT >= 16);
        assert_eq!(TEX_SIZE % SIDEWALK_SLAB_WIDTH, 0);
        assert_eq!(TEX_SIZE % SIDEWALK_SLAB_HEIGHT, 0);

        // Every horizontal slab boundary is a seam. Vertical seams alternate
        // by half a slab, avoiding the old small 4x4 checker/grid repetition.
        assert!(sidewalk_is_joint(7, 0));
        assert!(sidewalk_is_joint(0, 7));
        assert!(!sidewalk_is_joint(0, SIDEWALK_SLAB_HEIGHT + 7));
        assert!(sidewalk_is_joint(
            SIDEWALK_SLAB_WIDTH / 2,
            SIDEWALK_SLAB_HEIGHT + 7
        ));
    }
}
