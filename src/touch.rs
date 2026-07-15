use std::f32::consts::{PI, TAU};

use bevy::{prelude::*, text::FontSize, window::PrimaryWindow};

use crate::car::{Car, InputFrozen, PlayerInput, TouchInputSet};
use crate::game::{
    RestartRequested, StateAction, TouchStateSet, apply_state_action, settings_closed,
    state::GameState,
};

const PAUSE_TOP: f32 = 0.14;
const PAUSE_LEFT: f32 = 0.44;
const PAUSE_RIGHT: f32 = 0.56;
const ACTION_BRAKE_SPEED: f32 = 0.15;
const TOUCH_JITTER_PX: f32 = 6.0;
const TOUCH_ENGAGE_PX: f32 = 8.0;
const TOUCH_JITTER_EPSILON_PX: f32 = 0.01;
const TOUCH_DIRECTION_SMOOTH: f32 = 12.0;
const FLOATING_ORIGIN_RADIUS_PX: f32 = 96.0;
const FULL_STEER_ERROR: f32 = PI / 3.0;

// Fixed touch-only HUD composition. These values are shared by the live
// nodes in their owning modules and the pure all-panel layout audit below.
pub(crate) const TOUCH_COCKPIT_LEFT: f32 = 14.0;
pub(crate) const TOUCH_COCKPIT_TOP: f32 = 12.0;
pub(crate) const TOUCH_COCKPIT_WIDTH: f32 = 150.0;
pub(crate) const TOUCH_COCKPIT_HEIGHT: f32 = 100.0;
pub(crate) const TOUCH_HEALTH_LEFT: f32 = 14.0;
pub(crate) const TOUCH_HEALTH_TOP: f32 = 136.0;
pub(crate) const TOUCH_HEALTH_WIDTH: f32 = 190.0;
pub(crate) const TOUCH_HEALTH_HEIGHT: f32 = 52.0;
pub(crate) const TOUCH_POWERUP_LEFT: f32 = 210.0;
pub(crate) const TOUCH_POWERUP_TOP: f32 = 206.0;
pub(crate) const TOUCH_POWERUP_WIDTH: f32 = 142.0;
pub(crate) const TOUCH_POWERUP_HEIGHT: f32 = 52.0;
pub(crate) const TOUCH_OBJECTIVE_TOP: f32 = 54.0;
pub(crate) const TOUCH_OBJECTIVE_WIDTH: f32 = 300.0;
pub(crate) const TOUCH_OBJECTIVE_HEIGHT: f32 = 32.0;
pub(crate) const TOUCH_EVENT_LEFT: f32 = 370.0;
pub(crate) const TOUCH_EVENT_TOP: f32 = 194.0;
pub(crate) const TOUCH_EVENT_WIDTH: f32 = 250.0;
pub(crate) const TOUCH_EVENT_HEIGHT: f32 = 30.0;
pub(crate) const TOUCH_TIMER_TOP: f32 = 12.0;
pub(crate) const TOUCH_TIMER_RIGHT: f32 = 16.0;
pub(crate) const TOUCH_TIMER_WIDTH: f32 = 132.0;
pub(crate) const TOUCH_TIMER_HEIGHT: f32 = 36.0;
pub(crate) const TOUCH_MINIMAP_TOP: f32 = 60.0;
pub(crate) const TOUCH_MINIMAP_RIGHT: f32 = 16.0;
pub(crate) const TOUCH_MINIMAP_OUTER_SIZE: f32 = 108.0;
pub(crate) const TOUCH_LEVEL_TOP: f32 = 180.0;
pub(crate) const TOUCH_LEVEL_RIGHT: f32 = 16.0;
pub(crate) const TOUCH_LEVEL_WIDTH: f32 = 48.0;
pub(crate) const TOUCH_LEVEL_HEIGHT: f32 = 26.0;

const PORTRAIT_INSET: f32 = 8.0;
const PORTRAIT_COCKPIT_WIDTH: f32 = 120.0;
const PORTRAIT_TIMER_WIDTH: f32 = 110.0;
const PORTRAIT_OBJECTIVE_TOP: f32 = 270.0;
const PORTRAIT_OBJECTIVE_MAX_WIDTH: f32 = 320.0;
const PORTRAIT_MINIMAP_TOP: f32 = 56.0;
const PORTRAIT_MINIMAP_SIZE: f32 = 96.0;
const NARROW_PORTRAIT_MINIMAP_SIZE: f32 = 88.0;
const NARROW_PORTRAIT_WIDTH: f32 = 360.0;

const TOUCH_INSTRUCTION_HEIGHT: f32 = 44.0;
const TOUCH_INSTRUCTION_INSET: f32 = 14.0;

/// Top-left-origin pixel bounds used to verify HUD separation without an ECS
/// world or renderer. Touch, health, and pickup UI share this representation.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ScreenBounds {
    pub(crate) left: f32,
    pub(crate) top: f32,
    pub(crate) right: f32,
    pub(crate) bottom: f32,
}

#[allow(dead_code)]
impl ScreenBounds {
    pub(crate) fn overlaps(self, other: Self) -> bool {
        self.left < other.right
            && self.right > other.left
            && self.top < other.bottom
            && self.bottom > other.top
    }

    pub(crate) fn width(self) -> f32 {
        self.right - self.left
    }

    pub(crate) fn height(self) -> f32 {
        self.bottom - self.top
    }

    #[cfg(test)]
    fn contains(self, other: Self) -> bool {
        self.left <= other.left
            && self.top <= other.top
            && self.right >= other.right
            && self.bottom >= other.bottom
    }
}

