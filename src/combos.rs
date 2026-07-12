//! Combo multiplier (T10): consecutive chicken/coin hits within a time
//! window multiply the score, with UI feedback.
//!
//! Owns its own UI (combo badge + depleting timer bar) so it never touches
//! `ui.rs`. Reads `ChickenHit` and `CoinCollected` messages (already
//! registered in `game/mod.rs`) via `MessageReader` — never re-registers them.
//!
//! Score interaction: `chickens.rs::hit_chickens` and `world.rs::collect_coins`
//! already add +1 to `Score.chickens` / `Score.coins` per hit. This module
//! adds the **bonus** on top: `score.chickens += multiplier - 1` (and likewise
//! for coins). Standard therefore totals `multiplier` points per hit; round
//! modifiers and events may scale only the bonus while the originating +1
//! stays intact.

use bevy::prelude::*;
use bevy::text::FontSize;

use crate::game::SpawnSet;
use crate::game::events::{ChickenHit, CoinCollected};
use crate::game::resources::{RoundActive, Score};
use crate::game::state::GameState;
use crate::modifiers::ActiveModifier;
use crate::palette;
use crate::run_events::ActiveEvent;

// ---------------------------------------------------------------------------
// Tuning constants
// ---------------------------------------------------------------------------

/// Combo window: seconds after the last hit before the combo resets. Each new
/// hit refreshes this timer.
const COMBO_WINDOW: f32 = 2.5;

/// Width of the depleting timer bar (px) at full.
const BAR_WIDTH: f32 = 120.0;
/// Height of the depleting timer bar (px).
const BAR_HEIGHT: f32 = 6.0;

/// Combo display top offset — sits below the top-right timer (`top:12`,
/// ~24px text => ends ~y=40). Centered horizontally so it never overlaps the
/// timer or the minimap (both top-right).
const COMBO_TOP: f32 = 48.0;

/// Presentation-only animation tuning. These animate existing UI components;
/// combo hits never spawn transient entities.
const BASE_FONT_SIZE: f32 = 32.0;
const PUNCH_FONT_BOOST: f32 = 11.0;
const PUNCH_DURATION: f32 = 0.20;
const REVEAL_IN_SPEED: f32 = 12.0;
const REVEAL_OUT_SPEED: f32 = 8.0;
/// The final portion of the combo window pulses to warn that it is expiring.
const URGENCY_THRESHOLD: f32 = 0.35;
const URGENCY_PULSE_SPEED: f32 = 12.0;

// ---------------------------------------------------------------------------
// Resource
// ---------------------------------------------------------------------------

/// Combo state. `multiplier` starts at 1 (no combo). `count` is the internal
/// consecutive-hit counter that drives the multiplier; `timer` is the
/// remaining seconds before the combo expires (decremented each frame).
#[derive(Resource)]
pub struct Combo {
    /// Current score multiplier (1 = no combo, up to 5).
    pub multiplier: u32,
    /// Seconds remaining before the combo resets to 1x.
    pub timer: f32,
    /// Internal consecutive-hit counter (drives `multiplier`).
    count: u32,
}

impl Default for Combo {
    fn default() -> Self {
        Self {
            multiplier: 1,
            timer: 0.0,
            count: 0,
        }
    }
}

impl Combo {
    /// Recompute the multiplier from the consecutive-hit count.
    /// 1x default, 2x at 5, 3x at 10, 4x at 15, capped at 5x (20+).
    fn multiplier_from_count(count: u32) -> u32 {
        match count {
            0..=4 => 1,
            5..=9 => 2,
            10..=14 => 3,
            15..=19 => 4,
            _ => 5,
        }
    }
}

/// Bonus owned by this module for one hit. The hit's base point is awarded by
/// the originating gameplay system and is deliberately never multiplied.
fn combo_bonus_for_hit(multiplier: u32, modifier: &ActiveModifier, event: &ActiveEvent) -> u32 {
    multiplier
        .saturating_sub(1)
        .saturating_mul(modifier.combo_bonus_multiplier())
        .saturating_mul(event.combo_bonus_multiplier())
}

