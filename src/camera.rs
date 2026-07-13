use bevy::{camera::ScalingMode, prelude::*};
// T9 rendering beef-up: explicit tonemapping + bloom on the 3D camera.
// `Bloom` is a separate component in 0.19 (not a `Camera3d` field) and it
// `#[require(Hdr)]`s the HDR render target, which is what lets bloom +
// tonemapping work. `Tonemapping` is already a required component of `Camera3d`
// (default `TonyMcMapface`) but we set it explicitly to make the intent clear.
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::post_process::bloom::Bloom;

use crate::car::{Car, DrivingSet};
use crate::game::events::ObstacleHit;
use crate::game::resources::GameConfig;
use crate::game::state::GameState;
use crate::settings::Settings;
use crate::world::world_review_bounds;

/// Exponential smoothing rate for camera follow / zoom (higher = snappier).
const SMOOTH: f32 = 4.0;

// --- T13: camera shake (juice) tuning constants ---
/// Converts `impact_speed` into trauma added to the shake accumulator.
/// A full-speed (~12) hit yields `12 * 0.05 = 0.6` trauma — a strong but
/// not maxed-out shake, as specified.
const SHAKE_SCALE: f32 = 0.05;
/// Exponential trauma decay rate (per second). Higher = the shake stops
/// sooner. ~5 gives a ~0.2s half-life, a snappy crash jolt that washes out
/// quickly as the follow lerp re-centers the camera.
const SHAKE_DECAY: f32 = 5.0;
/// Max translational offset (in world units) at trauma = 1.0. The actual
/// offset applied is `MAX_SHAKE_OFFSET * trauma^2 * (random unit vector)`,
/// so at trauma 0.6 (a full-speed hit) the peak nudge is ~0.6² * 1.5 ≈ 0.54u —
/// visible but never nauseating under the ortho camera.
const MAX_SHAKE_OFFSET: f32 = 1.5;

/// T13: camera shake accumulator. `trauma` ranges 0..1; the visible offset
/// scales with `trauma^2` for a snappy quadratic decay. It is a resource so
/// the hit-reader system and `follow_camera` (which applies the offset) share
/// one value without ordering concerns.
#[derive(Resource, Default)]
pub struct Shake {
    pub trauma: f32,
    /// Frame-varying pseudo-random counter so the offset looks noisy without
    /// pulling in a rand crate (keeps the WASM build slim).
    noise: f32,
}

#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
struct CameraObstacleFeedbackSet;

/// Keep the ordering contract in one helper so plugin wiring and its focused
/// schedule test exercise the same relation.
fn configure_obstacle_feedback_order(app: &mut App) {
    app.configure_sets(Update, CameraObstacleFeedbackSet.after(DrivingSet));
}

pub struct CameraPlugin;

/// Fixed overhead camera used only by `?world_review=1`. It has no follow,
/// shake, speed zoom, bloom, or other frame-dependent behavior.
pub struct WorldReviewCameraPlugin;

/// Minimal deterministic studio used only by `?car_review=1`. It owns a
/// neutral platform, fixed key/fill lights and one non-animated camera; no
/// production world, UI, follow, shake or gameplay systems are installed.
pub struct CarReviewCameraPlugin;

impl Plugin for CarReviewCameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_car_review_studio);
    }
}

#[derive(Clone, Copy)]
enum CarReviewView {
    Front,
    Rear,
    Left,
    Right,
    FrontLeft,
    FrontRight,
    RearLeft,
    RearRight,
}

fn car_review_view_name() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let query = web_sys::window()
            .and_then(|window| js_sys::Reflect::get(window.as_ref(), &"location".into()).ok())
            .and_then(|location| js_sys::Reflect::get(&location, &"search".into()).ok())
            .and_then(|search| search.as_string())
            .unwrap_or_default();
        for part in query.trim_start_matches('?').split('&') {
            if let Some(value) = part.strip_prefix("car_view=") {
                return value.to_ascii_lowercase();
            }
        }
        "front_left".into()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var("ROADY_CAR_REVIEW_VIEW").unwrap_or_else(|_| "front_left".into())
    }
}

