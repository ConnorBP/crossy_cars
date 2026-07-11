use bevy::{prelude::*, text::FontSize};

use crate::car::Car;
use crate::game::resources::{GameTimer, Score};
use crate::game::state::GameState;
use crate::palette;

// --- UI root markers ---
#[derive(Component)]
struct MenuRoot;
#[derive(Component)]
struct HudRoot;
#[derive(Component)]
struct HintRoot;
#[derive(Component)]
struct PauseRoot;
#[derive(Component)]
struct GameOverRoot;

// --- Dynamic text span markers ---
#[derive(Component)]
struct SpeedText;
#[derive(Component)]
struct GearText;
#[derive(Component)]
struct CoinsText;
#[derive(Component)]
struct TimerText;

/// Countdown for the transient controls hint; entity is despawned when it hits zero.
#[derive(Component)]
struct Hint {
    t: f32,
}

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(GameState::Menu), spawn_menu)
            .add_systems(OnExit(GameState::Menu), despawn_marker::<MenuRoot>)
            .add_systems(
                OnEnter(GameState::Playing),
                (spawn_hud, spawn_hint),
            )
            .add_systems(
                OnExit(GameState::Playing),
                (despawn_marker::<HudRoot>, despawn_marker::<HintRoot>),
            )
            .add_systems(OnEnter(GameState::Paused), spawn_pause)
            .add_systems(OnExit(GameState::Paused), despawn_marker::<PauseRoot>)
            .add_systems(OnEnter(GameState::GameOver), spawn_gameover)
            .add_systems(
                OnExit(GameState::GameOver),
                despawn_marker::<GameOverRoot>,
            )
            .add_systems(
                Update,
                (
                    update_speed_text,
                    update_gear_text,
                    update_coins_text,
                    update_timer_text,
                    update_hint,
                )
                    .run_if(in_state(GameState::Playing)),
            );
    }
}

