use bevy::{prelude::*, text::FontSize, window::PrimaryWindow};

use crate::car::Car;
use crate::game::resources::{GameOverReason, Score, TimeLeft};
use crate::game::state::GameState;
use crate::game::{SpawnSet, TouchStateSet};
use crate::game_modes::{ActiveRunRules, Competition, Conduct};
use crate::modifiers::{ActiveModifier, ModifierKind};
use crate::objectives::ActiveObjective;
use crate::palette;
use crate::persist::{
    BestAtRoundStart, ConditionBests, ConditionBestsAtRoundStart, Medal, ProductBestAtRoundStart,
    medal_for, product_medal,
};
use crate::right_of_way::RightOfWayRun;
use crate::settings::Settings;
use crate::touch::{
    TOUCH_COCKPIT_HEIGHT, TOUCH_COCKPIT_LEFT, TOUCH_COCKPIT_TOP, TOUCH_COCKPIT_WIDTH,
    TOUCH_TIMER_HEIGHT, TOUCH_TIMER_RIGHT, TOUCH_TIMER_TOP, TOUCH_TIMER_WIDTH, TouchControlsActive,
    is_touch_portrait, touch_cockpit_bounds, touch_timer_bounds,
};

/// One shared breakpoint for every cross-plugin UI decision. Landscape phones
/// and the 960×480 boundary are mobile; 1440×900 retains the desktop layout.
pub(crate) fn is_mobile_viewport(width: f32, height: f32) -> bool {
    height <= 480.0 || width <= 960.0
}

/// The normal terminal composition owns the area above this leaderboard strip
/// on mobile. Keeping the value here prevents the two plugins from drifting.
pub(crate) const GAMEOVER_STATUS_STRIP_HEIGHT: f32 = 52.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct UiBounds {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

impl UiBounds {
    #[cfg(test)]
    pub(crate) fn is_disjoint(self, other: Self) -> bool {
        self.left + self.width <= other.left
            || other.left + other.width <= self.left
            || self.top + self.height <= other.top
            || other.top + other.height <= self.top
    }
}

pub(crate) fn gameover_core_bounds(width: f32, height: f32) -> UiBounds {
    UiBounds {
        left: 0.0,
        top: 0.0,
        width,
        height: if is_mobile_viewport(width, height) {
            (height - GAMEOVER_STATUS_STRIP_HEIGHT).max(0.0)
        } else {
            height
        },
    }
}

/// Conservative bounds for the two centered Pause text rows. The Pause root
/// remains full-screen, but only this central band contains visible content.
#[cfg(test)]
pub(crate) fn pause_content_bounds(width: f32, height: f32) -> UiBounds {
    const CONTENT_HEIGHT: f32 = 124.0;
    UiBounds {
        left: 0.0,
        top: ((height - CONTENT_HEIGHT) * 0.5).max(0.0),
        width,
        height: CONTENT_HEIGHT.min(height.max(0.0)),
    }
}

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
        medal => format!("{prefix} | {}", medal.label()),
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
    root_padding: 4.0,
    // Covers the replay prompt and the longest completed objective summary at
    // these font sizes without relying on wrapping for the height budget.
    content_width_budget: 540.0,
    title: gameover_text(36.0, 2.0),
    score_label: gameover_text(11.0, 0.0),
    score: gameover_text(25.0, 1.0),
    chicken: gameover_text(14.0, 0.0),
    coins: gameover_text(14.0, 2.0),
    best: gameover_text(16.0, 1.0),
    condition: gameover_text(15.0, 1.0),
    new_condition_best: gameover_text(14.0, 1.0),
    medal_upgrade: gameover_text(14.0, 2.0),
    objective: gameover_text(14.0, 2.0),
    prompt: gameover_text(13.0, 2.0),
};

/// Default font metrics are below this deliberately conservative multiplier.
/// Keeping it in the pure budget makes the maximal optional state testable
/// without coupling tests to Bevy's renderer or an installed font.
#[cfg(test)]
const GAMEOVER_LINE_HEIGHT_BUDGET: f32 = 1.4;
fn gameover_layout(viewport_width: f32, viewport_height: f32) -> GameOverLayout {
    if is_mobile_viewport(viewport_width, viewport_height) {
        GAMEOVER_COMPACT_LAYOUT
    } else {
        GAMEOVER_DESKTOP_LAYOUT
    }
}

