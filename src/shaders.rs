//! Custom-shader plugins: a gradient skydome and an animated water pond.
//!
//! Both materials are implemented with Bevy 0.19's `Material` / `AsBindGroup`
//! API. The WGSL sources live under `assets/shaders/` and are loaded by path
//! (the `trunk` copy-dir ships them next to the wasm binary for web builds).
//!
//! The skydome is a large inverted sphere centered on the origin. The camera
//! sits inside it, so one axis is scaled negatively to flip the face winding —
//! this makes the *inner* surface render (otherwise back-face culling would
//! hide it and only the existing `ClearColor` sky would show). If the shader
//! ever fails to load, the dome is simply invisible and the flat `ClearColor`
//! remains, so the scene degrades gracefully.

use bevy::asset::RenderAssetUsages;
use bevy::color::LinearRgba;
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor,
    TextureViewDimension,
};
use bevy::shader::ShaderRef;
use half::f16;

pub struct ShaderPlugin;

impl Plugin for ShaderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<SkyMaterial>::default())
            .add_plugins(MaterialPlugin::<WaterMaterial>::default())
            .add_systems(Startup, (spawn_sky, spawn_water))
            .add_systems(
                Update,
                (update_water, update_skydome, setup_environment_light),
            );
    }
}

// ---------------------------------------------------------------------------
// Skydome gradient
// ---------------------------------------------------------------------------

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct SkyMaterial {
    #[uniform(0)]
    top_color: LinearRgba,
    #[uniform(1)]
    bottom_color: LinearRgba,
}

impl Material for SkyMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/sky.wgsl".into()
    }
}

/// Tag for the gradient skydome sphere so `update_skydome` can follow the
/// car and keep the dome centered on it (the car now drives infinitely along
/// -Z, so a static origin-centered dome would eventually be left behind).
#[derive(Component)]
pub struct Skydome;

/// Spawn a giant sphere around the origin and render its inner surface as a
/// vertical color gradient. Negative-Z scale flips winding so the dome's
/// interior is visible from the camera (which lives inside it).
fn spawn_sky(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<SkyMaterial>>,
) {
    // Radius comfortably encloses the whole scene (ground is 100x100, the
    // camera follows the car within roughly +/-50 on each axis).
    commands.spawn((
        Mesh3d(meshes.add(Sphere::new(100.0).mesh().uv(32, 18))),
        MeshMaterial3d(materials.add(SkyMaterial {
            top_color: LinearRgba::new(0.55, 0.78, 0.95, 1.0),
            bottom_color: LinearRgba::new(0.85, 0.90, 0.95, 1.0),
        })),
        Transform::from_scale(Vec3::new(1.0, 1.0, -1.0)),
        Skydome,
    ));
}

/// Keep the skydome centered on the car's XZ position (y=0) so the endless
/// road never drives out from under the gradient sky.
fn update_skydome(
    car: Query<&Transform, (With<crate::car::Car>, Without<Skydome>)>,
    mut sky: Query<&mut Transform, (With<Skydome>, Without<crate::car::Car>)>,
) {
    let Ok(car_t) = car.single() else {
        return;
    };
    let Ok(mut sky_t) = sky.single_mut() else {
        return;
    };
    sky_t.translation = Vec3::new(car_t.translation.x, 0.0, car_t.translation.z);
}

// ---------------------------------------------------------------------------
// Animated water pond
// ---------------------------------------------------------------------------

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct WaterMaterial {
    #[uniform(0)]
    base: LinearRgba,
    // WebGL2 requires uniform buffer bindings to be 16-byte aligned, so the
    // animated phase is carried in a vec4 (we only use .x). A bare f32 (4B)
    // fails pipeline creation on ANGLE/WebGL2.
    #[uniform(1)]
    time: Vec4,
}

impl Material for WaterMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/water.wgsl".into()
    }
}

/// A small flat pond sitting on the grass, just outside the building row.
fn spawn_water(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<WaterMaterial>>,
) {
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(12.0, 12.0))),
        MeshMaterial3d(materials.add(WaterMaterial {
            base: LinearRgba::new(0.10, 0.40, 0.60, 1.0),
            time: Vec4::ZERO,
        })),
        // Slightly above the grass plane (y=0) to avoid z-fighting with it.
        Transform::from_xyz(30.0, 0.03, -10.0),
    ));
}

/// Advance the ripple phase every frame. `Assets::iter_mut` yields
/// `(AssetId, &mut Asset)` pairs, so we discard the id and bump `time`.
fn update_water(time: Res<Time>, mut water_materials: ResMut<Assets<WaterMaterial>>) {
    let t = time.elapsed_secs();
    for (_, mat) in water_materials.iter_mut() {
        mat.time = Vec4::splat(t);
    }
}

