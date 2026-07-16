use bevy::{camera::ScalingMode, prelude::*};
// T9 rendering beef-up: explicit tonemapping + bloom on the 3D camera.
// `Bloom` is a separate component in 0.19 (not a `Camera3d` field) and it
// `#[require(Hdr)]`s the HDR render target, which is what lets bloom +
// tonemapping work. `Tonemapping` is already a required component of `Camera3d`
// (default `TonyMcMapface`) but we set it explicitly to make the intent clear.
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::post_process::bloom::Bloom;
#[cfg(not(target_arch = "wasm32"))]
use bevy::{
    anti_alias::fxaa::Fxaa,
    pbr::{ContactShadows, ScreenSpaceAmbientOcclusion, ScreenSpaceAmbientOcclusionQualityLevel},
};

use crate::car::{Car, DrivingSet, ImportedCarReady, ImportedCarSceneRoot, PlayerCarVisual};
use crate::game::SpawnSet;
use crate::game::events::ObstacleHit;
use crate::game::resources::{Drowning, GameConfig, RoundActive};
use crate::game::state::GameState;
use crate::settings::Settings;
use crate::world::world_review_bounds;

/// Aspect ratio around which gameplay framing balances horizontal and vertical
/// coverage. At this ratio the former 10-to-12 unit vertical view is exact.
const GAMEPLAY_REFERENCE_ASPECT: f32 = 16.0 / 9.0;
const GAMEPLAY_BASELINE_HEIGHT: f32 = 10.0;
const GAMEPLAY_MAX_HEIGHT: f32 = 12.0;
const GAMEPLAY_MIN_ASPECT: f32 = 0.4;
const GAMEPLAY_MAX_ASPECT: f32 = 2.5;
/// Exponential smoothing rate for speed zoom (higher = snappier).
const SMOOTH: f32 = 4.0;
/// Direction and composition deliberately settle independently. Travel
/// direction damps slowly, while composition has its own response and tighter
/// screen-axis slew caps so a sharp correction cannot whip across the frame.
const DIRECTION_SMOOTH: f32 = 2.0;
const LEAD_SMOOTH: f32 = 3.0;
/// Full-speed lead is intentionally modest: Roady remains comfortably inside
/// the frame instead of riding the old 80% NDC edge.
const TRAILING_NDC_LIMIT: f32 = 0.42;
/// Normal driving (max speed 12) is followed without accumulating a permanent
/// absolute lag. Only exceptional collision pushout is rate limited.
const ANCHOR_MAX_SPEED: f32 = 18.0;
/// Screen-space lead slew bounds. The tighter vertical bound is important for
/// the fixed isometric projection: lateral world lead otherwise reads mostly
/// as a distracting up/down camera move during turns.
const LEAD_HORIZONTAL_NDC_PER_SECOND: f32 = 0.9;
const LEAD_VERTICAL_NDC_PER_SECOND: f32 = 0.28;
// Only genuine large relocations bypass the per-frame anchor cap. Ordinary
// obstacle/collision correction remains far below this 20-unit threshold.
const TELEPORT_DISTANCE_SQ: f32 = 400.0;

/// State local to the production gameplay camera. World-review cameras never
/// receive this component, so their deterministic atlas framing stays fixed.
#[derive(Component)]
struct GameplayCamera {
    previous_car_xz: Vec2,
    travel_direction: Vec2,
    smoothed_car_xz: Vec2,
    smoothed_lead_ndc: Vec2,
    /// Speed-driven height at the reference aspect. Keeping this independently
    /// from the derived projection height prevents a resize from becoming a
    /// false zoom input on the next frame.
    reference_viewport_height: f32,
    initialized: bool,
}

