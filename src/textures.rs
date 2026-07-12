//! Procedural textures generated entirely in code (no asset files).
//!
//! `TexturesPlugin` runs a `Startup` system that builds RGBA pixel data for
//! grass, road, sidewalk, and car paint, wraps each in a repeating `Image`,
//! and stores ready-to-use `StandardMaterial` handles in the `TextureAssets`
//! resource so other systems can apply them with a simple `clone()`.
//!
//! T15: realistic PBR metallic car paint (high metallic + low roughness +
//! fine orange-peel/metal-flake normal map for shimmer under IBL + bloom),
//! richer grass (multi-tone green + yellow/brown patches + mowing stripes),
//! richer asphalt (gravel grain), and improved sidewalk concrete. All color
//! textures tile seamlessly (Repeat sampler + wrapping noise indices). All
//! normal maps are linear `Rgba8Unorm` (lighting breaks if stored sRGB).

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

// Base sRGB values matching crate::palette constants (see palette.rs).
const GRASS_SRGB: [f32; 3] = [0.30, 0.60, 0.30]; // palette::GRASS_LIGHT
const ASPHALT_SRGB: [f32; 3] = [0.13, 0.13, 0.14]; // palette::ASPHALT
const CONCRETE_SRGB: [f32; 3] = [0.72, 0.71, 0.68]; // palette::CONCRETE
const CAR_BODY_SRGB: [f32; 3] = [0.90, 0.10, 0.10]; // palette::CAR_BODY

/// Ready-to-use textured `StandardMaterial` handles, inserted as a resource.
///
/// Field names (for orchestrator wiring):
/// - `grass`      → grass ground plane
/// - `road`       → asphalt road strip
/// - `sidewalk`   → concrete sidewalk curbs
/// - `car_paint`  → car body paint
#[derive(Resource)]
pub struct TextureAssets {
    pub grass: Handle<StandardMaterial>,
    pub road: Handle<StandardMaterial>,
    pub sidewalk: Handle<StandardMaterial>,
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
            let mut materials = world.resource_mut::<Assets<StandardMaterial>>();

        // --- Procedural normal maps ---
        // Road/asphalt gets a gravelly normal map (stronger) so the surface
        // catches light per-pixel instead of looking like flat paint.
        let road_normal = images.add(asphalt_normal_map());
        // Car paint gets a fine orange-peel / metal-flake normal map: subtle
        // high-frequency bumps that shimmer under IBL + bloom without
        // reintroducing the crawling sparkle (the base_color_texture stays
        // smooth — only the normal map carries the micro-surface detail).
        let car_normal = images.add(orange_peel_normal_map());
        // Grass gets gentle blade bumps for natural light scatter.
        let grass_normal = images.add(grass_normal_map());
        // Sidewalk gets a bumpy normal for concrete texture.
        let sidewalk_normal = images.add(concrete_normal_map());

        // GRASS — multi-tone green + subtle yellow/brown patches + faint
        // mowing stripes; tile 16×. T15: richer natural variation, fully
        // rough (matte) with a blade-bump normal map for natural scatter.
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

