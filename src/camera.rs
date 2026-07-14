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
const DIRECTION_SMOOTH: f32 = 4.0;
const TRAILING_NDC_LIMIT: f32 = 0.8;
const REVERSE_CONFIRM_SECS: f32 = 0.18;
const TRAVEL_EPSILON_SQ: f32 = 0.0001;
const TELEPORT_DISTANCE_SQ: f32 = 16.0;

/// State local to the production gameplay camera. World-review cameras never
/// receive this component, so their deterministic atlas framing stays fixed.
#[derive(Component)]
struct GameplayCamera {
    previous_car_xz: Vec2,
    travel_direction: Vec2,
    reverse_candidate_secs: f32,
    smoothed_ground_offset: Vec2,
    initialized: bool,
}

impl Default for GameplayCamera {
    fn default() -> Self {
        Self {
            previous_car_xz: Vec2::ZERO,
            travel_direction: Vec2::Y,
            reverse_candidate_secs: 0.0,
            smoothed_ground_offset: Vec2::ZERO,
            initialized: false,
        }
    }
}

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
        GameplayCamera::default(),
    ));
}

fn follow_camera(
    car: Query<(&Transform, &Car), (With<Car>, Without<Camera3d>)>,
    mut camera: Query<
        (&mut Transform, &mut Projection, &mut GameplayCamera),
        (With<Camera3d>, Without<Car>),
    >,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    cfg: Res<GameConfig>,
    time: Res<Time>,
    settings: Res<Settings>,
    mut shake: ResMut<Shake>,
) {
    let Ok((car_t, car)) = car.single() else {
        return;
    };
    let Ok((mut cam_t, mut proj, mut framing)) = camera.single_mut() else {
        return;
    };

    let dt = time.delta_secs();
    let t = exp_smoothing_alpha(SMOOTH, dt);
    let car_xz = Vec2::new(car_t.translation.x, car_t.translation.z);
    let displacement = car_xz - framing.previous_car_xz;
    let model_forward = car_t.rotation * Vec3::NEG_Z;
    let model_forward_xz = Vec2::new(model_forward.x, model_forward.z).normalize_or_zero();

    if !framing.initialized || displacement.length_squared() > TELEPORT_DISTANCE_SQ {
        framing.previous_car_xz = car_xz;
        framing.travel_direction = if model_forward_xz == Vec2::ZERO {
            Vec2::Y
        } else {
            model_forward_xz
        };
        framing.reverse_candidate_secs = 0.0;
        framing.smoothed_ground_offset = Vec2::ZERO;
        framing.initialized = true;
    } else if displacement.length_squared() > TRAVEL_EPSILON_SQ {
        let measured = displacement.normalize();
        if measured.dot(framing.travel_direction) < -0.8 {
            framing.reverse_candidate_secs += dt;
            if framing.reverse_candidate_secs >= REVERSE_CONFIRM_SECS {
                framing.travel_direction = smoothed_direction(
                    framing.travel_direction,
                    measured,
                    exp_smoothing_alpha(DIRECTION_SMOOTH, dt),
                );
            }
        } else {
            framing.reverse_candidate_secs = 0.0;
            framing.travel_direction = smoothed_direction(
                framing.travel_direction,
                measured,
                exp_smoothing_alpha(DIRECTION_SMOOTH, dt),
            );
        }
        framing.previous_car_xz = car_xz;
    } else {
        framing.reverse_candidate_secs = 0.0;
        framing.previous_car_xz = car_xz;
    }

    let aspect = windows
        .single()
        .ok()
        .and_then(|window| {
            let height = window.resolution.height();
            (height > 0.0).then(|| window.resolution.width() / height)
        })
        .filter(|aspect| aspect.is_finite() && *aspect > 0.0)
        .unwrap_or(16.0 / 9.0);
    let speed_ratio = if settings.reduced_motion || cfg.max_speed <= 0.0 {
        0.0
    } else {
        (car.speed.abs() / cfg.max_speed).clamp(0.0, 1.0)
    };
    let viewport_height = 10.0 + speed_ratio * 2.0;
    let projected_travel =
        projected_ground_direction(framing.travel_direction, cam_t.rotation, aspect);
    let desired_car_ndc = trailing_ndc_offset(projected_travel, TRAILING_NDC_LIMIT) * speed_ratio;
    let ground_offset =
        ground_offset_for_ndc(desired_car_ndc, cam_t.rotation, viewport_height, aspect);
    framing.smoothed_ground_offset = if settings.reduced_motion {
        Vec2::ZERO
    } else {
        framing.smoothed_ground_offset.lerp(ground_offset, t)
    };
    let desired = camera_target_translation(
        car_t.translation,
        cfg.cam_offset,
        framing.smoothed_ground_offset,
    );

    // Track the car directly and smooth only the framing offset. Smoothing the
    // absolute target creates permanent speed-dependent lag that pulls the car
    // back toward center and defeats the trailing inset.
    cam_t.translation = desired;

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
        (speed.abs() / max_speed).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let target = FIXED_VIEWPORT_HEIGHT + ratio * 2.0;
    current + (target - current) * smoothing.clamp(0.0, 1.0)
}