impl Default for GameplayCamera {
    fn default() -> Self {
        Self {
            previous_car_xz: Vec2::ZERO,
            travel_direction: Vec2::Y,
            smoothed_car_xz: Vec2::ZERO,
            smoothed_lead_ndc: Vec2::ZERO,
            reference_viewport_height: GAMEPLAY_BASELINE_HEIGHT,
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

fn camera_follow_allowed(drowning: Res<Drowning>) -> bool {
    !drowning.active || drowning.camera_capture_pending
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
        app.init_resource::<CarReviewReadyDelay>()
            .add_systems(Startup, spawn_car_review_studio)
            .add_systems(Update, mark_car_review_ready);
    }
}

/// The imported glTF hierarchy is instantiated asynchronously. Require one
/// complete ECS update after its semantic bindings become ready before the DOM
/// marker lets the capture harness begin its own multi-frame render checks.
#[derive(Resource, Default)]
struct CarReviewReadyDelay {
    observed_updates: u8,
    marked: bool,
}

fn mark_car_review_ready(
    visual: Res<PlayerCarVisual>,
    imported_roots: Query<(), With<ImportedCarSceneRoot>>,
    imported_ready: Query<(), (With<ImportedCarSceneRoot>, With<ImportedCarReady>)>,
    cars: Query<(), With<Car>>,
    mut delay: ResMut<CarReviewReadyDelay>,
) {
    if delay.marked {
        return;
    }
    let visual_ready = match *visual {
        PlayerCarVisual::ImportedConcept => {
            imported_roots.iter().count() == 1 && imported_ready.iter().count() == 1
        }
        PlayerCarVisual::LegacyProcedural => cars.iter().count() == 1,
    };
    if !visual_ready {
        delay.observed_updates = 0;
        return;
    }
    delay.observed_updates = delay.observed_updates.saturating_add(1);
    if delay.observed_updates < 2 {
        return;
    }

    #[cfg(target_arch = "wasm32")]
    if let Some(root) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.document_element())
    {
        let _ = root.set_attribute("data-roady-car-review-ready", "true");
    }
    delay.marked = true;
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
        // Cylinder height is 0.05, so -0.025 puts its top exactly at Y=0,
        // matching production road contact instead of clipping tire bottoms.
        Transform::from_xyz(0.0, -0.025, 0.0),
    ));
    commands.spawn((
        DirectionalLight {
            color: Color::srgb(1.0, 0.96, 0.90),
            illuminance: 12_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(-3.0, 5.0, -4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight {
            color: Color::srgb(0.72, 0.82, 1.0),
            illuminance: 5_000.0,
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
    let mut camera = commands.spawn((
        Camera3d::default(),
        Tonemapping::TonyMcMapface,
        Projection::from(OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical { viewport_height },
            ..OrthographicProjection::default_3d()
        }),
        Transform::from_xyz(center_x, 400.0, center_z)
            .looking_at(Vec3::new(center_x, 0.0, center_z), Vec3::NEG_Z),
    ));
    configure_platform_safe_camera(&mut camera);
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
                OnEnter(GameState::Playing),
                reset_camera_framing_for_fresh_round.before(SpawnSet),
            )
            .add_systems(
                Update,
                (handle_obstacle_hits, follow_camera)
                    .chain()
                    // The hit reader must observe collision messages emitted
                    // by this frame's driving chain, not schedule-dependent
                    // previous-frame data.
                    .in_set(CameraObstacleFeedbackSet)
                    .run_if(in_state(GameState::Playing))
                    .run_if(camera_follow_allowed),
            );
    }
}

fn spawn_camera(mut commands: Commands) {
    let mut camera = commands.spawn((
        Camera3d::default(),
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
                viewport_height: GAMEPLAY_BASELINE_HEIGHT,
            },
            ..OrthographicProjection::default_3d()
        }),
        // T7 fix preserved: the iso rotation is set ONCE here at spawn via
        // `looking_at(ZERO, Y)` and NEVER recomputed per frame. `follow_camera`
        // only changes translation + the ortho viewport height (zoom).
        Transform::from_xyz(12.0, 12.0, 12.0).looking_at(Vec3::ZERO, Vec3::Y),
        GameplayCamera::default(),
    ));
    configure_platform_safe_camera(&mut camera);
}

/// SSAO is unavailable on WebGL2 and is incompatible with multisampling.
/// Native cameras therefore pair medium SSAO with FXAA, while browser cameras
/// retain their established four-sample MSAA profile.
fn configure_platform_safe_camera(camera: &mut EntityCommands) {
    #[cfg(not(target_arch = "wasm32"))]
    camera.insert((
        Msaa::Off,
        ContactShadows {
            linear_steps: 8,
            thickness: 0.1,
            length: 0.3,
        },
        ScreenSpaceAmbientOcclusion {
            quality_level: ScreenSpaceAmbientOcclusionQualityLevel::Medium,
            ..default()
        },
        Fxaa::default(),
    ));

    #[cfg(target_arch = "wasm32")]
    camera.insert(Msaa::Sample4);
}