#[cfg(test)]
mod tests {
    use super::{Combo, combo_bonus_for_hit};
    use crate::modifiers::{ActiveModifier, ModifierKind};
    use crate::run_events::{ActiveEvent, EventKind};

    #[test]
    fn multiplier_thresholds_and_cap() {
        for (count, expected) in [
            (0, 1),
            (4, 1),
            (5, 2),
            (9, 2),
            (10, 3),
            (14, 3),
            (15, 4),
            (19, 4),
            (20, 5),
            (100, 5),
        ] {
            assert_eq!(
                Combo::multiplier_from_count(count),
                expected,
                "unexpected multiplier at count {count}"
            );
        }
    }

    #[test]
    fn standard_and_glass_cannon_only_scale_the_bonus() {
        let standard = ActiveModifier(ModifierKind::Standard);
        let glass_cannon = ActiveModifier(ModifierKind::GlassCannon);
        let event = ActiveEvent::default();

        assert_eq!(combo_bonus_for_hit(4, &standard, &event), 3);
        assert_eq!(combo_bonus_for_hit(4, &glass_cannon, &event), 6);
        // Including the point awarded by the hit's owning system, the awards
        // are 4 and 7 rather than 4 and 8: the base point stays unscaled.
        assert_eq!(
            1_u32.saturating_add(combo_bonus_for_hit(4, &standard, &event)),
            4
        );
        assert_eq!(
            1_u32.saturating_add(combo_bonus_for_hit(4, &glass_cannon, &event)),
            7
        );
    }

    #[test]
    fn combo_bonus_math_saturates() {
        let glass_cannon = ActiveModifier(ModifierKind::GlassCannon);
        let event = ActiveEvent::default();
        assert_eq!(combo_bonus_for_hit(0, &glass_cannon, &event), 0);
        assert_eq!(
            combo_bonus_for_hit(u32::MAX, &glass_cannon, &event),
            u32::MAX
        );
    }

    #[test]
    fn standard_and_combo_frenzy_compose_on_the_bonus_only() {
        let standard = ActiveModifier(ModifierKind::Standard);
        let frenzy = ActiveEvent(Some(EventKind::ComboFrenzy));

        assert_eq!(combo_bonus_for_hit(4, &standard, &frenzy), 6);
        assert_eq!(
            1_u32.saturating_add(combo_bonus_for_hit(4, &standard, &frenzy)),
            7
        );
    }

    #[test]
    fn glass_cannon_and_combo_frenzy_compose_and_saturate() {
        let glass_cannon = ActiveModifier(ModifierKind::GlassCannon);
        let frenzy = ActiveEvent(Some(EventKind::ComboFrenzy));

        assert_eq!(combo_bonus_for_hit(4, &glass_cannon, &frenzy), 12);
        assert_eq!(combo_bonus_for_hit(0, &glass_cannon, &frenzy), 0);
        // The modifier product still fits, then the event product saturates.
        let event_saturates = u32::MAX / 4 + 2;
        assert_eq!(
            combo_bonus_for_hit(event_saturates, &glass_cannon, &frenzy),
            u32::MAX
        );
    }
}

// ---------------------------------------------------------------------------
// UI markers
// ---------------------------------------------------------------------------

/// Root node of the combo display (top-center, absolute). Despawned on exit
/// from `Playing`; visibility toggled by [`update_combo_ui`] based on
/// multiplier.
#[derive(Component)]
struct ComboRoot;

/// Dynamic text span showing the current multiplier number ("2", "3", ...).
#[derive(Component)]
struct ComboText;

/// Both parts of the `xN` label. Font size, tier color, and alpha are animated
/// together by [`update_combo_ui`].
#[derive(Component)]
struct ComboLabel;

