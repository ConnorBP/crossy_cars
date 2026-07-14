use bevy::prelude::*;
use bevy::text::{FontSize, TextLayout};

use crate::countdown::Countdown;
use crate::game::SpawnSet;
use crate::game::resources::RoundActive;
use crate::game::state::GameState;
use crate::objectives::{
    ActiveObjective, ObjectiveHudRoot, ObjectiveSelectionSet, mission_announcement,
};
use crate::settings::Settings;

const HOLD_SECS: f32 = 1.2;
const END_SECS: f32 = 2.1;
const TOUCH_BAND_HEIGHT: f32 = 44.0;

#[derive(Resource, Default)]
struct RoundIntroState {
    active: bool,
}

#[derive(Component)]
struct RoundIntroRoot;

#[derive(Component)]
struct MissionAnnouncementText;

#[derive(Clone, Copy, Debug, PartialEq)]
struct MissionVisual {
    alpha: f32,
    visible: bool,
}

fn mission_visual(elapsed: f32, reduced_motion: bool) -> MissionVisual {
    let elapsed = elapsed.max(0.0);
    if reduced_motion {
        return MissionVisual {
            alpha: if elapsed < HOLD_SECS { 1.0 } else { 0.0 },
            visible: elapsed < HOLD_SECS,
        };
    }
    let alpha = if elapsed <= HOLD_SECS {
        1.0
    } else if elapsed < END_SECS {
        1.0 - (elapsed - HOLD_SECS) / (END_SECS - HOLD_SECS)
    } else {
        0.0
    }
    .clamp(0.0, 1.0);
    MissionVisual {
        alpha,
        visible: elapsed < END_SECS,
    }
}

fn mission_panel_node(width: f32, height: f32) -> Node {
    let panel_width = 560.0_f32.min((width - 32.0).max(280.0));
    let panel_height = if height <= 390.0 { 124.0 } else { 132.0 };
    Node {
        position_type: PositionType::Absolute,
        left: px((width - panel_width) * 0.5),
        top: px(((height - TOUCH_BAND_HEIGHT - panel_height) * 0.5).max(16.0)),
        width: px(panel_width),
        height: px(panel_height),
        align_items: AlignItems::Center,
        justify_content: JustifyContent::Center,
        padding: UiRect::all(px(12.0)),
        ..default()
    }
}

pub struct RoundIntroPlugin;

impl Plugin for RoundIntroPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RoundIntroState>()
            .add_systems(
                OnEnter(GameState::Playing),
                setup_round_intro
                    .in_set(SpawnSet)
                    .after(ObjectiveSelectionSet),
            )
            .add_systems(OnExit(GameState::Playing), cleanup_round_intro)
            .add_systems(
                Update,
                update_round_intro.run_if(in_state(GameState::Playing)),
            );
    }
}

fn setup_round_intro(
    mut commands: Commands,
    round_active: Res<RoundActive>,
    objective: Res<ActiveObjective>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut state: ResMut<RoundIntroState>,
) {
    if round_active.0 {
        return;
    }
    let (width, height) = windows
        .single()
        .map(|window| (window.width(), window.height()))
        .unwrap_or((960.0, 480.0));
    state.active = true;
    commands
        .spawn((
            mission_panel_node(width, height),
            BackgroundColor(Color::srgba(0.015, 0.02, 0.035, 0.90)),
            GlobalZIndex(80),
            RoundIntroRoot,
        ))
        .with_child((
            Text::new(mission_announcement(objective.kind)),
            TextFont {
                font_size: FontSize::Px(if height <= 390.0 { 24.0 } else { 28.0 }),
                ..default()
            },
            TextColor(Color::srgb(1.0, 0.86, 0.22)),
            TextLayout::justify(Justify::Center),
            MissionAnnouncementText,
        ));
}

fn update_round_intro(
    mut commands: Commands,
    countdown: Res<Countdown>,
    settings: Res<Settings>,
    mut state: ResMut<RoundIntroState>,
    roots: Query<Entity, With<RoundIntroRoot>>,
    mut mission_hud: Query<&mut Visibility, With<ObjectiveHudRoot>>,
    mut panels: Query<&mut BackgroundColor, With<RoundIntroRoot>>,
    mut texts: Query<&mut TextColor, With<MissionAnnouncementText>>,
) {
    if !state.active {
        return;
    }
    let elapsed = (3.0 - countdown.t).max(0.0);
    let visual = mission_visual(elapsed, settings.reduced_motion);
    for mut visibility in &mut mission_hud {
        *visibility = if visual.visible {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
    }
    for mut panel in &mut panels {
        panel.0 = panel.0.with_alpha(0.90 * visual.alpha);
    }
    for mut color in &mut texts {
        color.0 = color.0.with_alpha(visual.alpha);
    }
    if !visual.visible {
        for entity in &roots {
            commands.entity(entity).despawn();
        }
        state.active = false;
    }
}

fn cleanup_round_intro(
    mut commands: Commands,
    roots: Query<Entity, With<RoundIntroRoot>>,
    mut mission_hud: Query<&mut Visibility, With<ObjectiveHudRoot>>,
    mut state: ResMut<RoundIntroState>,
) {
    for entity in &roots {
        commands.entity(entity).despawn();
    }
    for mut visibility in &mut mission_hud {
        *visibility = Visibility::Inherited;
    }
    *state = RoundIntroState::default();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objectives::ObjectiveKind;

    #[test]
    fn mission_visual_has_exact_hold_fade_and_reduced_motion_boundaries() {
        assert_eq!(
            mission_visual(0.0, false),
            MissionVisual {
                alpha: 1.0,
                visible: true
            }
        );
        assert_eq!(mission_visual(HOLD_SECS, false).alpha, 1.0);
        assert!((mission_visual(1.65, false).alpha - 0.5).abs() < 1e-5);
        assert_eq!(
            mission_visual(END_SECS, false),
            MissionVisual {
                alpha: 0.0,
                visible: false
            }
        );
        assert_eq!(mission_visual(HOLD_SECS - 0.001, true).alpha, 1.0);
        assert_eq!(
            mission_visual(HOLD_SECS, true),
            MissionVisual {
                alpha: 0.0,
                visible: false
            }
        );
    }

    #[test]
    fn mission_copy_is_imperative_and_explains_one_time_bonus() {
        for kind in [
            ObjectiveKind::HitChickens { target: 20 },
            ObjectiveKind::CollectCoins { target: 8 },
            ObjectiveKind::ReachCombo { target: 4 },
        ] {
            let copy = mission_announcement(kind);
            assert!(copy.starts_with("ROUND MISSION\n"));
            assert!(copy.contains("Complete once: +10 bonus"));
            assert!(copy.is_ascii());
        }
    }

    #[test]
    fn mission_panel_fits_phone_viewports_and_clears_touch_band() {
        for (width, height) in [(844.0, 390.0), (960.0, 480.0), (1440.0, 900.0)] {
            let node = mission_panel_node(width, height);
            let Val::Px(left) = node.left else { panic!() };
            let Val::Px(top) = node.top else { panic!() };
            let Val::Px(panel_width) = node.width else {
                panic!()
            };
            let Val::Px(panel_height) = node.height else {
                panic!()
            };
            assert!(left >= 16.0 && left + panel_width <= width - 16.0 + 1e-5);
            assert!(top >= 16.0);
            assert!(top + panel_height <= height - TOUCH_BAND_HEIGHT);
        }
    }
}
