//! Procedural textures generated entirely in code (no asset files).
//!
//! `TexturesPlugin` runs a `Startup` system that builds RGBA pixel data for
//! grass, road, sidewalk, biome ground surfaces, foliage, hay, and car paint,
//! wraps each image in a repeating sampler, and stores `StandardMaterial` handles in
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
pub const GROUND_VARIANTS: usize = 2;

const GROUND_FLECK_CELL_SIZE: u32 = 4;
const ORCHARD_ROW_PERIOD: u32 = 16;
const FIELD_FURROW_PERIOD: u32 = 16;

// Grass structure uses periods that divide the 64px tile. Flecks use 4px
// candidate cells, but only a small hash-selected subset receives a fleck.
const GRASS_MOTTLE_SCALE: u32 = 16;
const GRASS_MOWING_BAND_HEIGHT: u32 = 16;
const GRASS_FLECK_CELL_SIZE: u32 = 4;

// A 64px tile contains only two long slabs across. Rows are staggered by half
// a slab, and all dimensions divide TEX_SIZE to preserve the exact period.
const SIDEWALK_SLAB_WIDTH: u32 = 32;
const SIDEWALK_SLAB_HEIGHT: u32 = 16;
const SIDEWALK_UV_REPEAT: f32 = 1.75;
const SIDEWALK_JOINT_DARKENING: i32 = 10;

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
/// - `park_ground`, `orchard_ground`, `field_ground` → two cached biome variants each
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
    #[allow(dead_code)]
    pub park_ground: [Handle<StandardMaterial>; GROUND_VARIANTS],
    #[allow(dead_code)]
    pub orchard_ground: [Handle<StandardMaterial>; GROUND_VARIANTS],
    #[allow(dead_code)]
    pub field_ground: [Handle<StandardMaterial>; GROUND_VARIANTS],
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
            let (
                grass,
                road,
                sidewalk,
                foliage,
                hay,
                park_ground,
                orchard_ground,
                field_ground,
                car_paint,
            ) = {
                let mut materials = world.resource_mut::<Assets<StandardMaterial>>();

                // --- Procedural normal maps ---
                // Road/asphalt gets a gravelly normal map (stronger) so the surface
                // catches light per-pixel instead of looking like flat paint.
                let road_normal = images.add(asphalt_normal_map());
                // Grass gets gentle blade bumps for natural light scatter.
                let grass_normal = images.add(grass_normal_map());
                // Sidewalk gets a bumpy normal for concrete texture.
                let sidewalk_normal = images.add(concrete_normal_map());

                // GRASS — subtle deterministic mottle, mowing bands, and sparse
                // color flecks; tile 16x with a matte blade-bump normal map.
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
                    uv_transform: Affine2::from_scale(Vec2::splat(SIDEWALK_UV_REPEAT)),
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

                let park_ground = std::array::from_fn(|variant| {
                    materials.add(StandardMaterial {
                        base_color: Color::WHITE,
                        base_color_texture: Some(images.add(park_ground_texture(variant))),
                        normal_map_texture: Some(images.add(park_ground_normal_map(variant))),
                        perceptual_roughness: 0.90,
                        metallic: 0.0,
                        uv_transform: Affine2::from_scale(Vec2::splat(10.0)),
                        ..default()
                    })
                });
                let orchard_ground = std::array::from_fn(|variant| {
                    materials.add(StandardMaterial {
                        base_color: Color::WHITE,
                        base_color_texture: Some(images.add(orchard_ground_texture(variant))),
                        normal_map_texture: Some(images.add(orchard_ground_normal_map(variant))),
                        perceptual_roughness: 0.93,
                        metallic: 0.0,
                        uv_transform: Affine2::from_scale(Vec2::splat(8.0)),
                        ..default()
                    })
                });
                let field_ground = std::array::from_fn(|variant| {
                    materials.add(StandardMaterial {
                        base_color: Color::WHITE,
                        base_color_texture: Some(images.add(field_ground_texture(variant))),
                        normal_map_texture: Some(images.add(field_ground_normal_map(variant))),
                        perceptual_roughness: 0.96,
                        metallic: 0.0,
                        uv_transform: Affine2::from_scale(Vec2::splat(6.0)),
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

                (
                    grass,
                    road,
                    sidewalk,
                    foliage,
                    hay,
                    park_ground,
                    orchard_ground,
                    field_ground,
                    car_paint,
                )
            };

            TextureAssets {
                grass,
                road,
                sidewalk,
                foliage,
                hay,
                park_ground,
                orchard_ground,
                field_ground,
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

/// Grass: restrained tileable mottle plus sparse, jittered color flecks.
/// Every structural scale divides 64, and `grass_pixel` wraps incoming
/// coordinates before hashing so callers see an exact deterministic period.
fn grass_texture() -> Image {
    make_image(grass_pixel)
}

/// Return 0 for ordinary grass, 1 for a cool-green fleck, and 2 for a muted
/// straw-green fleck. A 4x4 cell is only a candidate: seven eighths of cells
/// are empty, removing the old guaranteed, regularly spaced blade stamp.
fn grass_fleck_kind(x: u32, y: u32) -> u8 {
    let x = x % TEX_SIZE;
    let y = y % TEX_SIZE;
    let cell_x = x / GRASS_FLECK_CELL_SIZE;
    let cell_y = y / GRASS_FLECK_CELL_SIZE;
    let fleck_hash = noise2(cell_x, cell_y);

    if (fleck_hash >> 8) & 7 != 0 {
        return 0;
    }

    let origin_x = (fleck_hash >> 16) % GRASS_FLECK_CELL_SIZE;
    let origin_y = (fleck_hash >> 18) % GRASS_FLECK_CELL_SIZE;
    if x % GRASS_FLECK_CELL_SIZE != origin_x || y % GRASS_FLECK_CELL_SIZE != origin_y {
        return 0;
    }

    1 + ((fleck_hash >> 28) & 1) as u8
}

fn grass_pixel(x: u32, y: u32) -> [u8; 4] {
    let x = x % TEX_SIZE;
    let y = y % TEX_SIZE;
    let b = srgb_base(GRASS_SRGB);

    // Broad 16px clumps and a 32px two-band mowing cycle keep the pattern
    // quiet at the material's repeat scale. Fine grain prevents airbrushing.
    let mottle = signed_noise2(x / GRASS_MOTTLE_SCALE, y / GRASS_MOTTLE_SCALE) * 8 / 128;
    let fine = signed_noise(x, y) * 4 / 128;
    let stripe = if (y / GRASS_MOWING_BAND_HEIGHT) % 2 == 0 {
        2
    } else {
        -2
    };

    // Flecks retain two subtle color identities rather than becoming uniform
    // bright dots: cool new growth and a restrained warm/straw note.
    let fleck = match grass_fleck_kind(x, y) {
        1 => [-1, 6, 1],
        2 => [5, 3, -2],
        _ => [0, 0, 0],
    };

    [
        clamp_byte(b[0] + mottle / 2 + fine / 2 + stripe / 2 + fleck[0]),
        clamp_byte(b[1] + mottle + fine + stripe + fleck[1]),
        clamp_byte(b[2] + mottle / 3 + fine / 2 + stripe / 3 + fleck[2]),
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
    let slab_tone = signed_noise2(slab_x, row) * 5 / 128;
    let grain = signed_noise(x, y) * 6 / 128;
    let brushed = if (x + 2 * y) % 11 == 0 { 2 } else { 0 };
    // Expansion joints remain readable without becoming a near-black grid.
    let joint = if sidewalk_is_joint(x, y) {
        -SIDEWALK_JOINT_DARKENING
    } else {
        0
    };
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
// Biome ground surfaces
// ---------------------------------------------------------------------------

/// Smooth explicitly wrapped noise; spacing divides 64, so interpolated
/// structure crosses the repeat boundary without a hard seam.
fn tileable_value_noise(x: u32, y: u32, spacing: u32, seed: u32) -> f32 {
    let cells = TEX_SIZE / spacing;
    let x = x % TEX_SIZE;
    let y = y % TEX_SIZE;
    let gx = x / spacing;
    let gy = y / spacing;
    let smooth = |v: f32| v * v * (3.0 - 2.0 * v);
    let sx = smooth((x % spacing) as f32 / spacing as f32);
    let sy = smooth((y % spacing) as f32 / spacing as f32);
    let sample = |cx: u32, cy: u32| {
        let h = noise2(
            (cx % cells).wrapping_add(seed * 17),
            (cy % cells).wrapping_add(seed * 31),
        );
        ((h >> 8) & 0xff) as f32 / 127.5 - 1.0
    };
    let a = sample(gx, gy) * (1.0 - sx) + sample(gx + 1, gy) * sx;
    let b = sample(gx, gy + 1) * (1.0 - sx) + sample(gx + 1, gy + 1) * sx;
    a * (1.0 - sy) + b * sy
}

fn park_ground_texture(variant: usize) -> Image {
    assert!(variant < GROUND_VARIANTS);
    make_image(move |x, y| park_ground_pixel(variant, x, y))
}

/// 0=turf, 1=soft three-pixel clover, 2=single warm leaf fleck.
fn park_ground_fleck_kind(variant: usize, x: u32, y: u32) -> u8 {
    let x = x % TEX_SIZE;
    let y = y % TEX_SIZE;
    let h = noise2(
        x / GROUND_FLECK_CELL_SIZE + variant as u32 * 37,
        y / GROUND_FLECK_CELL_SIZE + 83,
    );
    if h & 7 != 0 {
        return 0;
    }
    let cx = 1 + ((h >> 8) & 1);
    let cy = 1 + ((h >> 9) & 1);
    let lx = x % GROUND_FLECK_CELL_SIZE;
    let ly = y % GROUND_FLECK_CELL_SIZE;
    let kind = 1 + ((h >> 17) & 1) as u8;
    let clover_lobe = kind == 1 && ((lx + 1 == cx && ly == cy) || (lx == cx && ly + 1 == cy));
    if (lx == cx && ly == cy) || clover_lobe {
        kind
    } else {
        0
    }
}

fn park_ground_pixel(variant: usize, x: u32, y: u32) -> [u8; 4] {
    let x = x % TEX_SIZE;
    let y = y % TEX_SIZE;
    let b = [[103, 157, 76], [96, 151, 70]][variant];
    let soft = (tileable_value_noise(x, y, 16, 3 + variant as u32) * 9.0).round() as i32;
    let grain = signed_noise(x + variant as u32 * 11, y) * 3 / 128;
    let fleck = match park_ground_fleck_kind(variant, x, y) {
        1 => [4, 12, 1],
        2 => [12, 7, -3],
        _ => [0, 0, 0],
    };
    [
        clamp_byte(b[0] + soft / 2 + grain + fleck[0]),
        clamp_byte(b[1] + soft + grain + fleck[1]),
        clamp_byte(b[2] + soft / 3 + grain + fleck[2]),
        255,
    ]
}

fn orchard_ground_texture(variant: usize) -> Image {
    assert!(variant < GROUND_VARIANTS);
    make_image(move |x, y| orchard_ground_pixel(variant, x, y))
}

fn orchard_ground_pixel(variant: usize, x: u32, y: u32) -> [u8; 4] {
    let x = x % TEX_SIZE;
    let y = y % TEX_SIZE;
    let b = [[105, 108, 57], [98, 103, 52]][variant];
    let (along, across) = if variant == 0 { (x, y) } else { (y, x) };
    let phase = across % ORCHARD_ROW_PERIOD;
    let row = ((phase as f32 / ORCHARD_ROW_PERIOD as f32 * std::f32::consts::TAU).cos() * 4.0)
        .round() as i32;
    let soil = (tileable_value_noise(x, y, 16, 11 + variant as u32) * 7.0).round() as i32;
    let grain = signed_noise2(x + 29, y + variant as u32 * 43) * 4 / 128;
    let mulch = if noise(along + variant as u32 * 17, across) & 0x3f == 0 {
        -10
    } else {
        0
    };
    let leaf = if noise2(x + 101, y + variant as u32 * 53) & 0xff == 0 {
        [13, 5, -9]
    } else {
        [0, 0, 0]
    };
    [
        clamp_byte(b[0] + row + soil + grain + mulch + leaf[0]),
        clamp_byte(b[1] + row + soil + grain + mulch + leaf[1]),
        clamp_byte(b[2] + row / 2 + soil / 2 + grain + mulch / 2 + leaf[2]),
        255,
    ]
}

fn field_ground_texture(variant: usize) -> Image {
    assert!(variant < GROUND_VARIANTS);
    make_image(move |x, y| field_ground_pixel(variant, x, y))
}

fn field_ground_pixel(variant: usize, x: u32, y: u32) -> [u8; 4] {
    let x = x % TEX_SIZE;
    let y = y % TEX_SIZE;
    let b = [[174, 126, 58], [166, 117, 51]][variant];
    let (along, across) = if variant == 0 { (x, y) } else { (y, x) };
    let phase = across % FIELD_FURROW_PERIOD;
    let furrow = ((phase as f32 / FIELD_FURROW_PERIOD as f32 * std::f32::consts::TAU).cos() * 7.0)
        .round() as i32;
    let earth = (tileable_value_noise(x, y, 16, 19 + variant as u32) * 6.0).round() as i32;
    let grain = signed_noise(along * 3 + variant as u32 * 31, across) * 5 / 128;
    let h = noise2(across + variant as u32 * 47, along / 4);
    let straw = if across % 8 == ((h >> 5) & 7) && along.wrapping_add(h >> 11) % 16 < 3 {
        11
    } else {
        0
    };
    [
        clamp_byte(b[0] + furrow + earth + grain + straw),
        clamp_byte(b[1] + furrow * 3 / 4 + earth + grain + straw),
        clamp_byte(b[2] + furrow / 3 + earth / 2 + grain / 2 + straw / 3),
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

fn park_ground_normal_map(variant: usize) -> Image {
    make_normal_map(
        move |x, y| {
            tileable_value_noise(x, y, 8, 31 + variant as u32) * 0.7
                + signed_noise2(x * 3 + variant as u32 * 13, y * 3) as f32 / 128.0 * 0.12
        },
        0.38,
    )
}

fn orchard_ground_normal_map(variant: usize) -> Image {
    make_normal_map(
        move |x, y| {
            let across = if variant == 0 { y } else { x };
            let row = ((across % ORCHARD_ROW_PERIOD) as f32 / ORCHARD_ROW_PERIOD as f32
                * std::f32::consts::TAU)
                .cos()
                * 0.22;
            row + tileable_value_noise(x, y, 8, 41 + variant as u32) * 0.55
        },
        0.42,
    )
}

fn field_ground_normal_map(variant: usize) -> Image {
    make_normal_map(
        move |x, y| {
            let across = if variant == 0 { y } else { x };
            let furrow = ((across % FIELD_FURROW_PERIOD) as f32 / FIELD_FURROW_PERIOD as f32
                * std::f32::consts::TAU)
                .cos()
                * 0.55;
            furrow + tileable_value_noise(x, y, 8, 51 + variant as u32) * 0.25
        },
        0.5,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixels(image: &Image) -> &[u8] {
        image.data.as_deref().expect("procedural image has data")
    }

    fn assert_repeat_format(image: &Image, format: TextureFormat) {
        assert_eq!(image.texture_descriptor.size.width, TEX_SIZE);
        assert_eq!(image.texture_descriptor.size.height, TEX_SIZE);
        assert_eq!(image.texture_descriptor.format, format);
        assert_eq!(pixels(image).len(), (TEX_SIZE * TEX_SIZE * 4) as usize);
        assert!(pixels(image).chunks_exact(4).all(|pixel| pixel[3] == 255));
        let ImageSampler::Descriptor(sampler) = &image.sampler else {
            panic!("procedural texture must have an explicit sampler")
        };
        assert_eq!(sampler.address_mode_u, ImageAddressMode::Repeat);
        assert_eq!(sampler.address_mode_v, ImageAddressMode::Repeat);
    }

    fn assert_repeat_srgb(image: &Image) {
        assert_repeat_format(image, TextureFormat::Rgba8UnormSrgb);
    }

    fn color_mean(image: &Image) -> [f32; 3] {
        let count = (TEX_SIZE * TEX_SIZE) as f32;
        let mut sum = [0u32; 3];
        for pixel in pixels(image).chunks_exact(4) {
            for channel in 0..3 {
                sum[channel] += pixel[channel] as u32;
            }
        }
        sum.map(|channel| channel as f32 / count)
    }

    fn max_opposite_edge_delta(image: &Image) -> u8 {
        let data = pixels(image);
        let pixel = |x: u32, y: u32| &data[((y * TEX_SIZE + x) * 4) as usize..][..3];
        let mut maximum = 0;
        for position in 0..TEX_SIZE {
            for (a, b) in [
                (pixel(0, position), pixel(TEX_SIZE - 1, position)),
                (pixel(position, 0), pixel(position, TEX_SIZE - 1)),
            ] {
                for channel in 0..3 {
                    maximum = maximum.max(a[channel].abs_diff(b[channel]));
                }
            }
        }
        maximum
    }

    fn luminance(pixel: [u8; 4]) -> u16 {
        // Integer Rec. 709 weights, scaled by 256.
        (54 * pixel[0] as u16 + 183 * pixel[1] as u16 + 19 * pixel[2] as u16) / 256
    }

    #[test]
    fn organic_texture_generation_is_deterministic() {
        assert_eq!(pixels(&grass_texture()), pixels(&grass_texture()));
        assert_eq!(pixels(&sidewalk_texture()), pixels(&sidewalk_texture()));
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
    fn ground_images_and_normals_are_deterministic_webgl2_repeat_textures() {
        for variant in 0..GROUND_VARIANTS {
            let colors = [
                park_ground_texture(variant),
                orchard_ground_texture(variant),
                field_ground_texture(variant),
            ];
            let colors_again = [
                park_ground_texture(variant),
                orchard_ground_texture(variant),
                field_ground_texture(variant),
            ];
            let normals = [
                park_ground_normal_map(variant),
                orchard_ground_normal_map(variant),
                field_ground_normal_map(variant),
            ];
            let normals_again = [
                park_ground_normal_map(variant),
                orchard_ground_normal_map(variant),
                field_ground_normal_map(variant),
            ];
            for index in 0..colors.len() {
                assert_repeat_format(&colors[index], TextureFormat::Rgba8UnormSrgb);
                assert_repeat_format(&normals[index], TextureFormat::Rgba8Unorm);
                assert_eq!(pixels(&colors[index]), pixels(&colors_again[index]));
                assert_eq!(pixels(&normals[index]), pixels(&normals_again[index]));
                // Tangent-space normals point outward and remain subtle.
                assert!(
                    pixels(&normals[index])
                        .chunks_exact(4)
                        .all(|pixel| pixel[2] >= 220)
                );
            }
        }
    }

    #[test]
    fn ground_variants_are_distinct_and_have_the_requested_palette_order() {
        let park = std::array::from_fn::<_, GROUND_VARIANTS, _>(park_ground_texture);
        let orchard = std::array::from_fn::<_, GROUND_VARIANTS, _>(orchard_ground_texture);
        let field = std::array::from_fn::<_, GROUND_VARIANTS, _>(field_ground_texture);
        for family in [&park, &orchard, &field] {
            assert_ne!(pixels(&family[0]), pixels(&family[1]));
        }
        for variant in 0..GROUND_VARIANTS {
            let p = color_mean(&park[variant]);
            let o = color_mean(&orchard[variant]);
            let f = color_mean(&field[variant]);
            assert!(p[1] > p[0] && p[0] > p[2], "park is bright green");
            assert!(o[1] > o[2] + 35.0 && o[0] > o[2] + 35.0, "orchard is olive");
            assert!(f[0] > f[1] + 35.0 && f[1] > f[2] + 45.0, "field is ochre");
            let mean_luminance = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
            assert!(mean_luminance(p) > mean_luminance(o) + 20.0);
        }
    }

    #[test]
    fn ground_contrast_and_repeat_edges_are_bounded() {
        for variant in 0..GROUND_VARIANTS {
            for image in [
                park_ground_texture(variant),
                orchard_ground_texture(variant),
                field_ground_texture(variant),
            ] {
                let luminances = pixels(&image)
                    .chunks_exact(4)
                    .map(|p| luminance([p[0], p[1], p[2], p[3]]))
                    .collect::<Vec<_>>();
                let range = luminances.iter().max().unwrap() - luminances.iter().min().unwrap();
                assert!(
                    range <= 42,
                    "ground contrast should remain restrained: {range}"
                );
                assert!(
                    max_opposite_edge_delta(&image) <= 32,
                    "opposite repeat edges must remain filter-compatible"
                );
            }
        }
    }

    #[test]
    fn park_clover_and_leaf_flecks_are_sparse() {
        assert_eq!(TEX_SIZE % GROUND_FLECK_CELL_SIZE, 0);
        for variant in 0..GROUND_VARIANTS {
            let mut counts = [0usize; 3];
            for y in 0..TEX_SIZE {
                for x in 0..TEX_SIZE {
                    counts[park_ground_fleck_kind(variant, x, y) as usize] += 1;
                    assert_eq!(
                        park_ground_fleck_kind(variant, x, y),
                        park_ground_fleck_kind(variant, x + TEX_SIZE, y + TEX_SIZE)
                    );
                }
            }
            let flecks = counts[1] + counts[2];
            assert!(counts[1] > 0 && counts[2] > 0);
            assert!(flecks > 20 && flecks < (TEX_SIZE * TEX_SIZE / 12) as usize);
        }
    }

    #[test]
    fn grass_flecks_are_sparse_jittered_and_keep_both_color_identities() {
        assert_eq!(TEX_SIZE % GRASS_MOTTLE_SCALE, 0);
        assert_eq!(TEX_SIZE % GRASS_MOWING_BAND_HEIGHT, 0);
        assert_eq!(TEX_SIZE % GRASS_FLECK_CELL_SIZE, 0);

        let mut counts = [0usize; 3];
        let mut occupied_cells = 0usize;
        let cells_per_axis = TEX_SIZE / GRASS_FLECK_CELL_SIZE;
        for cell_y in 0..cells_per_axis {
            for cell_x in 0..cells_per_axis {
                let mut cell_flecks = 0;
                for local_y in 0..GRASS_FLECK_CELL_SIZE {
                    for local_x in 0..GRASS_FLECK_CELL_SIZE {
                        let x = cell_x * GRASS_FLECK_CELL_SIZE + local_x;
                        let y = cell_y * GRASS_FLECK_CELL_SIZE + local_y;
                        let kind = grass_fleck_kind(x, y) as usize;
                        counts[kind] += 1;
                        cell_flecks += usize::from(kind != 0);

                        assert_eq!(grass_fleck_kind(x, y), grass_fleck_kind(x + TEX_SIZE, y));
                        assert_eq!(grass_fleck_kind(x, y), grass_fleck_kind(x, y + TEX_SIZE));
                    }
                }
                assert!(cell_flecks <= 1, "a candidate cell holds at most one fleck");
                occupied_cells += usize::from(cell_flecks != 0);
            }
        }

        let cell_count = (cells_per_axis * cells_per_axis) as usize;
        assert!(occupied_cells > 0 && occupied_cells < cell_count / 4);
        assert!(counts[1] > 0 && counts[2] > 0);
        assert_eq!(counts[1] + counts[2], occupied_cells);
    }

    #[test]
    fn polished_surface_luminance_is_bounded() {
        let grass_luminance = (0..TEX_SIZE)
            .flat_map(|y| (0..TEX_SIZE).map(move |x| luminance(grass_pixel(x, y))))
            .collect::<Vec<_>>();
        assert!(grass_luminance.iter().copied().min().unwrap() >= 120);
        assert!(grass_luminance.iter().copied().max().unwrap() <= 145);

        let sidewalk_luminance = (0..TEX_SIZE)
            .flat_map(|y| (0..TEX_SIZE).map(move |x| luminance(sidewalk_pixel(x, y))))
            .collect::<Vec<_>>();
        assert!(sidewalk_luminance.iter().copied().min().unwrap() >= 160);
        assert!(sidewalk_luminance.iter().copied().max().unwrap() <= 195);
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
                for variant in 0..GROUND_VARIANTS {
                    for pixel_fn in [park_ground_pixel, orchard_ground_pixel, field_ground_pixel] {
                        let ground = pixel_fn(variant, x, y);
                        assert_eq!(ground, pixel_fn(variant, x + TEX_SIZE, y));
                        assert_eq!(ground, pixel_fn(variant, x, y + TEX_SIZE));
                    }
                }
            }
        }
    }

    #[test]
    fn texture_resource_caches_distinct_material_and_image_variants() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<TextureAssets>();

        let textures = app.world().resource::<TextureAssets>();
        assert_ne!(textures.grass.id(), textures.road.id());
        assert_ne!(textures.grass.id(), textures.sidewalk.id());
        assert_ne!(textures.road.id(), textures.sidewalk.id());

        let surface_handles = [&textures.grass, &textures.road, &textures.sidewalk];
        let materials = app.world().resource::<Assets<StandardMaterial>>();
        let surface_image_ids = surface_handles.map(|handle| {
            materials
                .get(handle)
                .and_then(|material| material.base_color_texture.as_ref())
                .expect("cached surface material has a color texture")
                .id()
        });
        assert_ne!(surface_image_ids[0], surface_image_ids[1]);
        assert_ne!(surface_image_ids[0], surface_image_ids[2]);
        assert_ne!(surface_image_ids[1], surface_image_ids[2]);

        let sidewalk = materials
            .get(&textures.sidewalk)
            .expect("cached sidewalk material exists");
        assert_eq!(
            sidewalk.uv_transform,
            Affine2::from_scale(Vec2::splat(SIDEWALK_UV_REPEAT))
        );

        let handles = textures
            .foliage
            .iter()
            .chain(textures.hay.iter())
            .chain(textures.park_ground.iter())
            .chain(textures.orchard_ground.iter())
            .chain(textures.field_ground.iter())
            .collect::<Vec<_>>();
        for a in 0..handles.len() {
            for b in (a + 1)..handles.len() {
                assert_ne!(handles[a].id(), handles[b].id());
            }
        }

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

        for (family, roughness) in [
            (&textures.park_ground, 0.90),
            (&textures.orchard_ground, 0.93),
            (&textures.field_ground, 0.96),
        ] {
            for handle in family {
                let material = materials
                    .get(handle)
                    .expect("cached ground material exists");
                assert_eq!(material.perceptual_roughness, roughness);
                assert_eq!(material.metallic, 0.0);
                assert!(material.normal_map_texture.is_some());
            }
        }
    }

    #[test]
    fn sidewalk_uses_long_staggered_slab_period() {
        assert!(SIDEWALK_SLAB_WIDTH > 16);
        assert!(SIDEWALK_SLAB_HEIGHT >= 16);
        assert_eq!(TEX_SIZE % SIDEWALK_SLAB_WIDTH, 0);
        assert_eq!(TEX_SIZE % SIDEWALK_SLAB_HEIGHT, 0);
        assert!((1.5..=2.0).contains(&SIDEWALK_UV_REPEAT));
        assert!(SIDEWALK_JOINT_DARKENING <= 10);

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