const fn fixed_bounds(left: f32, top: f32, width: f32, height: f32) -> ScreenBounds {
    ScreenBounds {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

fn centered_bounds(viewport_width: f32, top: f32, width: f32, height: f32) -> ScreenBounds {
    let width = width.min(viewport_width);
    fixed_bounds((viewport_width - width) * 0.5, top, width, height)
}

/// Portrait is deliberately a touch-only policy. Keyboard-driven portrait
/// windows and every landscape viewport keep the established pixel layout.
pub(crate) fn is_touch_portrait(viewport: Vec2) -> bool {
    viewport.x.is_finite()
        && viewport.y.is_finite()
        && viewport.x > 0.0
        && viewport.y > 0.0
        && viewport.x < viewport.y
}

/// Painted bounds for every persistent/active touch HUD panel. Owning UI
/// modules read their geometry from this pure model when portrait touch is
/// active; the landscape branch is the original fixed composition.
#[allow(dead_code)]
pub(crate) fn touch_hud_bounds(viewport: Vec2) -> [ScreenBounds; 10] {
    let portrait = is_touch_portrait(viewport);
    let cockpit = if portrait {
        // Below 320px, preserve separation from the percentage-based pause
        // target by yielding width symmetrically with the timer.
        let width = PORTRAIT_COCKPIT_WIDTH.min((viewport.x * PAUSE_LEFT - 20.0).max(96.0));
        fixed_bounds(PORTRAIT_INSET, PORTRAIT_INSET, width, TOUCH_COCKPIT_HEIGHT)
    } else {
        fixed_bounds(
            TOUCH_COCKPIT_LEFT,
            TOUCH_COCKPIT_TOP,
            TOUCH_COCKPIT_WIDTH,
            TOUCH_COCKPIT_HEIGHT,
        )
    };
    let objective = if portrait {
        centered_bounds(
            viewport.x,
            PORTRAIT_OBJECTIVE_TOP,
            (viewport.x - PORTRAIT_INSET * 2.0)
                .max(0.0)
                .min(PORTRAIT_OBJECTIVE_MAX_WIDTH),
            TOUCH_OBJECTIVE_HEIGHT,
        )
    } else {
        centered_bounds(
            viewport.x,
            TOUCH_OBJECTIVE_TOP,
            TOUCH_OBJECTIVE_WIDTH,
            TOUCH_OBJECTIVE_HEIGHT,
        )
    };
    let timer_width = if portrait {
        PORTRAIT_TIMER_WIDTH.min((viewport.x * (1.0 - PAUSE_RIGHT) - 20.0).max(96.0))
    } else {
        TOUCH_TIMER_WIDTH
    };
    let timer_right = if portrait {
        PORTRAIT_INSET
    } else {
        TOUCH_TIMER_RIGHT
    };
    let minimap_size = if portrait {
        if viewport.x < NARROW_PORTRAIT_WIDTH {
            NARROW_PORTRAIT_MINIMAP_SIZE
        } else {
            PORTRAIT_MINIMAP_SIZE
        }
    } else {
        TOUCH_MINIMAP_OUTER_SIZE
    };
    let minimap_right = if portrait {
        PORTRAIT_INSET
    } else {
        TOUCH_MINIMAP_RIGHT
    };
    let minimap_top = if portrait {
        PORTRAIT_MINIMAP_TOP
    } else {
        TOUCH_MINIMAP_TOP
    };

    [
        cockpit,
        fixed_bounds(
            TOUCH_HEALTH_LEFT,
            TOUCH_HEALTH_TOP,
            TOUCH_HEALTH_WIDTH,
            TOUCH_HEALTH_HEIGHT,
        ),
        fixed_bounds(
            TOUCH_POWERUP_LEFT,
            TOUCH_POWERUP_TOP,
            TOUCH_POWERUP_WIDTH,
            TOUCH_POWERUP_HEIGHT,
        ),
        objective,
        centered_bounds(viewport.x, 98.0, 144.0, 80.0),
        fixed_bounds(
            TOUCH_EVENT_LEFT,
            TOUCH_EVENT_TOP,
            TOUCH_EVENT_WIDTH,
            TOUCH_EVENT_HEIGHT,
        ),
        fixed_bounds(
            viewport.x - timer_right - timer_width,
            if portrait {
                PORTRAIT_INSET
            } else {
                TOUCH_TIMER_TOP
            },
            timer_width,
            TOUCH_TIMER_HEIGHT,
        ),
        fixed_bounds(
            viewport.x - minimap_right - minimap_size,
            minimap_top,
            minimap_size,
            minimap_size,
        ),
        fixed_bounds(
            viewport.x - TOUCH_LEVEL_RIGHT - TOUCH_LEVEL_WIDTH,
            TOUCH_LEVEL_TOP,
            TOUCH_LEVEL_WIDTH,
            TOUCH_LEVEL_HEIGHT,
        ),
        fixed_bounds(
            viewport.x * PAUSE_LEFT,
            4.0,
            viewport.x * (PAUSE_RIGHT - PAUSE_LEFT),
            28.0,
        ),
    ]
}

pub(crate) fn touch_cockpit_bounds(viewport: Vec2) -> ScreenBounds {
    touch_hud_bounds(viewport)[0]
}

pub(crate) fn touch_objective_bounds(viewport: Vec2) -> ScreenBounds {
    touch_hud_bounds(viewport)[3]
}

pub(crate) fn touch_timer_bounds(viewport: Vec2) -> ScreenBounds {
    touch_hud_bounds(viewport)[6]
}

pub(crate) fn touch_minimap_bounds(viewport: Vec2) -> ScreenBounds {
    touch_hud_bounds(viewport)[7]
}

/// Painted bounds of the low-profile, full-width touch instruction band.
/// Touch roles themselves are position-independent and have no driving hitbox.
#[allow(dead_code)]
pub(crate) fn touch_driving_band_bounds(viewport: Vec2) -> ScreenBounds {
    ScreenBounds {
        left: 0.0,
        top: (viewport.y - TOUCH_INSTRUCTION_HEIGHT).max(0.0),
        right: viewport.x,
        bottom: viewport.y,
    }
}

#[cfg(test)]
fn touch_control_label_bounds(viewport: Vec2) -> [ScreenBounds; 2] {
    let band = touch_driving_band_bounds(viewport);
    let midpoint = viewport.x * 0.5;
    [
        ScreenBounds {
            left: TOUCH_INSTRUCTION_INSET,
            right: midpoint,
            ..band
        },
        ScreenBounds {
            left: midpoint,
            right: (viewport.x - TOUCH_INSTRUCTION_INSET).max(midpoint),
            ..band
        },
    ]
}

/// Becomes sticky after the first touch so touch-only guidance does not appear
/// for keyboard/mouse players.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct TouchControlsActive(pub bool);

#[derive(Component)]
struct TouchHudRoot;

#[derive(Component)]
struct TouchGuidanceRoot;

#[derive(Component)]
struct TouchInstructionBand;

#[derive(Component)]
struct TouchInstructionLabel(usize);

/// The first eligible live touch anywhere owns direction until it is
/// released/cancelled. Other eligible touches supply the action role.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
struct DriveTouchOwner(Option<u64>);

/// Stateful analog stick hidden behind the position-independent direction
/// touch. The origin follows only when a drag exceeds a generous radius;
/// direction itself is low-pass filtered and has separate engage/release
/// thresholds so finger noise around center cannot chatter steering.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq)]
struct TouchSteering {
    owner: Option<u64>,
    window_size: Vec2,
    origin_px: Vec2,
    filtered_drag: Vec2,
    engaged: bool,
}

#[derive(Debug, Clone, Copy)]
struct ActiveTouch {
    id: u64,
    start: Vec2,
    current: Vec2,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct TouchIntent {
    /// A live eligible owner always supplies gas, even before it moves.
    owner: bool,
    /// Ground-plane direction requested by a drag larger than touch jitter.
    desired_direction: Option<Vec3>,
    action: bool,
}

pub struct TouchPlugin;

impl Plugin for TouchPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TouchControlsActive>()
            .init_resource::<DriveTouchOwner>()
            .init_resource::<TouchSteering>()
            .add_systems(
                Update,
                touch_state_transitions
                    .in_set(TouchStateSet)
                    .run_if(settings_closed),
            )
            .add_systems(
                Update,
                read_touch_input
                    .in_set(TouchInputSet)
                    .run_if(in_state(GameState::Playing)),
            )
            .add_systems(OnEnter(GameState::Playing), spawn_touch_hud)
            .add_systems(
                OnExit(GameState::Playing),
                (despawn_marker::<TouchHudRoot>, reset_drive_touch_owner),
            )
            .add_systems(OnEnter(GameState::Paused), spawn_paused_guidance)
            .add_systems(
                OnExit(GameState::Paused),
                despawn_marker::<TouchGuidanceRoot>,
            )
            // GameOver touch guidance is owned by LeaderboardPlugin while
            // initials/submission controls are active; omitting the old broad
            // tap-zone banner avoids showing actions that are temporarily gated.
            .add_systems(
                OnExit(GameState::GameOver),
                despawn_marker::<TouchGuidanceRoot>,
            )
            .add_systems(
                Update,
                (update_touch_visibility, update_touch_hud_layout)
                    .chain()
                    .after(TouchStateSet),
            );
    }
}

/// Convert Bevy's logical, top-left-origin touch coordinates to a clamped
/// viewport fraction. A minimized/zero-sized window has no usable position.
fn normalize_position(position: Vec2, window_size: Vec2) -> Option<Vec2> {
    if window_size.x <= 0.0
        || window_size.y <= 0.0
        || !window_size.is_finite()
        || !position.is_finite()
    {
        return None;
    }
    Some(Vec2::new(
        (position.x / window_size.x).clamp(0.0, 1.0),
        (position.y / window_size.y).clamp(0.0, 1.0),
    ))
}

fn is_pause_hitbox(position: Vec2) -> bool {
    position.y < PAUSE_TOP && (PAUSE_LEFT..=PAUSE_RIGHT).contains(&position.x)
}

fn safe_ground_normalize(direction: Vec3) -> Option<Vec3> {
    if !direction.is_finite() {
        return None;
    }
    let ground = Vec3::new(direction.x, 0.0, direction.z);
    let length_squared = ground.length_squared();
    if length_squared <= f32::EPSILON {
        None
    } else {
        Some(ground / length_squared.sqrt())
    }
}

/// Project camera-local screen axes onto the ground and normalize safely so
/// equal screen-axis components remain an equal analog diagonal after tilt.
fn camera_ground_basis(camera: &GlobalTransform) -> (Vec3, Vec3) {
    camera_ground_basis_from_rotation(camera.rotation())
}

fn camera_ground_basis_from_rotation(rotation: Quat) -> (Vec3, Vec3) {
    (
        safe_ground_normalize(rotation * Vec3::X).unwrap_or(Vec3::ZERO),
        safe_ground_normalize(rotation * Vec3::Y).unwrap_or(Vec3::ZERO),
    )
}

/// Convert a top-left-origin screen drag (`current - start`, in logical pixels)
/// into a world-space desired heading. Positive screen Y points down, hence
/// the subtraction of the camera's screen-up basis. Six pixels or less is
/// treated only as touch jitter, not as a heading request.
fn screen_drag_to_world(drag: Vec2, screen_right: Vec3, screen_up: Vec3) -> Option<Vec3> {
    // Fractional viewport conversion can introduce a few ULPs when an exact
    // six-pixel touch coordinate is divided and multiplied back.
    if !drag.is_finite()
        || drag.length_squared() <= (TOUCH_JITTER_PX + TOUCH_JITTER_EPSILON_PX).powi(2)
    {
        return None;
    }
    let right = safe_ground_normalize(screen_right).unwrap_or(Vec3::ZERO);
    let up = safe_ground_normalize(screen_up).unwrap_or(Vec3::ZERO);
    safe_ground_normalize(right * drag.x - up * drag.y)
}

fn wrapped_angle_error(desired: f32, current: f32) -> f32 {
    let error = (desired - current + PI).rem_euclid(TAU) - PI;
    // The antipode has two equally short turns. Preserve the raw error's sign
    // so exact +PI requests steer left and exact -PI requests steer right.
    if error == -PI && desired - current > 0.0 {
        PI
    } else {
        error
    }
}

/// Roady's heading convention is forward = (-sin(h), 0, -cos(h)).
fn steer_toward_world_direction(direction: Vec3, heading: f32) -> f32 {
    let Some(direction) = safe_ground_normalize(direction) else {
        return 0.0;
    };
    if !heading.is_finite() {
        return 0.0;
    }
    let desired_heading = (-direction.x).atan2(-direction.z);
    (wrapped_angle_error(desired_heading, heading) / FULL_STEER_ERROR).clamp(-1.0, 1.0)
}

fn eligible_touch(touch: &ActiveTouch) -> bool {
    !is_pause_hitbox(touch.start)
}

fn update_drive_owner(owner: Option<u64>, touches: &[ActiveTouch]) -> Option<u64> {
    // Eligibility is checked only while acquiring a role. A live owner is
    // sticky until release even if orientation changes its normalized start
    // coordinate into the pause rectangle.
    if owner.is_some_and(|id| touches.iter().any(|touch| touch.id == id)) {
        owner
    } else {
        // Touch iteration order is not stable. IDs provide a deterministic tie
        // break when multiple eligible touches begin in the same update.
        touches
            .iter()
            .filter(|touch| eligible_touch(touch))
            .map(|touch| touch.id)
            .min()
    }
}

fn touch_intent(
    owner: Option<u64>,
    touches: &[ActiveTouch],
    window_size: Vec2,
    camera_basis: Option<(Vec3, Vec3)>,
) -> TouchIntent {
    // Ownership has already been selected by `update_drive_owner`.
    let owner_touch = owner.and_then(|id| touches.iter().find(|touch| touch.id == id));
    let desired_direction = owner_touch.and_then(|touch| {
        let drag = (touch.current - touch.start) * window_size;
        let (screen_right, screen_up) = camera_basis?;
        screen_drag_to_world(drag, screen_right, screen_up)
    });
    let action = owner_touch.is_some()
        && touches
            .iter()
            .any(|touch| eligible_touch(touch) && Some(touch.id) != owner);
    TouchIntent {
        owner: owner_touch.is_some(),
        desired_direction,
        action,
    }
}

fn exp_smoothing_alpha(rate: f32, dt: f32) -> f32 {
    1.0 - (-rate.max(0.0) * dt.max(0.0)).exp()
}

/// Update the virtual analog stick and return a filtered pixel drag. A newly
/// promoted owner starts at its current finger location, preventing a jump on
/// release promotion, pause/resume, or an orientation change.
fn filtered_owner_drag(
    steering: &mut TouchSteering,
    owner: Option<u64>,
    touches: &[ActiveTouch],
    window_size: Vec2,
    dt: f32,
) -> Option<Vec2> {
    let owner_touch = owner.and_then(|id| touches.iter().find(|touch| touch.id == id));
    let Some(touch) = owner_touch else {
        *steering = TouchSteering::default();
        return None;
    };
    let current_px = touch.current * window_size;
    if steering.owner != owner || steering.window_size != window_size || !current_px.is_finite() {
        *steering = TouchSteering {
            owner,
            window_size,
            origin_px: current_px,
            ..default()
        };
        return None;
    }

    let mut raw = current_px - steering.origin_px;
    let mut length = raw.length();
    if length > FLOATING_ORIGIN_RADIUS_PX {
        raw *= FLOATING_ORIGIN_RADIUS_PX / length;
        length = FLOATING_ORIGIN_RADIUS_PX;
        steering.origin_px = current_px - raw;
    }

    if steering.engaged {
        if length <= TOUCH_JITTER_PX + TOUCH_JITTER_EPSILON_PX {
            steering.engaged = false;
            steering.filtered_drag = Vec2::ZERO;
            return None;
        }
    } else if length > TOUCH_ENGAGE_PX {
        steering.engaged = true;
    } else {
        steering.filtered_drag = Vec2::ZERO;
        return None;
    }

    steering.filtered_drag = steering
        .filtered_drag
        .lerp(raw, exp_smoothing_alpha(TOUCH_DIRECTION_SMOOTH, dt));
    (steering.filtered_drag.length_squared() > (TOUCH_JITTER_PX + TOUCH_JITTER_EPSILON_PX).powi(2))
        .then_some(steering.filtered_drag)
}

fn smoothed_touch_intent(
    owner: Option<u64>,
    touches: &[ActiveTouch],
    window_size: Vec2,
    camera_basis: Option<(Vec3, Vec3)>,
    steering: &mut TouchSteering,
    dt: f32,
) -> TouchIntent {
    let mut intent = touch_intent(owner, touches, window_size, None);
    intent.desired_direction = filtered_owner_drag(steering, owner, touches, window_size, dt)
        .and_then(|drag| {
            let (right, up) = camera_basis?;
            // The state machine already applies the center deadzone.
            safe_ground_normalize(
                safe_ground_normalize(right).unwrap_or(Vec3::ZERO) * drag.x
                    - safe_ground_normalize(up).unwrap_or(Vec3::ZERO) * drag.y,
            )
        });
    intent
}

/// Merge touch intent onto keyboard input. The owner always requests forward
/// throttle and directly owns steering (zero while stationary/jittering).
/// Action retains its brake-then-reverse behavior and overrides that throttle.
fn merge_touch_input(
    keyboard: PlayerInput,
    intent: TouchIntent,
    speed: f32,
    heading: f32,
) -> PlayerInput {
    let mut result = keyboard;
    if intent.owner {
        result.throttle = 1.0;
        result.brake = false;
        result.steer = intent.desired_direction.map_or(0.0, |direction| {
            steer_toward_world_direction(direction, heading)
        });
    }
    if intent.action {
        if speed > ACTION_BRAKE_SPEED {
            result.throttle = 0.0;
            result.brake = true;
        } else {
            result.throttle = -1.0;
            result.brake = false;
        }
    }
    result
}

fn touch_state_action(state: GameState, position: Vec2) -> StateAction {
    match state {
        GameState::Menu => StateAction::Playing,
        GameState::Playing => {
            if is_pause_hitbox(position) {
                StateAction::Paused
            } else {
                StateAction::None
            }
        }
        GameState::Paused => {
            if position.x < 1.0 / 3.0 {
                StateAction::Playing
            } else if position.x < 2.0 / 3.0 {
                StateAction::Restart
            } else {
                StateAction::Menu
            }
        }
        GameState::GameOver => {
            if position.x < 2.0 / 3.0 {
                StateAction::Playing
            } else {
                StateAction::Menu
            }
        }
    }
}

/// Resolve every just-pressed touch without relying on hash iteration order.
/// The stable priority favors the most consequential state action.
fn resolve_touch_actions(actions: impl IntoIterator<Item = StateAction>) -> StateAction {
    fn priority(action: StateAction) -> u8 {
        match action {
            StateAction::None => 0,
            StateAction::Playing => 1,
            StateAction::Paused => 2,
            StateAction::Menu => 3,
            StateAction::Restart => 4,
        }
    }

    actions
        .into_iter()
        .max_by_key(|&action| priority(action))
        .unwrap_or(StateAction::None)
}

fn primary_window_size(windows: &Query<&Window, With<PrimaryWindow>>) -> Option<Vec2> {
    let window = windows.single().ok()?;
    Some(Vec2::new(window.width(), window.height()))
}

fn touch_state_transitions(
    touches: Res<Touches>,
    windows: Query<&Window, With<PrimaryWindow>>,
    state: Res<State<GameState>>,
    mut active: ResMut<TouchControlsActive>,
    mut restart: ResMut<RestartRequested>,
    mut next: ResMut<NextState<GameState>>,
) {
    let mut just_pressed = touches.iter_just_pressed().peekable();
    if just_pressed.peek().is_none() {
        return;
    }
    active.0 = true;

    let Some(window_size) = primary_window_size(&windows) else {
        return;
    };
    let action = resolve_touch_actions(just_pressed.filter_map(|touch| {
        let position = normalize_position(touch.position(), window_size)?;
        Some(touch_state_action(*state.get(), position))
    }));
    apply_state_action(action, &mut restart, &mut next);
}

fn read_touch_input(
    touches: Res<Touches>,
    windows: Query<&Window, With<PrimaryWindow>>,
    frozen: Res<InputFrozen>,
    car: Query<&Car>,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    mut owner: ResMut<DriveTouchOwner>,
    mut steering: ResMut<TouchSteering>,
    mut input: ResMut<PlayerInput>,
    time: Res<Time>,
) {
    let Some(window_size) = primary_window_size(&windows) else {
        return;
    };

    let active_touches: Vec<_> = touches
        .iter()
        .filter_map(|touch| {
            Some(ActiveTouch {
                id: touch.id(),
                start: normalize_position(touch.start_position(), window_size)?,
                current: normalize_position(touch.position(), window_size)?,
            })
        })
        .collect();
    owner.0 = update_drive_owner(owner.0, &active_touches);
    if frozen.0 {
        *input = PlayerInput::default();
        // Track role ownership during countdown/input freezes, but suppress its
        // effect. Playing state exit is the sole unconditional owner reset.
        return;
    }
    let camera_basis = cameras
        .iter()
        .find(|(camera, _)| camera.is_active)
        .map(|(_, transform)| camera_ground_basis(transform));
    let Ok(car) = car.single() else {
        return;
    };
    let (speed, heading) = (car.speed, car.heading);
    let intent = smoothed_touch_intent(
        owner.0,
        &active_touches,
        window_size,
        camera_basis,
        &mut steering,
        time.delta_secs(),
    );
    *input = merge_touch_input(*input, intent, speed, heading);
    // Touch never writes Handbrake: keyboard Shift remains wholly owned by the
    // keyboard system and cannot be clobbered by touch release/cancel.
}

fn reset_drive_touch_owner(
    owner: Option<ResMut<DriveTouchOwner>>,
    steering: Option<ResMut<TouchSteering>>,
) {
    if let Some(mut owner) = owner {
        owner.0 = None;
    }
    if let Some(mut steering) = steering {
        *steering = TouchSteering::default();
    }
}

fn spawn_touch_hud(
    mut commands: Commands,
    active: Res<TouchControlsActive>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let portrait = windows
        .single()
        .ok()
        .is_some_and(|window| is_touch_portrait(Vec2::new(window.width(), window.height())));
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: px(0.0),
                left: px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            if active.0 {
                Visibility::Visible
            } else {
                Visibility::Hidden
            },
            TouchHudRoot,
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: px(TOUCH_INSTRUCTION_INSET),
                    right: px(TOUCH_INSTRUCTION_INSET),
                    bottom: px(0.0),
                    height: px(TOUCH_INSTRUCTION_HEIGHT),
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    ..default()
                },
                BackgroundColor(if portrait {
                    Color::srgba(0.015, 0.02, 0.035, 0.68)
                } else {
                    Color::srgba(0.02, 0.02, 0.03, 0.22)
                }),
                TouchInstructionBand,
            ))
            .with_children(|band| {
                for (index, label) in touch_instruction_labels(portrait).into_iter().enumerate() {
                    band.spawn((
                        Node {
                            width: Val::Percent(50.0),
                            height: Val::Percent(100.0),
                            align_items: AlignItems::Center,
                            justify_content: JustifyContent::Center,
                            ..default()
                        },
                        Text::new(label),
                        TextFont {
                            font_size: FontSize::Px(if portrait { 12.0 } else { 15.0 }),
                            ..default()
                        },
                        TextColor(if portrait {
                            Color::srgba(1.0, 1.0, 1.0, 0.9)
                        } else {
                            Color::srgba(1.0, 1.0, 1.0, 0.58)
                        }),
                        TouchInstructionLabel(index),
                    ));
                }
            });
            root.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    top: px(4.0),
                    left: Val::Percent(44.0),
                    width: Val::Percent(12.0),
                    height: px(28.0),
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::Center,
                    ..default()
                },
                BackgroundColor(Color::srgba(0.02, 0.02, 0.03, 0.22)),
                Text::new("PAUSE"),
                TextFont {
                    font_size: FontSize::Px(15.0),
                    ..default()
                },
                TextColor(Color::srgba(1.0, 1.0, 1.0, 0.58)),
            ));
        });
}

