use bevy::{prelude::*, text::FontSize};

use crate::car::Car;
use crate::game::resources::{GameTimer, Score};
use crate::game::state::GameState;
use crate::palette;

// --- UI markers ---
#[derive(Component)]
struct MenuRoot;
#[derive(Component)]
struct HudRoot;
#[derive(Component)]
struct PauseRoot;
#[derive(Component)]
struct GameOverRoot;

#[derive(Component)]
struct SpeedText;
#[derive(Component)]
struct CoinsText;
#[derive(Component)]
struct TimerText;

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(GameState::Menu), spawn_menu)
            .add_systems(OnExit(GameState::Menu), despawn_marker::<MenuRoot>)
            .add_systems(OnEnter(GameState::Playing), spawn_hud)
            .add_systems(OnExit(GameState::Playing), despawn_marker::<HudRoot>)
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
                    update_coins_text,
                    update_timer_text,
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
            p.spawn((
                Text::new("ISO RACER"),
                TextFont {
                    font_size: FontSize::Px(72.0),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
            ));
            p.spawn((
                Text::new("Press ENTER / SPACE to drive"),
                TextFont {
                    font_size: FontSize::Px(28.0),
                    ..default()
                },
                TextColor(palette::HUD_TEXT.into()),
            ));
            p.spawn((
                Text::new("WASD / Arrows to drive  •  ESC to pause"),
                TextFont {
                    font_size: FontSize::Px(20.0),
                    ..default()
                },
                TextColor(Color::srgba(0.8, 0.8, 0.8, 1.0).into()),
            ));
        });
}

fn spawn_hud(mut commands: Commands) {
    // Speed (top-left)
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: px(12.0),
                left: px(14.0),
                ..default()
            },
            HudRoot,
            Text::new("Speed: "),
            TextFont {
                font_size: FontSize::Px(28.0),
                ..default()
            },
            TextColor(palette::HUD_TEXT.into()),
        ))
        .with_child((
            TextSpan::default(),
            TextFont {
                font_size: FontSize::Px(28.0),
                ..default()
            },
            TextColor(palette::HUD_ACCENT.into()),
            SpeedText,
        ));

    // Coins (top-left, below speed)
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: px(48.0),
                left: px(14.0),
                ..default()
            },
            HudRoot,
            Text::new("Coins: "),
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
            CoinsText,
        ));

    // Timer (top-right)
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
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.5)),
            PauseRoot,
        ))
        .with_children(|p| {
            p.spawn((
                Text::new("PAUSED\nESC to resume"),
                TextFont {
                    font_size: FontSize::Px(48.0),
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
            p.spawn((
                Text::new("FINISH!"),
                TextFont {
                    font_size: FontSize::Px(64.0),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
            ));
            p.spawn((
                Text::new(format!(
                    "Coins: {} / {}   Time: {:02.0}:{:05.2}",
                    score.collected,
                    score.total,
                    mins,
                    secs
                )),
                TextFont {
                    font_size: FontSize::Px(28.0),
                    ..default()
                },
                TextColor(palette::HUD_TEXT.into()),
            ));
            p.spawn((
                Text::new("ENTER to race again  •  ESC for menu"),
                TextFont {
                    font_size: FontSize::Px(22.0),
                    ..default()
                },
                TextColor(Color::srgba(0.8, 0.8, 0.8, 1.0).into()),
            ));
        });
}

fn despawn_marker<M: Component>(mut commands: Commands, q: Query<Entity, With<M>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}

fn update_speed_text(
    car: Query<&Car>,
    mut query: Query<&mut TextSpan, With<SpeedText>>,
) {
    let Ok(car) = car.single() else {
        return;
    };
    for mut span in &mut query {
        **span = format!("{:>4.0} u/s", car.speed.abs());
    }
}

fn update_coins_text(score: Res<Score>, mut query: Query<&mut TextSpan, With<CoinsText>>) {
    for mut span in &mut query {
        **span = format!("{} / {}", score.collected, score.total);
    }
}

fn update_timer_text(
    timer: Res<GameTimer>,
    mut query: Query<&mut TextSpan, With<TimerText>>,
) {
    let (mins, secs) = (timer.0 / 60.0, timer.0 % 60.0);
    for mut span in &mut query {
        **span = format!("{:02.0}:{:05.2}", mins, secs);
    }
}