fn gameover_title(reason: GameOverReason) -> &'static str {
    match reason {
        GameOverReason::Wrecked => "Wrecked!",
        GameOverReason::TimeUp => "Time's up!",
        GameOverReason::Drowned => "DROWNED",
    }
}

/// Pure fixed-budget height for the maximal terminal state: both record
/// notices, the medal upgrade, and the objective summary are all visible.
#[cfg(test)]
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
    let core = gameover_core_bounds(viewport_width, viewport_height);
    required_width <= core.width && maximal_gameover_content_height(layout) <= core.height
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
struct CockpitSecondaryInfo;
#[derive(Component)]
struct CockpitCurrentScore;
#[derive(Component)]
struct TimerRoot;
#[derive(Component)]
struct TimerLabel;
#[derive(Component)]
struct HintRoot;
#[derive(Component)]
struct PauseRoot;
#[derive(Component)]
pub(crate) struct GameOverCoreRoot;

#[derive(Component, Clone, Copy)]
enum GameOverTextRole {
    Title,
    ScoreLabel,
    Score,
    Chicken,
    Coins,
    Best,
    Condition,
    NewConditionBest,
    MedalUpgrade,
    Objective,
    Prompt,
}

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
struct ConductText;
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
        app // Modifier selection happens before SpawnSet on fresh rounds, so
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
            .add_systems(
                OnExit(GameState::GameOver),
                despawn_marker::<GameOverCoreRoot>,
            )
            .add_systems(
                Update,
                update_gameover_layout.run_if(in_state(GameState::GameOver)),
            )
            .add_systems(
                Update,
                (
                    update_speed_text,
                    update_gear_text,
                    update_score_text,
                    update_chickens_text,
                    update_coins_text,
                    update_conduct_text,
                    update_timer_text,
                    update_hint.after(TouchStateSet),
                    update_cockpit_layout.after(TouchStateSet),
                    update_timer_layout.after(TouchStateSet),
                )
                    .run_if(in_state(GameState::Playing)),
            );
    }
}

