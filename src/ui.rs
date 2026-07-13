use bevy::{prelude::*, text::FontSize, window::PrimaryWindow};

use crate::car::Car;
use crate::game::resources::{GameOverReason, Score, TimeLeft};
use crate::game::state::GameState;
use crate::game::{SpawnSet, TouchStateSet};
use crate::modifiers::{ActiveModifier, ModifierKind};
use crate::objectives::ActiveObjective;
use crate::palette;
use crate::persist::{
    BestAtRoundStart, ConditionBests, ConditionBestsAtRoundStart, Medal, medal_for,
};
use crate::settings::Settings;
use crate::touch::{
    TOUCH_COCKPIT_HEIGHT, TOUCH_COCKPIT_LEFT, TOUCH_COCKPIT_TOP, TOUCH_COCKPIT_WIDTH,
    TouchControlsActive,
};

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

/// Presentation policy derived from the live accessibility preference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TimerMotionFlags {
    alpha_pulse: bool,
}

fn timer_motion_flags(reduced_motion: bool) -> TimerMotionFlags {
    TimerMotionFlags {
        alpha_pulse: !reduced_motion,
    }
}

/// Pure timer styling: the last ten seconds pulse red, with the final five
/// pulsing twice as fast. Under reduced motion the alpha stays static while
/// urgency and color still escalate. No entities or effects are spawned.
fn timer_style(remaining: f32, elapsed: f32, flags: TimerMotionFlags) -> TimerStyle {
    let (urgency, pulse_hz) = if remaining > 10.0 {
        (TimerUrgency::Normal, 0.0)
    } else if remaining > 5.0 {
        (TimerUrgency::Urgent, 2.0)
    } else {
        (TimerUrgency::Critical, 4.0)
    };
    let alpha = if urgency == TimerUrgency::Normal {
        1.0
    } else if flags.alpha_pulse {
        let wave = (elapsed * pulse_hz * std::f32::consts::TAU).sin();
        0.725 + 0.275 * wave
    } else {
        // Reduced motion: hold the alpha steady. Urgency/color still convey
        // the escalation without the pulsing animation.
        1.0
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

/// Compact, non-color-only state for one condition in the Menu gallery.
/// `[X]` is earned and `[-]` is still locked; the B/S/G letters keep all
/// three tiers identifiable even when color is unavailable.
fn medal_gallery_state(kind: ModifierKind, best: u32) -> String {
    let earned = medal_points(medal_for(kind, best));
    let symbol = |tier: u32| if earned >= tier { "[X]" } else { "[-]" };
    format!(
        "BEST {best}   B{}  S{}  G{}",
        symbol(1),
        symbol(2),
        symbol(3)
    )
}

/// Conservative fixed-content budget for the compact Menu composition. The
/// actual root remains centered on large screens, while this bound keeps its
/// title, controls, five gallery rows, and start prompt inside the 390px-high
/// landscape mobile viewport targeted by the HUD.
#[cfg(test)]
const MENU_CONTENT_HEIGHT: f32 = 310.0;
#[cfg(test)]
const MENU_GALLERY_WIDTH: f32 = 360.0;

#[cfg(test)]
fn menu_content_fits(viewport_width: f32, viewport_height: f32) -> bool {
    viewport_width >= MENU_GALLERY_WIDTH + 16.0 && viewport_height >= MENU_CONTENT_HEIGHT
}

/// One single-line item in the fixed Game Over vertical budget. `margin` is
/// the separation after the item (before it for the final replay prompt).
#[derive(Clone, Copy, Debug, PartialEq)]
struct GameOverTextSpec {
    font: f32,
    margin: f32,
}

/// Responsive Game Over typography. The desktop values intentionally mirror
/// the original composition exactly; only short/narrow mobile viewports use
/// the compact budget.
#[derive(Clone, Copy, Debug, PartialEq)]
struct GameOverLayout {
    root_padding: f32,
    content_width_budget: f32,
    title: GameOverTextSpec,
    score_label: GameOverTextSpec,
    score: GameOverTextSpec,
    chicken: GameOverTextSpec,
    coins: GameOverTextSpec,
    best: GameOverTextSpec,
    condition: GameOverTextSpec,
    new_condition_best: GameOverTextSpec,
    medal_upgrade: GameOverTextSpec,
    objective: GameOverTextSpec,
    prompt: GameOverTextSpec,
}

const fn gameover_text(font: f32, margin: f32) -> GameOverTextSpec {
    GameOverTextSpec { font, margin }
}

const GAMEOVER_DESKTOP_LAYOUT: GameOverLayout = GameOverLayout {
    root_padding: 0.0,
    content_width_budget: 700.0,
    title: gameover_text(56.0, 14.0),
    score_label: gameover_text(16.0, 2.0),
    score: gameover_text(38.0, 10.0),
    chicken: gameover_text(21.0, 2.0),
    coins: gameover_text(21.0, 14.0),
    best: gameover_text(24.0, 10.0),
    condition: gameover_text(22.0, 6.0),
    new_condition_best: gameover_text(20.0, 4.0),
    medal_upgrade: gameover_text(20.0, 8.0),
    objective: gameover_text(20.0, 8.0),
    prompt: gameover_text(19.0, 8.0),
};

const GAMEOVER_COMPACT_LAYOUT: GameOverLayout = GameOverLayout {
    root_padding: 6.0,
    // Covers the replay prompt and the longest completed objective summary at
    // these font sizes without relying on wrapping for the height budget.
    content_width_budget: 540.0,
    title: gameover_text(40.0, 4.0),
    score_label: gameover_text(12.0, 1.0),
    score: gameover_text(28.0, 3.0),
    chicken: gameover_text(16.0, 1.0),
    coins: gameover_text(16.0, 5.0),
    best: gameover_text(18.0, 3.0),
    condition: gameover_text(17.0, 2.0),
    new_condition_best: gameover_text(15.0, 2.0),
    medal_upgrade: gameover_text(15.0, 4.0),
    objective: gameover_text(15.0, 4.0),
    prompt: gameover_text(14.0, 4.0),
};

/// Default font metrics are below this deliberately conservative multiplier.
/// Keeping it in the pure budget makes the maximal optional state testable
/// without coupling tests to Bevy's renderer or an installed font.
const GAMEOVER_LINE_HEIGHT_BUDGET: f32 = 1.4;
const GAMEOVER_DESKTOP_SAFETY_MARGIN: f32 = 6.0;

fn gameover_layout(viewport_width: f32, viewport_height: f32) -> GameOverLayout {
    if viewport_height
        <= maximal_gameover_content_height(GAMEOVER_DESKTOP_LAYOUT) + GAMEOVER_DESKTOP_SAFETY_MARGIN
        || viewport_width <= GAMEOVER_DESKTOP_LAYOUT.content_width_budget
    {
        GAMEOVER_COMPACT_LAYOUT
    } else {
        GAMEOVER_DESKTOP_LAYOUT
    }
}

fn gameover_title(reason: GameOverReason) -> &'static str {
    match reason {
        GameOverReason::Wrecked => "Wrecked!",
        GameOverReason::TimeUp => "Time's up!",
    }
}

/// Pure fixed-budget height for the maximal terminal state: both record
/// notices, the medal upgrade, and the objective summary are all visible.
fn maximal_gameover_content_height(layout: GameOverLayout) -> f32 {
    let rows = [
        layout.title,
        layout.score_label,
        layout.score,
        layout.chicken,
        layout.coins,
        layout.best,
        layout.condition,
        layout.new_condition_best,
        layout.medal_upgrade,
        layout.objective,
        layout.prompt,
    ];
    layout.root_padding * 2.0
        + rows
            .into_iter()
            .map(|row| row.font * GAMEOVER_LINE_HEIGHT_BUDGET + row.margin)
            .sum::<f32>()
}

/// Pure viewport check used to guard the worst-case mobile composition. The
/// reason participates in the width check so both terminal titles are covered.
#[cfg(test)]
fn maximal_gameover_state_fits(
    viewport_width: f32,
    viewport_height: f32,
    reason: GameOverReason,
) -> bool {
    let layout = gameover_layout(viewport_width, viewport_height);
    let title_width_budget =
        gameover_title(reason).chars().count() as f32 * layout.title.font * 0.75;
    let required_width =
        layout.content_width_budget.max(title_width_budget) + layout.root_padding * 2.0;
    required_width <= viewport_width && maximal_gameover_content_height(layout) <= viewport_height
}

// --- UI root markers ---
#[derive(Component)]
struct MenuRoot;
#[derive(Component)]
struct HudRoot;
#[derive(Component)]
struct CockpitRoot;
#[derive(Component)]
struct CompactCockpitApplied;
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
#[derive(Component)]
struct MenuMedalRow(ModifierKind);

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
                    update_hint.after(TouchStateSet),
                    update_cockpit_layout.after(TouchStateSet),
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
                padding: UiRect::all(px(8.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.62)),
            MenuRoot,
        ))
        .with_children(|p| {
            p.spawn((
                Text::new("ROADY CAR"),
                TextFont {
                    font_size: FontSize::Px(48.0),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
                Node {
                    margin: UiRect::bottom(px(2.0)),
                    ..default()
                },
            ));
            p.spawn((
                Text::new("Hit wandering chickens for score!\nCoins give bonus time."),
                TextFont {
                    font_size: FontSize::Px(15.0),
                    ..default()
                },
                TextColor(palette::HUD_TEXT.into()),
                Node {
                    margin: UiRect::bottom(px(7.0)),
                    ..default()
                },
            ));

            // Compact controls stay legible without pushing the five-row
            // gallery or start affordance below a short mobile viewport.
            p.spawn((
                Text::new("WASD / Arrows — Drive   •   Space — Brake   •   Esc — Pause"),
                TextFont {
                    font_size: FontSize::Px(13.0),
                    ..default()
                },
                TextColor(Color::srgba(0.84, 0.84, 0.90, 1.0).into()),
                Node {
                    margin: UiRect::bottom(px(6.0)),
                    ..default()
                },
            ));

            // Per-condition medal gallery. Rows use explicit B/S/G labels and
            // earned/locked symbols, so state never depends on medal color.
            p.spawn((
                Node {
                    width: px(360.0),
                    max_width: Val::Percent(96.0),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::axes(px(10.0), px(6.0)),
                    margin: UiRect::bottom(px(7.0)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.015, 0.02, 0.035, 0.78)),
            ))
            .with_children(|gallery| {
                gallery.spawn((
                    Text::new(format!("MEDAL GALLERY  {earned_medals} / 15")),
                    TextFont {
                        font_size: FontSize::Px(16.0),
                        ..default()
                    },
                    TextColor(palette::HUD_ACCENT.into()),
                    Node {
                        align_self: AlignSelf::Center,
                        margin: UiRect::bottom(px(1.0)),
                        ..default()
                    },
                    MenuMedalsText,
                ));
                gallery.spawn((
                    Text::new("B/S/G = BRONZE/SILVER/GOLD   [X] EARNED   [-] LOCKED"),
                    TextFont {
                        font_size: FontSize::Px(9.0),
                        ..default()
                    },
                    TextColor(Color::srgba(0.72, 0.74, 0.80, 1.0).into()),
                    Node {
                        align_self: AlignSelf::Center,
                        margin: UiRect::bottom(px(2.0)),
                        ..default()
                    },
                ));

                for kind in ALL_CONDITIONS {
                    let best = condition_bests.by_kind[kind.index()];
                    gallery
                        .spawn((Node {
                            width: Val::Percent(100.0),
                            justify_content: JustifyContent::SpaceBetween,
                            align_items: AlignItems::Center,
                            ..default()
                        },))
                        .with_children(|row| {
                            row.spawn((
                                Text::new(kind.display_name()),
                                TextFont {
                                    font_size: FontSize::Px(13.0),
                                    ..default()
                                },
                                TextColor(palette::HUD_TEXT.into()),
                            ));
                            row.spawn((
                                Text::new(medal_gallery_state(kind, best)),
                                TextFont {
                                    font_size: FontSize::Px(12.0),
                                    ..default()
                                },
                                TextColor(palette::HUD_ACCENT.into()),
                                MenuMedalsText,
                                MenuMedalRow(kind),
                            ));
                        });
                }
            });

            p.spawn((
                Text::new("Press ENTER / SPACE to drive"),
                TextFont {
                    font_size: FontSize::Px(22.0),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
            ));
        });
}

fn spawn_hud(
    mut commands: Commands,
    active_modifier: Res<ActiveModifier>,
    touch: Res<TouchControlsActive>,
) {
    // --- Top-left cockpit cluster on a semi-transparent panel ---
    commands
        .spawn((
            cockpit_root_node(touch.0),
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.35)),
            HudRoot,
            CockpitRoot,
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
                padding: UiRect::axes(px(9.0), px(7.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.015, 0.02, 0.035, 0.72)),
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

fn cockpit_root_node(touch_active: bool) -> Node {
    if touch_active {
        Node {
            position_type: PositionType::Absolute,
            top: px(TOUCH_COCKPIT_TOP),
            left: px(TOUCH_COCKPIT_LEFT),
            width: px(TOUCH_COCKPIT_WIDTH),
            height: px(TOUCH_COCKPIT_HEIGHT),
            flex_direction: FlexDirection::Column,
            padding: UiRect::all(px(6.0)),
            ..default()
        }
    } else {
        Node {
            position_type: PositionType::Absolute,
            top: px(12.0),
            left: px(14.0),
            flex_direction: FlexDirection::Column,
            padding: UiRect::all(px(10.0)),
            ..default()
        }
    }
}

fn scale_cockpit_descendant(
    entity: Entity,
    children: &Query<&Children>,
    fonts: &mut Query<&mut TextFont>,
    nodes: &mut Query<&mut Node>,
) {
    if let Ok(mut font) = fonts.get_mut(entity) {
        if let FontSize::Px(size) = font.font_size {
            font.font_size = FontSize::Px((size * 0.35).max(7.0));
        }
    }
    if let Ok(mut node) = nodes.get_mut(entity) {
        let scale = |value: &mut Val| {
            if let Val::Px(pixels) = value {
                *pixels *= 0.25;
            }
        };
        scale(&mut node.margin.left);
        scale(&mut node.margin.right);
        scale(&mut node.margin.top);
        scale(&mut node.margin.bottom);
    }
    if let Ok(descendants) = children.get(entity) {
        for child in descendants.iter() {
            scale_cockpit_descendant(child, children, fonts, nodes);
        }
    }
}

/// Touch activation is sticky, so compact typography is applied at most once
/// per cockpit entity. Running after touch-state detection makes the first
/// touch rearrange the already-spawned HUD in the same update.
fn update_cockpit_layout(
    mut commands: Commands,
    touch: Res<TouchControlsActive>,
    roots: Query<(Entity, Option<&CompactCockpitApplied>), With<CockpitRoot>>,
    children: Query<&Children>,
    mut fonts: Query<&mut TextFont>,
    mut nodes: Query<&mut Node>,
) {
    if !touch.0 {
        return;
    }
    for (entity, applied) in &roots {
        if let Ok(mut node) = nodes.get_mut(entity) {
            *node = cockpit_root_node(true);
        }
        if applied.is_none() {
            if let Ok(descendants) = children.get(entity) {
                for child in descendants.iter() {
                    scale_cockpit_descendant(child, &children, &mut fonts, &mut nodes);
                }
            }
            commands.entity(entity).insert(CompactCockpitApplied);
        }
    }
}

fn spawn_hint(mut commands: Commands, touch_active: Res<TouchControlsActive>) {
    // Touch-started rounds have persistent on-screen controls; the keyboard
    // hint would overlap them, especially on short landscape viewports.
    if touch_active.0 {
        return;
    }
    // Top-center transient hint, auto-dismissed after ~3.5s by `update_hint`.
    // Sits above the countdown content and between the top-left cockpit panel
    // and top-right timer at both 844x390 and 1440x900, clear of the bottom
    // health/power-up/touch zones. The 16px size keeps the pill inside the
    // narrow top-center band on short landscape viewports.
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: px(12.0),
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
                    font_size: FontSize::Px(16.0),
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
    primary_window: Query<&Window, With<PrimaryWindow>>,
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
    let (viewport_width, viewport_height) = primary_window
        .single()
        .map(|window| (window.width(), window.height()))
        .unwrap_or((1440.0, 900.0));
    let layout = gameover_layout(viewport_width, viewport_height);
    let title = gameover_title(*reason);
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
                padding: UiRect::all(px(layout.root_padding)),
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
                    font_size: FontSize::Px(layout.title.font),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
                Node {
                    margin: UiRect::bottom(px(layout.title.margin)),
                    ..default()
                },
            ));
            // SCORE (label + big accent value)
            p.spawn((
                Text::new("SCORE"),
                TextFont {
                    font_size: FontSize::Px(layout.score_label.font),
                    ..default()
                },
                TextColor(Color::srgba(0.7, 0.7, 0.75, 1.0).into()),
                Node {
                    margin: UiRect::bottom(px(layout.score_label.margin)),
                    ..default()
                },
            ));
            p.spawn((
                Text::new(format!("{}", total)),
                TextFont {
                    font_size: FontSize::Px(layout.score.font),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
                Node {
                    margin: UiRect::bottom(px(layout.score.margin)),
                    ..default()
                },
            ));
            // Chickens
            p.spawn((
                Text::new(format!("Chickens: {}", score.chickens)),
                TextFont {
                    font_size: FontSize::Px(layout.chicken.font),
                    ..default()
                },
                TextColor(palette::HUD_TEXT.into()),
                Node {
                    margin: UiRect::bottom(px(layout.chicken.margin)),
                    ..default()
                },
            ));
            // Coins
            p.spawn((
                Text::new(format!("Coins: {}", score.coins)),
                TextFont {
                    font_size: FontSize::Px(layout.coins.font),
                    ..default()
                },
                TextColor(palette::HUD_TEXT.into()),
                Node {
                    margin: UiRect::bottom(px(layout.coins.margin)),
                    ..default()
                },
            ));
            // Record status uses the pre-round snapshot plus terminal score,
            // independent of persistence-system ordering on this transition.
            p.spawn((
                Text::new(best_summary),
                TextFont {
                    font_size: FontSize::Px(layout.best.font),
                    ..default()
                },
                TextColor(if new_best {
                    palette::HUD_ACCENT.into()
                } else {
                    palette::HUD_TEXT.into()
                }),
                Node {
                    margin: UiRect::bottom(px(layout.best.margin)),
                    ..default()
                },
            ));
            p.spawn((
                Text::new(condition_summary),
                TextFont {
                    font_size: FontSize::Px(layout.condition.font),
                    ..default()
                },
                TextColor(active_modifier.color()),
                Node {
                    margin: UiRect::bottom(px(layout.condition.margin)),
                    ..default()
                },
            ));
            if condition_result.new_best {
                p.spawn((
                    Text::new("NEW CONDITION BEST"),
                    TextFont {
                        font_size: FontSize::Px(layout.new_condition_best.font),
                        ..default()
                    },
                    TextColor(palette::HUD_ACCENT.into()),
                    Node {
                        margin: UiRect::bottom(px(layout.new_condition_best.margin)),
                        ..default()
                    },
                ));
            }
            if condition_result.medal_upgrade {
                p.spawn((
                    Text::new(format!("MEDAL UPGRADE: {}", condition_result.medal.label())),
                    TextFont {
                        font_size: FontSize::Px(layout.medal_upgrade.font),
                        ..default()
                    },
                    TextColor(palette::HUD_ACCENT.into()),
                    Node {
                        margin: UiRect::bottom(px(layout.medal_upgrade.margin)),
                        ..default()
                    },
                ));
            }
            p.spawn((
                Text::new(format!("OBJECTIVE: {}", objective.summary())),
                TextFont {
                    font_size: FontSize::Px(layout.objective.font),
                    ..default()
                },
                TextColor(if objective.completed {
                    palette::HUD_ACCENT.into()
                } else {
                    palette::HUD_TEXT.into()
                }),
                Node {
                    margin: UiRect::bottom(px(layout.objective.margin)),
                    ..default()
                },
            ));
            // Restart / menu prompt
            p.spawn((
                Text::new("R / ENTER / SPACE to play again  •  Q / ESC for menu"),
                TextFont {
                    font_size: FontSize::Px(layout.prompt.font),
                    ..default()
                },
                TextColor(Color::srgba(0.8, 0.8, 0.85, 1.0).into()),
                Node {
                    margin: UiRect::top(px(layout.prompt.margin)),
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
    mut query: Query<(Option<&MenuMedalRow>, &mut Text), With<MenuMedalsText>>,
) {
    if !condition_bests.is_changed() {
        return;
    }
    let earned = total_medal_points(&condition_bests);
    for (row, mut text) in &mut query {
        **text = if let Some(row) = row {
            medal_gallery_state(row.0, condition_bests.by_kind[row.0.index()])
        } else {
            format!("MEDAL GALLERY  {earned} / 15")
        };
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
    settings: Res<Settings>,
    mut query: Query<(&mut TextSpan, &mut TextColor), With<TimerText>>,
) {
    let t = timeleft.0.max(0.0);
    let mins = (t / 60.0).floor() as u32;
    let secs = (t % 60.0).floor() as u32;
    let style = timer_style(
        t,
        time.elapsed_secs(),
        timer_motion_flags(settings.reduced_motion),
    );
    let color = match style.urgency {
        TimerUrgency::Normal => palette::HUD_ACCENT.into(),
        TimerUrgency::Urgent | TimerUrgency::Critical => Color::srgba(1.0, 0.08, 0.08, style.alpha),
    };
    for (mut span, mut text_color) in &mut query {
        **span = format!("{}:{:02}", mins, secs);
        text_color.0 = color;
    }
}

fn update_hint(
    mut commands: Commands,
    time: Res<Time>,
    touch: Res<TouchControlsActive>,
    mut q: Query<(Entity, &mut Hint)>,
) {
    for (e, mut hint) in &mut q {
        if touch.0 {
            commands.entity(e).despawn();
            continue;
        }
        hint.t -= time.delta_secs();
        if hint.t <= 0.0 {
            commands.entity(e).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GAMEOVER_COMPACT_LAYOUT, GAMEOVER_DESKTOP_LAYOUT, MENU_CONTENT_HEIGHT, TimerUrgency,
        condition_summary, gameover_layout, is_medal_upgrade, is_new_best,
        maximal_gameover_content_height, maximal_gameover_state_fits, medal_gallery_state,
        medal_points, menu_content_fits, terminal_condition_result, timer_motion_flags,
        timer_style,
    };
    use crate::game::resources::GameOverReason;
    use crate::modifiers::ModifierKind;
    use crate::persist::Medal;

    #[test]
    fn timer_urgency_has_inclusive_thresholds_and_faster_final_pulse() {
        let flags = timer_motion_flags(false);
        let normal = timer_style(10.01, 0.0, flags);
        let urgent = timer_style(10.0, 0.0625, flags);
        let critical = timer_style(5.0, 0.0625, flags);

        assert_eq!(normal.urgency, TimerUrgency::Normal);
        assert_eq!(normal.alpha, 1.0);
        assert_eq!(urgent.urgency, TimerUrgency::Urgent);
        assert_eq!(critical.urgency, TimerUrgency::Critical);
        // At the same early instant the 4 Hz critical wave is further
        // through its cycle than the 2 Hz urgent wave.
        assert!(critical.alpha > urgent.alpha);
    }

    #[test]
    fn timer_reduced_motion_keeps_alpha_static_while_urgency_remains() {
        let flags = timer_motion_flags(true);
        assert!(!flags.alpha_pulse);

        // Urgency and color escalation are preserved...
        let normal = timer_style(10.01, 0.0, flags);
        let urgent = timer_style(10.0, 0.0625, flags);
        let critical = timer_style(5.0, 0.0625, flags);
        assert_eq!(normal.urgency, TimerUrgency::Normal);
        assert_eq!(urgent.urgency, TimerUrgency::Urgent);
        assert_eq!(critical.urgency, TimerUrgency::Critical);

        // ...but the alpha no longer pulses: it holds steady at full opacity
        // for every non-normal urgency, regardless of elapsed time.
        assert_eq!(normal.alpha, 1.0);
        assert_eq!(urgent.alpha, 1.0);
        assert_eq!(critical.alpha, 1.0);
        assert_eq!(timer_style(10.0, 1.3, flags).alpha, urgent.alpha);
        assert_eq!(timer_style(5.0, 1.3, flags).alpha, critical.alpha);

        // Under normal motion the same elapsed time pulses away from the
        // static value, confirming the gating actually changes alpha.
        let moving = timer_motion_flags(false);
        assert_ne!(timer_style(10.0, 1.3, moving).alpha, urgent.alpha);
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
    fn medal_gallery_has_five_condition_rows_and_non_color_tier_states() {
        let scores = [19, 30, 100, 15, 80];
        let rows =
            super::ALL_CONDITIONS.map(|kind| medal_gallery_state(kind, scores[kind.index()]));

        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0], "BEST 19   B[-]  S[-]  G[-]");
        assert_eq!(rows[1], "BEST 30   B[X]  S[X]  G[-]");
        assert_eq!(rows[2], "BEST 100   B[X]  S[X]  G[X]");
        assert_eq!(rows[3], "BEST 15   B[X]  S[-]  G[-]");
        assert_eq!(rows[4], "BEST 80   B[X]  S[X]  G[X]");
    }

    #[test]
    fn compact_menu_budget_fits_short_mobile_and_desktop_viewports() {
        assert!(menu_content_fits(844.0, 390.0));
        assert!(menu_content_fits(1440.0, 900.0));
        assert!(!menu_content_fits(844.0, MENU_CONTENT_HEIGHT - 1.0));
        assert!(!menu_content_fits(360.0, 390.0));
    }

    #[test]
    fn maximal_gameover_states_fit_target_mobile_viewport() {
        let layout = gameover_layout(844.0, 390.0);
        assert_eq!(layout, GAMEOVER_COMPACT_LAYOUT);
        assert!(layout.prompt.font >= 14.0);
        assert!(layout.objective.font >= 15.0);
        assert!(layout.new_condition_best.font >= 15.0);
        assert!(maximal_gameover_content_height(layout) <= 390.0);

        // Exercise both terminal titles with every optional row budgeted:
        // NEW BEST + NEW CONDITION BEST + MEDAL UPGRADE + objective summary.
        assert!(maximal_gameover_state_fits(
            844.0,
            390.0,
            GameOverReason::TimeUp
        ));
        assert!(maximal_gameover_state_fits(
            844.0,
            390.0,
            GameOverReason::Wrecked
        ));
    }

    #[test]
    fn maximal_gameover_budget_rejects_clipping_and_preserves_desktop_style() {
        let compact_height = maximal_gameover_content_height(GAMEOVER_COMPACT_LAYOUT);
        assert!(!maximal_gameover_state_fits(
            844.0,
            compact_height - 1.0,
            GameOverReason::Wrecked
        ));
        assert!(!maximal_gameover_state_fits(
            GAMEOVER_COMPACT_LAYOUT.content_width_budget - 1.0,
            390.0,
            GameOverReason::TimeUp
        ));

        let desktop = gameover_layout(1440.0, 900.0);
        assert_eq!(desktop, GAMEOVER_DESKTOP_LAYOUT);
        assert_eq!(desktop.title.font, 56.0);
        assert_eq!(desktop.prompt.font, 19.0);
        assert!(maximal_gameover_state_fits(
            1440.0,
            900.0,
            GameOverReason::TimeUp
        ));
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