fn camera_target_translation(car: Vec3, base_offset: Vec3, ground_offset: Vec2) -> Vec3 {
    car + base_offset + Vec3::new(ground_offset.x, 0.0, ground_offset.y)
}

fn exp_smoothing_alpha(rate: f32, dt: f32) -> f32 {
    1.0 - (-rate.max(0.0) * dt.max(0.0)).exp()
}

/// Smooth direction by a bounded angular step. Unlike normalized linear
/// interpolation, this can cross a 180-degree reversal without getting stuck.
fn smoothed_direction(previous: Vec2, desired: Vec2, alpha: f32) -> Vec2 {
    let previous = previous.normalize_or_zero();
    let desired = desired.normalize_or_zero();
    if previous == Vec2::ZERO {
        return desired;
    }
    if desired == Vec2::ZERO {
        return previous;
    }
    let cross = previous.perp_dot(desired);
    let dot = previous.dot(desired).clamp(-1.0, 1.0);
    let signed_angle = if cross.abs() < 1e-6 && dot < 0.0 {
        std::f32::consts::PI
    } else {
        cross.atan2(dot)
    };
    let step = signed_angle.clamp(-1.5 * alpha, 1.5 * alpha);
    Vec2::from_angle(step).rotate(previous).normalize_or_zero()
}

/// Project a world XZ travel vector into normalized screen direction.
fn projected_ground_direction(travel: Vec2, rotation: Quat, aspect: f32) -> Vec2 {
    let travel = Vec3::new(travel.x, 0.0, travel.y);
    let right = rotation * Vec3::X;
    let up = rotation * Vec3::Y;
    Vec2::new(travel.dot(right) / aspect.max(0.001), travel.dot(up)).normalize_or_zero()
}

/// Put the car on the opposite edge of an inset NDC rectangle from travel.
fn trailing_ndc_offset(projected_travel: Vec2, limit: f32) -> Vec2 {
    let trailing = -projected_travel.normalize_or_zero();
    let extent = trailing.x.abs().max(trailing.y.abs());
    if extent <= f32::EPSILON {
        Vec2::ZERO
    } else {
        trailing * (limit.clamp(0.0, 1.0) / extent)
    }
}