#[allow(dead_code)]
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
                Text::new(
                    "CORE SCORE - Chickens give at least +1. Coins give +1 and time.\nROUND MISSION - Complete the shown target once for a +10 bonus.",
                ),
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
                Text::new("WASD / Arrows - Drive   |   Space - Brake   |   Esc - Pause"),
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
    windows: Query<&Window, With<PrimaryWindow>>,
    rules: Option<Res<ActiveRunRules>>,
    right_of_way: Option<Res<RightOfWayRun>>,
) {
    let viewport = windows
        .single()
        .ok()
        .map(|window| Vec2::new(window.width(), window.height()));
    // --- Top-left cockpit cluster on a semi-transparent panel ---
    let mut cockpit = commands.spawn((
        cockpit_root_node(touch.0, viewport),
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.35)),
        HudRoot,
        CockpitRoot,
    ));
    if touch.0 {
        cockpit.insert(CompactCockpitApplied);
    }
    cockpit.with_children(|p| {
        // SPEED label
        p.spawn((
            Text::new("SPEED"),
            TextFont {
                font_size: FontSize::Px(cockpit_font_size(13.0, touch.0)),
                ..default()
            },
            TextColor(Color::srgba(0.75, 0.75, 0.8, 1.0).into()),
            Node {
                margin: UiRect::bottom(px(cockpit_margin(2.0, touch.0))),
                ..default()
            },
        ));
        // Big digital speed (40px accent) + smaller " u/s" unit, laid out inline.
        p.spawn((Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::End,
            margin: UiRect::bottom(px(cockpit_margin(6.0, touch.0))),
            ..default()
        },))
            .with_children(|s| {
                s.spawn((
                    Text::new(""),
                    TextFont {
                        font_size: FontSize::Px(cockpit_font_size(40.0, touch.0)),
                        ..default()
                    },
                    TextColor(palette::HUD_ACCENT.into()),
                ))
                .with_child((
                    TextSpan::default(),
                    TextFont {
                        font_size: FontSize::Px(cockpit_font_size(40.0, touch.0)),
                        ..default()
                    },
                    TextColor(palette::HUD_ACCENT.into()),
                    SpeedText,
                ));
                s.spawn((
                    Text::new(" u/s"),
                    TextFont {
                        font_size: FontSize::Px(cockpit_font_size(20.0, touch.0)),
                        ..default()
                    },
                    TextColor(palette::HUD_TEXT.into()),
                    Node {
                        margin: UiRect::left(px(cockpit_margin(4.0, touch.0))),
                        ..default()
                    },
                ));
            });
        // Gear / direction line
        p.spawn((
            Text::new("GEAR "),
            TextFont {
                font_size: FontSize::Px(cockpit_font_size(18.0, touch.0)),
                ..default()
            },
            TextColor(Color::srgba(0.75, 0.75, 0.8, 1.0).into()),
            Node {
                margin: UiRect::bottom(px(cockpit_margin(6.0, touch.0))),
                ..default()
            },
        ))
        .with_child((
            TextSpan::default(),
            TextFont {
                font_size: FontSize::Px(cockpit_font_size(18.0, touch.0)),
                ..default()
            },
            TextColor(palette::HUD_ACCENT.into()),
            GearText,
        ));
        // Current score remains visible in the compact phone cockpit. Only
        // the detailed chickens/coins/condition rows below are secondary.
        p.spawn((
            Text::new("SCORE"),
            TextFont {
                font_size: FontSize::Px(cockpit_font_size(13.0, touch.0)),
                ..default()
            },
            TextColor(Color::srgba(0.75, 0.75, 0.8, 1.0).into()),
            Node {
                margin: UiRect {
                    top: px(cockpit_margin(4.0, touch.0)),
                    bottom: px(cockpit_margin(1.0, touch.0)),
                    ..default()
                },
                ..default()
            },
            CockpitCurrentScore,
        ));
        // Big score number (chickens + coins)
        p.spawn((
            Text::new(""),
            TextFont {
                font_size: FontSize::Px(cockpit_font_size(36.0, touch.0)),
                ..default()
            },
            TextColor(palette::HUD_ACCENT.into()),
            Node {
                margin: UiRect::bottom(px(cockpit_margin(3.0, touch.0))),
                ..default()
            },
            CockpitCurrentScore,
        ))
        .with_child((
            TextSpan::default(),
            TextFont {
                font_size: FontSize::Px(cockpit_font_size(36.0, touch.0)),
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
                display: if touch.0 {
                    Display::None
                } else {
                    Display::Flex
                },
                ..default()
            },
            CockpitSecondaryInfo,
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
                display: if touch.0 {
                    Display::None
                } else {
                    Display::Flex
                },
                ..default()
            },
            CockpitSecondaryInfo,
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
                display: if touch.0 {
                    Display::None
                } else {
                    Display::Flex
                },
                ..default()
            },
            CockpitSecondaryInfo,
        ));
        p.spawn((
            Text::new(active_modifier.display_name()),
            TextFont {
                font_size: FontSize::Px(18.0),
                ..default()
            },
            TextColor(active_modifier.color()),
            Node {
                display: if touch.0 {
                    Display::None
                } else {
                    Display::Flex
                },
                ..default()
            },
            CockpitSecondaryInfo,
        ));
        // Reuse the audited cockpit panel. The compact line stays visible on
        // touch and replaces no drive-control or status-panel real estate.
        let conduct_copy = if rules
            .as_ref()
            .is_some_and(|rules| rules.conduct == Conduct::RightOfWay)
        {
            if let Some(run) = right_of_way.as_ref() {
                right_of_way_hud_copy(run, touch.0)
            } else {
                if touch.0 {
                    "P0/3 100% G0.0\nD0 C0 H0 X0".into()
                } else {
                    "CARRY 0/3 | PREMIUM 100.00% | GUILT 0.0s\nDELIVERED 0 | COURTESY 0 | HITS 0 | CHAIN 0".into()
                }
            }
        } else {
            String::new()
        };
        p.spawn((
            Text::new(conduct_copy),
            TextFont {
                font_size: FontSize::Px(if touch.0 { 10.0 } else { 14.0 }),
                ..default()
            },
            TextColor(Color::srgb(0.30, 1.0, 0.55)),
            Node {
                margin: UiRect::top(px(if touch.0 { 1.0 } else { 5.0 })),
                display: if rules
                    .as_ref()
                    .is_some_and(|rules| rules.conduct == Conduct::RightOfWay)
                {
                    Display::Flex
                } else {
                    Display::None
                },
                ..default()
            },
            ConductText,
        ));
    });

    // --- Timer (top-right) ---
    commands
        .spawn((
            timer_root_node(touch.0, viewport),
            BackgroundColor(Color::srgba(0.015, 0.02, 0.035, 0.72)),
            HudRoot,
            TimerRoot,
            TimerLabel,
            Text::new(if touch.0 { "TIME " } else { "Time Left: " }),
            timer_font(touch.0),
            TextColor(palette::HUD_TEXT.into()),
        ))
        .with_child((
            TextSpan::default(),
            timer_font(touch.0),
            TextColor(palette::HUD_ACCENT.into()),
            TimerText,
        ));
}

