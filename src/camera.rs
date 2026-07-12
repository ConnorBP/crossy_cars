use bevy::{camera::ScalingMode, prelude::*};
// T9 rendering beef-up: explicit tonemapping + bloom on the 3D camera.
// `Bloom` is a separate component in 0.19 (not a `Camera3d` field) and it
// `#[require(Hdr)]`s the HDR render target, which is what lets bloom +
// tonemapping work. `Tonemapping` is already a required component of `Camera3d`
// (default `TonyMcMapface`) but we set it explicitly to make the intent clear.
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::post_process::bloom::Bloom;

use crate::car::Car;
use crate::game::events::ObstacleHit;
use crate::game::resources::GameConfig;
use crate::game::state::GameState;

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

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Shake>()
            .add_systems(Startup, spawn_camera)
            .add_systems(
                Update,
                (handle_obstacle_hits, follow_camera)
                    .chain()
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
        // Exponential decay so the shake tails off smoothly, not linearly.
        shake.trauma *= (-SHAKE_DECAY * dt).exp();
        if shake.trauma < 0.001 {
            shake.trauma = 0.0;
        }
    }

    // Speed zoom: widen the orthographic viewport as speed rises. The current
    // viewport_height lives in the projection itself, so we read-modify it.
    let ratio = if cfg.max_speed > 0.0 {
        car.speed.abs() / cfg.max_speed
    } else {
        0.0
    };
    let target_vh = 10.0 + ratio * 2.0;
    let current_vh = match &*proj {
        Projection::Orthographic(o) => match o.scaling_mode {
            ScalingMode::FixedVertical { viewport_height } => viewport_height,
            _ => 10.0,
        },
        _ => 10.0,
    };
    let vh = current_vh + (target_vh - current_vh) * t;
    if let Projection::Orthographic(ref mut o) = *proj {
        o.scaling_mode = ScalingMode::FixedVertical {
            viewport_height: vh,
        };
    }
}

/// T13: feed `ObstacleHit` messages into the `Shake` trauma accumulator. Runs
/// before `follow_camera` (chained) so the freshly-added trauma is applied
/// this same frame. Reads only — never touches the camera or car transform.
fn handle_obstacle_hits(mut hits: MessageReader<ObstacleHit>, mut shake: ResMut<Shake>) {
    for hit in hits.read() {
        shake.trauma = (shake.trauma + hit.impact_speed * SHAKE_SCALE).clamp(0.0, 1.0);
    }
}
