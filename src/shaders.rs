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
            .init_resource::<EnvironmentAssets>()
            .add_systems(Startup, spawn_sky)
            .add_systems(Update, update_water)
            .add_systems(Update, (update_skydome, setup_environment_light))
            .add_systems(Update, update_modifier_atmosphere);
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
            environment_intensity: 600.0,
        },
        ModifierKind::RushHour => AtmosphereDescriptor {
            // Warm, muted haze evokes a traffic-heavy late afternoon.
            sky_top: LinearRgba::new(0.78, 0.55, 0.42, 1.0),
            sky_bottom: LinearRgba::new(0.95, 0.78, 0.60, 1.0),
            environment_intensity: 666.6667,
        },
        ModifierKind::ChickenFrenzy => AtmosphereDescriptor {
            // A saturated golden sky and stronger IBL make the round lively.
            sky_top: LinearRgba::new(0.82, 0.66, 0.28, 1.0),
            sky_bottom: LinearRgba::new(1.00, 0.88, 0.52, 1.0),
            environment_intensity: 833.3333,
        },
        ModifierKind::Stampede => AtmosphereDescriptor {
            // Dusty earth tones sell a churned-up, critter-filled road.
            sky_top: LinearRgba::new(0.50, 0.38, 0.27, 1.0),
            sky_bottom: LinearRgba::new(0.72, 0.58, 0.40, 1.0),
            environment_intensity: 500.0,
        },
        ModifierKind::GlassCannon => AtmosphereDescriptor {
            // Cool twilight and reduced fill light sharpen the dangerous mood.
            sky_top: LinearRgba::new(0.22, 0.38, 0.58, 1.0),
            sky_bottom: LinearRgba::new(0.46, 0.58, 0.70, 1.0),
            environment_intensity: 333.3333,
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
    active: Option<Res<ActiveModifier>>,
    game_state: Option<Res<State<GameState>>>,
    skydomes: Query<&MeshMaterial3d<SkyMaterial>, With<Skydome>>,
    mut sky_materials: ResMut<Assets<SkyMaterial>>,
    mut cameras: Query<&mut EnvironmentMapLight, With<Camera3d>>,
) {
    // ShaderPlugin is also used by the gameplay-free world-review harness.
    // Preserve the production Playing-only behavior when game state exists,
    // while making the rendering plugin safe in an isolated app.
    if game_state
        .as_ref()
        .is_some_and(|state| *state.get() != GameState::Playing)
    {
        return;
    }
    let Some(active) = active else {
        return;
    };
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

/// Registers the water asset type independently of the skydome. World asset
/// construction depends on this plugin, including in the gameplay-free review
/// harness, so callers install it before either world plugin.
pub struct WaterMaterialPlugin;

impl Plugin for WaterMaterialPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<WaterMaterial>::default());
    }
}

/// Four vec4 uniforms keep every WebGL2 binding naturally 16-byte-sized.
///
/// `motion.x` is the only value mutated at runtime. The remaining components
/// are authored preset data, which means reduced motion can freeze the exact
/// phase-zero composition without flattening its attractive static detail.
#[repr(C)]
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone, PartialEq)]
pub struct WaterMaterial {
    #[uniform(0)]
    pub(crate) deep: Vec4,
    #[uniform(1)]
    pub(crate) shallow: Vec4,
    /// Phase, spatial frequency, counter-wave rate, color-wave amount.
    #[uniform(2)]
    pub(crate) motion: Vec4,
    /// Pseudo-depth curve, edge width, poster steps, edge highlight amount.
    #[uniform(3)]
    pub(crate) detail: Vec4,
}

impl Material for WaterMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/water.wgsl".into()
    }
}

/// Restrained presentation presets corresponding to the three pond families.
/// Geometry/gameplay identity remains in `world`; this type owns shader data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WaterFamilyPreset {
    GardenOval,
    ReedMarsh,
    FarmReservoir,
}

impl WaterFamilyPreset {
    pub(crate) const ALL: [Self; 3] = [Self::GardenOval, Self::ReedMarsh, Self::FarmReservoir];

    pub(crate) const fn index(self) -> usize {
        match self {
            Self::GardenOval => 0,
            Self::ReedMarsh => 1,
            Self::FarmReservoir => 2,
        }
    }

    pub(crate) const fn material(self) -> WaterMaterial {
        match self {
            // Clear blue-green ornamental water.
            Self::GardenOval => WaterMaterial {
                deep: Vec4::new(0.035, 0.190, 0.255, 1.0),
                shallow: Vec4::new(0.105, 0.380, 0.440, 1.0),
                motion: Vec4::new(0.0, 3.00, 0.64, 0.070),
                detail: Vec4::new(0.82, 0.075, 4.0, 0.070),
            },
            // Slightly muted, organic marsh water.
            Self::ReedMarsh => WaterMaterial {
                deep: Vec4::new(0.035, 0.115, 0.105, 1.0),
                shallow: Vec4::new(0.120, 0.285, 0.220, 1.0),
                motion: Vec4::new(0.0, 2.35, 0.46, 0.055),
                detail: Vec4::new(1.12, 0.065, 3.0, 0.045),
            },
            // Cooler, tidier farm-reservoir water.
            Self::FarmReservoir => WaterMaterial {
                deep: Vec4::new(0.025, 0.205, 0.310, 1.0),
                shallow: Vec4::new(0.075, 0.405, 0.500, 1.0),
                motion: Vec4::new(0.0, 3.55, 0.78, 0.060),
                detail: Vec4::new(0.70, 0.055, 5.0, 0.055),
            },
        }
    }
}