fn timer_root_node(touch_active: bool, viewport: Option<Vec2>) -> Node {
    if touch_active {
        let portrait = viewport.filter(|viewport| is_touch_portrait(*viewport));
        let bounds = portrait.map(touch_timer_bounds);
        Node {
            position_type: PositionType::Absolute,
            top: px(bounds.map_or(TOUCH_TIMER_TOP, |bounds| bounds.top)),
            right: px(bounds.map_or(TOUCH_TIMER_RIGHT, |bounds| {
                viewport.unwrap().x - bounds.right
            })),
            width: px(bounds.map_or(TOUCH_TIMER_WIDTH, |bounds| bounds.width())),
            height: px(bounds.map_or(TOUCH_TIMER_HEIGHT, |bounds| bounds.height())),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            padding: UiRect::axes(px(6.0), px(4.0)),
            ..default()
        }
    } else {
        Node {
            position_type: PositionType::Absolute,
            top: px(12.0),
            right: px(16.0),
            padding: UiRect::axes(px(9.0), px(7.0)),
            ..default()
        }
    }
}

fn timer_font(touch_active: bool) -> TextFont {
    TextFont {
        font_size: FontSize::Px(if touch_active { 16.0 } else { 24.0 }),
        ..default()
    }
}

fn cockpit_font_size(size: f32, touch_active: bool) -> f32 {
    if touch_active {
        if size >= 30.0 { 17.0 } else { 12.0 }
    } else {
        size
    }
}

fn cockpit_margin(size: f32, touch_active: bool) -> f32 {
    if touch_active { size * 0.3 } else { size }
}