/// The depleting timer bar fill; its width and color are refreshed each frame.
#[derive(Component)]
struct ComboBarFill;

/// Dark track behind the timer fill. Marked separately so its fade can be
/// queried disjointly from [`ComboBarFill`].
#[derive(Component)]
struct ComboBarTrack;

/// Animation state stored on the persistent combo root. It is intentionally a
/// component rather than a stream of short-lived entities, keeping hit juice
/// allocation-free after the UI is spawned.
#[derive(Component)]
struct ComboPresentation {
    reveal: f32,
    punch: f32,
    urgency_phase: f32,
    last_count: u32,
    displayed_multiplier: u32,
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct CombosPlugin;

impl Plugin for CombosPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Combo>()
            // Fresh-round reset (skipped on resume from Paused). MUST run in
            // SpawnSet so it executes before reset_run flips RoundActive
            // (risk E11 — mirrors chickens.rs / health.rs).
            .add_systems(OnEnter(GameState::Playing), reset_combo.in_set(SpawnSet))
            // UI lifecycle tied to Playing (despawned on exit so a
            // pause/resume cycle respawns it cleanly, like the HUD). Spawn
            // after reset so presentation state always snapshots the correct
            // fresh-round/resumed combo.
            .add_systems(
                OnEnter(GameState::Playing),
                spawn_combo_ui.after(reset_combo),
            )
            .add_systems(OnExit(GameState::Playing), despawn_marker::<ComboRoot>)
            .add_systems(
                Update,
                // Chain so hits register first, then the timer ticks, then
                // the UI reflects the final state for this frame.
                (register_hit, tick_combo, update_combo_ui)
                    .chain()
                    .run_if(in_state(GameState::Playing)),
            );
    }
}

// ---------------------------------------------------------------------------
// Hit registration + score bonus
// ---------------------------------------------------------------------------

/// Read `ChickenHit` and `CoinCollected` messages. On any hit: increment the
/// combo counter, refresh the timer to [`COMBO_WINDOW`], recompute the
/// multiplier, and add the bonus (`multiplier - 1`) to `Score` on top of the
/// +1 the existing systems already added. Glass Cannon and Combo Frenzy scale
/// only that bonus; with no active event, Standard keeps the original
/// total-per-hit behavior exactly.
///
/// `ChickenHit` / `CoinCollected` are already registered as messages in
/// `game/mod.rs`; this module only **reads** them (never re-registers).
fn register_hit(
    mut chicken_hits: MessageReader<ChickenHit>,
    mut coin_hits: MessageReader<CoinCollected>,
    mut combo: ResMut<Combo>,
    mut score: ResMut<Score>,
    modifier: Res<ActiveModifier>,
    event: Res<ActiveEvent>,
) {
    // Process each hit individually so the counter + multiplier advance
    // correctly even if multiple hits arrive in one frame (rare but possible
    // — e.g. hitting two chickens simultaneously).
    for _ in chicken_hits.read() {
        combo.count = combo.count.saturating_add(1);
        combo.timer = COMBO_WINDOW;
        combo.multiplier = Combo::multiplier_from_count(combo.count);
        // Bonus on top of the base +1 from chickens.rs::hit_chickens.
        score.chickens =
            score
                .chickens
                .saturating_add(combo_bonus_for_hit(combo.multiplier, &modifier, &event));
    }
    for _ in coin_hits.read() {
        combo.count = combo.count.saturating_add(1);
        combo.timer = COMBO_WINDOW;
        combo.multiplier = Combo::multiplier_from_count(combo.count);
        // Bonus on top of the base +1 from world.rs::collect_coins.
        score.coins =
            score
                .coins
                .saturating_add(combo_bonus_for_hit(combo.multiplier, &modifier, &event));
    }
}

// ---------------------------------------------------------------------------
// Timer tick
// ---------------------------------------------------------------------------

