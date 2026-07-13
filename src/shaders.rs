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

use bevy::color::LinearRgba;
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

use crate::game::state::GameState;
use crate::modifiers::{ActiveModifier, ModifierKind};
use crate::settings::Settings;

pub struct ShaderPlugin;

impl Plugin for ShaderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<SkyMaterial>::default())
            .add_plugins(MaterialPlugin::<WaterMaterial>::default())
            .add_systems(Startup, (spawn_sky, spawn_water))
            .add_systems(
                Update,
                (update_water, update_skydome, setup_environment_light),
            )
            .add_systems(
                Update,
                update_modifier_atmosphere.run_if(in_state(GameState::Playing)),
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

/// Target rendering atmosphere associated with one round modifier.
///
/// This is deliberately only data: [`atmosphere_descriptor`] is pure, while
/// the update system below owns all Bevy asset/component mutation.
#[derive(Clone, Copy, Debug, PartialEq)]
struct AtmosphereDescriptor {
    sky_top: LinearRgba,
    sky_bottom: LinearRgba,
    environment_intensity: f32,
}

/// Pure mapping from gameplay modifier to its visual atmosphere.
fn atmosphere_descriptor(kind: ModifierKind) -> AtmosphereDescriptor {
    match kind {
        ModifierKind::Standard => AtmosphereDescriptor {
            // Preserve the original clear daytime presentation.
            sky_top: LinearRgba::new(0.55, 0.78, 0.95, 1.0),
            sky_bottom: LinearRgba::new(0.85, 0.90, 0.95, 1.0),
            environment_intensity: 900.0,
        },
        ModifierKind::RushHour => AtmosphereDescriptor {
            // Warm, muted haze evokes a traffic-heavy late afternoon.
            sky_top: LinearRgba::new(0.78, 0.55, 0.42, 1.0),
            sky_bottom: LinearRgba::new(0.95, 0.78, 0.60, 1.0),
            environment_intensity: 1_000.0,
        },
        ModifierKind::ChickenFrenzy => AtmosphereDescriptor {
            // A saturated golden sky and stronger IBL make the round lively.
            sky_top: LinearRgba::new(0.82, 0.66, 0.28, 1.0),
            sky_bottom: LinearRgba::new(1.00, 0.88, 0.52, 1.0),
            environment_intensity: 1_250.0,
        },
        ModifierKind::Stampede => AtmosphereDescriptor {
            // Dusty earth tones sell a churned-up, critter-filled road.
            sky_top: LinearRgba::new(0.50, 0.38, 0.27, 1.0),
            sky_bottom: LinearRgba::new(0.72, 0.58, 0.40, 1.0),
            environment_intensity: 750.0,
        },
        ModifierKind::GlassCannon => AtmosphereDescriptor {
            // Cool twilight and reduced fill light sharpen the dangerous mood.
            sky_top: LinearRgba::new(0.22, 0.38, 0.58, 1.0),
            sky_bottom: LinearRgba::new(0.46, 0.58, 0.70, 1.0),
            environment_intensity: 500.0,
        },
    }
}

/// Exponential response rate used for both gradient and IBL transitions.
const ATMOSPHERE_LERP_RATE: f32 = 2.5;

/// Move the existing sky asset and camera light toward the active modifier's
/// descriptor. Missing components/assets are expected briefly during startup;
/// each half updates independently as soon as it exists. No handles or assets
/// are created here.
fn update_modifier_atmosphere(
    time: Res<Time>,
    active: Res<ActiveModifier>,
    skydomes: Query<&MeshMaterial3d<SkyMaterial>, With<Skydome>>,
    mut sky_materials: ResMut<Assets<SkyMaterial>>,
    mut cameras: Query<&mut EnvironmentMapLight, With<Camera3d>>,
) {
    let target = atmosphere_descriptor(active.0);
    let amount = (1.0 - (-ATMOSPHERE_LERP_RATE * time.delta_secs()).exp()).clamp(0.0, 1.0);

    for handle in &skydomes {
        let Some(mut material) = sky_materials.get_mut(&handle.0) else {
            continue;
        };
        material.top_color = lerp_linear_rgba(material.top_color, target.sky_top, amount);
        material.bottom_color = lerp_linear_rgba(material.bottom_color, target.sky_bottom, amount);
    }

    for mut environment in &mut cameras {
        environment.intensity += (target.environment_intensity - environment.intensity) * amount;
    }
}

fn lerp_linear_rgba(from: LinearRgba, to: LinearRgba, amount: f32) -> LinearRgba {
    LinearRgba::new(
        from.red + (to.red - from.red) * amount,
        from.green + (to.green - from.green) * amount,
        from.blue + (to.blue - from.blue) * amount,
        from.alpha + (to.alpha - from.alpha) * amount,
    )
}

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
            top_color: atmosphere_descriptor(ModifierKind::Standard).sky_top,
            bottom_color: atmosphere_descriptor(ModifierKind::Standard).sky_bottom,
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
fn update_water(
    time: Res<Time>,
    settings: Res<Settings>,
    mut water_materials: ResMut<Assets<WaterMaterial>>,
) {
    let t = water_ripple_time(time.elapsed_secs(), settings.reduced_motion);
    for (_, mat) in water_materials.iter_mut() {
        mat.time = Vec4::splat(t);
    }
}

/// Reduced motion freezes the shader phase at its neutral startup value.
const fn water_ripple_time(elapsed: f32, reduced_motion: bool) -> f32 {
    if reduced_motion { 0.0 } else { elapsed }
}

// ---------------------------------------------------------------------------
// Static prefiltered image-based lighting
// ---------------------------------------------------------------------------
// A view environment map is supported on WebGL2. It does not capture the live
// city: it reflects the static scene baked into these cubemaps. The diffuse
// file is Lambertian-filtered; the specular file contains the GGX roughness mip
// chain required by Bevy's split-sum PBR shader. Both files are the known-good
// Bevy 0.19 Pisa assets, used to validate the real IBL path before replacing
// them with a similarly prefiltered Palermo Sidewalk pair.
fn setup_environment_light(
    mut commands: Commands,
    cameras: Query<Entity, (With<Camera3d>, Without<EnvironmentMapLight>)>,
    asset_server: Res<AssetServer>,
) {
    let Ok(camera) = cameras.single() else {
        return;
    };
    commands.entity(camera).insert(EnvironmentMapLight {
        diffuse_map: asset_server.load("environment_maps/pisa_diffuse_rgb9e5_zstd.ktx2"),
        specular_map: asset_server.load("environment_maps/pisa_specular_rgb9e5_zstd.ktx2"),
        intensity: atmosphere_descriptor(ModifierKind::Standard).environment_intensity,
        ..default()
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_MODIFIERS: [ModifierKind; 5] = [
        ModifierKind::Standard,
        ModifierKind::RushHour,
        ModifierKind::ChickenFrenzy,
        ModifierKind::Stampede,
        ModifierKind::GlassCannon,
    ];

    #[test]
    fn reduced_motion_freezes_water_ripple_time() {
        assert_eq!(water_ripple_time(42.0, true), 0.0);
        assert_eq!(water_ripple_time(42.0, false), 42.0);
    }

    #[test]
    fn modifier_atmospheres_are_distinct() {
        for (index, left) in ALL_MODIFIERS.into_iter().enumerate() {
            for right in ALL_MODIFIERS.into_iter().skip(index + 1) {
                assert_ne!(atmosphere_descriptor(left), atmosphere_descriptor(right));
            }
        }
    }

    #[test]
    fn modifier_atmospheres_stay_in_sane_ranges() {
        for kind in ALL_MODIFIERS {
            let descriptor = atmosphere_descriptor(kind);
            assert!(
                (200.0..=1_500.0).contains(&descriptor.environment_intensity),
                "{kind:?} environment intensity was {}",
                descriptor.environment_intensity
            );

            for color in [descriptor.sky_top, descriptor.sky_bottom] {
                for channel in [color.red, color.green, color.blue, color.alpha] {
                    assert!(
                        channel.is_finite() && (0.0..=1.0).contains(&channel),
                        "{kind:?} sky channel was {channel}"
                    );
                }
            }
        }
    }
}
