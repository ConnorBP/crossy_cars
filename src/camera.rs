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
        CameraObstacleFeedbackSet, camera_viewport_height, configure_obstacle_feedback_order,
        next_trauma,
    };
    use crate::car::DrivingSet;
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
