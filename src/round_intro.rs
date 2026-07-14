use bevy::prelude::*;
use bevy::text::{FontSize, TextLayout};

use crate::car::Car;
use crate::chickens::{ChickenAssets, spawn_chicken_visual};
use crate::countdown::Countdown;
use crate::game::SpawnSet;
use crate::game::resources::RoundActive;
use crate::game::state::GameState;
use crate::objectives::{
    ActiveObjective, ObjectiveHudRoot, ObjectiveSelectionSet, mission_announcement,
};
use crate::settings::Settings;

const HOLD_SECS: f32 = 1.0;
const END_SECS: f32 = PLUS_ONE_IMPACT_SECS;
const TOUCH_BAND_HEIGHT: f32 = 44.0;

#[derive(Resource, Default)]
struct RoundIntroState {
    active: bool,
}

#[derive(Component)]
struct RoundIntroRoot;

#[derive(Component)]
struct MissionAnnouncementText;

#[derive(Component)]
struct DemoChicken {
    start: Vec3,
    target: Vec3,
}

#[derive(Component)]
struct DemoPlusOne;

#[derive(Clone, Copy, Debug, PartialEq)]
struct PlusOneVisual {
    alpha: f32,
    visible: bool,
}

const PLUS_ONE_FADE_START: f32 = 2.1;
const PLUS_ONE_FADE_END: f32 = 2.70;
const PLUS_ONE_POP_TOP: f32 = 72.0;
const PLUS_ONE_RISE_TOP: f32 = 64.0;
const PLUS_ONE_FONT: f32 = 38.0;
const PLUS_ONE_IMPACT_SECS: f32 = 1.35;

fn plus_one_visual(elapsed: f32, reduced_motion: bool) -> PlusOneVisual {
    if elapsed < PLUS_ONE_IMPACT_SECS {
        return PlusOneVisual {
            alpha: 0.0,
            visible: false,
        };
    }
    if reduced_motion {
        return PlusOneVisual {
            alpha: 1.0,
            visible: elapsed < PLUS_ONE_FADE_END,
        };
    }
    if elapsed < PLUS_ONE_IMPACT_SECS {
        return PlusOneVisual {
            alpha: 0.0,
            visible: false,
        };
    }
    let alpha = if elapsed < PLUS_ONE_FADE_START {
        1.0
    } else if elapsed < PLUS_ONE_FADE_END {
        1.0 - (elapsed - PLUS_ONE_FADE_START) / (PLUS_ONE_FADE_END - PLUS_ONE_FADE_START)
    } else {
        0.0
    }
    .clamp(0.0, 1.0);
    PlusOneVisual {
        alpha,
        visible: elapsed < PLUS_ONE_FADE_END,
    }
}

