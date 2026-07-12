//! Procedural textures generated entirely in code (no asset files).
//!
//! `TexturesPlugin` runs a `Startup` system that builds 64×64 RGBA pixel data
//! for grass, road, sidewalk, and car paint, wraps each in a repeating `Image`,
//! and stores ready-to-use `StandardMaterial` handles in the `TextureAssets`
//! resource so other systems can apply them with a simple `clone()`.

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

        // --- T9: procedural normal maps (shared generators) ---
        // Road/asphalt gets a gravelly normal map (stronger) so the surface
        // catches light per-pixel instead of looking like flat paint.
        let road_normal = images.add(asphalt_normal_map());
        // Car paint gets a very subtle orange-peel normal map (nearly flat)
        // for a soft micro-sheen without reintroducing the crawling sparkle.
        let car_normal = images.add(smooth_normal_map(0.25));
        // Grass gets a gentle bumpy normal for natural light scatter.
        let grass_normal = images.add(smooth_normal_map(0.6));
        // Sidewalk gets a subtle bumpy normal for concrete texture.
        let sidewalk_normal = images.add(smooth_normal_map(0.5));

        // GRASS — green base with subtle per-pixel noise; tile 16×.
        // T9: tuned PBR — fully rough (matte), with a gentle bumpy normal map
        // so the surface scatters light naturally instead of looking flat.
        let grass = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: Some(images.add(grass_texture())),
            normal_map_texture: Some(grass_normal),
            perceptual_roughness: 1.0,
            metallic: 0.0,
            uv_transform: Affine2::from_scale(Vec2::splat(16.0)),
            ..default()
        });

        // ROAD — dark asphalt with faint noise + gravel specks; tile 8×.
        // T9: tuned PBR — near-fully rough with a gravelly normal map for
        // per-pixel surface detail (the main visual win from normal mapping).
        let road = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: Some(images.add(road_texture())),
            normal_map_texture: Some(road_normal),
            perceptual_roughness: 0.92,
            metallic: 0.0,
            uv_transform: Affine2::from_scale(Vec2::splat(8.0)),
            ..default()
        });

        // SIDEWALK — concrete with a subtle checker + noise; tile 6×.
        // T9: tuned PBR — rough concrete with a bumpy normal map.
        let sidewalk = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: Some(images.add(sidewalk_texture())),
            normal_map_texture: Some(sidewalk_normal),
            perceptual_roughness: 0.88,
            metallic: 0.0,
            uv_transform: Affine2::from_scale(Vec2::splat(6.0)),
            ..default()
        });

        // CAR_PAINT — smooth glossy red. A busy sparkle/noise texture reads as
        // a crawling/shimmering pattern as the car moves (the texels are fixed
        // to the body but the eye sees the high-frequency pattern shift), so we
        // keep the texture very smooth and rely on PBR gloss + reflections for
        // the paint look instead. Tile 1× (no visible repeat).
        // T9: tuned PBR — metallic 0.35 / roughness 0.22 for a clearcoat-ish
        // gloss, plus a very subtle orange-peel normal map for micro-sheen.
        // `emissive` stays off here (the body isn't a light source).
        let car_paint = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: Some(images.add(car_paint_texture())),
            normal_map_texture: Some(car_normal),
            metallic: 0.35,
            perceptual_roughness: 0.22,
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
/// sampler.
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

/// Grass: green base with green-channel-boosted noise for natural variation.
fn grass_texture() -> Image {
    let b = srgb_base(GRASS_SRGB);
    make_image(move |x, y| {
        let v = signed_noise(x, y) * 18 / 128;
        // Boost green variation; keep red/blue subtler.
        let r = clamp_byte(b[0] + v / 2);
        let g = clamp_byte(b[1] + v);
        let bl = clamp_byte(b[2] + v / 2);
        [r, g, bl, 255]
    })
}

/// Road: dark asphalt with faint noise and occasional lighter gravel specks.
fn road_texture() -> Image {
    let b = srgb_base(ASPHALT_SRGB);
    make_image(move |x, y| {
        let n = noise(x, y);
        let v = signed_noise(x, y) * 12 / 128;
        // Occasional lighter speck (gravel).
        let speck = if (n & 0x3F) == 0 { 15 } else { 0 };
        let r = clamp_byte(b[0] + v + speck);
        let g = clamp_byte(b[1] + v + speck);
        let bl = clamp_byte(b[2] + v + speck);
        [r, g, bl, 255]
    })
}

/// Sidewalk: concrete with a subtle 4×4 checker pattern and per-pixel noise.
fn sidewalk_texture() -> Image {
    let b = srgb_base(CONCRETE_SRGB);
    let cell = TEX_SIZE / 4; // 16px cells → 4×4 checker on 64px texture
    make_image(move |x, y| {
        let checker = ((x / cell) + (y / cell)) % 2;
        let cell_off = if checker == 0 { 10 } else { -10 };
        let v = signed_noise(x, y) * 10 / 128;
        let r = clamp_byte(b[0] + cell_off + v);
        let g = clamp_byte(b[1] + cell_off + v);
        let bl = clamp_byte(b[2] + cell_off + v);
        [r, g, bl, 255]
    })
}

/// Car paint: smooth glossy red with only very subtle large-scale variation
/// (no high-frequency sparkle — that reads as a crawling pattern as the car
/// moves). The gloss comes from PBR roughness/metallic + reflections, not a
/// noisy texture.
fn car_paint_texture() -> Image {
    let b = srgb_base(CAR_BODY_SRGB);
    make_image(move |x, y| {
        // Gentle low-amplitude variation only — smooth paint, no sparkles.
        let v = signed_noise(x, y) * 6 / 128;
        let r = clamp_byte(b[0] + v);
        let g = clamp_byte(b[1] + v / 3);
        let bl = clamp_byte(b[2] + v / 3);
        [r, g, bl, 255]
    })
}

// ---------------------------------------------------------------------------
// T9: Procedural normal maps
// ---------------------------------------------------------------------------
//
// A normal map encodes perturbed surface normals in tangent space as RGB:
// (128, 128, 255) ≈ flat (normal pointing +Z). We derive the normal from a
// noise height field via finite differences: n = normalize(-dh/dx, -dh/dy, 1)
// scaled by a `strength` factor. The result is an RGBA image in **linear**
// space (`Rgba8Unorm`, NOT sRGB) — lighting breaks if normal maps are stored
// sRGB. Kept subtle so surfaces get per-pixel light scatter without looking
// bumpy/low-poly.

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

/// A smooth, low-amplitude noise normal map for surfaces that should have a
/// gentle micro-relief (grass, concrete, car paint orange-peel). `strength`
/// tunes the perturbation; ~0.25 is near-flat, ~0.6 is a gentle bumpy surface.
fn smooth_normal_map(strength: f32) -> Image {
    make_normal_map(
        |x, y| signed_noise(x, y) as f32 / 128.0,
        strength,
    )
}

/// Asphalt/road normal map: a mix of low-frequency bumps + a few sharper
/// gravel specks so the road catches light with a gravelly grain. Stronger
/// than `smooth_normal_map` because asphalt has real surface texture.
fn asphalt_normal_map() -> Image {
    make_normal_map(
        |x, y| {
            let base = signed_noise(x, y) as f32 / 128.0;
            // Sharper speckles at a different frequency for gravel grain.
            let speck = signed_noise(x * 3, y * 3) as f32 / 128.0;
            base * 0.7 + speck * 0.3
        },
        0.8,
    )
}
