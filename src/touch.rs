use bevy::{prelude::*, text::FontSize, window::PrimaryWindow};

use crate::car::{InputFrozen, PlayerInput, TouchInputSet};
use crate::game::{RestartRequested, state::GameState};

const ACTIVE_Y: f32 = 0.55;
const STEER_END_X: f32 = 0.45;
const BRAKE_START_X: f32 = 0.55;
const BRAKE_END_X: f32 = 0.75;
const STEER_CENTER_X: f32 = STEER_END_X * 0.5;
const STEER_DEADZONE: f32 = 0.12;

/// Becomes sticky after the first touch so touch-only guidance does not appear
/// for keyboard/mouse players.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct TouchControlsActive(pub bool);

#[derive(Component)]
struct TouchHudRoot;

#[derive(Component)]
struct TouchGuidanceRoot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlZone {
    Steering,
    Brake,
    Throttle,
}

#[derive(Debug, Clone, Copy)]
struct ActiveTouch {
    start: Vec2,
    current: Vec2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TouchStateAction {
    None,
    Playing,
    Paused,
    Menu,
    Restart,
}

pub struct TouchPlugin;

impl Plugin for TouchPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TouchControlsActive>()
            .add_systems(Update, touch_state_transitions)
            .add_systems(
                Update,
                read_touch_input
                    .in_set(TouchInputSet)
                    .run_if(in_state(GameState::Playing)),
            )
            .add_systems(OnEnter(GameState::Playing), spawn_touch_hud)
            .add_systems(OnExit(GameState::Playing), despawn_marker::<TouchHudRoot>)
            .add_systems(OnEnter(GameState::Paused), spawn_paused_guidance)
            .add_systems(
                OnExit(GameState::Paused),
                despawn_marker::<TouchGuidanceRoot>,
            )
            .add_systems(OnEnter(GameState::GameOver), spawn_gameover_guidance)
            .add_systems(
                OnExit(GameState::GameOver),
                despawn_marker::<TouchGuidanceRoot>,
            )
            .add_systems(Update, update_touch_visibility);
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

fn classify_zone(start: Vec2) -> Option<ControlZone> {
    if start.y < ACTIVE_Y {
        None
    } else if start.x < STEER_END_X {
        Some(ControlZone::Steering)
    } else if start.x >= BRAKE_START_X && start.x < BRAKE_END_X {
        Some(ControlZone::Brake)
    } else if start.x >= BRAKE_END_X {
        Some(ControlZone::Throttle)
    } else {
        None
    }
}

/// Steering is positive on the left and negative on the right. The deadzone
/// is centered in the steering pad and the remaining range is rescaled.
fn steering_value(current_x: f32) -> f32 {
    let raw = ((STEER_CENTER_X - current_x) / STEER_CENTER_X).clamp(-1.0, 1.0);
    if raw.abs() <= STEER_DEADZONE {
        0.0
    } else {
        raw.signum() * (raw.abs() - STEER_DEADZONE) / (1.0 - STEER_DEADZONE)
    }
}

/// Merge currently active touch controls onto the keyboard snapshot. Zones
/// that have no active touch leave their keyboard field unchanged. Therefore
/// an empty active list (including the frame after release/cancel) is identity.
fn merge_touch_input(keyboard: PlayerInput, touches: &[ActiveTouch]) -> PlayerInput {
    let mut result = keyboard;
    let mut steer_sum = 0.0;
    let mut has_steer = false;
    let mut touch_brake = false;
    let mut touch_throttle = false;

    for touch in touches {
        match classify_zone(touch.start) {
            Some(ControlZone::Steering) => {
                has_steer = true;
                steer_sum += steering_value(touch.current.x);
            }
            Some(ControlZone::Brake) => touch_brake = true,
            Some(ControlZone::Throttle) => touch_throttle = true,
            None => {}
        }
    }

    if has_steer {
        result.steer = steer_sum.clamp(-1.0, 1.0);
    }
    if touch_brake {
        result.brake = true;
    } else if touch_throttle {
        result.throttle = 1.0;
    }
    result
}

fn touch_state_action(state: GameState, position: Vec2) -> TouchStateAction {
    match state {
        GameState::Menu => TouchStateAction::Playing,
        GameState::Playing => {
            if position.y < 0.14 && (0.44..=0.56).contains(&position.x) {
                TouchStateAction::Paused
            } else {
                TouchStateAction::None
            }
        }
        GameState::Paused => {
            if position.x < 1.0 / 3.0 {
                TouchStateAction::Playing
            } else if position.x < 2.0 / 3.0 {
                TouchStateAction::Restart
            } else {
                TouchStateAction::Menu
            }
        }
        GameState::GameOver => {
            if position.x < 2.0 / 3.0 {
                TouchStateAction::Playing
            } else {
                TouchStateAction::Menu
            }
        }
    }
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
    let Some(touch) = touches.iter_just_pressed().next() else {
        return;
    };
    active.0 = true;

    let Some(window_size) = primary_window_size(&windows) else {
        return;
    };
    let Some(position) = normalize_position(touch.position(), window_size) else {
        return;
    };

    match touch_state_action(*state.get(), position) {
        TouchStateAction::None => {}
        TouchStateAction::Playing => next.set(GameState::Playing),
        TouchStateAction::Paused => next.set(GameState::Paused),
        TouchStateAction::Menu => next.set(GameState::Menu),
        TouchStateAction::Restart => {
            restart.0 = true;
            next.set(GameState::Menu);
        }
    }
}

fn read_touch_input(
    touches: Res<Touches>,
    windows: Query<&Window, With<PrimaryWindow>>,
    frozen: Res<InputFrozen>,
    mut input: ResMut<PlayerInput>,
) {
    if frozen.0 {
        *input = PlayerInput::default();
        return;
    }
    let Some(window_size) = primary_window_size(&windows) else {
        return;
    };

    let active_touches: Vec<_> = touches
        .iter()
        .filter_map(|touch| {
            Some(ActiveTouch {
                start: normalize_position(touch.start_position(), window_size)?,
                current: normalize_position(touch.position(), window_size)?,
            })
        })
        .collect();
    *input = merge_touch_input(*input, &active_touches);
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
            // Visual bands are intentionally shorter than their generous
            // logical touch zones so a short landscape viewport stays clear.
            root.spawn(control_label("STEER", 2.0, 43.0, 28.0));
            root.spawn(control_label("BRAKE", 56.0, 21.0, 28.0));
            root.spawn(control_label("GO", 78.0, 20.0, 28.0));
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

fn control_label(
    label: &'static str,
    left_percent: f32,
    width_percent: f32,
    height_percent: f32,
) -> impl Bundle {
    (
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Percent(2.0),
            left: Val::Percent(left_percent),
            width: Val::Percent(width_percent),
            height: Val::Percent(height_percent),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            ..default()
        },
        BackgroundColor(Color::srgba(0.02, 0.02, 0.03, 0.18)),
        Text::new(label),
        TextFont {
            font_size: FontSize::Px(21.0),
            ..default()
        },
        TextColor(Color::srgba(1.0, 1.0, 1.0, 0.52)),
    )
}

fn spawn_paused_guidance(mut commands: Commands, active: Res<TouchControlsActive>) {
    spawn_guidance(
        &mut commands,
        active.0,
        "LEFT: RESUME     MIDDLE: RESTART     RIGHT: MENU",
    );
}

fn spawn_gameover_guidance(mut commands: Commands, active: Res<TouchControlsActive>) {
    spawn_guidance(
        &mut commands,
        active.0,
        "LEFT 2/3: PLAY AGAIN          RIGHT 1/3: MENU",
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

    fn touch(start: Vec2, current: Vec2) -> ActiveTouch {
        ActiveTouch { start, current }
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

    #[test]
    fn control_zone_boundaries_leave_safe_gaps() {
        assert_eq!(classify_zone(Vec2::new(0.1, 0.549)), None);
        assert_eq!(
            classify_zone(Vec2::new(0.449, 0.55)),
            Some(ControlZone::Steering)
        );
        assert_eq!(classify_zone(Vec2::new(0.45, 0.8)), None);
        assert_eq!(classify_zone(Vec2::new(0.549, 0.8)), None);
        assert_eq!(
            classify_zone(Vec2::new(0.55, 0.8)),
            Some(ControlZone::Brake)
        );
        assert_eq!(
            classify_zone(Vec2::new(0.749, 0.8)),
            Some(ControlZone::Brake)
        );
        assert_eq!(
            classify_zone(Vec2::new(0.75, 0.8)),
            Some(ControlZone::Throttle)
        );
    }

    #[test]
    fn steering_has_left_positive_right_negative_deadzone_and_clamp() {
        assert_eq!(steering_value(-1.0), 1.0);
        assert_eq!(steering_value(2.0), -1.0);
        assert_eq!(steering_value(STEER_CENTER_X), 0.0);
        assert_eq!(steering_value(STEER_CENTER_X + 0.01), 0.0);
        assert!(steering_value(0.0) > 0.99);
        assert!(steering_value(STEER_END_X) < -0.99);
    }

    #[test]
    fn simultaneous_touches_merge_controls_and_brake_overrides_touch_go() {
        let keyboard = PlayerInput {
            throttle: -1.0,
            steer: -0.5,
            brake: false,
        };
        let merged = merge_touch_input(
            keyboard,
            &[
                touch(Vec2::new(0.1, 0.8), Vec2::new(0.0, 0.8)),
                touch(Vec2::new(0.6, 0.8), Vec2::new(0.6, 0.8)),
                touch(Vec2::new(0.9, 0.8), Vec2::new(0.9, 0.8)),
            ],
        );
        assert_eq!(merged.steer, 1.0);
        assert!(merged.brake);
        // The brake wins over touch GO; the keyboard snapshot is not erased.
        assert_eq!(merged.throttle, -1.0);
    }

    #[test]
    fn untouched_keyboard_fields_and_empty_release_frame_are_preserved() {
        let keyboard = PlayerInput {
            throttle: 0.4,
            steer: -0.3,
            brake: true,
        };
        assert_eq!(merge_touch_input(keyboard, &[]), keyboard);
        let throttle_only =
            merge_touch_input(keyboard, &[touch(Vec2::new(0.9, 0.8), Vec2::new(0.9, 0.8))]);
        assert_eq!(throttle_only.steer, keyboard.steer);
        assert_eq!(throttle_only.brake, keyboard.brake);
        assert_eq!(throttle_only.throttle, 1.0);
    }

    #[test]
    fn state_actions_cover_menu_pause_thirds_and_gameover() {
        assert_eq!(
            touch_state_action(GameState::Menu, Vec2::ZERO),
            TouchStateAction::Playing
        );
        assert_eq!(
            touch_state_action(GameState::Playing, Vec2::new(0.5, 0.1)),
            TouchStateAction::Paused
        );
        assert_eq!(
            touch_state_action(GameState::Playing, Vec2::new(0.9, 0.8)),
            TouchStateAction::None
        );
        assert_eq!(
            touch_state_action(GameState::Paused, Vec2::new(0.1, 0.9)),
            TouchStateAction::Playing
        );
        assert_eq!(
            touch_state_action(GameState::Paused, Vec2::new(0.5, 0.1)),
            TouchStateAction::Restart
        );
        assert_eq!(
            touch_state_action(GameState::Paused, Vec2::new(0.9, 0.1)),
            TouchStateAction::Menu
        );
        assert_eq!(
            touch_state_action(GameState::GameOver, Vec2::new(0.65, 0.5)),
            TouchStateAction::Playing
        );
        assert_eq!(
            touch_state_action(GameState::GameOver, Vec2::new(0.8, 0.5)),
            TouchStateAction::Menu
        );
    }
}