fn spawn_menu(mut commands: Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: px(0.0),
                left: px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                flex_direction: FlexDirection::Column,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
            MenuRoot,
        ))
        .with_children(|p| {
            // Title
            p.spawn((
                Text::new("ISO RACER"),
                TextFont {
                    font_size: FontSize::Px(72.0),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
                Node {
                    margin: UiRect::bottom(px(10.0)),
                    ..default()
                },
            ));
            // Subtitle
            p.spawn((
                Text::new("Collect every coin. Chase your best time."),
                TextFont {
                    font_size: FontSize::Px(22.0),
                    ..default()
                },
                TextColor(palette::HUD_TEXT.into()),
                Node {
                    margin: UiRect::bottom(px(30.0)),
                    ..default()
                },
            ));
            // Controls list
            p.spawn((
                Text::new("WASD / Arrows  —  Drive"),
                TextFont {
                    font_size: FontSize::Px(20.0),
                    ..default()
                },
                TextColor(Color::srgba(0.8, 0.8, 0.85, 1.0).into()),
                Node {
                    margin: UiRect::bottom(px(4.0)),
                    ..default()
                },
            ));
            p.spawn((
                Text::new("Space  —  Brake"),
                TextFont {
                    font_size: FontSize::Px(20.0),
                    ..default()
                },
                TextColor(Color::srgba(0.8, 0.8, 0.85, 1.0).into()),
                Node {
                    margin: UiRect::bottom(px(4.0)),
                    ..default()
                },
            ));
            p.spawn((
                Text::new("Esc  —  Pause"),
                TextFont {
                    font_size: FontSize::Px(20.0),
                    ..default()
                },
                TextColor(Color::srgba(0.8, 0.8, 0.85, 1.0).into()),
                Node {
                    margin: UiRect::bottom(px(32.0)),
                    ..default()
                },
            ));
            // Prompt
            p.spawn((
                Text::new("Press ENTER / SPACE to drive"),
                TextFont {
                    font_size: FontSize::Px(30.0),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
            ));
        });
}

fn spawn_hud(mut commands: Commands) {
    // --- Top-left cockpit cluster on a semi-transparent panel ---
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: px(12.0),
                left: px(14.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(px(10.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.35)),
            HudRoot,
        ))
        .with_children(|p| {
            // SPEED label
            p.spawn((
                Text::new("SPEED"),
                TextFont {
                    font_size: FontSize::Px(13.0),
                    ..default()
                },
                TextColor(Color::srgba(0.75, 0.75, 0.8, 1.0).into()),
                Node {
                    margin: UiRect::bottom(px(2.0)),
                    ..default()
                },
            ));
            // Big digital speed (40px accent) + smaller " u/s" unit, laid out inline.
            p.spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::End,
                    margin: UiRect::bottom(px(6.0)),
                    ..default()
                },
            ))
            .with_children(|s| {
                s.spawn((
                    Text::new(""),
                    TextFont {
                        font_size: FontSize::Px(40.0),
                        ..default()
                    },
                    TextColor(palette::HUD_ACCENT.into()),
                ))
                .with_child((
                    TextSpan::default(),
                    TextFont {
                        font_size: FontSize::Px(40.0),
                        ..default()
                    },
                    TextColor(palette::HUD_ACCENT.into()),
                    SpeedText,
                ));
                s.spawn((
                    Text::new(" u/s"),
                    TextFont {
                        font_size: FontSize::Px(20.0),
                        ..default()
                    },
                    TextColor(palette::HUD_TEXT.into()),
                    Node {
                        margin: UiRect::left(px(4.0)),
                        ..default()
                    },
                ));
            });
            // Gear / direction line
            p.spawn((
                Text::new("GEAR "),
                TextFont {
                    font_size: FontSize::Px(18.0),
                    ..default()
                },
                TextColor(Color::srgba(0.75, 0.75, 0.8, 1.0).into()),
                Node {
                    margin: UiRect::bottom(px(6.0)),
                    ..default()
                },
            ))
            .with_child((
                TextSpan::default(),
                TextFont {
                    font_size: FontSize::Px(18.0),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
                GearText,
            ));
            // Coins line
            p.spawn((
                Text::new("Coins: "),
                TextFont {
                    font_size: FontSize::Px(22.0),
                    ..default()
                },
                TextColor(palette::HUD_TEXT.into()),
            ))
            .with_child((
                TextSpan::default(),
                TextFont {
                    font_size: FontSize::Px(22.0),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
                CoinsText,
            ));
        });

    // --- Timer (top-right) ---
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: px(12.0),
                right: px(16.0),
                ..default()
            },
            HudRoot,
            Text::new("Time: "),
            TextFont {
                font_size: FontSize::Px(24.0),
                ..default()
            },
            TextColor(palette::HUD_TEXT.into()),
        ))
        .with_child((
            TextSpan::default(),
            TextFont {
                font_size: FontSize::Px(24.0),
                ..default()
            },
            TextColor(palette::HUD_ACCENT.into()),
            TimerText,
        ));
}

fn spawn_hint(mut commands: Commands) {
    // Lower-center transient hint, auto-dismissed after ~3.5s by `update_hint`.
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                bottom: px(28.0),
                left: px(0.0),
                width: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            HintRoot,
            Hint { t: 3.5 },
        ))
        .with_children(|p| {
            // Pill background + label
            p.spawn((
                Node {
                    padding: UiRect::all(px(8.0)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.45)),
            ))
            .with_child((
                Text::new("WASD to drive  •  Space to brake  •  Esc to pause"),
                TextFont {
                    font_size: FontSize::Px(20.0),
                    ..default()
                },
                TextColor(Color::srgba(0.92, 0.92, 0.96, 1.0).into()),
            ));
        });
}

fn spawn_pause(mut commands: Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: px(0.0),
                left: px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                flex_direction: FlexDirection::Column,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.5)),
            PauseRoot,
        ))
        .with_children(|p| {
            p.spawn((
                Text::new("PAUSED"),
                TextFont {
                    font_size: FontSize::Px(64.0),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
                Node {
                    margin: UiRect::bottom(px(14.0)),
                    ..default()
                },
            ));
            p.spawn((
                Text::new("ESC to resume"),
                TextFont {
                    font_size: FontSize::Px(26.0),
                    ..default()
                },
                TextColor(palette::HUD_TEXT.into()),
            ));
        });
}

