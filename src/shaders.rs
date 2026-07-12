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

use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

pub struct ShaderPlugin;

impl Plugin for ShaderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<SkyMaterial>::default())
            .add_plugins(MaterialPlugin::<WaterMaterial>::default())
            .add_systems(Startup, (spawn_sky, spawn_water))
            .add_systems(Update, (update_water, update_skydome));
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