        // SIDEWALK — concrete with a subtle checker + noise + expansion-joint
        // lines; tile 6×. T15: minor enrichment (joint lines + slightly
        // stronger noise), rough concrete with a bumpy normal map.
        let sidewalk = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: Some(images.add(sidewalk_texture())),
            normal_map_texture: Some(sidewalk_normal),
            perceptual_roughness: 0.88,
            metallic: 0.0,
            uv_transform: Affine2::from_scale(Vec2::splat(6.0)),
            ..default()
        });

        // CAR_PAINT — realistic metallic car paint. T15: high metallic
        // (0.8) + low roughness (0.18) for a glossy clearcoat that reflects
        // the IBL environment map; a fine orange-peel/metal-flake NORMAL
        // map provides the micro-surface shimmer under bloom. The
        // base_color_texture stays SMOOTH (no high-frequency sparkle — that
        // reads as a crawling pattern as the car moves; the micro-detail
        // lives in the normal map, which stays fixed to the body and
        // shimmers via reflections instead of crawling). Tile 1× (no
        // visible repeat). `emissive` stays off (the body isn't a light).
        let car_paint = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: Some(images.add(car_paint_texture())),
            normal_map_texture: Some(car_normal),
            // Car-paint metallic: 0.85 (mostly metal) + very low roughness
            // (0.12) so the procedural sun-disc env map + directional sun
            // produce a sharp bright reflection/glint — the metallic cue.
            // The sun-disc cubemap (shaders.rs) gives metal something bright
            // to reflect; without a detailed env map a high-metallic surface
            // just looks dark/flat.
            metallic: 0.85,
            perceptual_roughness: 0.12,
            uv_transform: Affine2::from_scale(Vec2::splat(1.0)),
            ..default()
        });

        TextureAssets {
            grass,
            road,
            sidewalk,
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

/// Grass: multi-tone green base with finer noise, subtle yellow/brown
/// patches, and a faint mowing-stripe banding for a richer natural look.
/// Tiles seamlessly (wrapping noise indices + stripe period divides TEX_SIZE).
fn grass_texture() -> Image {
    let b = srgb_base(GRASS_SRGB);
    // Stripe period must divide TEX_SIZE (64) so the texture tiles seamlessly.
    // 8px stripes → 8 bands across the texture, faint enough to read as mowing.
    const STRIPE_PERIOD: u32 = 8;
    make_image(move |x, y| {
        // Two octaves of noise: low-frequency for patches, high for fine grain.
        let fine = signed_noise(x * 2, y * 2) * 10 / 128;
        let coarse = signed_noise2(x / 3, y / 3) * 28 / 128;

        // Yellow/brown dry patches: where a third noise channel is high,
        // shift toward yellow-brown (more red, less green/blue).
        let dry = signed_noise(x / 5 + 17, y / 5 + 31) * 30 / 128; // -30..30ish
        let dry_amt = ((dry + 30).clamp(0, 60) as f32) / 60.0; // 0..1 bias toward dry

        // Faint mowing stripe: alternate bands along Y by a few brightness
        // units. Kept subtle so it reads as mowing, not a checkerboard.
        let stripe = if ((y / STRIPE_PERIOD) % 2) == 0 { 6 } else { -6 };

        // Apply variation. Green channel gets the most variation; red/blue
        // get less so the hue stays green-dominant. Dry patches push
        // red up and green/blue down (toward yellow-brown).
        let r = b[0] + fine / 2 + coarse / 2 + (dry_amt * 22.0) as i32 + stripe / 2;
        let g = b[1] + fine + coarse - (dry_amt * 18.0) as i32 + stripe;
        let bl = b[2] + fine / 2 + coarse / 2 - (dry_amt * 14.0) as i32 + stripe / 3;

        [clamp_byte(r), clamp_byte(g), clamp_byte(bl), 255]
    })
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

/// Sidewalk: concrete with a subtle 4×4 checker pattern, per-pixel noise,
/// and faint expansion-joint lines along the cell borders for a minor
/// enrichment over the plain checker.
fn sidewalk_texture() -> Image {
    let b = srgb_base(CONCRETE_SRGB);
    let cell = TEX_SIZE / 4; // 16px cells → 4×4 checker on 64px texture
    make_image(move |x, y| {
        let checker = ((x / cell) + (y / cell)) % 2;
        let cell_off = if checker == 0 { 10 } else { -10 };
        let v = signed_noise(x, y) * 10 / 128;

        // Faint expansion-joint darkening at cell borders (1px grooves).
        let on_joint = x % cell == 0 || y % cell == 0;
        let joint = if on_joint { -18 } else { 0 };

        let r = clamp_byte(b[0] + cell_off + v + joint);
        let g = clamp_byte(b[1] + cell_off + v + joint);
        let bl = clamp_byte(b[2] + cell_off + v + joint);
        [r, g, bl, 255]
    })
}

/// Car paint: smooth glossy red base_color_texture. T15: kept deliberately
/// SMOOTH — no high-frequency sparkle/noise (that reads as a crawling
/// pattern as the car moves because the texels are fixed to the body but
/// the eye sees the high-frequency pattern shift under reflections). The
/// metallic micro-surface shimmer comes from the fine orange-peel NORMAL
/// map (which stays fixed to the body and shimmers via IBL reflections),
/// not from the color texture. Only very gentle low-amplitude variation
/// remains so the paint doesn't look like a flat fill.
fn car_paint_texture() -> Image {
    let b = srgb_base(CAR_BODY_SRGB);
    make_image(move |x, y| {
        // Gentle low-amplitude, low-frequency variation only — smooth paint.
        let v = signed_noise(x / 2, y / 2) * 6 / 128;
        let r = clamp_byte(b[0] + v);
        let g = clamp_byte(b[1] + v / 3);
        let bl = clamp_byte(b[2] + v / 3);
        [r, g, bl, 255]
    })
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
            h[((y * size + x)) as usize] = height(x, y);
        }
    }
    let at = |x: i32, y: i32| -> f32 {
        // Wrap (tileable) indices.
        let x = x.rem_euclid(size as i32) as u32;
        let y = y.rem_euclid(size as i32) as u32;
        h[((y * size + x)) as usize]
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

/// Orange-peel / metal-flake normal map for car paint. T15: fine
/// high-frequency bumps (the metal-flake / orange-peel micro-surface of real
/// automotive paint) that shimmer under IBL + bloom. The bumps are subtle in
/// amplitude (so the clearcoat still reads as glossy, not rough) but
/// high-frequency (so there are many tiny facets for the environment map to
/// catch). Two octaves: a very fine flake layer + a slightly broader
/// orange-peel undulation.
fn orange_peel_normal_map() -> Image {
    make_normal_map(
        |x, y| {
            // Fine metal-flake (high frequency, small amplitude).
            let flake = signed_noise(x * 4, y * 4) as f32 / 128.0 * 0.55;
            // Slightly broader orange-peel undulation.
            let peel = signed_noise2(x * 2, y * 2) as f32 / 128.0 * 0.45;
            flake + peel
        },
        // Modest strength: enough to create micro-facets for shimmer, not
        // enough to make the clearcoat look rough/bumpy.
        0.5,
    )
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