// ---------------------------------------------------------------------------
// T9: Static pre-baked environment map for IBL reflections
// ---------------------------------------------------------------------------
//
// `EnvironmentMapGenerationPlugin` (runtime cubemap filtering) is disabled on
// WebGL2 because it needs compute shaders. But a **pre-baked** cubemap works
// fine — it's just a static `Image` sampled in the PBR shader, no compute.
// Bevy 0.19 ships `EnvironmentMapLight::hemispherical_gradient` which builds a
// tiny 1×1×6 cubemap from three colors (sky / horizon / ground) procedurally,
// so we get IBL diffuse + specular reflections on metallic/glossy surfaces
// (car paint, lamp poles, coins) on BOTH native and web.
//
// `EnvironmentMapLight` is a **view** component (read off the camera entity),
// so we insert it onto the `Camera3d` here rather than spawning a `LightProbe`
// entity. This runs every Update but exits early once the camera has the
// component, so startup ordering with `spawn_camera` doesn't matter.
fn setup_environment_light(
    mut commands: Commands,
    cameras: Query<Entity, (With<Camera3d>, Without<EnvironmentMapLight>)>,
    mut images: ResMut<Assets<Image>>,
) {
    let Ok(cam) = cameras.single() else {
        return;
    };
    // A procedural HDR cubemap with a sky gradient + a bright sun disc. The
    // disc gives low-roughness metallic surfaces (car paint) a sharp bright
    // streak to reflect — the classic metallic cue. The smooth
    // `hemispherical_gradient` env was too featureless, so the car read as a
    // flat red surface instead of metal.
    let cubemap = images.add(sun_disc_cubemap());
    let env = EnvironmentMapLight {
        diffuse_map: cubemap.clone(),
        specular_map: cubemap,
        // Bright so reflections are obvious (GlobalAmbientLight was lowered to
        // 40 in Wave 3; the IBL carries the metallic look).
        intensity: 6.0,
        ..default()
    };
    commands.entity(cam).insert(env);
}

/// Build a small HDR cubemap (`Rgba16Float`, `RES`×`RES`×6 faces) with a sky
/// gradient and a bright sun disc on the +Y (up) face. Face order is the wgpu
/// cubemap order: +X, -X, +Y, -Y, +Z, -Z. Each pixel is 4 f16 in linear space.
/// The sun disc is HDR (>1.0) so it blooms + reflects strongly on metal.
fn sun_disc_cubemap() -> Image {
    const RES: u32 = 64;
    const FACE: usize = 6;
    let sky = LinearRgba::rgb(0.35, 0.55, 0.85); // sky blue
    let horizon = LinearRgba::rgb(0.85, 0.88, 0.93); // pale haze (bright band)
    let ground = LinearRgba::rgb(0.16, 0.14, 0.11); // warm earth
    let sun = LinearRgba::rgb(20.0, 18.0, 15.0); // HDR sun (>>1.0 -> strong bloom)

    // Cubemap face indices: 0=+X, 1=-X, 2=+Y, 3=-Y, 4=+Z, 5=-Z.
    let face_dirs: [(f32, f32, f32); FACE] = [
        (1.0, 0.0, 0.0),
        (-1.0, 0.0, 0.0),
        (0.0, 1.0, 0.0),
        (0.0, -1.0, 0.0),
        (0.0, 0.0, 1.0),
        (0.0, 0.0, -1.0),
    ];
    // Sun direction (unit), roughly matching the world.rs directional sun at
    // (30,25,15) -> normalized.
    let sun_dir = Vec3::new(30.0, 25.0, 15.0).normalize();

    let mut data: Vec<u8> = Vec::with_capacity(FACE * (RES * RES) as usize * 8);
    for (_, dir) in face_dirs.iter().enumerate() {
        let face_normal = Vec3::new(dir.0, dir.1, dir.2);
        // Build an orthonormal basis (right, up) for this face. Use Z as the
        // reference "up" for the +Y/-Y faces (whose normal is parallel to Y so
        // the cross with Y is zero).
        let world_up = if face_normal.abs().dot(Vec3::Y) > 0.9 {
            Vec3::Z
        } else {
            Vec3::Y
        };
        let right = face_normal.cross(world_up).normalize_or_zero();
        let up = right.cross(face_normal).normalize_or_zero();
        for y in 0..RES {
            for x in 0..RES {
                // Map pixel to a direction within the face ([-1,1] in right/up).
                let u = (x as f32 / (RES - 1) as f32) * 2.0 - 1.0;
                let v = (y as f32 / (RES - 1) as f32) * 2.0 - 1.0;
                let sample_dir = (face_normal + right * u + up * v).normalize();
                // Gradient by vertical (world Y) component.
                let sy = sample_dir.y;
                let mut col = if sy > 0.0 {
                    // Sky: horizon -> sky blue with height.
                    let t = sy.clamp(0.0, 1.0);
                    horizon.mix(&sky, t)
                } else {
                    // Ground: horizon -> earth with depth.
                    let t = (-sy).clamp(0.0, 1.0);
                    horizon.mix(&ground, t)
                };
                // Sun disc: bright where the sample direction is near the sun.
                // Use a relatively LARGE disc (~18 deg) so the reflection on
                // the car is an obvious bright streak, not a tiny dot.
                let d = sample_dir.dot(sun_dir).max(0.0);
                let disc = ((d - 0.95) / 0.05).clamp(0.0, 1.0); // ~18 deg disc
                if disc > 0.0 {
                    col = col.mix(&sun, disc);
                    // Soft halo around the disc.
                    let halo = ((d - 0.90) / 0.05).clamp(0.0, 1.0) * 0.5;
                    col = col + sun * halo * 0.25;
                }
                let c = [col.red, col.green, col.blue, 1.0f32];
                for chan in c {
                    let h = f16::from_f32(chan);
                    data.extend_from_slice(&h.to_le_bytes());
                }
            }
        }
    }

    Image {
        texture_view_descriptor: Some(TextureViewDescriptor {
            dimension: Some(TextureViewDimension::Cube),
            ..Default::default()
        }),
        ..Image::new(
            Extent3d {
                width: RES,
                height: RES,
                depth_or_array_layers: FACE as u32,
            },
            TextureDimension::D2,
            data,
            TextureFormat::Rgba16Float,
            RenderAssetUsages::RENDER_WORLD,
        )
    }
}