fn car_review_view(name: &str) -> CarReviewView {
    match name {
        "front" => CarReviewView::Front,
        "rear" => CarReviewView::Rear,
        "left" => CarReviewView::Left,
        "right" => CarReviewView::Right,
        "front_right" => CarReviewView::FrontRight,
        "rear_left" => CarReviewView::RearLeft,
        "rear_right" => CarReviewView::RearRight,
        _ => CarReviewView::FrontLeft,
    }
}

fn car_review_camera_position(view: CarReviewView) -> Vec3 {
    let (x, z) = match view {
        CarReviewView::Front => (0.0, -3.6),
        CarReviewView::Rear => (0.0, 3.6),
        CarReviewView::Left => (-3.6, 0.0),
        CarReviewView::Right => (3.6, 0.0),
        CarReviewView::FrontLeft => (-2.75, -2.75),
        CarReviewView::FrontRight => (2.75, -2.75),
        CarReviewView::RearLeft => (-2.75, 2.75),
        CarReviewView::RearRight => (2.75, 2.75),
    };
    Vec3::new(x, 1.75, z)
}

fn spawn_car_review_studio(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let platform = materials.add(StandardMaterial {
        base_color: Color::srgb(0.33, 0.34, 0.35),
        metallic: 0.0,
        perceptual_roughness: 0.82,
        ..default()
    });
    commands.spawn((
        Mesh3d(meshes.add(Cylinder::new(1.55, 0.05))),
        MeshMaterial3d(platform),
        Transform::from_xyz(0.0, -0.01, 0.0),
    ));
    commands.spawn((
        DirectionalLight {
            color: Color::srgb(1.0, 0.96, 0.90),
            illuminance: 8_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(-3.0, 5.0, -4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight {
            color: Color::srgb(0.72, 0.82, 1.0),
            illuminance: 2_200.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(4.0, 3.0, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    let position = car_review_camera_position(car_review_view(&car_review_view_name()));
    commands.spawn((
        Camera3d::default(),
        Msaa::Sample4,
        Tonemapping::TonyMcMapface,
        Projection::Perspective(PerspectiveProjection {
            fov: 32.0_f32.to_radians(),
            near: 0.1,
            far: 20.0,
            ..default()
        }),
        Transform::from_translation(position).looking_at(Vec3::new(0.0, 0.43, 0.0), Vec3::Y),
    ));
}

impl Plugin for WorldReviewCameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_world_review_camera)
            .add_systems(Update, fit_world_review_camera);
    }
}

const REVIEW_FRAME_MARGIN: f32 = 1.04;

/// Fit both dimensions, including portrait viewports. Returned values are
/// `(viewport_height, center_x, center_z)` and are pure for focused tests.
fn review_projection_for_bounds(min: Vec2, max: Vec2, aspect: f32) -> (f32, f32, f32) {
    let width = (max.x - min.x).max(1.0);
    let height = (max.y - min.y).max(1.0);
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        1.0
    };
    let viewport_height = height.max(width / aspect) * REVIEW_FRAME_MARGIN;
    (
        viewport_height,
        (min.x + max.x) * 0.5,
        (min.y + max.y) * 0.5,
    )
}

fn spawn_world_review_camera(mut commands: Commands) {
    let (min, max) = world_review_bounds();
    let (viewport_height, center_x, center_z) = review_projection_for_bounds(min, max, 4.0 / 3.0);
    commands.spawn((
        Camera3d::default(),
        Msaa::Sample4,
        Tonemapping::TonyMcMapface,
        Projection::from(OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical { viewport_height },
            ..OrthographicProjection::default_3d()
        }),
        Transform::from_xyz(center_x, 400.0, center_z)
            .looking_at(Vec3::new(center_x, 0.0, center_z), Vec3::NEG_Z),
    ));
}

fn fit_world_review_camera(
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut cameras: Query<(&mut Transform, &mut Projection), With<Camera3d>>,
) {
    let Ok(window) = windows.single() else { return };
    let width = window.resolution.width();
    let height = window.resolution.height();
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let (min, max) = world_review_bounds();
    let (viewport_height, center_x, center_z) =
        review_projection_for_bounds(min, max, width / height);
    for (mut transform, mut projection) in &mut cameras {
        if let Projection::Orthographic(orthographic) = &mut *projection {
            orthographic.scaling_mode = ScalingMode::FixedVertical { viewport_height };
        }
        *transform = Transform::from_xyz(center_x, 400.0, center_z)
            .looking_at(Vec3::new(center_x, 0.0, center_z), Vec3::NEG_Z);
    }
}

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        configure_obstacle_feedback_order(app);
        app.init_resource::<Shake>()
            .add_systems(Startup, spawn_camera)
            .add_systems(
                Update,
                (handle_obstacle_hits, follow_camera)
                    .chain()
                    // The hit reader must observe collision messages emitted
                    // by this frame's driving chain, not schedule-dependent
                    // previous-frame data.
                    .in_set(CameraObstacleFeedbackSet)
                    .run_if(in_state(GameState::Playing)),
            );
    }
}