fn plus_one_top(elapsed: f32, reduced_motion: bool) -> f32 {
    if reduced_motion || elapsed <= PLUS_ONE_IMPACT_SECS {
        return PLUS_ONE_POP_TOP;
    }
    let t = ((elapsed - PLUS_ONE_IMPACT_SECS) / (PLUS_ONE_FADE_END - PLUS_ONE_IMPACT_SECS))
        .clamp(0.0, 1.0);
    let eased = t * t * (3.0 - 2.0 * t);
    PLUS_ONE_POP_TOP - (PLUS_ONE_POP_TOP - PLUS_ONE_RISE_TOP) * eased
}

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
    assets: Res<ChickenAssets>,
    car: Query<&Transform, With<Car>>,
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
    let car_t = car.single().ok().copied().unwrap_or_default();
    let start = car_t.translation + Vec3::new(2.4, 0.0, -2.8);
    let target = car_t.translation + Vec3::new(0.45, 0.0, -0.75);
    let demo = spawn_chicken_visual(
        &mut commands,
        &assets,
        Transform::from_translation(start).with_rotation(Quat::from_rotation_y(-0.6)),
    );
    commands
        .entity(demo)
        .insert((DemoChicken { start, target }, RoundIntroRoot));
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: px(0.0),
            top: Val::Percent(PLUS_ONE_POP_TOP),
            width: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            ..default()
        },
        Text::new("DEMO +1 BASE SCORE"),
        TextFont {
            font_size: FontSize::Px(PLUS_ONE_FONT),
            ..default()
        },
        TextColor(Color::srgba(1.0, 0.72, 0.18, 0.0)),
        TextLayout::justify(Justify::Center),
        Visibility::Hidden,
        GlobalZIndex(81),
        DemoPlusOne,
        RoundIntroRoot,
    ));
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
    mut demo_chickens: Query<(&DemoChicken, &mut Transform), Without<Car>>,
    plus_ones: Query<Entity, With<DemoPlusOne>>,
    mut plus_one_visuals: Query<
        (&mut Visibility, &mut Node, &mut TextColor),
        (
            With<DemoPlusOne>,
            Without<MissionAnnouncementText>,
            Without<ObjectiveHudRoot>,
        ),
    >,
    mut mission_hud: Query<&mut Visibility, (With<ObjectiveHudRoot>, Without<DemoPlusOne>)>,
    mut panels: Query<&mut BackgroundColor, With<RoundIntroRoot>>,
    mut texts: Query<&mut TextColor, (With<MissionAnnouncementText>, Without<DemoPlusOne>)>,
) {
    if !state.active {
        return;
    }
    let elapsed = (3.0 - countdown.t).max(0.0);
    let visual = mission_visual(elapsed, settings.reduced_motion);
    for (demo, mut transform) in &mut demo_chickens {
        if elapsed >= PLUS_ONE_IMPACT_SECS {
            transform.translation = demo.target;
            transform.scale = Vec3::ZERO;
        } else if settings.reduced_motion {
            transform.translation = demo.target + Vec3::new(0.9, 0.0, 0.0);
        } else {
            let t = ((elapsed - 0.35) / 1.0).clamp(0.0, 1.0);
            let eased = t * t * (3.0 - 2.0 * t);
            transform.translation = demo.start.lerp(demo.target, eased);
        }
    }
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
    let p1_visual = plus_one_visual(elapsed, settings.reduced_motion);
    for (mut visibility, mut node, mut color) in &mut plus_one_visuals {
        *visibility = if p1_visual.visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        node.top = Val::Percent(plus_one_top(elapsed, settings.reduced_motion));
        color.0 = color.0.with_alpha(p1_visual.alpha);
    }
    if elapsed >= PLUS_ONE_FADE_END {
        for entity in &plus_ones {
            commands.entity(entity).despawn();
        }
    }
    if !visual.visible {
        for entity in &roots {
            if plus_ones.get(entity).is_err() {
                commands.entity(entity).despawn();
            }
        }
        state.active = elapsed < PLUS_ONE_FADE_END;
    }
    if elapsed >= PLUS_ONE_FADE_END {
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
    use crate::chickens::Chicken;
    use crate::combos::Combo;
    use crate::game::events::{ChickenHit, CoinCollected};
    use crate::game::resources::{Score, TimeLeft};
    use crate::health::Health;
    use crate::objectives::{ObjectiveCompleted, ObjectiveKind};

    #[test]
    fn real_intro_is_presentation_only_and_cleanup_is_complete() {
        let mut app = App::new();
        app.init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<ChickenAssets>()
            .insert_resource(RoundActive(false))
            .insert_resource(ActiveObjective::new(ObjectiveKind::HitChickens {
                target: 1,
            }))
            .insert_resource(Settings::default())
            .insert_resource({
                let mut countdown = Countdown::default();
                countdown.t = 3.0;
                countdown
            })
            .insert_resource(Score {
                chickens: 7,
                coins: 3,
            })
            .insert_resource(Combo::default())
            .insert_resource(TimeLeft(42.0))
            .insert_resource(Health(73.0))
            .init_resource::<RoundIntroState>()
            .add_message::<ChickenHit>()
            .add_message::<CoinCollected>()
            .add_message::<ObjectiveCompleted>()
            .add_systems(Startup, setup_round_intro)
            .add_systems(Update, update_round_intro);
        app.world_mut()
            .spawn((Window::default(), bevy::window::PrimaryWindow));
        app.world_mut().spawn((
            Car {
                speed: 0.0,
                heading: 0.0,
                drift: 0.0,
            },
            Transform::default(),
        ));

        app.update();
        let world = app.world_mut();
        let mut demos = world.query::<(&DemoChicken, Option<&Chicken>)>();
        let demo_rows: Vec<_> = demos.iter(world).collect();
        assert_eq!(demo_rows.len(), 1);
        assert!(
            demo_rows[0].1.is_none(),
            "demo must not enter real Chicken queries"
        );

        world.resource_mut::<Countdown>().t = 0.7;
        app.update();
        assert_eq!(app.world().resource::<Score>().chickens, 7);
        assert_eq!(app.world().resource::<Score>().coins, 3);
        assert_eq!(app.world().resource::<Combo>().multiplier, 1);
        assert_eq!(app.world().resource::<Combo>().timer, 0.0);
        assert_eq!(app.world().resource::<TimeLeft>().0, 42.0);
        assert_eq!(app.world().resource::<Health>().0, 73.0);
        assert_eq!(app.world().resource::<ActiveObjective>().progress, 0);
        assert_eq!(app.world().resource::<Messages<ChickenHit>>().len(), 0);
        assert_eq!(app.world().resource::<Messages<CoinCollected>>().len(), 0);
        assert_eq!(
            app.world().resource::<Messages<ObjectiveCompleted>>().len(),
            0
        );

        app.add_systems(PostUpdate, cleanup_round_intro);
        app.update();
        let world = app.world_mut();
        assert_eq!(
            world
                .query_filtered::<Entity, With<RoundIntroRoot>>()
                .iter(world)
                .count(),
            0
        );
        assert_eq!(
            world
                .query_filtered::<Entity, With<DemoChicken>>()
                .iter(world)
                .count(),
            0
        );
        assert_eq!(
            world
                .query_filtered::<Entity, With<DemoPlusOne>>()
                .iter(world)
                .count(),
            0
        );
    }

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
        let midpoint = (HOLD_SECS + END_SECS) * 0.5;
        assert!((mission_visual(midpoint, false).alpha - 0.5).abs() < 1e-5);
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

    #[test]
    fn plus_one_visual_fades_between_2_1_and_2_45_normal() {
        assert_eq!(
            plus_one_visual(1.5, false),
            PlusOneVisual {
                alpha: 1.0,
                visible: true
            }
        );
        assert_eq!(
            plus_one_visual(PLUS_ONE_FADE_START, false),
            PlusOneVisual {
                alpha: 1.0,
                visible: true
            }
        );
        let mid = (PLUS_ONE_FADE_START + PLUS_ONE_FADE_END) / 2.0;
        assert!((plus_one_visual(mid, false).alpha - 0.5).abs() < 1e-5);
        assert!(plus_one_visual(mid, false).visible);
        assert_eq!(
            plus_one_visual(PLUS_ONE_FADE_END, false),
            PlusOneVisual {
                alpha: 0.0,
                visible: false
            }
        );
        assert_eq!(
            plus_one_visual(2.8, false),
            PlusOneVisual {
                alpha: 0.0,
                visible: false
            }
        );
    }

    #[test]
    fn plus_one_visual_reduced_motion_stays_full_alpha_then_disappears() {
        assert_eq!(
            plus_one_visual(1.5, true),
            PlusOneVisual {
                alpha: 1.0,
                visible: true
            }
        );
        assert_eq!(
            plus_one_visual(PLUS_ONE_FADE_START, true),
            PlusOneVisual {
                alpha: 1.0,
                visible: true
            }
        );
        assert_eq!(
            plus_one_visual(PLUS_ONE_FADE_END - 0.001, true),
            PlusOneVisual {
                alpha: 1.0,
                visible: true
            }
        );
        assert_eq!(
            plus_one_visual(PLUS_ONE_FADE_END, true),
            PlusOneVisual {
                alpha: 1.0,
                visible: false
            }
        );
        assert_eq!(
            plus_one_visual(2.8, true),
            PlusOneVisual {
                alpha: 1.0,
                visible: false
            }
        );
    }
}