fn touch_instruction_labels(portrait: bool) -> [&'static str; 2] {
    if portrait {
        ["DRAG TO DRIVE\n1ST TOUCH", "BRAKE / REVERSE\n2ND TOUCH"]
    } else {
        ["1ST TOUCH: DRAG TO DRIVE", "2ND TOUCH: BRAKE / REVERSE"]
    }
}

fn update_touch_hud_layout(
    active: Res<TouchControlsActive>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut bands: Query<&mut BackgroundColor, With<TouchInstructionBand>>,
    mut labels: Query<
        (
            &TouchInstructionLabel,
            &mut Text,
            &mut TextFont,
            &mut TextColor,
        ),
        With<TouchInstructionLabel>,
    >,
) {
    if !active.0 {
        return;
    }
    let Some(window) = windows.single().ok() else {
        return;
    };
    let portrait = is_touch_portrait(Vec2::new(window.width(), window.height()));
    for mut background in &mut bands {
        background.0 = if portrait {
            Color::srgba(0.015, 0.02, 0.035, 0.68)
        } else {
            Color::srgba(0.02, 0.02, 0.03, 0.22)
        };
    }
    let copy = touch_instruction_labels(portrait);
    for (label, mut text, mut font, mut color) in &mut labels {
        **text = copy[label.0].to_string();
        font.font_size = FontSize::Px(if portrait { 12.0 } else { 15.0 });
        color.0 = if portrait {
            Color::srgba(1.0, 1.0, 1.0, 0.9)
        } else {
            Color::srgba(1.0, 1.0, 1.0, 0.58)
        };
    }
}

