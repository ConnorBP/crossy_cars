use bevy::{prelude::*, text::FontSize};

use crate::car::Car;
use crate::game::SpawnSet;
use crate::game::resources::{GameOverReason, Score, TimeLeft};
use crate::game::state::GameState;
use crate::modifiers::{ActiveModifier, ModifierKind};
use crate::objectives::ActiveObjective;
use crate::palette;
use crate::persist::{
    BestAtRoundStart, ConditionBests, ConditionBestsAtRoundStart, Medal, medal_for,
};
use crate::touch::TouchControlsActive;

const ALL_CONDITIONS: [ModifierKind; 5] = [
    ModifierKind::Standard,
    ModifierKind::RushHour,
    ModifierKind::ChickenFrenzy,
    ModifierKind::Stampede,
    ModifierKind::GlassCannon,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TimerUrgency {
    Normal,
    Urgent,
    Critical,
}

#[derive(Clone, Copy, Debug)]
struct TimerStyle {
    urgency: TimerUrgency,
    alpha: f32,
}

/// Pure countdown styling: the last ten seconds pulse red, with the final
/// five pulsing twice as fast. No entities or effects are spawned.
fn timer_style(remaining: f32, elapsed: f32) -> TimerStyle {
    let (urgency, pulse_hz) = if remaining > 10.0 {
        (TimerUrgency::Normal, 0.0)
    } else if remaining > 5.0 {
        (TimerUrgency::Urgent, 2.0)
    } else {
        (TimerUrgency::Critical, 4.0)
    };
    let alpha = if urgency == TimerUrgency::Normal {
        1.0
    } else {
        let wave = (elapsed * pulse_hz * std::f32::consts::TAU).sin();
        0.725 + 0.275 * wave
    };
    TimerStyle { urgency, alpha }
}

fn is_new_best(total: u32, best_at_round_start: u32) -> bool {
    total > best_at_round_start
}

/// Format one condition's record without displaying a meaningless None tier.
fn condition_summary(kind: ModifierKind, best: u32) -> String {
    let prefix = format!("{} BEST: {}", kind.display_name(), best);
    match medal_for(kind, best) {
        Medal::None => prefix,
        medal => format!("{prefix} · {}", medal.label()),
    }
}

/// Number of earned tiers represented by a medal (maximum three per condition).
const fn medal_points(medal: Medal) -> u32 {
    match medal {
        Medal::None => 0,
        Medal::Bronze => 1,
        Medal::Silver => 2,
        Medal::Gold => 3,
    }
}

fn is_medal_upgrade(previous: Medal, current: Medal) -> bool {
    medal_points(current) > medal_points(previous)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TerminalConditionResult {
    displayed_best: u32,
    new_best: bool,
    medal: Medal,
    medal_upgrade: bool,
}

/// Derive all condition record presentation from the round-start snapshot and
/// terminal score. This deliberately ignores the live `ConditionBests`
/// resource so OnEnter persistence ordering cannot change the Game Over UI.
fn terminal_condition_result(
    kind: ModifierKind,
    best_at_round_start: u32,
    terminal_total: u32,
) -> TerminalConditionResult {
    let displayed_best = best_at_round_start.max(terminal_total);
    let previous_medal = medal_for(kind, best_at_round_start);
    let medal = medal_for(kind, displayed_best);
    TerminalConditionResult {
        displayed_best,
        new_best: terminal_total > best_at_round_start,
        medal,
        medal_upgrade: is_medal_upgrade(previous_medal, medal),
    }
}

fn total_medal_points(condition_bests: &ConditionBests) -> u32 {
    ALL_CONDITIONS
        .into_iter()
        .map(|kind| medal_points(medal_for(kind, condition_bests.by_kind[kind.index()])))
        .sum()
}

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
struct ScoreText;
#[derive(Component)]
struct ChickensText;
#[derive(Component)]
struct CoinsText;
#[derive(Component)]
struct TimerText;
#[derive(Component)]
struct MenuMedalsText;

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
            // Modifier selection happens before SpawnSet on fresh rounds, so
            // the HUD always captures the condition selected for this run.
            .add_systems(
                OnEnter(GameState::Playing),
                (spawn_hud.after(SpawnSet), spawn_hint),
            )
            .add_systems(
                OnExit(GameState::Playing),
                (despawn_marker::<HudRoot>, despawn_marker::<HintRoot>),
            )
            .add_systems(OnEnter(GameState::Paused), spawn_pause)
            .add_systems(OnExit(GameState::Paused), despawn_marker::<PauseRoot>)
            .add_systems(OnEnter(GameState::GameOver), spawn_gameover)
            .add_systems(OnExit(GameState::GameOver), despawn_marker::<GameOverRoot>)
            .add_systems(
                Update,
                (
                    update_speed_text,
                    update_gear_text,
                    update_score_text,
                    update_chickens_text,
                    update_coins_text,
                    update_timer_text,
                    update_hint,
                )
                    .run_if(in_state(GameState::Playing)),
            )
            // Persistence loads during Startup, after the initial Menu may
            // already exist. Refreshing this marker makes the saved medal
            // tally visible immediately without rebuilding the menu.
            .add_systems(Update, update_menu_medals);
    }
}