/// Review-only water motion choice. Production never inserts this resource.
#[derive(Resource)]
pub(crate) struct WaterReviewMotion {
    pub(crate) reduced: bool,
}

/// Advance only the ripple phase every frame. `Assets::iter_mut` yields
/// `(AssetId, &mut Asset)` pairs, so we discard the id.
fn update_water(
    time: Res<Time>,
    settings: Option<Res<Settings>>,
    review_motion: Option<Res<WaterReviewMotion>>,
    mut water_materials: ResMut<Assets<WaterMaterial>>,
) {
    // The review harness deliberately has no SettingsPlugin. Treat that mode
    // as reduced motion so captures remain pixel-stable at phase exactly zero.
    let reduced_motion = review_motion.as_ref().map_or_else(
        || {
            settings
                .as_ref()
                .is_none_or(|settings| settings.reduced_motion)
        },
        |motion| motion.reduced,
    );
    let phase = water_ripple_time(time.elapsed_secs(), reduced_motion);
    for (_, material) in water_materials.iter_mut() {
        set_water_phase(material, phase);
    }
}

fn set_water_phase(material: &mut WaterMaterial, phase: f32) {
    material.motion.x = phase;
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
const ENVIRONMENT_DIFFUSE_PATH: &str = "environment_maps/pisa_diffuse_rgb9e5_zstd.ktx2";
const ENVIRONMENT_SPECULAR_PATH: &str = "environment_maps/pisa_specular_rgb9e5_zstd.ktx2";

/// Cached IBL handles shared by every camera configured by [`ShaderPlugin`].
/// Loading once also guarantees late-spawned cameras reuse the same assets.
#[derive(Resource)]
pub struct EnvironmentAssets {
    pub diffuse_map: Handle<Image>,
    pub specular_map: Handle<Image>,
}

impl FromWorld for EnvironmentAssets {
    fn from_world(world: &mut World) -> Self {
        let asset_server = world.resource::<AssetServer>();
        Self {
            diffuse_map: asset_server.load(ENVIRONMENT_DIFFUSE_PATH),
            specular_map: asset_server.load(ENVIRONMENT_SPECULAR_PATH),
        }
    }
}

/// Idempotently attach the cached environment to every unconfigured 3D view.
/// Existing environments are intentionally left untouched (for example, a
/// future camera may provide a deliberately authored studio environment).
fn setup_environment_light(
    mut commands: Commands,
    cameras: Query<Entity, (With<Camera3d>, Without<EnvironmentMapLight>)>,
    environment_assets: Res<EnvironmentAssets>,
) {
    for camera in &cameras {
        commands.entity(camera).insert(EnvironmentMapLight {
            diffuse_map: environment_assets.diffuse_map.clone(),
            specular_map: environment_assets.specular_map.clone(),
            intensity: atmosphere_descriptor(ModifierKind::Standard).environment_intensity,
            ..default()
        });
    }
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
    fn water_material_plugin_initializes_assets_without_shader_plugin() {
        let mut app = App::new();
        // MaterialPlugin requires Bevy's asset infrastructure; the review app
        // receives this from DefaultPlugins, so mirror that minimum here.
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<Shader>()
            .init_asset::<Mesh>()
            .init_asset::<Image>()
            .add_plugins(WaterMaterialPlugin);
        assert!(app.world().contains_resource::<Assets<WaterMaterial>>());
        assert!(!app.world().contains_resource::<Assets<SkyMaterial>>());
    }

    fn environment_test_app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_asset::<Image>()
            .init_resource::<EnvironmentAssets>()
            .add_systems(Update, setup_environment_light);
        app
    }

    #[test]
    fn environment_attachment_is_safe_with_zero_cameras() {
        let mut app = environment_test_app();
        app.update();
        let world = app.world_mut();
        assert_eq!(world.query::<&EnvironmentMapLight>().iter(world).count(), 0);
    }

    #[test]
    fn environment_attaches_to_every_camera_with_initial_intensity_and_reused_handles() {
        let mut app = environment_test_app();
        app.world_mut().spawn(Camera3d::default());
        app.world_mut().spawn(Camera3d::default());

        app.update();

        let cached = app.world().resource::<EnvironmentAssets>();
        let diffuse_id = cached.diffuse_map.id();
        let specular_id = cached.specular_map.id();
        let world = app.world_mut();
        let mut environments = world.query::<(&Camera3d, &EnvironmentMapLight)>();
        let environments: Vec<_> = environments.iter(world).collect();
        assert_eq!(environments.len(), 2);
        for (_, environment) in environments {
            assert_eq!(environment.diffuse_map.id(), diffuse_id);
            assert_eq!(environment.specular_map.id(), specular_id);
            assert_eq!(
                environment.intensity,
                atmosphere_descriptor(ModifierKind::Standard).environment_intensity
            );
        }

        // Idempotence: a later frame neither duplicates nor replaces them,
        // and a late-spawned camera receives the same cached handles.
        app.world_mut().spawn(Camera3d::default());
        app.update();
        let world = app.world_mut();
        let mut environments = world.query::<(&Camera3d, &EnvironmentMapLight)>();
        let environments: Vec<_> = environments.iter(world).collect();
        assert_eq!(environments.len(), 3);
        for (_, environment) in environments {
            assert_eq!(environment.diffuse_map.id(), diffuse_id);
            assert_eq!(environment.specular_map.id(), specular_id);
        }
    }

    #[test]
    fn existing_camera_environment_is_not_overwritten() {
        let mut app = environment_test_app();
        let (diffuse_map, specular_map) = {
            let asset_server = app.world().resource::<AssetServer>();
            (
                asset_server.load("environment_maps/custom_diffuse.ktx2"),
                asset_server.load("environment_maps/custom_specular.ktx2"),
            )
        };
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                EnvironmentMapLight {
                    diffuse_map: diffuse_map.clone(),
                    specular_map: specular_map.clone(),
                    intensity: 321.0,
                    rotation: Quat::from_rotation_y(0.75),
                    ..default()
                },
            ))
            .id();

        app.update();

        let environment = app.world().get::<EnvironmentMapLight>(camera).unwrap();
        assert_eq!(environment.diffuse_map.id(), diffuse_map.id());
        assert_eq!(environment.specular_map.id(), specular_map.id());
        assert_eq!(environment.intensity, 321.0);
        assert_eq!(environment.rotation, Quat::from_rotation_y(0.75));
    }

    #[test]
    fn modifier_update_is_safe_without_game_state_or_modifier_resources() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<Assets<SkyMaterial>>()
            .add_systems(Update, update_modifier_atmosphere);
        app.update();
    }

    #[test]
    fn water_material_is_four_contiguous_vec4_fields() {
        assert_eq!(std::mem::size_of::<WaterMaterial>(), 64);
        assert_eq!(std::mem::align_of::<WaterMaterial>(), 16);
        assert_eq!(std::mem::offset_of!(WaterMaterial, deep), 0);
        assert_eq!(std::mem::offset_of!(WaterMaterial, shallow), 16);
        assert_eq!(std::mem::offset_of!(WaterMaterial, motion), 32);
        assert_eq!(std::mem::offset_of!(WaterMaterial, detail), 48);
    }

    #[test]
    fn water_family_presets_are_finite_and_restrained() {
        for preset in WaterFamilyPreset::ALL {
            let material = preset.material();
            for color in [material.deep, material.shallow] {
                assert!(color.is_finite(), "{preset:?} had a non-finite color");
                for channel in color.to_array() {
                    assert!(
                        (0.0..=1.0).contains(&channel),
                        "{preset:?} color channel was {channel}"
                    );
                }
                assert_eq!(color.w, 1.0);
            }

            assert!(material.motion.is_finite());
            assert_eq!(material.motion.x, 0.0);
            assert!((1.0..=6.0).contains(&material.motion.y));
            assert!((0.1..=2.0).contains(&material.motion.z));
            assert!((0.0..=0.15).contains(&material.motion.w));

            assert!(material.detail.is_finite());
            assert!((0.25..=2.0).contains(&material.detail.x));
            assert!((0.01..=0.15).contains(&material.detail.y));
            assert!((2.0..=8.0).contains(&material.detail.z));
            assert_eq!(material.detail.z.fract(), 0.0);
            assert!((0.0..=0.15).contains(&material.detail.w));
        }
    }

    #[test]
    fn water_family_presets_are_distinct_and_stably_indexed() {
        for (index, left) in WaterFamilyPreset::ALL.into_iter().enumerate() {
            assert_eq!(left.index(), index);
            for right in WaterFamilyPreset::ALL.into_iter().skip(index + 1) {
                assert_ne!(left.material(), right.material());
            }
        }
    }

    #[test]
    fn reduced_motion_freezes_exact_phase_zero_only() {
        assert_eq!(water_ripple_time(42.0, true), 0.0);
        assert_eq!(water_ripple_time(42.0, false), 42.0);

        let mut material = WaterFamilyPreset::GardenOval.material();
        let authored_motion = material.motion;
        set_water_phase(&mut material, water_ripple_time(42.0, true));
        assert_eq!(material.motion.x, 0.0);
        assert_eq!(material.motion.y, authored_motion.y);
        assert_eq!(material.motion.z, authored_motion.z);
        assert_eq!(material.motion.w, authored_motion.w);
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