fn spawn_gameover(mut commands: Commands, score: Res<Score>, timer: Res<GameTimer>) {
    let (mins, secs) = (timer.0 / 60.0, timer.0 % 60.0);
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: px(0.0),
                left: px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                flex_direction: FlexDirection::Column,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
            GameOverRoot,
        ))
        .with_children(|p| {
            // Title
            p.spawn((
                Text::new("FINISH!"),
                TextFont {
                    font_size: FontSize::Px(64.0),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
                Node {
                    margin: UiRect::bottom(px(22.0)),
                    ..default()
                },
            ));
            // Coins (label + big accent value)
            p.spawn((
                Text::new("COINS"),
                TextFont {
                    font_size: FontSize::Px(16.0),
                    ..default()
                },
                TextColor(Color::srgba(0.7, 0.7, 0.75, 1.0).into()),
                Node {
                    margin: UiRect::bottom(px(2.0)),
                    ..default()
                },
            ));
            p.spawn((
                Text::new(format!("{} / {}", score.collected, score.total)),
                TextFont {
                    font_size: FontSize::Px(34.0),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
                Node {
                    margin: UiRect::bottom(px(16.0)),
                    ..default()
                },
            ));
            // Time (label + big accent value)
            p.spawn((
                Text::new("TIME"),
                TextFont {
                    font_size: FontSize::Px(16.0),
                    ..default()
                },
                TextColor(Color::srgba(0.7, 0.7, 0.75, 1.0).into()),
                Node {
                    margin: UiRect::bottom(px(2.0)),
                    ..default()
                },
            ));
            p.spawn((
                Text::new(format!("{:02.0}:{:05.2}", mins, secs)),
                TextFont {
                    font_size: FontSize::Px(34.0),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
                Node {
                    margin: UiRect::bottom(px(28.0)),
                    ..default()
                },
            ));
            // Restart / menu prompt
            p.spawn((
                Text::new("ENTER to race again  •  ESC for menu"),
                TextFont {
                    font_size: FontSize::Px(22.0),
                    ..default()
                },
                TextColor(Color::srgba(0.8, 0.8, 0.85, 1.0).into()),
            ));
        });
}

fn despawn_marker<M: Component>(mut commands: Commands, q: Query<Entity, With<M>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}

fn update_speed_text(car: Query<&Car>, mut query: Query<&mut TextSpan, With<SpeedText>>) {
    let Ok(car) = car.single() else {
        return;
    };
    for mut span in &mut query {
        **span = format!("{:>4.0}", car.speed.abs());
    }
}

fn update_gear_text(car: Query<&Car>, mut query: Query<&mut TextSpan, With<GearText>>) {
    let Ok(car) = car.single() else {
        return;
    };
    let label = if car.speed > 0.05 {
        "FWD"
    } else if car.speed < -0.05 {
        "REV"
    } else {
        "IDLE"
    };
    for mut span in &mut query {
        **span = label.to_string();
    }
}

fn update_coins_text(score: Res<Score>, mut query: Query<&mut TextSpan, With<CoinsText>>) {
    for mut span in &mut query {
        **span = format!("{} / {}", score.collected, score.total);
    }
}

fn update_timer_text(timer: Res<GameTimer>, mut query: Query<&mut TextSpan, With<TimerText>>) {
    let (mins, secs) = (timer.0 / 60.0, timer.0 % 60.0);
    for mut span in &mut query {
        **span = format!("{:02.0}:{:05.2}", mins, secs);
    }
}

fn update_hint(mut commands: Commands, time: Res<Time>, mut q: Query<(Entity, &mut Hint)>) {
    for (e, mut hint) in &mut q {
        hint.t -= time.delta_secs();
        if hint.t <= 0.0 {
            commands.entity(e).despawn();
        }
    }
}