fn reset_camera_framing_for_fresh_round(
    round_active: Res<RoundActive>,
    mut cameras: Query<&mut GameplayCamera>,
) {
    // RoundActive remains true across pause/resume and is false only for a
    // genuinely fresh round. Reset explicitly instead of inferring restart
    // from distance to the origin, which fails for nearby terminal positions.
    if round_active.0 {
        return;
    }
    for mut framing in &mut cameras {
        *framing = GameplayCamera::default();
    }
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
    mut drowning: ResMut<Drowning>,
) {
    // Follow normally, or exactly once after entry so the camera captures the
    // final resolved pose. A failed query keeps pending set for a later frame.
    if drowning.active && !drowning.camera_capture_pending {
        return;
    }
    let Ok((car_t, car)) = car.single() else {
        return;
    };
    let Ok((mut cam_t, mut proj, mut framing)) = camera.single_mut() else {
        return;
    };
    let capture_pending = drowning.camera_capture_pending;

    let dt = time.delta_secs();
    let t = exp_smoothing_alpha(SMOOTH, dt);
    let car_xz = Vec2::new(car_t.translation.x, car_t.translation.z);
    let displacement = car_xz - framing.previous_car_xz;
    let desired_travel = travel_direction(car.heading, car.drift, car.speed);
    let follow_error = car_xz - framing.smoothed_car_xz;

    if !framing.initialized
        || !displacement.is_finite()
        || !follow_error.is_finite()
        || displacement.length_squared() > TELEPORT_DISTANCE_SQ
        || follow_error.length_squared() > TELEPORT_DISTANCE_SQ
    {
        framing.previous_car_xz = car_xz;
        framing.smoothed_car_xz = car_xz;
        framing.travel_direction = desired_travel;
        framing.smoothed_lead_ndc = Vec2::ZERO;
        framing.initialized = true;
    } else {
        framing.previous_car_xz = car_xz;
        framing.smoothed_car_xz = capped_anchor_step(framing.smoothed_car_xz, car_xz, dt);
        if car.speed.abs() > 0.1 {
            framing.travel_direction = smoothed_direction(
                framing.travel_direction,
                desired_travel,
                exp_smoothing_alpha(DIRECTION_SMOOTH, dt),
            );
        }
    }

    let aspect = windows
        .single()
        .ok()
        .map(|window| {
            gameplay_aspect_for_size(window.resolution.width(), window.resolution.height())
        })
        .unwrap_or(GAMEPLAY_REFERENCE_ASPECT);
    let speed_ratio = if settings.reduced_motion || cfg.max_speed <= 0.0 {
        0.0
    } else {
        (car.speed.abs() / cfg.max_speed).clamp(0.0, 1.0)
    };
    // Smooth the speed scalar at the reference aspect, then derive both
    // dimensions together. This keeps area constant across aspect changes and
    // avoids feeding a resize-derived projection height back into speed zoom.
    framing.reference_viewport_height = camera_reference_viewport_height(
        framing.reference_viewport_height,
        car.speed,
        cfg.max_speed,
        t,
        settings.reduced_motion,
    );
    let viewport = gameplay_viewport_size(framing.reference_viewport_height, aspect);
    let projected_travel =
        projected_ground_direction(framing.travel_direction, cam_t.rotation, viewport);
    let desired_car_ndc = trailing_ndc_offset(projected_travel, TRAILING_NDC_LIMIT) * speed_ratio;
    framing.smoothed_lead_ndc = if settings.reduced_motion {
        Vec2::ZERO
    } else {
        damped_lead_ndc(framing.smoothed_lead_ndc, desired_car_ndc, dt)
    };
    let ground_offset = ground_offset_for_ndc(framing.smoothed_lead_ndc, cam_t.rotation, viewport);
    let desired = camera_target_translation(
        Vec3::new(
            framing.smoothed_car_xz.x,
            if capture_pending {
                drowning.entry_position.y
            } else {
                car_t.translation.y
            },
            framing.smoothed_car_xz.y,
        ),
        cfg.cam_offset,
        ground_offset,
    );

    // Smooth the car anchor quickly enough to avoid visible collision/pushout
    // jolts, while the slower lead offset changes the composition gently.
    cam_t.translation = desired;

    // T13: translational shake offset. Applied AFTER the independently
    // smoothed anchor/lead so the crash jolt rides on top without contaminating
    // either follow state. Trauma decay washes it out without a framing snap.
    // ONLY translation is touched; `cam_t.rotation` is never modified
    // (preserves the T7 fixed-iso-rotation no-tilt fix).
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

    // FixedVertical is updated from the fused dimensions each frame. Reduced
    // motion pins the reference scalar immediately to 10, with no fast-frame
    // smoothing residue.
    if let Projection::Orthographic(ref mut o) = *proj {
        o.scaling_mode = ScalingMode::FixedVertical {
            viewport_height: viewport.y,
        };
    }
    if capture_pending {
        drowning.camera_capture_pending = false;
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

/// Speed-driven gameplay zoom scalar at the reference aspect.
fn camera_reference_viewport_height(
    current: f32,
    speed: f32,
    max_speed: f32,
    smoothing: f32,
    reduced_motion: bool,
) -> f32 {
    if reduced_motion {
        return GAMEPLAY_BASELINE_HEIGHT;
    }
    let ratio = if max_speed.is_finite() && max_speed > 0.0 && speed.is_finite() {
        (speed.abs() / max_speed).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let target =
        GAMEPLAY_BASELINE_HEIGHT + ratio * (GAMEPLAY_MAX_HEIGHT - GAMEPLAY_BASELINE_HEIGHT);
    let current = if current.is_finite() {
        current.clamp(GAMEPLAY_BASELINE_HEIGHT, GAMEPLAY_MAX_HEIGHT)
    } else {
        GAMEPLAY_BASELINE_HEIGHT
    };
    current + (target - current) * smoothing.clamp(0.0, 1.0)
}

fn sanitize_gameplay_aspect(aspect: f32) -> f32 {
    if aspect.is_finite() && aspect > 0.0 {
        aspect.clamp(GAMEPLAY_MIN_ASPECT, GAMEPLAY_MAX_ASPECT)
    } else {
        GAMEPLAY_REFERENCE_ASPECT
    }
}

fn gameplay_aspect_for_size(width: f32, height: f32) -> f32 {
    if width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0 {
        sanitize_gameplay_aspect(width / height)
    } else {
        GAMEPLAY_REFERENCE_ASPECT
    }
}

/// Balanced constant-area fusion. Its area is always H²R, its width-to-height
/// ratio is the sanitized display aspect, and at R it exactly reproduces the
/// former fixed-vertical dimensions.
fn gameplay_viewport_size(reference_height: f32, aspect: f32) -> Vec2 {
    let reference_height = if reference_height.is_finite() {
        reference_height.clamp(GAMEPLAY_BASELINE_HEIGHT, GAMEPLAY_MAX_HEIGHT)
    } else {
        GAMEPLAY_BASELINE_HEIGHT
    };
    let aspect = sanitize_gameplay_aspect(aspect);
    if aspect == GAMEPLAY_REFERENCE_ASPECT {
        return Vec2::new(
            reference_height * GAMEPLAY_REFERENCE_ASPECT,
            reference_height,
        );
    }
    if aspect == GAMEPLAY_REFERENCE_ASPECT.recip() {
        return Vec2::new(
            reference_height,
            reference_height * GAMEPLAY_REFERENCE_ASPECT,
        );
    }
    Vec2::new(
        reference_height * (GAMEPLAY_REFERENCE_ASPECT * aspect).sqrt(),
        reference_height * (GAMEPLAY_REFERENCE_ASPECT / aspect).sqrt(),
    )
}

fn travel_direction(heading: f32, drift: f32, speed: f32) -> Vec2 {
    let angle = heading + drift;
    let forward = Vec2::new(-angle.sin(), -angle.cos());
    if speed < -0.1 { -forward } else { forward }
}

fn camera_target_translation(car: Vec3, base_offset: Vec3, ground_offset: Vec2) -> Vec3 {
    car + base_offset + Vec3::new(ground_offset.x, 0.0, ground_offset.y)
}

fn exp_smoothing_alpha(rate: f32, dt: f32) -> f32 {
    1.0 - (-rate.max(0.0) * dt.max(0.0)).exp()
}

fn capped_anchor_step(previous: Vec2, desired: Vec2, dt: f32) -> Vec2 {
    let delta = desired - previous;
    let max_step = ANCHOR_MAX_SPEED * dt.max(0.0);
    if !delta.is_finite() || max_step <= 0.0 {
        return previous;
    }
    previous + delta.clamp_length_max(max_step)
}

fn damped_lead_ndc(previous: Vec2, desired: Vec2, dt: f32) -> Vec2 {
    if !previous.is_finite() || !desired.is_finite() {
        return Vec2::ZERO;
    }
    let dt = dt.max(0.0);
    let candidate = previous.lerp(desired, exp_smoothing_alpha(LEAD_SMOOTH, dt));
    let delta = candidate - previous;
    previous
        + Vec2::new(
            delta.x.clamp(
                -LEAD_HORIZONTAL_NDC_PER_SECOND * dt,
                LEAD_HORIZONTAL_NDC_PER_SECOND * dt,
            ),
            delta.y.clamp(
                -LEAD_VERTICAL_NDC_PER_SECOND * dt,
                LEAD_VERTICAL_NDC_PER_SECOND * dt,
            ),
        )
}

/// Smooth direction along the shortest arc. Scaling the remaining angle by an
/// exponential alpha is frame-rate independent for a fixed target, cannot
/// overshoot, and still crosses an exact 180-degree reversal deterministically.
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
    let step = signed_angle * alpha.clamp(0.0, 1.0);
    Vec2::from_angle(step).rotate(previous).normalize_or_zero()
}

/// Project a world XZ travel vector into normalized screen direction.
fn projected_ground_direction(travel: Vec2, rotation: Quat, viewport: Vec2) -> Vec2 {
    let travel = Vec3::new(travel.x, 0.0, travel.y);
    let right = rotation * Vec3::X;
    let up = rotation * Vec3::Y;
    Vec2::new(
        travel.dot(right) / viewport.x.max(0.001),
        travel.dot(up) / viewport.y.max(0.001),
    )
    .normalize_or_zero()
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
fn ground_offset_for_ndc(ndc: Vec2, rotation: Quat, viewport: Vec2) -> Vec2 {
    let right = rotation * Vec3::X;
    let up = rotation * Vec3::Y;
    let target_x = -ndc.x * viewport.x * 0.5;
    let target_y = -ndc.y * viewport.y * 0.5;
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
        ANCHOR_MAX_SPEED, CameraObstacleFeedbackSet, CarReviewView, DIRECTION_SMOOTH,
        GAMEPLAY_BASELINE_HEIGHT, GAMEPLAY_MAX_ASPECT, GAMEPLAY_MAX_HEIGHT, GAMEPLAY_MIN_ASPECT,
        GAMEPLAY_REFERENCE_ASPECT, GameplayCamera, LEAD_VERTICAL_NDC_PER_SECOND,
        TRAILING_NDC_LIMIT, camera_reference_viewport_height, camera_target_translation,
        capped_anchor_step, car_review_camera_position, car_review_view,
        configure_obstacle_feedback_order, damped_lead_ndc, exp_smoothing_alpha,
        gameplay_aspect_for_size, gameplay_viewport_size, ground_offset_for_ndc, next_trauma,
        projected_ground_direction, reset_camera_framing_for_fresh_round,
        review_projection_for_bounds, smoothed_direction, spawn_camera, spawn_world_review_camera,
        trailing_ndc_offset, travel_direction,
    };
    use crate::{car::DrivingSet, game::resources::RoundActive, world::world_review_bounds};
    use bevy::pbr::ContactShadows;
    use bevy::prelude::*;
    #[cfg(not(target_arch = "wasm32"))]
    use bevy::{
        anti_alias::fxaa::Fxaa,
        pbr::{ScreenSpaceAmbientOcclusion, ScreenSpaceAmbientOcclusionQualityLevel},
    };

    #[derive(Resource, Default)]
    struct ExecutionOrder(Vec<&'static str>);

    fn record_driving(mut order: ResMut<ExecutionOrder>) {
        order.0.push("driving");
    }

    fn record_feedback(mut order: ResMut<ExecutionOrder>) {
        order.0.push("camera feedback");
    }

    fn assert_platform_safe_camera(world: &mut World, entity: Entity) {
        let msaa = world.get::<Msaa>(entity).unwrap();
        assert_eq!(
            *msaa,
            if cfg!(target_arch = "wasm32") {
                Msaa::Sample4
            } else {
                Msaa::Off
            }
        );

        #[cfg(not(target_arch = "wasm32"))]
        {
            let contact = world.get::<ContactShadows>(entity).unwrap();
            assert_eq!(contact.linear_steps, 8);
            assert_eq!(contact.thickness, 0.1);
            assert_eq!(contact.length, 0.3);
            assert_eq!(
                world
                    .get::<ScreenSpaceAmbientOcclusion>(entity)
                    .unwrap()
                    .quality_level,
                ScreenSpaceAmbientOcclusionQualityLevel::Medium
            );
            assert!(world.get::<Fxaa>(entity).unwrap().enabled);
        }

        #[cfg(target_arch = "wasm32")]
        {
            assert!(world.get::<ContactShadows>(entity).is_none());
            assert!(
                world
                    .get::<bevy::pbr::ScreenSpaceAmbientOcclusion>(entity)
                    .is_none()
            );
        }
    }

    #[test]
    fn gameplay_camera_uses_platform_safe_ao_aa_and_contact_shadows() {
        let mut app = App::new();
        app.add_systems(Startup, spawn_camera);
        app.update();
        let world = app.world_mut();
        let entity = world
            .query_filtered::<Entity, With<GameplayCamera>>()
            .single(world)
            .unwrap();
        assert_platform_safe_camera(world, entity);
    }

    #[test]
    fn world_review_camera_uses_platform_safe_ao_aa_and_contact_shadows() {
        let mut app = App::new();
        app.add_systems(Startup, spawn_world_review_camera);
        app.update();
        let world = app.world_mut();
        let entity = world
            .query_filtered::<Entity, With<Camera3d>>()
            .single(world)
            .unwrap();
        assert_platform_safe_camera(world, entity);
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
    fn reference_aspect_preserves_exact_former_rest_and_max_dimensions() {
        assert_eq!(
            gameplay_viewport_size(GAMEPLAY_BASELINE_HEIGHT, GAMEPLAY_REFERENCE_ASPECT),
            Vec2::new(160.0 / 9.0, 10.0)
        );
        assert_eq!(
            gameplay_viewport_size(GAMEPLAY_MAX_HEIGHT, GAMEPLAY_REFERENCE_ASPECT),
            Vec2::new(64.0 / 3.0, 12.0)
        );
    }

    #[test]
    fn reciprocal_aspects_swap_viewport_dimensions() {
        for height in [GAMEPLAY_BASELINE_HEIGHT, GAMEPLAY_MAX_HEIGHT] {
            let landscape = gameplay_viewport_size(height, GAMEPLAY_REFERENCE_ASPECT);
            let portrait = gameplay_viewport_size(height, GAMEPLAY_REFERENCE_ASPECT.recip());
            assert_eq!(portrait, Vec2::new(landscape.y, landscape.x));
        }
    }

    #[test]
    fn fused_viewport_has_equal_area_across_supported_aspects() {
        for height in [GAMEPLAY_BASELINE_HEIGHT, GAMEPLAY_MAX_HEIGHT] {
            let expected = height * height * GAMEPLAY_REFERENCE_ASPECT;
            for aspect in [
                GAMEPLAY_REFERENCE_ASPECT,
                4.0 / 3.0,
                1.0,
                GAMEPLAY_REFERENCE_ASPECT.recip(),
            ] {
                let viewport = gameplay_viewport_size(height, aspect);
                assert!((viewport.x * viewport.y - expected).abs() < 1e-4);
                assert!((viewport.x / viewport.y - aspect).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn fusion_interpolates_monotonically_between_landscape_and_portrait() {
        let aspects = [
            GAMEPLAY_REFERENCE_ASPECT.recip(),
            1.0,
            4.0 / 3.0,
            GAMEPLAY_REFERENCE_ASPECT,
        ];
        let mut previous = gameplay_viewport_size(GAMEPLAY_BASELINE_HEIGHT, aspects[0]);
        for aspect in aspects.into_iter().skip(1) {
            let next = gameplay_viewport_size(GAMEPLAY_BASELINE_HEIGHT, aspect);
            assert!(next.x > previous.x);
            assert!(next.y < previous.y);
            previous = next;
        }
    }

    #[test]
    fn gameplay_aspect_falls_back_and_clamps_pathological_sizes() {
        for (width, height) in [
            (0.0, 720.0),
            (1280.0, 0.0),
            (f32::NAN, 720.0),
            (1280.0, f32::INFINITY),
            (f32::INFINITY, f32::NAN),
        ] {
            assert_eq!(
                gameplay_aspect_for_size(width, height),
                GAMEPLAY_REFERENCE_ASPECT
            );
        }
        assert_eq!(gameplay_aspect_for_size(1.0, 1000.0), GAMEPLAY_MIN_ASPECT);
        assert_eq!(gameplay_aspect_for_size(1000.0, 1.0), GAMEPLAY_MAX_ASPECT);

        for aspect in [f32::NAN, 0.0, -1.0, f32::INFINITY, 0.01, 100.0] {
            let viewport = gameplay_viewport_size(GAMEPLAY_BASELINE_HEIGHT, aspect);
            let projected = projected_ground_direction(Vec2::X, gameplay_rotation(), viewport);
            let offset = ground_offset_for_ndc(Vec2::new(0.2, -0.2), gameplay_rotation(), viewport);
            assert!(viewport.is_finite() && viewport.min_element() > 0.0);
            assert!(projected.is_finite());
            assert!(offset.is_finite());
        }
    }

    #[test]
    fn reduced_motion_pins_reference_height_at_every_speed() {
        assert_eq!(
            camera_reference_viewport_height(GAMEPLAY_MAX_HEIGHT, 12.0, 12.0, 0.1, true),
            GAMEPLAY_BASELINE_HEIGHT
        );
        assert_eq!(
            camera_reference_viewport_height(9.0, 0.0, 12.0, 0.9, true),
            GAMEPLAY_BASELINE_HEIGHT
        );
        assert!(
            camera_reference_viewport_height(GAMEPLAY_BASELINE_HEIGHT, 12.0, 12.0, 0.5, false)
                > GAMEPLAY_BASELINE_HEIGHT
        );
    }

    fn gameplay_rotation() -> Quat {
        Transform::from_xyz(12.0, 12.0, 12.0)
            .looking_at(Vec3::ZERO, Vec3::Y)
            .rotation
    }

    #[test]
    fn full_speed_lead_inversion_uses_actual_landscape_square_and_portrait_viewports() {
        let rotation = gameplay_rotation();
        for aspect in [
            GAMEPLAY_REFERENCE_ASPECT,
            1.0,
            GAMEPLAY_REFERENCE_ASPECT.recip(),
        ] {
            let viewport = gameplay_viewport_size(GAMEPLAY_MAX_HEIGHT, aspect);
            for travel in [Vec2::X, Vec2::Y, Vec2::new(1.0, 1.0).normalize()] {
                let projected = projected_ground_direction(travel, rotation, viewport);
                let desired_ndc = trailing_ndc_offset(projected, TRAILING_NDC_LIMIT);
                let ground_offset = ground_offset_for_ndc(desired_ndc, rotation, viewport);
                let actual_ndc = projected_car_ndc(ground_offset, rotation, viewport);
                assert!((actual_ndc - desired_ndc).length() < 1e-4);
                assert!(actual_ndc.x.abs() <= 0.4201 && actual_ndc.y.abs() <= 0.4201);
                assert!(
                    (actual_ndc.x.abs().max(actual_ndc.y.abs()) - TRAILING_NDC_LIMIT).abs() < 1e-4
                );
                assert!(actual_ndc.dot(projected) < 0.0, "car must trail travel");
            }
        }
    }

    fn projected_car_ndc(offset: Vec2, rotation: Quat, viewport: Vec2) -> Vec2 {
        let relative = Vec3::new(-offset.x, 0.0, -offset.y);
        let right = rotation * Vec3::X;
        let up = rotation * Vec3::Y;
        Vec2::new(
            relative.dot(right) / (viewport.x * 0.5),
            relative.dot(up) / (viewport.y * 0.5),
        )
    }

    #[test]
    fn fresh_round_resets_near_origin_framing_but_pause_resume_preserves_it() {
        let stale = GameplayCamera {
            previous_car_xz: Vec2::new(3.0, 0.0),
            travel_direction: -Vec2::Y,
            smoothed_car_xz: Vec2::new(3.0, 0.0),
            smoothed_lead_ndc: Vec2::new(0.3, -0.2),
            reference_viewport_height: GAMEPLAY_MAX_HEIGHT,
            initialized: true,
        };
        let mut app = App::new();
        app.insert_resource(RoundActive(false));
        let camera = app.world_mut().spawn(stale).id();
        app.add_systems(Update, reset_camera_framing_for_fresh_round);
        app.update();

        let framing = app.world().get::<GameplayCamera>(camera).unwrap();
        assert!(!framing.initialized);
        assert_eq!(framing.smoothed_lead_ndc, Vec2::ZERO);
        assert_eq!(framing.reference_viewport_height, GAMEPLAY_BASELINE_HEIGHT);

        app.world_mut().resource_mut::<RoundActive>().0 = true;
        {
            let mut framing = app.world_mut().get_mut::<GameplayCamera>(camera).unwrap();
            framing.initialized = true;
            framing.smoothed_lead_ndc = Vec2::new(0.2, 0.1);
            framing.reference_viewport_height = 11.25;
        }
        app.update();
        let framing = app.world().get::<GameplayCamera>(camera).unwrap();
        assert!(framing.initialized);
        assert_eq!(framing.smoothed_lead_ndc, Vec2::new(0.2, 0.1));
        assert_eq!(framing.reference_viewport_height, 11.25);
    }

    #[test]
    fn travel_direction_uses_slip_and_reverses_only_for_reverse_speed() {
        let forward = travel_direction(0.0, 0.2, 8.0);
        let reverse = travel_direction(0.0, 0.2, -4.0);
        assert!((forward + reverse).length() < 1e-6);
        assert!(forward.x < 0.0 && forward.y < 0.0);
    }

    #[test]
    fn smoothed_anchor_bounds_one_frame_collision_pushout() {
        let dt = 1.0 / 60.0;
        let after = capped_anchor_step(Vec2::ZERO, Vec2::X, dt);
        assert!(after.x > 0.0);
        assert!(after.length() <= ANCHOR_MAX_SPEED * dt + 1e-6);
    }

    #[test]
    fn stable_framing_tracks_normal_car_delta_without_absolute_follow_lag() {
        let dt = 1.0 / 60.0;
        let normal_max_speed_delta = Vec2::new(0.0, 12.0 * dt);
        assert_eq!(
            capped_anchor_step(Vec2::ZERO, normal_max_speed_delta, dt),
            normal_max_speed_delta
        );

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
        let alpha = exp_smoothing_alpha(DIRECTION_SMOOTH, 1.0 / 60.0);
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
    fn ninety_degree_turn_has_bounded_anchor_and_reduced_vertical_lead_motion() {
        let dt = 1.0 / 60.0;
        let anchor = capped_anchor_step(Vec2::ZERO, Vec2::new(4.0, 4.0), dt);
        assert!(anchor.length() <= ANCHOR_MAX_SPEED * dt + 1e-6);

        let previous = Vec2::new(-TRAILING_NDC_LIMIT, 0.0);
        let desired = Vec2::new(0.0, TRAILING_NDC_LIMIT);
        let next = damped_lead_ndc(previous, desired, dt);
        assert!((next.y - previous.y).abs() <= LEAD_VERTICAL_NDC_PER_SECOND * dt + 1e-6);
        assert!((next.y - previous.y).abs() < (desired.y - previous.y).abs() * 0.05);
    }

    #[test]
    fn direction_and_lead_match_at_30_60_and_120_fps_without_overshoot() {
        let run = |hz: usize| {
            let dt = 1.0 / hz as f32;
            (0..hz).fold((Vec2::Y, Vec2::ZERO), |(direction, lead), _| {
                (
                    smoothed_direction(
                        direction,
                        Vec2::X,
                        exp_smoothing_alpha(DIRECTION_SMOOTH, dt),
                    ),
                    damped_lead_ndc(lead, Vec2::new(0.3, 0.08), dt),
                )
            })
        };
        let at_30 = run(30);
        for result in [run(60), run(120)] {
            assert!((at_30.0 - result.0).length() < 1e-5);
            assert!((at_30.1 - result.1).length() < 1e-5);
        }
        assert!(
            at_30.0.x > 0.0 && at_30.0.y > 0.0,
            "must approach, not overshoot, target"
        );
    }

    #[test]
    fn lead_release_is_monotonic_and_has_no_jitter_overshoot() {
        let mut lead = Vec2::new(0.3, -0.2);
        let mut previous_length = lead.length();
        for _ in 0..180 {
            lead = damped_lead_ndc(lead, Vec2::ZERO, 1.0 / 60.0);
            assert!(lead.length() <= previous_length + 1e-6);
            previous_length = lead.length();
        }
        assert!(lead.length() < 0.001);
    }

    #[test]
    fn production_speed_zoom_height_is_symmetric_smooth_and_bounded() {
        let forward =
            camera_reference_viewport_height(GAMEPLAY_BASELINE_HEIGHT, 12.0, 12.0, 1.0, false);
        assert_eq!(
            forward,
            camera_reference_viewport_height(GAMEPLAY_BASELINE_HEIGHT, -12.0, 12.0, 1.0, false)
        );
        assert_eq!(
            forward,
            camera_reference_viewport_height(GAMEPLAY_BASELINE_HEIGHT, 120.0, 12.0, 1.0, false)
        );
        assert_eq!(
            camera_reference_viewport_height(GAMEPLAY_BASELINE_HEIGHT, 0.0, 12.0, 1.0, false),
            GAMEPLAY_BASELINE_HEIGHT
        );
        assert_eq!(forward, GAMEPLAY_MAX_HEIGHT);

        let mut current = GAMEPLAY_BASELINE_HEIGHT;
        for _ in 0..20 {
            let next = camera_reference_viewport_height(current, 12.0, 12.0, 0.2, false);
            assert!(next > current && next <= GAMEPLAY_MAX_HEIGHT);
            current = next;
        }

        for current in [
            f32::NEG_INFINITY,
            -100.0,
            GAMEPLAY_BASELINE_HEIGHT,
            GAMEPLAY_MAX_HEIGHT,
            100.0,
            f32::INFINITY,
        ] {
            let height = camera_reference_viewport_height(current, 6.0, 12.0, 0.5, false);
            assert!(height.is_finite());
            assert!((GAMEPLAY_BASELINE_HEIGHT..=GAMEPLAY_MAX_HEIGHT).contains(&height));
        }
    }
}