fn spawn_camera(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        // T9: MSAA 4× for edge anti-aliasing. In Bevy 0.19 `Msaa` is a camera
        // component (not a resource). Works on both native and WebGL2
        // (WebGL2 supports up to 4× MSAA via renderable sample counts).
        Msaa::Sample4,
        // T9: tonemapping — TonyMcMapface is a neutral, filmic display transform
        // that desaturates brights cleanly. Pairs well with bloom.
        Tonemapping::TonyMcMapface,
        // T9: bloom — `Bloom` auto-inserts the `Hdr` component (via
        // `#[require(Hdr)]`), switching the camera to an HDR intermediate
        // target so emissive materials (headlights/brake lamps/coins/lamps)
        // can glow. `NATURAL` is the energy-conserving default preset; we tune
        // the intensity down slightly so the high ambient light (set in
        // main.rs) doesn't over-bloom.
        Bloom {
            intensity: 0.1,
            ..Bloom::NATURAL
        },
        Projection::from(OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical {
                viewport_height: 10.0,
            },
            ..OrthographicProjection::default_3d()
        }),
        // T7 fix preserved: the iso rotation is set ONCE here at spawn via
        // `looking_at(ZERO, Y)` and NEVER recomputed per frame. `follow_camera`
        // only lerps translation + adjusts the ortho viewport_height (zoom).
        Transform::from_xyz(12.0, 12.0, 12.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

fn follow_camera(
    car: Query<(&Transform, &Car), (With<Car>, Without<Camera3d>)>,
    mut camera: Query<(&mut Transform, &mut Projection), (With<Camera3d>, Without<Car>)>,
    cfg: Res<GameConfig>,
    time: Res<Time>,
    settings: Res<Settings>,
    mut shake: ResMut<Shake>,
) {
    let Ok((car_t, car)) = car.single() else {
        return;
    };
    let Ok((mut cam_t, mut proj)) = camera.single_mut() else {
        return;
    };

    let dt = time.delta_secs();
    let t = 1.0 - (-SMOOTH * dt).exp();

    // Look-ahead: nudge the follow target in the car's forward direction (model -Z).
    // Only the POSITION is lerped; rotation stays fixed from spawn so the iso
    // angle never tilts (a fixed rotation can't wobble as the camera lags).
    let fwd = car_t.rotation * Vec3::new(0.0, 0.0, -1.0);
    let desired = car_t.translation + cfg.cam_offset + fwd * 1.5;

    // Smoothed follow: exponential lerp toward the desired iso position.
    // Rotation is intentionally left untouched — recomputing it per frame from
    // a live look target while the position lags is what caused the tilt.
    cam_t.translation = cam_t.translation.lerp(desired, t);

    // T13: translational shake offset. Applied AFTER the follow lerp so the
    // crash jolt rides on top of the smoothed position. Because next frame's
    // lerp starts from this shaken translation and re-aims at `desired`, the
    // offset naturally washes out as trauma decays — no snap-back. ONLY
    // translation is touched; `cam_t.rotation` is never modified (preserves
    // the T7 fixed-iso-rotation no-tilt fix).
    if shake.trauma > 0.001 {
        if !settings.reduced_motion {
            // Cheap deterministic pseudo-randomness (no crate). Fract-of-sin gives
            // a smooth but chaotic-looking signal; advancing `noise` per frame
            // keeps the direction jittering so the shake feels alive.
            shake.noise += dt * 53.0;
            let n = shake.noise;
            let rand_x = (n.sin() * 43758.5453).fract() * 2.0 - 1.0;
            let rand_z = ((n * 1.7 + 3.0).sin() * 12543.987).fract() * 2.0 - 1.0;
            // trauma^2 for a snappy quadratic decay feel. Offset is horizontal
            // only (XZ) — y is left at the lerped value so the iso height stays
            // stable (vertical bob would read as a tilt under ortho).
            let amp = shake.trauma * shake.trauma * MAX_SHAKE_OFFSET;
            cam_t.translation.x += rand_x * amp;
            cam_t.translation.z += rand_z * amp;
        }
        // Trauma still decays while reduced motion is enabled, but without
        // producing any visible offset.
        shake.trauma *= (-SHAKE_DECAY * dt).exp();
        if shake.trauma < 0.001 {
            shake.trauma = 0.0;
        }
    }

    // Speed zoom: widen the orthographic viewport as speed rises. Reduced
    // motion pins the viewport immediately to its fixed baseline: no residual
    // smoothing from a previously fast frame and no speed-driven zoom motion.
    let current_vh = match &*proj {
        Projection::Orthographic(o) => match o.scaling_mode {
            ScalingMode::FixedVertical { viewport_height } => viewport_height,
            _ => 10.0,
        },
        _ => 10.0,
    };
    let vh = camera_viewport_height(
        current_vh,
        car.speed,
        cfg.max_speed,
        t,
        settings.reduced_motion,
    );
    if let Projection::Orthographic(ref mut o) = *proj {
        o.scaling_mode = ScalingMode::FixedVertical {
            viewport_height: vh,
        };
    }
}

/// T13: feed `ObstacleHit` messages into the `Shake` trauma accumulator. Runs
/// before `follow_camera` (chained) so the freshly-added trauma is applied
/// this same frame. Reads only — never touches the camera or car transform.
fn handle_obstacle_hits(
    mut hits: MessageReader<ObstacleHit>,
    settings: Res<Settings>,
    mut shake: ResMut<Shake>,
) {
    for hit in hits.read() {
        shake.trauma = next_trauma(shake.trauma, hit.impact_speed, settings.reduced_motion);
    }
}

/// Return the trauma after one impact, respecting the reduced-motion gate.
fn next_trauma(current: f32, impact_speed: f32, reduced_motion: bool) -> f32 {
    if reduced_motion {
        current
    } else {
        (current + impact_speed * SHAKE_SCALE).clamp(0.0, 1.0)
    }
}

/// Orthographic zoom transition shared with focused accessibility tests.
fn camera_viewport_height(
    current: f32,
    speed: f32,
    max_speed: f32,
    smoothing: f32,
    reduced_motion: bool,
) -> f32 {
    const FIXED_VIEWPORT_HEIGHT: f32 = 10.0;
    if reduced_motion {
        return FIXED_VIEWPORT_HEIGHT;
    }
    let ratio = if max_speed > 0.0 {
        speed.abs() / max_speed
    } else {
        0.0
    };
    let target = FIXED_VIEWPORT_HEIGHT + ratio * 2.0;
    current + (target - current) * smoothing
}

#[cfg(test)]
mod tests {
    use super::{
        CameraObstacleFeedbackSet, CarReviewView, camera_viewport_height,
        car_review_camera_position, car_review_view, configure_obstacle_feedback_order,
        next_trauma, review_projection_for_bounds,
    };
    use crate::{car::DrivingSet, world::world_review_bounds};
    use bevy::prelude::*;

    #[derive(Resource, Default)]
    struct ExecutionOrder(Vec<&'static str>);

    fn record_driving(mut order: ResMut<ExecutionOrder>) {
        order.0.push("driving");
    }

    fn record_feedback(mut order: ResMut<ExecutionOrder>) {
        order.0.push("camera feedback");
    }

    #[test]
    fn car_review_names_map_to_distinct_deterministic_orbits() {
        let cases = [
            ("front", CarReviewView::Front),
            ("rear", CarReviewView::Rear),
            ("left", CarReviewView::Left),
            ("right", CarReviewView::Right),
            ("front_left", CarReviewView::FrontLeft),
            ("front_right", CarReviewView::FrontRight),
            ("rear_left", CarReviewView::RearLeft),
            ("rear_right", CarReviewView::RearRight),
        ];
        let mut positions = Vec::new();
        for (name, expected) in cases {
            let actual = car_review_camera_position(car_review_view(name));
            let expected = car_review_camera_position(expected);
            assert_eq!(actual, expected);
            assert!(actual.y > 0.0);
            assert!(positions.iter().all(|position| *position != actual));
            positions.push(actual);
        }
    }

    #[test]
    fn review_projection_fits_landscape_portrait_and_target_bounds() {
        let (min, max) = world_review_bounds();
        let targets = [
            min,
            max,
            Vec2::new(min.x, max.y),
            Vec2::new(max.x, min.y),
            Vec2::ZERO,
        ];
        for aspect in [16.0 / 9.0, 4.0 / 3.0, 9.0 / 16.0] {
            let (height, center_x, center_z) = review_projection_for_bounds(min, max, aspect);
            let half_width = height * aspect * 0.5;
            let half_height = height * 0.5;
            assert!(targets.iter().all(|target| {
                (target.x - center_x).abs() <= half_width
                    && (target.y - center_z).abs() <= half_height
            }));
        }
    }

    #[test]
    fn review_projection_sanitizes_unsupported_aspect() {
        let (height, _, _) = review_projection_for_bounds(Vec2::ZERO, Vec2::new(200.0, 100.0), 0.0);
        assert!(height >= 200.0);
    }

    #[test]
    fn feedback_order_helper_runs_reader_after_driving() {
        let mut app = App::new();
        app.init_resource::<ExecutionOrder>();
        configure_obstacle_feedback_order(&mut app);
        app.add_systems(Update, record_driving.in_set(DrivingSet));
        app.add_systems(Update, record_feedback.in_set(CameraObstacleFeedbackSet));
        app.update();

        assert_eq!(
            app.world().resource::<ExecutionOrder>().0,
            ["driving", "camera feedback"]
        );
    }

    #[test]
    fn normal_motion_adds_and_clamps_trauma() {
        assert_eq!(next_trauma(0.2, 4.0, false), 0.4);
        assert_eq!(next_trauma(0.8, 12.0, false), 1.0);
    }

    #[test]
    fn reduced_motion_does_not_add_trauma() {
        assert_eq!(next_trauma(0.35, 12.0, true), 0.35);
    }

    #[test]
    fn reduced_motion_uses_fixed_zoom_at_every_speed() {
        assert_eq!(camera_viewport_height(12.0, 12.0, 12.0, 0.1, true), 10.0);
        assert_eq!(camera_viewport_height(9.0, 0.0, 12.0, 0.9, true), 10.0);
        assert!(camera_viewport_height(10.0, 12.0, 12.0, 0.5, false) > 10.0);
    }
}