/// Decrement the combo timer each frame; when it reaches 0, reset the combo
/// (multiplier back to 1, counter to 0). Runs only during `Playing`.
fn tick_combo(mut combo: ResMut<Combo>, time: Res<Time>) {
    if combo.timer > 0.0 {
        combo.timer -= time.delta_secs();
        if combo.timer <= 0.0 {
            combo.timer = 0.0;
            combo.multiplier = 1;
            combo.count = 0;
        }
    }
}

// ---------------------------------------------------------------------------
// Fresh-round reset
// ---------------------------------------------------------------------------

/// Reset the combo to defaults on a fresh round. Skipped when resuming from
/// `Paused` (the round is still active), per the fresh-round rule (risk E11).
fn reset_combo(mut combo: ResMut<Combo>, round_active: Res<RoundActive>) {
    if round_active.0 {
        return;
    }
    *combo = Combo::default();
}

// ---------------------------------------------------------------------------
// Combo UI
// ---------------------------------------------------------------------------

/// Spawn the combo display (top-center, absolute): a big "x{N}" multiplier
/// badge above a thin depleting timer bar. Lives only while `Playing`
/// (despawned by [`despawn_marker::<ComboRoot>`] on exit). Starts hidden;
/// [`update_combo_ui`] reveals it when `multiplier > 1`.
fn spawn_combo_ui(mut commands: Commands, combo: Res<Combo>) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: px(COMBO_TOP),
                left: px(0.0),
                width: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                flex_direction: FlexDirection::Column,
                ..default()
            },
            ComboRoot,
            ComboPresentation {
                reveal: 0.0,
                punch: 0.0,
                urgency_phase: 0.0,
                last_count: combo.count,
                displayed_multiplier: combo.multiplier.max(2),
            },
            Visibility::Hidden,
        ))
        .with_children(|col| {
            // "x{multiplier}" — the "x" prefix is static, the number is a
            // dynamic span refreshed by `update_combo_ui`.
            col.spawn((
                Text::new("x"),
                TextFont {
                    font_size: FontSize::Px(BASE_FONT_SIZE),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
                Node {
                    margin: UiRect::bottom(px(4.0)),
                    ..default()
                },
                ComboLabel,
            ))
            .with_child((
                TextSpan::default(),
                TextFont {
                    font_size: FontSize::Px(BASE_FONT_SIZE),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
                ComboText,
                ComboLabel,
            ));
            // Timer bar track (dark background) with the colored fill child.
            col.spawn((
                Node {
                    width: px(BAR_WIDTH),
                    height: px(BAR_HEIGHT),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.0)),
                ComboBarTrack,
            ))
            .with_child((
                Node {
                    width: px(BAR_WIDTH),
                    height: Val::Percent(100.0),
                    ..default()
                },
                BackgroundColor(palette::HUD_ACCENT),
                ComboBarFill,
            ));
        });
}