/// Invert the fixed camera's ground-XZ to NDC projection. The result is the
/// horizontal world offset added to the camera, relative to a centered follow.
fn ground_offset_for_ndc(ndc: Vec2, rotation: Quat, height: f32, aspect: f32) -> Vec2 {
    let right = rotation * Vec3::X;
    let up = rotation * Vec3::Y;
    let target_x = -ndc.x * height * aspect * 0.5;
    let target_y = -ndc.y * height * 0.5;
    let a = right.x;
    let b = right.z;
    let c = up.x;
    let d = up.z;
    let determinant = a * d - b * c;
    if determinant.abs() < 1e-6 {
        return Vec2::ZERO;
    }
    Vec2::new(
        (target_x * d - b * target_y) / determinant,
        (a * target_y - target_x * c) / determinant,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        CameraObstacleFeedbackSet, camera_target_translation, camera_viewport_height,
        configure_obstacle_feedback_order, exp_smoothing_alpha, ground_offset_for_ndc, next_trauma,
        projected_ground_direction, review_projection_for_bounds, smoothed_direction,
        trailing_ndc_offset,
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

    fn gameplay_rotation() -> Quat {
        Transform::from_xyz(12.0, 12.0, 12.0)
            .looking_at(Vec3::ZERO, Vec3::Y)
            .rotation
    }

    #[test]
    fn full_speed_framing_places_car_at_ten_percent_trailing_margin() {
        let rotation = gameplay_rotation();
        for (width, height) in [(844.0, 390.0), (960.0, 480.0), (1280.0, 720.0)] {
            let aspect = width / height;
            for travel in [Vec2::X, Vec2::Y, Vec2::new(1.0, 1.0).normalize()] {
                let projected = projected_ground_direction(travel, rotation, aspect);
                let desired_ndc = trailing_ndc_offset(projected, 0.8);
                let ground_offset = ground_offset_for_ndc(desired_ndc, rotation, 12.0, aspect);
                let actual_ndc = projected_car_ndc(ground_offset, rotation, 12.0, aspect);
                assert!((actual_ndc - desired_ndc).length() < 1e-4);
                assert!(actual_ndc.x.abs() <= 0.8001 && actual_ndc.y.abs() <= 0.8001);
                assert!((actual_ndc.x.abs().max(actual_ndc.y.abs()) - 0.8).abs() < 1e-4);
                assert!(actual_ndc.dot(projected) < 0.0, "car must trail travel");
            }
        }
    }

    fn projected_car_ndc(offset: Vec2, rotation: Quat, height: f32, aspect: f32) -> Vec2 {
        let relative = Vec3::new(-offset.x, 0.0, -offset.y);
        let right = rotation * Vec3::X;
        let up = rotation * Vec3::Y;
        Vec2::new(
            relative.dot(right) / (height * aspect * 0.5),
            relative.dot(up) / (height * 0.5),
        )
    }

    #[test]
    fn stable_framing_tracks_car_delta_without_absolute_follow_lag() {
        let base = Vec3::new(12.0, 12.0, 12.0);
        let lead = Vec2::new(4.0, -3.0);
        let before = camera_target_translation(Vec3::new(2.0, 0.0, 5.0), base, lead);
        let car_delta = Vec3::new(-1.5, 0.0, 0.75);
        let after = camera_target_translation(Vec3::new(2.0, 0.0, 5.0) + car_delta, base, lead);
        assert!((after - before - car_delta).length() < 1e-6);
    }

    #[test]
    fn direction_smoothing_rejects_one_frame_reverse_noise() {
        let forward = Vec2::Y;
        let alpha = exp_smoothing_alpha(4.0, 1.0 / 60.0);
        let after_noise = smoothed_direction(forward, -forward, alpha);
        assert!(after_noise.dot(forward) > 0.99);

        let sustained = (0..120).fold(forward, |dir, _| smoothed_direction(dir, -forward, alpha));
        assert!(sustained.dot(forward) < -0.99);
    }

    #[test]
    fn exponential_smoothing_is_framerate_independent() {
        let run = |hz: usize| {
            (0..hz).fold(0.0_f32, |value, _| {
                let alpha = exp_smoothing_alpha(4.0, 1.0 / hz as f32);
                value + (1.0 - value) * alpha
            })
        };
        let at_30 = run(30);
        assert!((at_30 - run(60)).abs() < 1e-5);
        assert!((at_30 - run(120)).abs() < 1e-5);
    }

    #[test]
    fn speed_zoom_is_symmetric_and_bounded() {
        let forward = camera_viewport_height(10.0, 12.0, 12.0, 1.0, false);
        assert_eq!(
            forward,
            camera_viewport_height(10.0, -12.0, 12.0, 1.0, false)
        );
        assert_eq!(
            forward,
            camera_viewport_height(10.0, 120.0, 12.0, 1.0, false)
        );
        assert_eq!(camera_viewport_height(10.0, 0.0, 12.0, 1.0, false), 10.0);
        assert!((10.0..=12.0).contains(&forward));
    }
}
