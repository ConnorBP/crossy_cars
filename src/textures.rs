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

        // GRASS — green base with subtle per-pixel noise; tile 16×.
        let grass = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: Some(images.add(grass_texture())),
            perceptual_roughness: 1.0,
            uv_transform: Affine2::from_scale(Vec2::splat(16.0)),
            ..default()
        });

        // ROAD — dark asphalt with faint noise + gravel specks; tile 8×.
        let road = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: Some(images.add(road_texture())),
            perceptual_roughness: 0.95,
            uv_transform: Affine2::from_scale(Vec2::splat(8.0)),
            ..default()
        });

        // SIDEWALK — concrete with a subtle checker + noise; tile 6×.
        let sidewalk = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: Some(images.add(sidewalk_texture())),
            perceptual_roughness: 0.9,
            uv_transform: Affine2::from_scale(Vec2::splat(6.0)),
            ..default()
        });

        // CAR_PAINT — metallic red with flake-style variation + sparkles; tile 2×.
        let car_paint = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: Some(images.add(car_paint_texture())),
            metallic: 0.6,
            perceptual_roughness: 0.35,
            uv_transform: Affine2::from_scale(Vec2::splat(2.0)),
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

/// Car paint: metallic red with flake-style brightness variation + sparkles.
fn car_paint_texture() -> Image {
    let b = srgb_base(CAR_BODY_SRGB);
    make_image(move |x, y| {
        let n = noise(x, y);
        let v = signed_noise(x, y) * 22 / 128;
        // Bright sparkle for metallic flake highlights.
        let sparkle = if (n & 0xF) == 0 { 35 } else { 0 };
        let r = clamp_byte(b[0] + v + sparkle);
        let g = clamp_byte(b[1] + v / 3 + sparkle / 4);
        let bl = clamp_byte(b[2] + v / 3 + sparkle / 4);
        [r, g, bl, 255]
    })
}