fn spawn_paused_guidance(mut commands: Commands, active: Res<TouchControlsActive>) {
    spawn_guidance(
        &mut commands,
        active.0,
        "LEFT: RESUME     MIDDLE: RESTART     RIGHT: MENU",
    );
}

fn spawn_guidance(commands: &mut Commands, active: bool, label: &'static str) {
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Percent(8.0),
            left: Val::Percent(4.0),
            width: Val::Percent(92.0),
            padding: UiRect::all(px(10.0)),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            ..default()
        },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.3)),
        Text::new(label),
        TextFont {
            font_size: FontSize::Px(19.0),
            ..default()
        },
        TextColor(Color::srgba(1.0, 1.0, 1.0, 0.72)),
        if active {
            Visibility::Visible
        } else {
            Visibility::Hidden
        },
        TouchGuidanceRoot,
    ));
}

fn update_touch_visibility(
    active: Res<TouchControlsActive>,
    mut roots: Query<&mut Visibility, Or<(With<TouchHudRoot>, With<TouchGuidanceRoot>)>>,
) {
    let visibility = if active.0 {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
    for mut current in &mut roots {
        *current = visibility;
    }
}

fn despawn_marker<M: Component>(mut commands: Commands, roots: Query<Entity, With<M>>) {
    for entity in &roots {
        commands.entity(entity).despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::{InputPlugin, touch::TouchPhase};

    fn touch(id: u64, start: Vec2, current: Vec2) -> ActiveTouch {
        ActiveTouch { id, start, current }
    }

    fn intent(owner: bool, desired_direction: Option<Vec3>, action: bool) -> TouchIntent {
        TouchIntent {
            owner,
            desired_direction,
            action,
        }
    }

    fn assert_vec3_close(actual: Vec3, expected: Vec3) {
        assert!(
            actual.distance(expected) < 1.0e-5,
            "expected {expected:?}, got {actual:?}"
        );
    }

    #[test]
    fn normalization_handles_zero_size_and_clamps_normal_windows() {
        assert_eq!(normalize_position(Vec2::ONE, Vec2::ZERO), None);
        assert_eq!(
            normalize_position(Vec2::new(50.0, 25.0), Vec2::new(100.0, 100.0)),
            Some(Vec2::new(0.5, 0.25))
        );
        assert_eq!(
            normalize_position(Vec2::new(-10.0, 120.0), Vec2::splat(100.0)),
            Some(Vec2::new(0.0, 1.0))
        );
    }

    fn panel_separation(a: ScreenBounds, b: ScreenBounds) -> f32 {
        let horizontal = (b.left - a.right).max(a.left - b.right);
        let vertical = (b.top - a.bottom).max(a.top - b.bottom);
        horizontal.max(vertical)
    }

    fn assert_touch_hud_layout(viewport: Vec2) {
        let bounds = touch_hud_bounds(viewport);
        let driving = touch_driving_band_bounds(viewport);
        for (index, left) in bounds.into_iter().enumerate() {
            assert!(left.left >= 0.0 && left.top >= 0.0, "{left:?}");
            assert!(
                left.right <= viewport.x && left.bottom <= viewport.y,
                "{left:?}"
            );
            assert!(!left.overlaps(driving), "{left:?} overlaps driving band");
            for right in bounds.into_iter().skip(index + 1) {
                assert!(!left.overlaps(right), "{left:?} overlaps {right:?}");
                assert!(
                    panel_separation(left, right) >= 12.0,
                    "less than 12px between {left:?} and {right:?}"
                );
            }
        }
    }

    #[test]
    fn compact_touch_hud_has_twelve_pixel_clearance_at_target_sizes() {
        for viewport in [Vec2::new(844.0, 390.0), Vec2::new(960.0, 480.0)] {
            assert_touch_hud_layout(viewport);
        }

        let viewport = Vec2::new(844.0, 390.0);
        let [
            cockpit,
            health,
            powerups,
            objective,
            combo,
            event,
            timer,
            minimap,
            level,
            pause,
        ] = touch_hud_bounds(viewport);
        assert_eq!(cockpit, fixed_bounds(14.0, 12.0, 150.0, 100.0));
        assert_eq!(health, fixed_bounds(14.0, 136.0, 190.0, 52.0));
        assert_eq!(powerups, fixed_bounds(210.0, 206.0, 142.0, 52.0));
        assert_eq!(objective, fixed_bounds(272.0, 54.0, 300.0, 32.0));
        assert_eq!(combo, fixed_bounds(350.0, 98.0, 144.0, 80.0));
        assert_eq!(event, fixed_bounds(370.0, 194.0, 250.0, 30.0));
        assert_eq!(timer, fixed_bounds(696.0, 12.0, 132.0, 36.0));
        assert_eq!(minimap, fixed_bounds(720.0, 60.0, 108.0, 108.0));
        assert_eq!(level, fixed_bounds(780.0, 180.0, 48.0, 26.0));
        assert_eq!(pause, fixed_bounds(371.36, 4.0, 101.28, 28.0));
    }

    #[test]
    fn portrait_primary_hud_bounds_are_disjoint_and_inside_narrow_viewports() {
        // These are the always-visible primary panels requested for portrait:
        // cockpit, health, objective, timer, minimap, level, and pause.
        const PRIMARY: [usize; 7] = [0, 1, 3, 6, 7, 8, 9];
        for viewport in [Vec2::new(390.0, 844.0), Vec2::new(320.0, 700.0)] {
            let bounds = touch_hud_bounds(viewport);
            for (position, index) in PRIMARY.into_iter().enumerate() {
                let panel = bounds[index];
                assert!(panel.left >= 0.0 && panel.top >= 0.0, "{panel:?}");
                assert!(
                    panel.right <= viewport.x && panel.bottom <= viewport.y,
                    "{panel:?}"
                );
                for other in PRIMARY.into_iter().skip(position + 1) {
                    assert!(
                        !panel.overlaps(bounds[other]),
                        "{panel:?} overlaps {:?}",
                        bounds[other]
                    );
                }
            }
        }

        let wide = touch_hud_bounds(Vec2::new(390.0, 844.0));
        let narrow = touch_hud_bounds(Vec2::new(320.0, 700.0));
        assert_eq!(wide[7].width(), 96.0);
        assert_eq!(narrow[7].width(), 88.0);
        assert_eq!(wide[3].width(), 320.0);
        assert_eq!(narrow[3].width(), 304.0);
        assert!(wide[3].top > wide[0].bottom);
        assert_eq!(
            touch_instruction_labels(true),
            ["DRAG TO DRIVE\n1ST TOUCH", "BRAKE / REVERSE\n2ND TOUCH"]
        );
    }

    #[test]
    fn compact_touch_hud_is_pairwise_disjoint_on_desktop_sized_touch_viewport() {
        assert_touch_hud_layout(Vec2::new(1440.0, 900.0));
    }

    #[test]
    fn full_width_instruction_band_is_low_profile_and_disjoint_from_hud() {
        for viewport in [
            Vec2::new(844.0, 390.0),
            Vec2::new(960.0, 480.0),
            Vec2::new(1440.0, 900.0),
        ] {
            let band = touch_driving_band_bounds(viewport);
            assert_eq!(band, fixed_bounds(0.0, viewport.y - 44.0, viewport.x, 44.0));

            let [first, second] = touch_control_label_bounds(viewport);
            assert!(band.contains(first));
            assert!(band.contains(second));
            assert_eq!(first.left, TOUCH_INSTRUCTION_INSET);
            assert_eq!(second.right, viewport.x - TOUCH_INSTRUCTION_INSET);
            assert_eq!(first.right, second.left);
            assert!(!first.overlaps(second));
            for panel in touch_hud_bounds(viewport) {
                assert!(!panel.overlaps(band), "{panel:?} overlaps {band:?}");
            }
        }
    }

    #[test]
    fn screen_drag_cardinals_and_diagonals_are_analog_world_directions() {
        let right = Vec3::X;
        let up = Vec3::Z;
        let cases = [
            (Vec2::new(10.0, 0.0), Vec3::X),
            (Vec2::new(-10.0, 0.0), Vec3::NEG_X),
            (Vec2::new(0.0, -10.0), Vec3::Z),
            (Vec2::new(0.0, 10.0), Vec3::NEG_Z),
            (Vec2::new(10.0, -10.0), (Vec3::X + Vec3::Z).normalize()),
            (Vec2::new(-10.0, -10.0), (Vec3::NEG_X + Vec3::Z).normalize()),
            (Vec2::new(10.0, 10.0), (Vec3::X + Vec3::NEG_Z).normalize()),
            (
                Vec2::new(-10.0, 10.0),
                (Vec3::NEG_X + Vec3::NEG_Z).normalize(),
            ),
        ];
        for (drag, expected) in cases {
            assert_vec3_close(screen_drag_to_world(drag, right, up).unwrap(), expected);
        }

        // Unequal components remain analog rather than snapping to eight ways.
        assert_vec3_close(
            screen_drag_to_world(Vec2::new(12.0, -9.0), right, up).unwrap(),
            Vec3::new(0.8, 0.0, 0.6),
        );
    }

    #[test]
    fn camera_basis_projects_local_right_and_up_onto_ground() {
        let camera = Transform::from_xyz(12.0, 12.0, 12.0).looking_at(Vec3::ZERO, Vec3::Y);
        let rotation = camera.rotation;
        let transform = GlobalTransform::from(camera);
        let (right, up) = camera_ground_basis(&transform);
        assert_vec3_close(right, Vec3::new(1.0, 0.0, -1.0).normalize());
        assert_vec3_close(up, Vec3::new(-1.0, 0.0, -1.0).normalize());
        assert!(right.dot(up).abs() < 1.0e-5);

        // Follow translation, collision shake, and world streaming must not
        // rotate touch steering: only the fixed gameplay rotation is used.
        let translated = GlobalTransform::from(Transform {
            translation: Vec3::new(-400.0, 80.0, 900.0),
            rotation,
            ..default()
        });
        assert_eq!(camera_ground_basis(&translated), (right, up));
    }

    #[test]
    fn desired_heading_uses_roady_convention_and_wraps_at_plus_minus_pi() {
        assert_eq!(steer_toward_world_direction(Vec3::NEG_Z, 0.0), 0.0);
        assert_eq!(steer_toward_world_direction(Vec3::NEG_X, 0.0), 1.0);
        assert_eq!(steer_toward_world_direction(Vec3::X, 0.0), -1.0);
        assert_eq!(wrapped_angle_error(PI, 0.0), PI);
        assert_eq!(wrapped_angle_error(-PI, 0.0), -PI);
        assert!(
            (steer_toward_world_direction(
                Vec3::new(-FULL_STEER_ERROR.sin(), 0.0, -FULL_STEER_ERROR.cos()),
                0.0,
            ) - 1.0)
                .abs()
                < 1.0e-5
        );
        assert!(
            (steer_toward_world_direction(Vec3::new(-0.5, 0.0, -0.866_025_4), 0.0) - 0.5).abs()
                < 1.0e-5
        );

        let epsilon = 0.02;
        let positive_to_negative = wrapped_angle_error(-PI + epsilon, PI - epsilon);
        let negative_to_positive = wrapped_angle_error(PI - epsilon, -PI + epsilon);
        assert!((positive_to_negative - epsilon * 2.0).abs() < 1.0e-5);
        assert!((negative_to_positive + epsilon * 2.0).abs() < 1.0e-5);
    }

    #[test]
    fn stationary_owner_immediately_holds_forward_gas() {
        let keyboard = PlayerInput {
            throttle: -0.4,
            steer: 0.8,
            brake: false,
        };
        assert_eq!(
            merge_touch_input(keyboard, intent(true, None, false), 0.0, 1.2),
            PlayerInput {
                throttle: 1.0,
                steer: 0.0,
                brake: false,
            }
        );
    }

    #[test]
    fn six_pixel_jitter_has_zero_steer_while_owner_holds_gas() {
        let basis = Some((Vec3::X, Vec3::Z));
        let size = Vec2::new(844.0, 390.0);
        let start = Vec2::new(0.25, 0.5);
        for drag in [Vec2::ZERO, Vec2::new(6.0, 0.0), Vec2::new(3.0, -4.0)] {
            let current = start + drag / size;
            let touch_intent = touch_intent(Some(1), &[touch(1, start, current)], size, basis);
            assert_eq!(touch_intent.desired_direction, None);
            assert_eq!(
                merge_touch_input(PlayerInput::default(), touch_intent, 0.0, 1.2),
                PlayerInput {
                    throttle: 1.0,
                    steer: 0.0,
                    brake: false,
                }
            );
        }
        assert!(screen_drag_to_world(Vec2::new(6.1, 0.0), Vec3::X, Vec3::Z).is_some());
    }

    #[test]
    fn filtered_drag_has_deadzone_hysteresis_no_overshoot_and_clean_release() {
        let size = Vec2::new(844.0, 390.0);
        let start = Vec2::new(0.25, 0.5);
        let mut steering = TouchSteering::default();
        let at = |pixels: Vec2| [touch(1, start, start + pixels / size)];

        assert_eq!(
            filtered_owner_drag(&mut steering, Some(1), &at(Vec2::ZERO), size, 1.0 / 60.0),
            None
        );
        for jitter in [Vec2::new(6.0, 0.0), Vec2::new(7.9, 0.0)] {
            assert_eq!(
                filtered_owner_drag(&mut steering, Some(1), &at(jitter), size, 1.0 / 60.0),
                None
            );
        }
        let mut previous = 0.0;
        for _ in 0..20 {
            if let Some(drag) = filtered_owner_drag(
                &mut steering,
                Some(1),
                &at(Vec2::new(40.0, -30.0)),
                size,
                1.0 / 60.0,
            ) {
                assert!(drag.x >= previous && drag.x <= 40.0);
                previous = drag.x;
            }
        }
        assert!(steering.engaged);
        assert_eq!(
            filtered_owner_drag(
                &mut steering,
                Some(1),
                &at(Vec2::new(6.0, 0.0)),
                size,
                1.0 / 60.0
            ),
            None
        );
        assert!(!steering.engaged);
        assert_eq!(
            filtered_owner_drag(&mut steering, None, &[], size, 1.0 / 60.0),
            None
        );
        assert_eq!(steering, TouchSteering::default());
    }

    #[test]
    fn analog_diagonal_filter_is_framerate_independent() {
        let run = |hz: usize| {
            let size = Vec2::new(844.0, 390.0);
            let start = Vec2::new(0.25, 0.5);
            let moved = [touch(1, start, start + Vec2::new(80.0, -60.0) / size)];
            let mut steering = TouchSteering::default();
            let _ = filtered_owner_drag(
                &mut steering,
                Some(1),
                &[touch(1, start, start)],
                size,
                1.0 / hz as f32,
            );
            for _ in 0..hz {
                let _ = filtered_owner_drag(&mut steering, Some(1), &moved, size, 1.0 / hz as f32);
            }
            steering.filtered_drag
        };
        let at_30 = run(30);
        assert!((at_30 - run(60)).length() < 1e-3);
        assert!((at_30 - run(120)).length() < 1e-3);
        assert!((at_30.x / -at_30.y - 4.0 / 3.0).abs() < 1e-4);
    }

    #[test]
    fn floating_origin_is_bounded_and_preserves_direction() {
        let size = Vec2::new(844.0, 390.0);
        let start = Vec2::new(0.25, 0.5);
        let mut steering = TouchSteering::default();
        let _ = filtered_owner_drag(
            &mut steering,
            Some(7),
            &[touch(7, start, start)],
            size,
            1.0 / 60.0,
        );
        let far = [touch(7, start, start + Vec2::new(300.0, 400.0) / size)];
        let _ = filtered_owner_drag(&mut steering, Some(7), &far, size, 1.0);
        let drag = far[0].current * size - steering.origin_px;
        assert!((drag.length() - FLOATING_ORIGIN_RADIUS_PX).abs() < 1e-4);
        assert!(drag.x > 0.0 && drag.y > 0.0);
    }

    #[test]
    fn owner_stays_sticky_and_filter_recenters_when_orientation_changes() {
        let original = touch(11, Vec2::new(0.8, 0.5), Vec2::new(0.8, 0.5));
        let rotated_coordinates = touch(11, Vec2::new(0.5, 0.1), Vec2::new(0.5, 0.1));
        assert_eq!(update_drive_owner(None, &[original]), Some(11));
        assert_eq!(
            update_drive_owner(Some(11), &[rotated_coordinates]),
            Some(11)
        );

        let mut steering = TouchSteering::default();
        let landscape = Vec2::new(844.0, 390.0);
        let portrait = Vec2::new(390.0, 844.0);
        let _ = filtered_owner_drag(&mut steering, Some(11), &[original], landscape, 1.0 / 60.0);
        steering.engaged = true;
        steering.filtered_drag = Vec2::splat(30.0);
        assert_eq!(
            filtered_owner_drag(
                &mut steering,
                Some(11),
                &[rotated_coordinates],
                portrait,
                1.0 / 60.0,
            ),
            None
        );
        assert_eq!(steering.window_size, portrait);
        assert!(!steering.engaged);
        assert_eq!(steering.filtered_drag, Vec2::ZERO);
    }

    #[test]
    fn drive_owner_resets_across_playing_state_exit() {
        let mut app = App::new();
        app.init_resource::<DriveTouchOwner>()
            .init_resource::<TouchSteering>()
            .add_systems(Update, reset_drive_touch_owner);
        app.world_mut().resource_mut::<DriveTouchOwner>().0 = Some(42);
        app.world_mut().resource_mut::<TouchSteering>().engaged = true;
        app.update();
        assert_eq!(app.world().resource::<DriveTouchOwner>().0, None);
        assert_eq!(
            *app.world().resource::<TouchSteering>(),
            TouchSteering::default()
        );
    }

    #[test]
    fn ecs_touch_events_flow_through_touches_and_read_touch_input() {
        let keyboard_snapshot = PlayerInput {
            throttle: -0.6,
            steer: 0.7,
            brake: true,
        };
        let mut app = App::new();
        app.add_plugins(InputPlugin)
            .init_resource::<DriveTouchOwner>()
            .init_resource::<TouchSteering>()
            .insert_resource(Time::<()>::default())
            .insert_resource(InputFrozen(false))
            .insert_resource(keyboard_snapshot)
            .add_systems(Update, read_touch_input.in_set(TouchInputSet));

        let window = app
            .world_mut()
            .spawn((
                Window {
                    resolution: (844, 390).into(),
                    ..default()
                },
                PrimaryWindow,
            ))
            .id();
        let car = app
            .world_mut()
            .spawn(Car {
                speed: 0.0,
                heading: 0.0,
                drift: 0.0,
            })
            .id();
        app.world_mut().spawn((
            Camera3d::default(),
            Camera::default(),
            GlobalTransform::from(
                Transform::from_xyz(12.0, 12.0, 12.0).looking_at(Vec3::ZERO, Vec3::Y),
            ),
        ));

        let send_touch = |app: &mut App, phase: TouchPhase, id: u64, position: Vec2| {
            app.world_mut().write_message(TouchInput {
                phase,
                position,
                window,
                force: None,
                id,
            });
        };

        // The real InputPlugin converts TouchInput messages into the Touches
        // resource in PreUpdate before the gameplay system reads it in Update.
        send_touch(&mut app, TouchPhase::Started, 10, Vec2::new(700.0, 250.0));
        app.update();
        assert!(app.world().resource::<Touches>().get_pressed(10).is_some());
        assert_eq!(
            *app.world().resource::<PlayerInput>(),
            PlayerInput {
                throttle: 1.0,
                steer: 0.0,
                brake: false,
            },
            "the sole owner must replace keyboard brake/throttle"
        );

        app.world_mut()
            .entity_mut(car)
            .get_mut::<Car>()
            .unwrap()
            .speed = 0.150_01;
        app.world_mut().insert_resource(keyboard_snapshot);
        send_touch(&mut app, TouchPhase::Started, 20, Vec2::new(120.0, 250.0));
        app.update();
        assert_eq!(
            *app.world().resource::<PlayerInput>(),
            PlayerInput {
                throttle: 0.0,
                steer: 0.0,
                brake: true,
            },
            "a second touch above the action threshold must brake"
        );

        app.world_mut()
            .entity_mut(car)
            .get_mut::<Car>()
            .unwrap()
            .speed = ACTION_BRAKE_SPEED;
        app.world_mut().insert_resource(keyboard_snapshot);
        app.update();
        assert_eq!(
            *app.world().resource::<PlayerInput>(),
            PlayerInput {
                throttle: -1.0,
                steer: 0.0,
                brake: false,
            },
            "at the action threshold the second touch must request reverse"
        );
    }

    #[test]
    fn owner_is_position_independent_sticky_and_promotes_on_release() {
        let first = touch(70, Vec2::new(0.84, 0.31), Vec2::new(0.74, 0.11));
        assert_eq!(update_drive_owner(None, &[first]), Some(70));

        // A later lower-ID touch cannot steal the live owner's role, regardless
        // of position or iteration order.
        let second = touch(9, Vec2::new(0.08, 0.42), Vec2::new(0.18, 0.22));
        assert_eq!(update_drive_owner(Some(70), &[second, first]), Some(70));
        assert_eq!(update_drive_owner(Some(70), &[first, second]), Some(70));
        let size = Vec2::new(844.0, 390.0);
        let basis = Some((Vec3::X, Vec3::Z));
        assert_eq!(
            touch_intent(Some(70), &[second, first], size, basis),
            touch_intent(Some(70), &[first, second], size, basis)
        );
        assert!(touch_intent(Some(70), &[second, first], size, basis).action);

        // Releasing the owner promotes the remaining touch. Its action role
        // clears immediately and the filtered analog origin is initialized at
        // its current point, so promotion cannot kick the wheel.
        let promoted = update_drive_owner(Some(70), &[second]);
        assert_eq!(promoted, Some(9));
        let mut steering = TouchSteering {
            owner: Some(70),
            window_size: size,
            origin_px: Vec2::splat(100.0),
            filtered_drag: Vec2::splat(40.0),
            engaged: true,
        };
        let promoted_intent =
            smoothed_touch_intent(promoted, &[second], size, basis, &mut steering, 1.0 / 60.0);
        assert!(promoted_intent.owner);
        assert!(!promoted_intent.action);
        assert_eq!(promoted_intent.desired_direction, None);
        assert_eq!(steering.owner, Some(9));
        assert_eq!(update_drive_owner(promoted, &[]), None);
    }

    #[test]
    fn simultaneous_owner_tie_uses_lowest_id_independent_of_iteration() {
        let low = touch(3, Vec2::new(0.9, 0.7), Vec2::new(0.8, 0.5));
        let high = touch(40, Vec2::new(0.1, 0.2), Vec2::new(0.1, 0.2));
        assert_eq!(update_drive_owner(None, &[low, high]), Some(3));
        assert_eq!(update_drive_owner(None, &[high, low]), Some(3));
    }

    #[test]
    fn pause_start_is_never_eligible_for_either_role() {
        let pause = touch(1, Vec2::new(0.5, 0.1), Vec2::new(0.1, 0.9));
        let drive = touch(8, Vec2::new(0.7, 0.4), Vec2::new(0.6, 0.2));
        assert_eq!(update_drive_owner(None, &[pause]), None);
        assert_eq!(update_drive_owner(None, &[pause, drive]), Some(8));
        let intent = touch_intent(
            Some(8),
            &[pause, drive],
            Vec2::new(844.0, 390.0),
            Some((Vec3::X, Vec3::Z)),
        );
        assert!(intent.owner);
        assert!(!intent.action);
        assert!(intent.desired_direction.is_some());
    }

    #[test]
    fn action_brakes_forward_then_holds_reverse_at_boundary_and_overrides_gas() {
        let keyboard = PlayerInput {
            throttle: 0.2,
            steer: -0.4,
            brake: false,
        };
        let desired_left = Some(Vec3::NEG_X);
        let braking = merge_touch_input(keyboard, intent(true, desired_left, true), 0.150_01, 0.0);
        assert_eq!(
            braking,
            PlayerInput {
                throttle: 0.0,
                steer: 1.0,
                brake: true
            }
        );
        for speed in [0.15, 0.0, -2.0] {
            let reverse = merge_touch_input(keyboard, intent(true, desired_left, true), speed, 0.0);
            assert_eq!(
                reverse,
                PlayerInput {
                    throttle: -1.0,
                    steer: 1.0,
                    brake: false
                }
            );
        }
        assert_eq!(
            merge_touch_input(keyboard, intent(false, None, false), 0.0, 0.0),
            keyboard
        );
    }

    #[test]
    fn direction_owner_and_action_are_independent_of_touch_positions() {
        let size = Vec2::new(844.0, 390.0);
        let basis = Some((Vec3::X, Vec3::Z));
        let touches = [
            touch(1, Vec2::new(0.85, 0.85), Vec2::new(0.75, 0.65)),
            touch(2, Vec2::new(0.15, 0.35), Vec2::new(0.95, 0.05)),
        ];
        let combined_intent = touch_intent(Some(1), &touches, size, basis);
        assert!(combined_intent.owner);
        assert!(combined_intent.desired_direction.is_some());
        assert!(combined_intent.action);

        // The second touch's large drag cannot alter owner direction.
        let stationary_action = touch(2, touches[1].start, touches[1].start);
        assert_eq!(
            touch_intent(Some(1), &[touches[0], stationary_action], size, basis).desired_direction,
            combined_intent.desired_direction
        );
    }

    #[test]
    fn state_actions_cover_menu_pause_thirds_and_gameover() {
        assert_eq!(
            touch_state_action(GameState::Menu, Vec2::ZERO),
            StateAction::Playing
        );
        assert_eq!(
            touch_state_action(GameState::Playing, Vec2::new(0.5, 0.1)),
            StateAction::Paused
        );
        assert_eq!(
            touch_state_action(GameState::Playing, Vec2::new(0.9, 0.8)),
            StateAction::None
        );
        assert_eq!(
            touch_state_action(GameState::Paused, Vec2::new(0.1, 0.9)),
            StateAction::Playing
        );
        assert_eq!(
            touch_state_action(GameState::Paused, Vec2::new(0.5, 0.1)),
            StateAction::Restart
        );
        assert_eq!(
            touch_state_action(GameState::Paused, Vec2::new(0.9, 0.1)),
            StateAction::Menu
        );
        assert_eq!(
            touch_state_action(GameState::GameOver, Vec2::new(0.65, 0.5)),
            StateAction::Playing
        );
        assert_eq!(
            touch_state_action(GameState::GameOver, Vec2::new(0.8, 0.5)),
            StateAction::Menu
        );
    }

    #[test]
    fn simultaneous_touch_actions_have_order_independent_priority() {
        let forward = resolve_touch_actions([
            StateAction::Playing,
            StateAction::Menu,
            StateAction::Restart,
        ]);
        let reverse = resolve_touch_actions([
            StateAction::Restart,
            StateAction::Menu,
            StateAction::Playing,
        ]);
        assert_eq!(forward, StateAction::Restart);
        assert_eq!(reverse, StateAction::Restart);
        assert_eq!(
            resolve_touch_actions([StateAction::Playing, StateAction::Menu]),
            StateAction::Menu
        );
    }
}