fn cockpit_root_node(touch_active: bool, viewport: Option<Vec2>) -> Node {
    if touch_active {
        let bounds = viewport
            .filter(|viewport| is_touch_portrait(*viewport))
            .map(touch_cockpit_bounds);
        Node {
            position_type: PositionType::Absolute,
            top: px(bounds.map_or(TOUCH_COCKPIT_TOP, |bounds| bounds.top)),
            left: px(bounds.map_or(TOUCH_COCKPIT_LEFT, |bounds| bounds.left)),
            width: px(bounds.map_or(TOUCH_COCKPIT_WIDTH, |bounds| bounds.width())),
            height: px(bounds.map_or(TOUCH_COCKPIT_HEIGHT, |bounds| bounds.height())),
            flex_direction: FlexDirection::Column,
            padding: UiRect::all(px(5.0)),
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
    fonts: &mut Query<&mut TextFont, (Without<TimerRoot>, Without<CockpitSecondaryInfo>)>,
    nodes: &mut Query<&mut Node, (Without<TimerRoot>, Without<CockpitSecondaryInfo>)>,
) {
    if let Ok(mut font) = fonts.get_mut(entity) {
        if let FontSize::Px(size) = font.font_size {
            font.font_size = FontSize::Px(if size >= 30.0 { 17.0 } else { 12.0 });
        }
    }
    if let Ok(mut node) = nodes.get_mut(entity) {
        let scale = |value: &mut Val| {
            if let Val::Px(pixels) = value {
                *pixels *= 0.3;
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
    windows: Query<&Window, With<PrimaryWindow>>,
    roots: Query<(Entity, Option<&CompactCockpitApplied>), With<CockpitRoot>>,
    mut secondary_nodes: Query<&mut Node, With<CockpitSecondaryInfo>>,
    children: Query<&Children>,
    mut fonts: Query<&mut TextFont, (Without<TimerRoot>, Without<CockpitSecondaryInfo>)>,
    mut nodes: Query<&mut Node, (Without<TimerRoot>, Without<CockpitSecondaryInfo>)>,
) {
    if !touch.0 {
        return;
    }
    let viewport = windows
        .single()
        .ok()
        .map(|window| Vec2::new(window.width(), window.height()));
    for mut node in &mut secondary_nodes {
        node.display = Display::None;
    }
    for (entity, applied) in &roots {
        if let Ok(mut node) = nodes.get_mut(entity) {
            *node = cockpit_root_node(true, viewport);
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

fn update_timer_layout(
    touch: Res<TouchControlsActive>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut roots: Query<
        (&mut Node, &mut TextFont, &mut Text),
        (With<TimerRoot>, With<TimerLabel>, Without<TimerText>),
    >,
    mut spans: Query<&mut TextFont, (With<TimerText>, Without<TimerRoot>)>,
) {
    if !touch.0 {
        return;
    }
    let viewport = windows
        .single()
        .ok()
        .map(|window| Vec2::new(window.width(), window.height()));
    for (mut node, mut font, mut text) in &mut roots {
        *node = timer_root_node(true, viewport);
        *font = timer_font(true);
        **text = "TIME ".to_string();
    }
    for mut font in &mut spans {
        *font = timer_font(true);
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
                Text::new("WASD to drive  |  Space to brake  |  Esc to pause"),
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
                Text::new("ESC  Resume  |  R  Restart  |  Q  Menu"),
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
    product_best_at_start: Res<ProductBestAtRoundStart>,
    rules: Option<Res<ActiveRunRules>>,
    right_of_way: Option<Res<crate::right_of_way::RightOfWayRun>>,
    objective: Res<ActiveObjective>,
) {
    let total = if rules
        .as_deref()
        .is_some_and(|rules| rules.conduct == Conduct::RightOfWay)
    {
        right_of_way
            .as_deref()
            .and_then(|run| run.score.terminal_total().ok())
            .unwrap_or(0)
    } else {
        score.chickens.saturating_add(score.coins)
    };
    let new_best = is_new_best(total, best_at_start.0);
    let best_summary = if new_best {
        "NEW BEST".to_string()
    } else {
        format!("BEST: {}", best_at_start.0)
    };

    let kind = active_modifier.0;
    let (condition_result, condition_summary) = if let Some(rules) = rules.as_deref() {
        let displayed_best = product_best_at_start.best.max(total);
        let previous_medal = product_medal(
            rules.competition,
            rules.conduct,
            kind,
            product_best_at_start.best,
        );
        let medal = product_medal(rules.competition, rules.conduct, kind, displayed_best);
        (
            TerminalConditionResult {
                displayed_best,
                new_best: total > product_best_at_start.best,
                medal,
                medal_upgrade: is_medal_upgrade(previous_medal, medal),
            },
            format!(
                "{} {} BEST: {} | {}",
                if rules.competition == Competition::Ranked {
                    "Ranked"
                } else {
                    "Casual"
                },
                match rules.conduct {
                    Conduct::CluckHunt => "Cluck Hunt",
                    Conduct::RightOfWay => "Right of Way",
                },
                displayed_best,
                medal.label()
            ),
        )
    } else {
        let result =
            terminal_condition_result(kind, conditions_at_start.by_kind[kind.index()], total);
        let summary = condition_summary(kind, result.displayed_best);
        (result, summary)
    };
    let (viewport_width, viewport_height) = primary_window
        .single()
        .map(|window| (window.width(), window.height()))
        .unwrap_or((1440.0, 900.0));
    let layout = gameover_layout(viewport_width, viewport_height);
    let title = gameover_title(*reason);
    let core_bounds = gameover_core_bounds(viewport_width, viewport_height);
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: px(0.0),
                left: px(0.0),
                width: Val::Percent(100.0),
                height: if is_mobile_viewport(viewport_width, viewport_height) {
                    px(core_bounds.height)
                } else {
                    Val::Percent(100.0)
                },
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(px(layout.root_padding)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
            GameOverCoreRoot,
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
                GameOverTextRole::Title,
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
                GameOverTextRole::ScoreLabel,
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
                GameOverTextRole::Score,
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
                GameOverTextRole::Chicken,
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
                GameOverTextRole::Coins,
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
                GameOverTextRole::Best,
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
                GameOverTextRole::Condition,
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
                    GameOverTextRole::NewConditionBest,
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
                    GameOverTextRole::MedalUpgrade,
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
                GameOverTextRole::Objective,
            ));
            // Restart / menu prompt
            p.spawn((
                Text::new("R / ENTER / SPACE to play again  |  Q / ESC for menu"),
                TextFont {
                    font_size: FontSize::Px(layout.prompt.font),
                    ..default()
                },
                TextColor(Color::srgba(0.8, 0.8, 0.85, 1.0).into()),
                Node {
                    margin: UiRect::top(px(layout.prompt.margin)),
                    ..default()
                },
                GameOverTextRole::Prompt,
            ));
        });
}

fn gameover_text_spec(layout: GameOverLayout, role: GameOverTextRole) -> GameOverTextSpec {
    match role {
        GameOverTextRole::Title => layout.title,
        GameOverTextRole::ScoreLabel => layout.score_label,
        GameOverTextRole::Score => layout.score,
        GameOverTextRole::Chicken => layout.chicken,
        GameOverTextRole::Coins => layout.coins,
        GameOverTextRole::Best => layout.best,
        GameOverTextRole::Condition => layout.condition,
        GameOverTextRole::NewConditionBest => layout.new_condition_best,
        GameOverTextRole::MedalUpgrade => layout.medal_upgrade,
        GameOverTextRole::Objective => layout.objective,
        GameOverTextRole::Prompt => layout.prompt,
    }
}

/// Reflows the existing terminal entities in place. In particular, crossing
/// the breakpoint does not recreate score/record state or invalidate entity
/// references held by the leaderboard plugin.
fn update_gameover_layout(
    windows: Query<&Window, With<PrimaryWindow>>,
    mut roots: Query<&mut Node, With<GameOverCoreRoot>>,
    mut texts: Query<(&GameOverTextRole, &mut TextFont, &mut Node), Without<GameOverCoreRoot>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let width = window.width();
    let height = window.height();
    let bounds = gameover_core_bounds(width, height);
    let layout = gameover_layout(width, height);

    for mut node in &mut roots {
        node.top = px(bounds.top);
        node.left = px(bounds.left);
        node.width = Val::Percent(100.0);
        node.height = if is_mobile_viewport(width, height) {
            px(bounds.height)
        } else {
            Val::Percent(100.0)
        };
        node.padding = UiRect::all(px(layout.root_padding));
    }
    for (role, mut font, mut node) in &mut texts {
        let spec = gameover_text_spec(layout, *role);
        font.font_size = FontSize::Px(spec.font);
        node.margin = if matches!(role, GameOverTextRole::Prompt) {
            UiRect::top(px(spec.margin))
        } else {
            UiRect::bottom(px(spec.margin))
        };
    }
}

fn despawn_marker<M: Component>(mut commands: Commands, q: Query<Entity, With<M>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}

#[allow(dead_code)]
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

fn update_score_text(
    score: Res<Score>,
    rules: Option<Res<ActiveRunRules>>,
    right_of_way: Option<Res<RightOfWayRun>>,
    mut query: Query<&mut TextSpan, With<ScoreText>>,
) {
    let total = if rules
        .as_ref()
        .is_some_and(|rules| rules.conduct == Conduct::RightOfWay)
    {
        right_of_way
            .as_ref()
            .map_or(0, |run| run.score.accumulator.max(0))
            .to_string()
    } else {
        score.chickens.saturating_add(score.coins).to_string()
    };
    for mut span in &mut query {
        **span = total.clone();
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

fn right_of_way_hud_copy(run: &RightOfWayRun, compact: bool) -> String {
    let guilt_tenths = run.score.guilt_remaining_ms.div_ceil(100);
    if compact {
        format!(
            "P{}/{} {}% G{}.{:01}\nD{} C{} H{} X{}",
            run.score.carried_packages,
            roady_score_rules::v3::PACKAGE_CAPACITY,
            run.score.premium_bps / 100,
            guilt_tenths / 10,
            guilt_tenths % 10,
            run.score.packages_delivered,
            run.score.courtesy_count,
            run.score.animal_hits,
            run.score.delivery_chain,
        )
    } else {
        format!(
            "CARRY {}/{} | PREMIUM {}.{:02}% | GUILT {}.{:01}s\nDELIVERED {} | COURTESY {} | HITS {} | CHAIN {}",
            run.score.carried_packages,
            roady_score_rules::v3::PACKAGE_CAPACITY,
            run.score.premium_bps / 100,
            run.score.premium_bps % 100,
            guilt_tenths / 10,
            guilt_tenths % 10,
            run.score.packages_delivered,
            run.score.courtesy_count,
            run.score.animal_hits,
            run.score.delivery_chain,
        )
    }
}

fn update_conduct_text(
    run: Option<Res<RightOfWayRun>>,
    touch: Res<TouchControlsActive>,
    mut query: Query<&mut Text, With<ConductText>>,
) {
    let Some(run) = run else { return };
    let copy = right_of_way_hud_copy(&run, touch.0);
    for mut text in &mut query {
        **text = copy.clone();
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
        CockpitCurrentScore, CockpitRoot, GAMEOVER_COMPACT_LAYOUT, GAMEOVER_DESKTOP_LAYOUT,
        GAMEOVER_STATUS_STRIP_HEIGHT, MENU_CONTENT_HEIGHT, ScoreText, TimerRoot, TimerUrgency,
        condition_summary, gameover_core_bounds, gameover_layout, gameover_title, is_medal_upgrade,
        is_mobile_viewport, is_new_best, maximal_gameover_content_height,
        maximal_gameover_state_fits, medal_gallery_state, medal_points, menu_content_fits,
        pause_content_bounds, right_of_way_hud_copy, spawn_hud, terminal_condition_result,
        timer_motion_flags, timer_style, update_cockpit_layout, update_score_text,
        update_timer_layout,
    };
    use crate::game::resources::{GameOverReason, Score};
    use crate::modifiers::{ActiveModifier, ModifierKind};
    use crate::persist::Medal;
    use crate::touch::TouchControlsActive;
    use bevy::prelude::*;
    use bevy::window::PrimaryWindow;

    #[test]
    fn touch_cockpit_keeps_live_score_visible_and_updates_it() {
        let mut app = App::new();
        app.insert_resource(ActiveModifier::default());
        app.insert_resource(TouchControlsActive(true));
        app.insert_resource(Score {
            chickens: 7,
            coins: 5,
        });
        app.add_systems(Update, (spawn_hud, update_score_text).chain());
        app.update();

        let world = app.world_mut();
        let mut score_rows = world.query_filtered::<&Node, With<CockpitCurrentScore>>();
        let rows: Vec<_> = score_rows.iter(world).collect();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|node| node.display != Display::None));

        let mut score_spans = world.query_filtered::<&TextSpan, With<ScoreText>>();
        let spans: Vec<_> = score_spans.iter(world).collect();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].as_str(), "12");
    }

    #[test]
    fn right_of_way_hud_is_complete_and_compact_at_audited_sizes() {
        let mut run = crate::right_of_way::RightOfWayRun::default();
        run.score.accumulator = -5;
        run.score.carried_packages = 3;
        run.score.packages_delivered = 12;
        run.score.courtesy_count = 9;
        run.score.animal_hits = 2;
        run.score.delivery_chain = 4;
        run.score.premium_bps = 8_100;
        run.score.guilt_remaining_ms = 4_001;
        let desktop = right_of_way_hud_copy(&run, false);
        let touch = right_of_way_hud_copy(&run, true);
        for token in [
            "CARRY",
            "PREMIUM",
            "GUILT",
            "DELIVERED",
            "COURTESY",
            "HITS",
            "CHAIN",
        ] {
            assert!(desktop.contains(token));
        }
        assert!(desktop.lines().all(|line| line.len() <= 76));
        assert_eq!(touch.lines().count(), 2);
        assert!(touch.lines().all(|line| line.len() <= 24));
        assert!(desktop.is_ascii() && touch.is_ascii());
        // The cockpit geometry itself is shared and already audited at these
        // representative desktop, portrait-touch and short-landscape sizes.
        for viewport in [
            Vec2::new(1440.0, 900.0),
            Vec2::new(390.0, 844.0),
            Vec2::new(844.0, 390.0),
        ] {
            let node = super::cockpit_root_node(viewport.x < 1000.0, Some(viewport));
            if let Val::Px(width) = node.width {
                assert!(width <= viewport.x);
            }
            if let Val::Px(height) = node.height {
                assert!(height <= viewport.y);
            }
        }
    }

    #[test]
    fn live_touch_hud_switches_only_when_viewport_becomes_portrait() {
        let mut app = App::new();
        app.insert_resource(ActiveModifier::default());
        app.insert_resource(TouchControlsActive(true));
        app.world_mut().spawn((
            Window {
                resolution: (844, 390).into(),
                ..default()
            },
            PrimaryWindow,
        ));
        app.add_systems(Startup, spawn_hud)
            .add_systems(Update, (update_cockpit_layout, update_timer_layout));
        app.update();

        let world = app.world_mut();
        let mut cockpit_query = world.query_filtered::<&Node, With<CockpitRoot>>();
        let cockpit = cockpit_query.single(world).unwrap();
        assert_eq!(cockpit.left, px(14.0));
        assert_eq!(cockpit.width, px(150.0));
        let mut timer_query = world.query_filtered::<&Node, With<TimerRoot>>();
        let timer = timer_query.single(world).unwrap();
        assert_eq!(timer.right, px(16.0));
        assert_eq!(timer.width, px(132.0));

        let mut windows = world.query_filtered::<&mut Window, With<PrimaryWindow>>();
        windows
            .single_mut(world)
            .unwrap()
            .resolution
            .set(390.0, 844.0);
        app.update();

        let world = app.world_mut();
        let mut cockpit_query = world.query_filtered::<&Node, With<CockpitRoot>>();
        let cockpit = cockpit_query.single(world).unwrap();
        assert_eq!(cockpit.left, px(8.0));
        assert_eq!(cockpit.width, px(120.0));
        let mut timer_query = world.query_filtered::<&Node, With<TimerRoot>>();
        let timer = timer_query.single(world).unwrap();
        assert_eq!(timer.right, px(8.0));
        assert_eq!(timer.width, px(110.0));
    }

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
    fn drowned_title_is_explicit_and_never_wrecked() {
        assert_eq!(gameover_title(GameOverReason::Drowned), "DROWNED");
        assert_ne!(gameover_title(GameOverReason::Drowned), "Wrecked!");
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
            "Standard BEST: 20 | Bronze"
        );
        assert_eq!(
            condition_summary(ModifierKind::RushHour, 30),
            "Rush Hour BEST: 30 | Silver"
        );
        assert_eq!(
            condition_summary(ModifierKind::GlassCannon, 80),
            "Glass Cannon BEST: 80 | Gold"
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
    fn rendered_static_labels_are_ascii() {
        for label in [
            "ROADY CAR",
            "CORE SCORE - Chickens give at least +1. Coins give +1 and time.\nROUND MISSION - Complete the shown target once for a +10 bonus.",
            "WASD / Arrows - Drive   |   Space - Brake   |   Esc - Pause",
            "B/S/G = BRONZE/SILVER/GOLD   [X] EARNED   [-] LOCKED",
            "Press ENTER / SPACE to drive",
            "WASD to drive  |  Space to brake  |  Esc to pause",
            "ESC  Resume  |  R  Restart  |  Q  Menu",
            "R / ENTER / SPACE to play again  |  Q / ESC for menu",
        ] {
            assert!(label.is_ascii(), "{label:?}");
        }
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
    fn shared_mobile_breakpoint_and_core_reservation_cover_boundaries() {
        assert!(is_mobile_viewport(844.0, 390.0));
        assert!(is_mobile_viewport(960.0, 480.0));
        assert!(!is_mobile_viewport(1440.0, 900.0));
        assert_eq!(gameover_core_bounds(844.0, 390.0).height, 338.0);
        assert_eq!(gameover_core_bounds(960.0, 480.0).height, 428.0);
        assert_eq!(gameover_core_bounds(1440.0, 900.0).height, 900.0);
        assert_eq!(pause_content_bounds(844.0, 390.0).top, 133.0);
    }

    #[test]
    fn maximal_gameover_states_fit_target_mobile_viewport() {
        let layout = gameover_layout(844.0, 390.0);
        assert_eq!(layout, GAMEOVER_COMPACT_LAYOUT);
        assert!(layout.prompt.font >= 13.0);
        assert!(layout.objective.font >= 14.0);
        assert!(layout.new_condition_best.font >= 14.0);
        assert!(
            maximal_gameover_content_height(layout) <= gameover_core_bounds(844.0, 390.0).height
        );

        // Exercise every terminal title with every optional row budgeted:
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
            compact_height + GAMEOVER_STATUS_STRIP_HEIGHT - 1.0,
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
