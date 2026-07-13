use bevy::{prelude::*, text::FontSize, window::PrimaryWindow};

use crate::car::{Car, InputFrozen, PlayerInput, TouchInputSet};
use crate::game::{
    RestartRequested, StateAction, TouchStateSet, apply_state_action, settings_closed,
    state::GameState,
};

const ACTIVE_Y: f32 = 0.55;
const PAUSE_TOP: f32 = 0.14;
const PAUSE_LEFT: f32 = 0.44;
const PAUSE_RIGHT: f32 = 0.56;
const ACTION_BRAKE_SPEED: f32 = 0.15;

// These are intentionally identical to wasm_battle_arena's touch joystick.
const INPUT_UP: u8 = 1 << 0;
const INPUT_DOWN: u8 = 1 << 1;
const INPUT_LEFT: u8 = 1 << 2;
const INPUT_RIGHT: u8 = 1 << 3;
const AXIS_DEADZONE: f32 = 0.2;
const DIAGONAL_NORMALIZED: f32 = 0.707107;
const UNIT_TL: Vec2 = Vec2::new(DIAGONAL_NORMALIZED, DIAGONAL_NORMALIZED);
const UNIT_TR: Vec2 = Vec2::new(-DIAGONAL_NORMALIZED, DIAGONAL_NORMALIZED);
const UNIT_BL: Vec2 = Vec2::new(DIAGONAL_NORMALIZED, -DIAGONAL_NORMALIZED);
const UNIT_BR: Vec2 = Vec2::new(-DIAGONAL_NORMALIZED, -DIAGONAL_NORMALIZED);

// Fixed touch-only HUD composition. These values are shared by the live
// nodes in their owning modules and the pure all-panel layout audit below.
pub(crate) const TOUCH_COCKPIT_LEFT: f32 = 14.0;
pub(crate) const TOUCH_COCKPIT_TOP: f32 = 12.0;
pub(crate) const TOUCH_COCKPIT_WIDTH: f32 = 170.0;
pub(crate) const TOUCH_COCKPIT_HEIGHT: f32 = 116.0;
pub(crate) const TOUCH_HEALTH_LEFT: f32 = 14.0;
pub(crate) const TOUCH_HEALTH_TOP: f32 = 136.0;
pub(crate) const TOUCH_HEALTH_WIDTH: f32 = 190.0;
pub(crate) const TOUCH_HEALTH_HEIGHT: f32 = 52.0;
pub(crate) const TOUCH_POWERUP_LEFT: f32 = 205.0;
pub(crate) const TOUCH_POWERUP_TOP: f32 = 136.0;
pub(crate) const TOUCH_POWERUP_WIDTH: f32 = 142.0;
pub(crate) const TOUCH_POWERUP_HEIGHT: f32 = 52.0;
pub(crate) const TOUCH_EVENT_LEFT: f32 = 370.0;
pub(crate) const TOUCH_EVENT_TOP: f32 = 182.0;
pub(crate) const TOUCH_EVENT_WIDTH: f32 = 250.0;
pub(crate) const TOUCH_EVENT_HEIGHT: f32 = 30.0;

const DRIVE_PAD_LEFT: f32 = 32.0;
const DRIVE_PAD_BOTTOM: f32 = 10.0;
const DRIVE_PAD_SIZE: f32 = 136.0;
const ACTION_PAD_RIGHT: f32 = 32.0;
const ACTION_PAD_BOTTOM: f32 = 18.0;
const ACTION_PAD_WIDTH: f32 = 180.0;
const ACTION_PAD_HEIGHT: f32 = 112.0;

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

/// Painted bounds for every persistent/active touch HUD panel. Objective and
/// combo retain their existing centered placement, and the minimap keeps its
/// current right-side placement; the other four use the compact composition.
#[allow(dead_code)]
pub(crate) fn touch_hud_bounds(viewport: Vec2) -> [ScreenBounds; 7] {
    [
        fixed_bounds(
            TOUCH_COCKPIT_LEFT,
            TOUCH_COCKPIT_TOP,
            TOUCH_COCKPIT_WIDTH,
            TOUCH_COCKPIT_HEIGHT,
        ),
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
        centered_bounds(viewport.x, 54.0, 420.0, 38.0),
        centered_bounds(viewport.x, 98.0, 144.0, 80.0),
        fixed_bounds(
            TOUCH_EVENT_LEFT,
            TOUCH_EVENT_TOP,
            TOUCH_EVENT_WIDTH,
            TOUCH_EVENT_HEIGHT,
        ),
        fixed_bounds(viewport.x - 72.0 - 136.0, 62.0, 136.0, 136.0),
    ]
}