fn spawn_menu(mut commands: Commands, condition_bests: Res<ConditionBests>) {
    let earned_medals = total_medal_points(&condition_bests);

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
                Text::new("ROADY CAR"),
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
                Text::new("Hit wandering chickens for score! Coins give bonus time."),
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
                    margin: UiRect::bottom(px(18.0)),
                    ..default()
                },
            ));
            p.spawn((
                Text::new(format!("MEDALS: {earned_medals} / 15")),
                TextFont {
                    font_size: FontSize::Px(20.0),
                    ..default()
                },
                TextColor(palette::HUD_TEXT.into()),
                Node {
                    margin: UiRect::bottom(px(18.0)),
                    ..default()
                },
                MenuMedalsText,
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

fn spawn_hud(mut commands: Commands, active_modifier: Res<ActiveModifier>) {
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
            p.spawn((Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::End,
                margin: UiRect::bottom(px(6.0)),
                ..default()
            },))
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
            // SCORE label
            p.spawn((
                Text::new("SCORE"),
                TextFont {
                    font_size: FontSize::Px(13.0),
                    ..default()
                },
                TextColor(Color::srgba(0.75, 0.75, 0.8, 1.0).into()),
                Node {
                    margin: UiRect {
                        top: px(8.0),
                        bottom: px(2.0),
                        ..default()
                    },
                    ..default()
                },
            ));
            // Big score number (chickens + coins)
            p.spawn((
                Text::new(""),
                TextFont {
                    font_size: FontSize::Px(36.0),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
                Node {
                    margin: UiRect::bottom(px(6.0)),
                    ..default()
                },
            ))
            .with_child((
                TextSpan::default(),
                TextFont {
                    font_size: FontSize::Px(36.0),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
                ScoreText,
            ));
            // Chickens line
            p.spawn((
                Text::new("Chickens: "),
                TextFont {
                    font_size: FontSize::Px(20.0),
                    ..default()
                },
                TextColor(palette::HUD_TEXT.into()),
                Node {
                    margin: UiRect::bottom(px(2.0)),
                    ..default()
                },
            ))
            .with_child((
                TextSpan::default(),
                TextFont {
                    font_size: FontSize::Px(20.0),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
                ChickensText,
            ));
            // Coins line
            p.spawn((
                Text::new("Coins: "),
                TextFont {
                    font_size: FontSize::Px(20.0),
                    ..default()
                },
                TextColor(palette::HUD_TEXT.into()),
                Node {
                    margin: UiRect::bottom(px(8.0)),
                    ..default()
                },
            ))
            .with_child((
                TextSpan::default(),
                TextFont {
                    font_size: FontSize::Px(20.0),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
                CoinsText,
            ));
            // Current road condition. It is created once with the HUD and
            // removed by the existing HudRoot lifecycle (including pauses).
            p.spawn((
                Text::new("ROAD CONDITION"),
                TextFont {
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
                TextColor(Color::srgba(0.75, 0.75, 0.8, 1.0).into()),
                Node {
                    margin: UiRect::bottom(px(1.0)),
                    ..default()
                },
            ));
            p.spawn((
                Text::new(active_modifier.display_name()),
                TextFont {
                    font_size: FontSize::Px(18.0),
                    ..default()
                },
                TextColor(active_modifier.color()),
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
            Text::new("Time Left: "),
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

fn spawn_hint(mut commands: Commands, touch_active: Res<TouchControlsActive>) {
    // Touch-started rounds have persistent on-screen controls; the keyboard
    // hint would overlap them, especially on short landscape viewports.
    if touch_active.0 {
        return;
    }
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
                Text::new("ESC  Resume  •  R  Restart  •  Q  Menu"),
                TextFont {
                    font_size: FontSize::Px(26.0),
                    ..default()
                },
                TextColor(palette::HUD_TEXT.into()),
            ));
        });
}

fn spawn_gameover(
    mut commands: Commands,
    score: Res<Score>,
    reason: Res<GameOverReason>,
    best_at_start: Res<BestAtRoundStart>,
    active_modifier: Res<ActiveModifier>,
    conditions_at_start: Res<ConditionBestsAtRoundStart>,
    objective: Res<ActiveObjective>,
) {
    let total = score.chickens + score.coins;
    let new_best = is_new_best(total, best_at_start.0);
    let best_summary = if new_best {
        "NEW BEST".to_string()
    } else {
        format!("BEST: {}", best_at_start.0)
    };

    let kind = active_modifier.0;
    let condition_result =
        terminal_condition_result(kind, conditions_at_start.by_kind[kind.index()], total);
    let condition_summary = condition_summary(kind, condition_result.displayed_best);

    let title = match *reason {
        GameOverReason::Wrecked => "Wrecked!",
        GameOverReason::TimeUp => "Time's up!",
    };
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
                Text::new(title),
                TextFont {
                    font_size: FontSize::Px(56.0),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
                Node {
                    margin: UiRect::bottom(px(14.0)),
                    ..default()
                },
            ));
            // SCORE (label + big accent value)
            p.spawn((
                Text::new("SCORE"),
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
                Text::new(format!("{}", total)),
                TextFont {
                    font_size: FontSize::Px(38.0),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
                Node {
                    margin: UiRect::bottom(px(10.0)),
                    ..default()
                },
            ));
            // Chickens
            p.spawn((
                Text::new(format!("Chickens: {}", score.chickens)),
                TextFont {
                    font_size: FontSize::Px(21.0),
                    ..default()
                },
                TextColor(palette::HUD_TEXT.into()),
                Node {
                    margin: UiRect::bottom(px(2.0)),
                    ..default()
                },
            ));
            // Coins
            p.spawn((
                Text::new(format!("Coins: {}", score.coins)),
                TextFont {
                    font_size: FontSize::Px(21.0),
                    ..default()
                },
                TextColor(palette::HUD_TEXT.into()),
                Node {
                    margin: UiRect::bottom(px(14.0)),
                    ..default()
                },
            ));
            // Record status uses the pre-round snapshot plus terminal score,
            // independent of persistence-system ordering on this transition.
            p.spawn((
                Text::new(best_summary),
                TextFont {
                    font_size: FontSize::Px(24.0),
                    ..default()
                },
                TextColor(if new_best {
                    palette::HUD_ACCENT.into()
                } else {
                    palette::HUD_TEXT.into()
                }),
                Node {
                    margin: UiRect::bottom(px(10.0)),
                    ..default()
                },
            ));
            p.spawn((
                Text::new(condition_summary),
                TextFont {
                    font_size: FontSize::Px(22.0),
                    ..default()
                },
                TextColor(active_modifier.color()),
                Node {
                    margin: UiRect::bottom(px(6.0)),
                    ..default()
                },
            ));
            if condition_result.new_best {
                p.spawn((
                    Text::new("NEW CONDITION BEST"),
                    TextFont {
                        font_size: FontSize::Px(20.0),
                        ..default()
                    },
                    TextColor(palette::HUD_ACCENT.into()),
                    Node {
                        margin: UiRect::bottom(px(4.0)),
                        ..default()
                    },
                ));
            }
            if condition_result.medal_upgrade {
                p.spawn((
                    Text::new(format!("MEDAL UPGRADE: {}", condition_result.medal.label())),
                    TextFont {
                        font_size: FontSize::Px(20.0),
                        ..default()
                    },
                    TextColor(palette::HUD_ACCENT.into()),
                    Node {
                        margin: UiRect::bottom(px(8.0)),
                        ..default()
                    },
                ));
            }
            p.spawn((
                Text::new(format!("OBJECTIVE: {}", objective.summary())),
                TextFont {
                    font_size: FontSize::Px(20.0),
                    ..default()
                },
                TextColor(if objective.completed {
                    palette::HUD_ACCENT.into()
                } else {
                    palette::HUD_TEXT.into()
                }),
                Node {
                    margin: UiRect::bottom(px(8.0)),
                    ..default()
                },
            ));
            // Restart / menu prompt
            p.spawn((
                Text::new("R / ENTER / SPACE to play again  •  Q / ESC for menu"),
                TextFont {
                    font_size: FontSize::Px(19.0),
                    ..default()
                },
                TextColor(Color::srgba(0.8, 0.8, 0.85, 1.0).into()),
                Node {
                    margin: UiRect::top(px(8.0)),
                    ..default()
                },
            ));
        });
}

fn despawn_marker<M: Component>(mut commands: Commands, q: Query<Entity, With<M>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}

fn update_menu_medals(
    condition_bests: Res<ConditionBests>,
    mut query: Query<&mut Text, With<MenuMedalsText>>,
) {
    if !condition_bests.is_changed() {
        return;
    }
    let earned = total_medal_points(&condition_bests);
    for mut text in &mut query {
        **text = format!("MEDALS: {earned} / 15");
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

fn update_score_text(score: Res<Score>, mut query: Query<&mut TextSpan, With<ScoreText>>) {
    for mut span in &mut query {
        **span = format!("{}", score.chickens + score.coins);
    }
}

fn update_chickens_text(score: Res<Score>, mut query: Query<&mut TextSpan, With<ChickensText>>) {
    for mut span in &mut query {
        **span = format!("{}", score.chickens);
    }
}

fn update_coins_text(score: Res<Score>, mut query: Query<&mut TextSpan, With<CoinsText>>) {
    for mut span in &mut query {
        **span = format!("{}", score.coins);
    }
}

fn update_timer_text(
    timeleft: Res<TimeLeft>,
    time: Res<Time>,
    mut query: Query<(&mut TextSpan, &mut TextColor), With<TimerText>>,
) {
    let t = timeleft.0.max(0.0);
    let mins = (t / 60.0).floor() as u32;
    let secs = (t % 60.0).floor() as u32;
    let style = timer_style(t, time.elapsed_secs());
    let color = match style.urgency {
        TimerUrgency::Normal => palette::HUD_ACCENT.into(),
        TimerUrgency::Urgent | TimerUrgency::Critical => Color::srgba(1.0, 0.08, 0.08, style.alpha),
    };
    for (mut span, mut text_color) in &mut query {
        **span = format!("{}:{:02}", mins, secs);
        text_color.0 = color;
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

#[cfg(test)]
mod tests {
    use super::{
        TimerUrgency, condition_summary, is_medal_upgrade, is_new_best, medal_points,
        terminal_condition_result, timer_style,
    };
    use crate::modifiers::ModifierKind;
    use crate::persist::Medal;

    #[test]
    fn timer_urgency_has_inclusive_thresholds_and_faster_final_pulse() {
        let normal = timer_style(10.01, 0.0);
        let urgent = timer_style(10.0, 0.0625);
        let critical = timer_style(5.0, 0.0625);

        assert_eq!(normal.urgency, TimerUrgency::Normal);
        assert_eq!(normal.alpha, 1.0);
        assert_eq!(urgent.urgency, TimerUrgency::Urgent);
        assert_eq!(critical.urgency, TimerUrgency::Critical);
        // At the same early instant the 4 Hz critical wave is further
        // through its cycle than the 2 Hz urgent wave.
        assert!(critical.alpha > urgent.alpha);
    }

    #[test]
    fn new_best_requires_strict_improvement() {
        assert!(is_new_best(11, 10));
        assert!(!is_new_best(10, 10));
        assert!(!is_new_best(9, 10));
    }

    #[test]
    fn condition_summary_omits_none_and_includes_threshold_medals() {
        assert_eq!(
            condition_summary(ModifierKind::Standard, 19),
            "Standard BEST: 19"
        );
        assert_eq!(
            condition_summary(ModifierKind::Standard, 20),
            "Standard BEST: 20 · Bronze"
        );
        assert_eq!(
            condition_summary(ModifierKind::RushHour, 30),
            "Rush Hour BEST: 30 · Silver"
        );
        assert_eq!(
            condition_summary(ModifierKind::GlassCannon, 80),
            "Glass Cannon BEST: 80 · Gold"
        );
    }

    #[test]
    fn medal_points_and_upgrades_follow_tier_boundaries() {
        assert_eq!(medal_points(Medal::None), 0);
        assert_eq!(medal_points(Medal::Bronze), 1);
        assert_eq!(medal_points(Medal::Silver), 2);
        assert_eq!(medal_points(Medal::Gold), 3);

        assert!(is_medal_upgrade(Medal::None, Medal::Bronze));
        assert!(is_medal_upgrade(Medal::Bronze, Medal::Silver));
        assert!(is_medal_upgrade(Medal::Silver, Medal::Gold));
        assert!(is_medal_upgrade(Medal::None, Medal::Gold));
        assert!(!is_medal_upgrade(Medal::Bronze, Medal::Bronze));
        assert!(!is_medal_upgrade(Medal::Gold, Medal::Silver));
    }

    #[test]
    fn terminal_condition_score_drives_summary_and_medal_upgrade() {
        let upgraded = terminal_condition_result(ModifierKind::Standard, 19, 20);
        assert_eq!(upgraded.displayed_best, 20);
        assert!(upgraded.new_best);
        assert_eq!(upgraded.medal, Medal::Bronze);
        assert!(upgraded.medal_upgrade);

        let lower_terminal = terminal_condition_result(ModifierKind::Standard, 40, 18);
        assert_eq!(lower_terminal.displayed_best, 40);
        assert!(!lower_terminal.new_best);
        assert_eq!(lower_terminal.medal, Medal::Silver);
        assert!(!lower_terminal.medal_upgrade);

        let equal = terminal_condition_result(ModifierKind::Standard, 20, 20);
        assert!(!equal.new_best);
        assert!(!equal.medal_upgrade);
    }
}
