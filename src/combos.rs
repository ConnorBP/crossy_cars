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
//! for coins), so the total per hit = base 1 + (multiplier - 1) = `multiplier`.

use bevy::prelude::*;
use bevy::text::FontSize;

use crate::game::events::{ChickenHit, CoinCollected};
use crate::game::resources::{RoundActive, Score};
use crate::game::state::GameState;
use crate::game::SpawnSet;
use crate::palette;

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

/// The depleting timer bar fill; its width is refreshed each frame.
#[derive(Component)]
struct ComboBarFill;

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
            .add_systems(
                OnEnter(GameState::Playing),
                reset_combo.in_set(SpawnSet),
            )
            // UI lifecycle tied to Playing (despawned on exit so a
            // pause/resume cycle respawns it cleanly, like the HUD).
            .add_systems(OnEnter(GameState::Playing), spawn_combo_ui)
            .add_systems(
                OnExit(GameState::Playing),
                despawn_marker::<ComboRoot>,
            )
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
/// +1 the existing systems already added — so the total per hit equals
/// `multiplier`.
///
/// `ChickenHit` / `CoinCollected` are already registered as messages in
/// `game/mod.rs`; this module only **reads** them (never re-registers).
fn register_hit(
    mut chicken_hits: MessageReader<ChickenHit>,
    mut coin_hits: MessageReader<CoinCollected>,
    mut combo: ResMut<Combo>,
    mut score: ResMut<Score>,
) {
    // Process each hit individually so the counter + multiplier advance
    // correctly even if multiple hits arrive in one frame (rare but possible
    // — e.g. hitting two chickens simultaneously).
    for _ in chicken_hits.read() {
        combo.count += 1;
        combo.timer = COMBO_WINDOW;
        combo.multiplier = Combo::multiplier_from_count(combo.count);
        // Bonus on top of the base +1 from chickens.rs::hit_chickens.
        score.chickens += combo.multiplier.saturating_sub(1);
    }
    for _ in coin_hits.read() {
        combo.count += 1;
        combo.timer = COMBO_WINDOW;
        combo.multiplier = Combo::multiplier_from_count(combo.count);
        // Bonus on top of the base +1 from world.rs::collect_coins.
        score.coins += combo.multiplier.saturating_sub(1);
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
fn spawn_combo_ui(mut commands: Commands) {
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
            Visibility::Hidden,
        ))
        .with_children(|col| {
            // "x{multiplier}" — the "x" prefix is static, the number is a
            // dynamic span refreshed by `update_combo_ui`.
            col.spawn((
                Text::new("x"),
                TextFont {
                    font_size: FontSize::Px(32.0),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
                Node {
                    margin: UiRect::bottom(px(4.0)),
                    ..default()
                },
            ))
            .with_child((
                TextSpan::default(),
                TextFont {
                    font_size: FontSize::Px(32.0),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
                ComboText,
            ));
            // Timer bar track (dark background) with the colored fill child.
            col.spawn((
                Node {
                    width: px(BAR_WIDTH),
                    height: px(BAR_HEIGHT),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
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

/// Refresh the multiplier text, the timer bar width, and the root visibility
/// each frame. The display is shown only when `multiplier > 1`.
fn update_combo_ui(
    combo: Res<Combo>,
    mut root: Query<&mut Visibility, With<ComboRoot>>,
    mut text: Query<&mut TextSpan, With<ComboText>>,
    mut bar: Query<&mut Node, With<ComboBarFill>>,
) {
    let Ok(mut vis) = root.single_mut() else {
        return;
    };

    if combo.multiplier > 1 {
        *vis = Visibility::Visible;
        for mut span in &mut text {
            **span = format!("{}", combo.multiplier);
        }
        let frac = (combo.timer / COMBO_WINDOW).clamp(0.0, 1.0);
        for mut node in &mut bar {
            node.width = px(BAR_WIDTH * frac);
        }
    } else {
        *vis = Visibility::Hidden;
    }
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