/// Union of all lower driving hitboxes in top-left-origin screen pixels.
#[allow(dead_code)]
pub(crate) fn touch_driving_band_bounds(viewport: Vec2) -> ScreenBounds {
    ScreenBounds {
        left: 0.0,
        top: viewport.y * ACTIVE_Y,
        right: viewport.x,
        bottom: viewport.y,
    }
}

#[cfg(test)]
fn touch_control_zone_bounds(viewport: Vec2) -> [ScreenBounds; 2] {
    [
        ScreenBounds {
            left: 0.0,
            top: viewport.y * ACTIVE_Y,
            right: viewport.x * 0.5,
            bottom: viewport.y,
        },
        ScreenBounds {
            left: viewport.x * 0.5,
            top: viewport.y * ACTIVE_Y,
            right: viewport.x,
            bottom: viewport.y,
        },
    ]
}

#[cfg(test)]
fn touch_control_label_bounds(viewport: Vec2) -> [ScreenBounds; 2] {
    [
        ScreenBounds {
            left: DRIVE_PAD_LEFT,
            top: viewport.y - DRIVE_PAD_BOTTOM - DRIVE_PAD_SIZE,
            right: DRIVE_PAD_LEFT + DRIVE_PAD_SIZE,
            bottom: viewport.y - DRIVE_PAD_BOTTOM,
        },
        ScreenBounds {
            left: viewport.x - ACTION_PAD_RIGHT - ACTION_PAD_WIDTH,
            top: viewport.y - ACTION_PAD_BOTTOM - ACTION_PAD_HEIGHT,
            right: viewport.x - ACTION_PAD_RIGHT,
            bottom: viewport.y - ACTION_PAD_BOTTOM,
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

/// The first live touch on the left half owns the drive stick until it is
/// released/cancelled, matching wasm_battle_arena's `TouchMap` resource.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
struct DriveTouchOwner(Option<u64>);

#[derive(Debug, Clone, Copy)]
struct ActiveTouch {
    id: u64,
    start: Vec2,
    current: Vec2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TouchIntent {
    direction: u8,
    action: bool,
}

pub struct TouchPlugin;

impl Plugin for TouchPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TouchControlsActive>()
            .init_resource::<DriveTouchOwner>()
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
            .add_systems(Update, update_touch_visibility.after(TouchStateSet));
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

fn adaptive_deadzone(window_size: Vec2) -> f32 {
    (window_size.x.min(window_size.y) * 0.08).clamp(32.0, 72.0)
}

/// Exact eight-way quantization ported from wasm_battle_arena. `dir` is
/// start-current, so dragging upward produces UP and dragging left produces
/// LEFT. A displacement exactly on the pixel deadzone remains idle.
fn input_from_vec(dir: Vec2, deadzone: f32) -> u8 {
    if dir.length() <= deadzone {
        return 0;
    }
    let dir = dir.normalize_or_zero();
    let left = dir.distance_squared(Vec2::X);
    let top_left = dir.distance_squared(UNIT_TL);
    let top = dir.distance_squared(Vec2::Y);
    let top_right = dir.distance_squared(UNIT_TR);
    let right = dir.distance_squared(-Vec2::X);
    let bottom_left = dir.distance_squared(UNIT_BL);
    let bottom = dir.distance_squared(-Vec2::Y);
    let bottom_right = dir.distance_squared(UNIT_BR);

    if top < AXIS_DEADZONE {
        INPUT_UP
    } else if bottom < AXIS_DEADZONE {
        INPUT_DOWN
    } else if left < right {
        if left < AXIS_DEADZONE {
            INPUT_LEFT
        } else if top_left < left {
            INPUT_LEFT | INPUT_UP
        } else if bottom_left < left {
            INPUT_LEFT | INPUT_DOWN
        } else {
            0
        }
    } else if right < AXIS_DEADZONE {
        INPUT_RIGHT
    } else if top_right < right {
        INPUT_RIGHT | INPUT_UP
    } else if bottom_right < right {
        INPUT_RIGHT | INPUT_DOWN
    } else {
        0
    }
}

fn update_drive_owner(owner: Option<u64>, touches: &[ActiveTouch]) -> Option<u64> {
    if owner.is_some_and(|id| touches.iter().any(|touch| touch.id == id)) {
        owner
    } else {
        touches
            .iter()
            .find(|touch| touch.start.x < 0.5)
            .map(|touch| touch.id)
    }
}

fn touch_intent(owner: Option<u64>, touches: &[ActiveTouch], window_size: Vec2) -> TouchIntent {
    let direction = owner
        .and_then(|id| touches.iter().find(|touch| touch.id == id))
        .map(|touch| {
            let delta = (touch.start - touch.current) * window_size;
            input_from_vec(delta, adaptive_deadzone(window_size))
        })
        .unwrap_or(0);
    let action = touches
        .iter()
        .any(|touch| touch.start.x >= 0.5 && !is_pause_hitbox(touch.start));
    TouchIntent { direction, action }
}

/// Merge touch intent onto keyboard input. DOWN never requests reverse; it
/// zeroes touch steering unless diagonal, where horizontal steering remains.
/// Action owns throttle/brake, using speed to switch braking to held reverse.
fn merge_touch_input(keyboard: PlayerInput, intent: TouchIntent, speed: f32) -> PlayerInput {
    let mut result = keyboard;
    let horizontal = intent.direction & (INPUT_LEFT | INPUT_RIGHT);
    if intent.direction != 0 {
        result.steer = match horizontal {
            INPUT_LEFT => 1.0,
            INPUT_RIGHT => -1.0,
            _ => 0.0,
        };
    }
    if intent.action {
        if speed > ACTION_BRAKE_SPEED {
            result.throttle = 0.0;
            result.brake = true;
        } else {
            result.throttle = -1.0;
            result.brake = false;
        }
    } else if intent.direction & INPUT_UP != 0 {
        result.throttle = 1.0;
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
    mut owner: ResMut<DriveTouchOwner>,
    mut input: ResMut<PlayerInput>,
) {
    if frozen.0 {
        *input = PlayerInput::default();
        owner.0 = None;
        return;
    }
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
    let intent = touch_intent(owner.0, &active_touches, window_size);
    let speed = car.single().map_or(0.0, |car| car.speed);
    *input = merge_touch_input(*input, intent, speed);
    // Touch never writes Handbrake: keyboard Shift remains wholly owned by the
    // keyboard system and cannot be clobbered by touch release/cancel.
}

fn reset_drive_touch_owner(mut owner: ResMut<DriveTouchOwner>) {
    owner.0 = None;
}

fn spawn_touch_hud(mut commands: Commands, active: Res<TouchControlsActive>) {
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
                    left: px(DRIVE_PAD_LEFT),
                    bottom: px(DRIVE_PAD_BOTTOM),
                    width: px(DRIVE_PAD_SIZE),
                    height: px(DRIVE_PAD_SIZE),
                    border: UiRect::all(px(3.0)),
                    border_radius: BorderRadius::all(Val::Percent(50.0)),
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::Center,
                    ..default()
                },
                BorderColor::all(Color::srgba(1.0, 1.0, 1.0, 0.34)),
                BackgroundColor(Color::srgba(0.02, 0.02, 0.03, 0.18)),
                Text::new("DRIVE"),
                TextFont {
                    font_size: FontSize::Px(19.0),
                    ..default()
                },
                TextColor(Color::srgba(1.0, 1.0, 1.0, 0.58)),
            ));
            root.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    right: px(ACTION_PAD_RIGHT),
                    bottom: px(ACTION_PAD_BOTTOM),
                    width: px(ACTION_PAD_WIDTH),
                    height: px(ACTION_PAD_HEIGHT),
                    border: UiRect::all(px(3.0)),
                    border_radius: BorderRadius::all(px(24.0)),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::Center,
                    ..default()
                },
                BorderColor::all(Color::srgba(1.0, 0.55, 0.4, 0.44)),
                BackgroundColor(Color::srgba(0.08, 0.02, 0.02, 0.24)),
            ))
            .with_children(|button| {
                button.spawn((
                    Text::new("BRAKE"),
                    TextFont {
                        font_size: FontSize::Px(23.0),
                        ..default()
                    },
                    TextColor(Color::srgba(1.0, 0.86, 0.8, 0.72)),
                ));
                button.spawn((
                    Text::new("REVERSE"),
                    TextFont {
                        font_size: FontSize::Px(13.0),
                        ..default()
                    },
                    TextColor(Color::srgba(1.0, 1.0, 1.0, 0.48)),
                ));
            });
            root.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Percent(2.0),
                    left: Val::Percent(44.0),
                    width: Val::Percent(12.0),
                    height: Val::Percent(10.0),
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

    fn touch(id: u64, start: Vec2, current: Vec2) -> ActiveTouch {
        ActiveTouch { id, start, current }
    }

    fn intent(direction: u8, action: bool) -> TouchIntent {
        TouchIntent { direction, action }
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
            }
        }
    }

    #[test]
    fn compact_touch_hud_is_pairwise_disjoint_at_844x390() {
        let viewport = Vec2::new(844.0, 390.0);
        assert_touch_hud_layout(viewport);
        let [cockpit, health, powerups, objective, combo, event, _minimap] =
            touch_hud_bounds(viewport);
        assert_eq!(cockpit, fixed_bounds(14.0, 12.0, 170.0, 116.0));
        assert_eq!(health, fixed_bounds(14.0, 136.0, 190.0, 52.0));
        assert_eq!(powerups, fixed_bounds(205.0, 136.0, 142.0, 52.0));
        assert_eq!(objective, fixed_bounds(212.0, 54.0, 420.0, 38.0));
        assert_eq!(combo, fixed_bounds(350.0, 98.0, 144.0, 80.0));
        assert_eq!(event, fixed_bounds(370.0, 182.0, 250.0, 30.0));
    }

    #[test]
    fn compact_touch_hud_is_pairwise_disjoint_at_1440x900() {
        assert_touch_hud_layout(Vec2::new(1440.0, 900.0));
    }

    #[test]
    fn touch_bands_and_labels_match_hitboxes_at_target_viewports() {
        for viewport in [Vec2::new(844.0, 390.0), Vec2::new(1440.0, 900.0)] {
            let band = touch_driving_band_bounds(viewport);
            assert_eq!(band.top, viewport.y * ACTIVE_Y);
            assert_eq!(band.bottom, viewport.y);

            for zone in touch_control_zone_bounds(viewport) {
                assert!(band.contains(zone));
            }
            for label in touch_control_label_bounds(viewport) {
                assert!(band.contains(label));
            }
        }
    }

    #[test]
    fn control_visuals_are_disjoint_at_844x390() {
        let [drive, action] = touch_control_label_bounds(Vec2::new(844.0, 390.0));
        assert!(!drive.overlaps(action));
        assert!(drive.right <= 844.0 * 0.5);
        assert!(action.left >= 844.0 * 0.5);
    }

    #[test]
    fn exact_eight_way_quantization_and_adaptive_deadzone() {
        assert_eq!(adaptive_deadzone(Vec2::new(844.0, 390.0)), 32.0);
        assert_eq!(adaptive_deadzone(Vec2::new(1440.0, 900.0)), 72.0);
        for deadzone in [32.0, 72.0] {
            assert_eq!(input_from_vec(Vec2::X * deadzone, deadzone), 0);
            let d = deadzone + 1.0;
            assert_eq!(input_from_vec(Vec2::Y * d, deadzone), INPUT_UP);
            assert_eq!(input_from_vec(-Vec2::Y * d, deadzone), INPUT_DOWN);
            assert_eq!(input_from_vec(Vec2::X * d, deadzone), INPUT_LEFT);
            assert_eq!(input_from_vec(-Vec2::X * d, deadzone), INPUT_RIGHT);
            assert_eq!(
                input_from_vec(Vec2::splat(d), deadzone),
                INPUT_LEFT | INPUT_UP
            );
            assert_eq!(
                input_from_vec(Vec2::new(-d, d), deadzone),
                INPUT_RIGHT | INPUT_UP
            );
            assert_eq!(
                input_from_vec(Vec2::new(d, -d), deadzone),
                INPUT_LEFT | INPUT_DOWN
            );
            assert_eq!(
                input_from_vec(Vec2::splat(-d), deadzone),
                INPUT_RIGHT | INPUT_DOWN
            );
        }
    }

    #[test]
    fn drive_owner_resets_across_playing_state_exit() {
        let mut app = App::new();
        app.init_resource::<DriveTouchOwner>()
            .add_systems(Update, reset_drive_touch_owner);
        app.world_mut().resource_mut::<DriveTouchOwner>().0 = Some(42);
        app.update();
        assert_eq!(app.world().resource::<DriveTouchOwner>().0, None);
    }

    #[test]
    fn first_left_touch_owns_until_release_then_hands_off() {
        let first = touch(70, Vec2::new(0.2, 0.8), Vec2::new(0.2, 0.6));
        let second = touch(9, Vec2::new(0.3, 0.8), Vec2::new(0.1, 0.8));
        assert_eq!(update_drive_owner(None, &[first, second]), Some(70));
        assert_eq!(update_drive_owner(Some(70), &[second, first]), Some(70));
        assert_eq!(update_drive_owner(Some(70), &[second]), Some(9));
        assert_eq!(update_drive_owner(Some(70), &[]), None);
        assert_eq!(
            update_drive_owner(None, &[touch(1, Vec2::new(0.8, 0.8), Vec2::ZERO)]),
            None
        );
    }

    #[test]
    fn down_never_reverses_but_diagonals_steer() {
        let keyboard = PlayerInput {
            throttle: 0.3,
            steer: 0.2,
            brake: false,
        };
        let down = merge_touch_input(keyboard, intent(INPUT_DOWN, false), 0.0);
        assert_eq!(down.throttle, keyboard.throttle);
        assert_eq!(down.steer, 0.0);
        assert_eq!(down.brake, keyboard.brake);
        let left_down = merge_touch_input(keyboard, intent(INPUT_LEFT | INPUT_DOWN, false), 0.0);
        assert_eq!(left_down.throttle, keyboard.throttle);
        assert_eq!(left_down.steer, 1.0);
        let right_down = merge_touch_input(keyboard, intent(INPUT_RIGHT | INPUT_DOWN, false), 0.0);
        assert_eq!(right_down.steer, -1.0);
    }

    #[test]
    fn action_brakes_forward_then_holds_reverse_at_boundary() {
        let keyboard = PlayerInput {
            throttle: 1.0,
            steer: 0.4,
            brake: false,
        };
        let braking = merge_touch_input(keyboard, intent(INPUT_UP, true), 0.150_01);
        assert_eq!(
            braking,
            PlayerInput {
                throttle: 0.0,
                steer: 0.0,
                brake: true
            }
        );
        for speed in [0.15, 0.0, -2.0] {
            let reverse = merge_touch_input(keyboard, intent(INPUT_UP, true), speed);
            assert_eq!(
                reverse,
                PlayerInput {
                    throttle: -1.0,
                    steer: 0.0,
                    brake: false
                }
            );
        }
        assert_eq!(merge_touch_input(keyboard, intent(0, false), 0.0), keyboard);
        assert_eq!(
            merge_touch_input(PlayerInput::default(), intent(0, false), -1.0),
            PlayerInput::default(),
            "releasing action coasts when no keyboard control is held"
        );
    }

    #[test]
    fn simultaneous_drive_and_action_and_keyboard_preservation() {
        let size = Vec2::new(844.0, 390.0);
        let touches = [
            touch(1, Vec2::new(0.25, 0.85), Vec2::new(0.15, 0.65)),
            touch(2, Vec2::new(0.8, 0.85), Vec2::new(0.8, 0.85)),
        ];
        let touch_intent = touch_intent(Some(1), &touches, size);
        assert_eq!(touch_intent.direction, INPUT_LEFT | INPUT_UP);
        assert!(touch_intent.action);
        let keyboard = PlayerInput {
            throttle: 0.4,
            steer: -0.3,
            brake: false,
        };
        let merged = merge_touch_input(keyboard, touch_intent, 4.0);
        assert_eq!(
            merged,
            PlayerInput {
                throttle: 0.0,
                steer: 1.0,
                brake: true
            }
        );

        let up_only = merge_touch_input(keyboard, intent(INPUT_UP, false), 0.0);
        assert_eq!(up_only.steer, 0.0);
        assert_eq!(up_only.brake, keyboard.brake);
        assert_eq!(up_only.throttle, 1.0);
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

    #[test]
    fn right_half_is_action_except_pause_hitbox() {
        let size = Vec2::new(844.0, 390.0);
        let action = touch(1, Vec2::new(0.7, 0.2), Vec2::new(0.7, 0.2));
        assert!(touch_intent(None, &[action], size).action);
        let pause = touch(2, Vec2::new(0.5, 0.1), Vec2::new(0.5, 0.1));
        assert!(!touch_intent(None, &[pause], size).action);
    }
}