/// Refresh and animate the multiplier label, timer bar, and visibility. A hit
/// punches the existing text, tier changes recolor it, reveal/hide is eased,
/// and the final part of the timer pulses toward red. No entities are spawned
/// by this per-frame system.
fn update_combo_ui(
    combo: Res<Combo>,
    time: Res<Time>,
    mut root: Query<(&mut Visibility, &mut ComboPresentation), With<ComboRoot>>,
    mut text: Query<&mut TextSpan, With<ComboText>>,
    mut labels: Query<(&mut TextFont, &mut TextColor), With<ComboLabel>>,
    mut bar_fill: Query<
        (&mut Node, &mut BackgroundColor),
        (With<ComboBarFill>, Without<ComboBarTrack>),
    >,
    mut bar_track: Query<&mut BackgroundColor, (With<ComboBarTrack>, Without<ComboBarFill>)>,
) {
    let Ok((mut visibility, mut presentation)) = root.single_mut() else {
        return;
    };

    let dt = time.delta_secs();
    let active = combo.multiplier > 1;

    // A count increase means one or more hits landed this frame. Restarting a
    // single component timer gives every increment a crisp punch without
    // creating transient UI entities.
    if combo.count > presentation.last_count && active {
        presentation.punch = 1.0;
    }
    presentation.last_count = combo.count;
    if active {
        presentation.displayed_multiplier = combo.multiplier;
    }

    let reveal_target = if active { 1.0 } else { 0.0 };
    let reveal_speed = if active {
        REVEAL_IN_SPEED
    } else {
        REVEAL_OUT_SPEED
    };
    let reveal_step = 1.0 - (-reveal_speed * dt).exp();
    presentation.reveal += (reveal_target - presentation.reveal) * reveal_step;
    if !active && presentation.reveal < 0.01 {
        presentation.reveal = 0.0;
    }
    let reveal = presentation.reveal.clamp(0.0, 1.0);
    // Smoothstep keeps the first and last reveal frames from popping.
    let eased_reveal = reveal * reveal * (3.0 - 2.0 * reveal);
    *visibility = if active || reveal > 0.0 {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };

    let timer_frac = (combo.timer / COMBO_WINDOW).clamp(0.0, 1.0);
    let urgent = active && timer_frac <= URGENCY_THRESHOLD;
    if urgent {
        presentation.urgency_phase += dt * URGENCY_PULSE_SPEED;
    } else {
        presentation.urgency_phase = 0.0;
    }
    let urgency_pulse = if urgent {
        (presentation.urgency_phase.sin() + 1.0) * 0.5
    } else {
        0.0
    };

    let punch = presentation.punch * presentation.punch;
    let font_size =
        BASE_FONT_SIZE + PUNCH_FONT_BOOST * punch + if urgent { 2.0 * urgency_pulse } else { 0.0 };
    presentation.punch = (presentation.punch - dt / PUNCH_DURATION).max(0.0);

    let alpha_pulse = if urgent {
        0.72 + 0.28 * urgency_pulse
    } else {
        1.0
    };
    let alpha = eased_reveal * alpha_pulse;
    let warning_mix = if urgent {
        0.25 + 0.55 * urgency_pulse
    } else {
        0.0
    };
    let color = combo_tier_color(presentation.displayed_multiplier, warning_mix, alpha);

    let multiplier_text = format!("{}", presentation.displayed_multiplier);
    for mut span in &mut text {
        **span = multiplier_text.clone();
    }
    for (mut font, mut text_color) in &mut labels {
        font.font_size = FontSize::Px(font_size);
        text_color.0 = color;
    }
    for (mut node, mut fill_color) in &mut bar_fill {
        node.width = px(BAR_WIDTH * timer_frac);
        fill_color.0 = color;
    }
    for mut track_color in &mut bar_track {
        track_color.0 = Color::srgba(0.0, 0.0, 0.0, 0.6 * eased_reveal);
    }
}

/// Distinct, increasingly hot colors make multiplier tiers readable at a
/// glance. During urgency the current tier smoothly leans toward warning red.
fn combo_tier_color(multiplier: u32, warning_mix: f32, alpha: f32) -> Color {
    let tier_rgb = match multiplier {
        2 => Vec3::new(0.20, 0.88, 1.00),   // cyan
        3 => Vec3::new(1.00, 0.82, 0.12),   // gold
        4 => Vec3::new(1.00, 0.42, 0.08),   // orange
        5.. => Vec3::new(1.00, 0.18, 0.62), // hot pink
        _ => Vec3::new(1.00, 0.80, 0.00),
    };
    let rgb = tier_rgb.lerp(Vec3::new(1.0, 0.08, 0.04), warning_mix.clamp(0.0, 1.0));
    Color::srgba(rgb.x, rgb.y, rgb.z, alpha.clamp(0.0, 1.0))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Despawn every entity tagged with marker `M` (mirrors `ui.rs` / `health.rs`).
fn despawn_marker<M: Component>(mut commands: Commands, q: Query<Entity, With<M>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}
