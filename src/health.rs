//! Car health & damage system (T2).
//!
//! The car takes damage when it slams into a solid obstacle (building / tree
//! / lamp post) at speed. `car.rs::physics_collisions` emits an
//! [`ObstacleHit`](crate::game::events::ObstacleHit) message carrying the
//! impact speed; [`apply_damage`] converts that into health loss and, when the
//! car is wrecked, transitions to [`GameState::GameOver`].
//!
//! Owns its own UI (health bar + damage-flash vignette) so it never touches
//! `ui.rs`. Audio (thud on damage, crash on destroy) is preloaded via a
//! [`FromWorld`] resource and fired with [`AudioPlayer`] +
//! [`PlaybackSettings::DESPAWN`], matching the `audio.rs` pattern.

use bevy::audio::{AudioPlayer, AudioSource, PlaybackSettings, Volume};
use bevy::prelude::*;
use bevy::text::FontSize;

use crate::audio::AudioBaseGain;
use crate::car::{Car, DrivingSet};
use crate::game::events::ObstacleHit;
use crate::game::resources::{Drowning, RoundActive};
use crate::game::state::GameState;
use crate::game::{SpawnSet, TouchStateSet};
use crate::modifiers::ActiveModifier;
use crate::settings::Settings;
#[cfg(test)]
use crate::touch::ScreenBounds;
use crate::touch::{
    TOUCH_HEALTH_HEIGHT, TOUCH_HEALTH_LEFT, TOUCH_HEALTH_TOP, TOUCH_HEALTH_WIDTH,
    TouchControlsActive,
};

/// Damage multiplier: `impact_speed * DAMAGE_K` health lost per hit.
/// A full-speed (~12 u/s) hit => ~48 dmg, so 2-3 hard hits wreck the car.
const DAMAGE_K: f32 = 4.0;

/// Maximum / full health value.
const HEALTH_MAX: f32 = 100.0;

/// Damage-flash vignette lifetime in seconds.
const FLASH_DURATION: f32 = 0.25;
/// Damage-flash peak alpha (fades to 0 over [`FLASH_DURATION`]).
const FLASH_PEAK_ALPHA: f32 = 0.45;

/// Minimum interval between damaging obstacle contacts. Collision systems may
/// report repeated overlaps while resolving contact, but only one can damage
/// the car during this window.
const OBSTACLE_DAMAGE_COOLDOWN_SECS: f32 = 0.5;

/// Width of the health bar track in UI pixels.
const BAR_TRACK_W: f32 = 240.0;
const TOUCH_BAR_TRACK_W: f32 = 178.0;
/// Height of the health bar fill in UI pixels.
const BAR_H: f32 = 14.0;

/// Explicit panel footprint used by both the touch node and pure overlap
/// checks. The desktop root remains the existing full-width centered strip.
#[cfg(test)]
const HEALTH_PANEL_SIZE: Vec2 = Vec2::new(256.0, 76.0);
const HEALTH_DESKTOP_BOTTOM: f32 = 12.0;

// ---------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------

/// Current car health (0..=100). Starts full; reset to full on a fresh round.
#[derive(Resource)]
pub struct Health(pub f32);

impl Default for Health {
    fn default() -> Self {
        Self(HEALTH_MAX)
    }
}

/// Gate preventing repeated obstacle-contact messages from grinding health.
/// It pauses with gameplay and is cleared at the start of each fresh round.
#[derive(Resource, Debug, Default)]
pub struct ObstacleDamageCooldown {
    remaining: f32,
}

impl ObstacleDamageCooldown {
    fn tick(&mut self, dt: f32) {
        if dt > 0.0 && dt.is_finite() {
            self.remaining = if dt >= self.remaining {
                0.0
            } else {
                self.remaining - dt
            };
        }
    }

    fn try_start(&mut self) -> bool {
        if self.remaining > 0.0 {
            return false;
        }
        self.remaining = OBSTACLE_DAMAGE_COOLDOWN_SECS;
        true
    }
}

/// Preloaded audio handles for damage/destroy SFX. Built eagerly via
/// [`FromWorld`] (mirroring `audio.rs::AudioHandles`) so the handles exist
/// before any `Update` system fires them.
#[derive(Resource)]
struct DamageAudioHandles {
    /// `penalty.wav` played at reduced volume on every damaging (non-terminal)
    /// hit. Shares the same sample as `audio.rs::play_penalty` so obstacle
    /// scrapes layer consistently with critter strikes.
    penalty: Handle<AudioSource>,
    /// Distinct `crash.wav` played once when the car is wrecked.
    crash: Handle<AudioSource>,
}

impl FromWorld for DamageAudioHandles {
    fn from_world(world: &mut World) -> Self {
        let asset_server = world.resource::<AssetServer>();
        DamageAudioHandles {
            penalty: asset_server.load("audio/penalty.wav"),
            crash: asset_server.load("audio/crash.wav"),
        }
    }
}

// ---------------------------------------------------------------------------
// UI markers
// ---------------------------------------------------------------------------

/// Root node of the health bar (bottom-center on desktop, upper-left above
/// touch driving controls). Despawned and respawned with `Playing`.
#[derive(Component)]
struct HealthBarRoot;

/// The colored fill inside the bar track; its width + color are refreshed
/// each frame by [`update_health_bar`].
#[derive(Component)]
struct HealthBarFill;

#[derive(Component)]
struct HealthBarTrack;

#[derive(Component)]
struct HealthPanel;

/// Dynamic number span showing the live health value.
#[derive(Component)]
struct HealthText;

#[derive(Component)]
struct HealthNumeric;

/// Full-screen red vignette spawned on damage; fades out and despawns.
#[derive(Component)]
struct DamageFlash {
    /// Seconds remaining in the flash.
    t: f32,
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct HealthPlugin;

impl Plugin for HealthPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Health>()
            .init_resource::<ObstacleDamageCooldown>()
            .init_resource::<DamageAudioHandles>()
            // Fresh-round reset (skipped on resume from Paused). MUST run
            // before `reset_run` flips `RoundActive` on, so it's placed in
            // `SpawnSet` (which `reset_run` follows via `.after(SpawnSet)`).
            .add_systems(OnEnter(GameState::Playing), reset_health.in_set(SpawnSet))
            // UI lifecycle tied to the Playing state (despawned on exit so a
            // pause/resume cycle respawns it cleanly, like the HUD).
            .add_systems(OnEnter(GameState::Playing), spawn_health_bar)
            .add_systems(
                OnExit(GameState::Playing),
                (
                    despawn_marker::<HealthBarRoot>,
                    // A transient flash must never survive a pause/resume or
                    // any other departure from active gameplay.
                    despawn_marker::<DamageFlash>,
                ),
            )
            .add_systems(
                Update,
                (
                    tick_obstacle_damage_cooldown,
                    apply_damage,
                    update_health_bar,
                    update_damage_flash,
                )
                    .chain()
                    // Consume impacts after the driving/collision chain that
                    // emits them, avoiding schedule-dependent extra latency.
                    .after(DrivingSet)
                    .run_if(in_state(GameState::Playing)),
            )
            // UI-only refreshes remain outside the gameplay chain. Queries
            // are empty/trivial whenever the Playing-owned root is absent.
            .add_systems(
                Update,
                (update_health_bar_color, update_health_layout).after(TouchStateSet),
            );
    }
}

// ---------------------------------------------------------------------------
// Damage application
// ---------------------------------------------------------------------------

/// Calculate damage for one qualifying obstacle impact. Collision code owns
/// the low-speed hit threshold; this keeps the existing positive-speed rule
/// while applying the round modifier and bounding exceptional float inputs.
fn obstacle_damage(impact_speed: f32, modifier: &ActiveModifier) -> f32 {
    if impact_speed.is_nan() || impact_speed <= 0.0 {
        return 0.0;
    }

    let multiplier = modifier.damage_multiplier();
    if multiplier.is_nan() || multiplier <= 0.0 {
        return 0.0;
    }

    ((impact_speed as f64) * (DAMAGE_K as f64) * (multiplier as f64)).clamp(0.0, f32::MAX as f64)
        as f32
}

/// Convert [`ObstacleHit`] impacts into health loss, accepting at most one
/// damaging contact per cooldown window. On damage, play a low-volume thud
/// and flash the vignette. When health hits zero, stop the car, play the crash
/// SFX, and transition to [`GameState::GameOver`].
///
/// `ObstacleHit` is already registered via `game/mod.rs` — not re-registered.
fn apply_damage(
    mut hits: MessageReader<ObstacleHit>,
    mut health: ResMut<Health>,
    mut cooldown: ResMut<ObstacleDamageCooldown>,
    mut car: Query<&mut Car>,
    mut next: ResMut<NextState<GameState>>,
    mut reason: ResMut<crate::game::resources::GameOverReason>,
    mut commands: Commands,
    audio: Res<DamageAudioHandles>,
    modifier: Res<ActiveModifier>,
    settings: Res<Settings>,
    drowning: Res<Drowning>,
) {
    // Detection already ran in DrivingSet. Discard this frame's obstacle
    // messages once pond entry wins, preventing damage or Wrecked terminal.
    if drowning.active {
        hits.clear();
        return;
    }
    // Read the complete frame before touching the cooldown. Although the car
    // collision system emits at most one hit, this keeps health deterministic
    // if another producer contributes events: the strongest valid event owns
    // the one available cooldown slot, independent of reader order.
    let strongest = strongest_impact(hits.read().map(|hit| hit.impact_speed));
    let Some(impact_speed) = strongest else {
        return;
    };
    let dmg = obstacle_damage(impact_speed, &modifier);
    if dmg <= 0.0 || !cooldown.try_start() {
        return;
    }
    health.0 = (health.0 - dmg).max(0.0);

    if health.0 <= 0.0 {
        // Wrecked: stop the car, play the crash, end the round.
        if let Ok(mut c) = car.single_mut() {
            c.speed = 0.0;
        }
        // Authored crash gain (0.9). Carrying the matching `AudioBaseGain`
        // lets the live master bridge in `audio.rs` rescale this DESPAWN
        // one-shot without compounding, just like the other gameplay SFX.
        commands.spawn((
            AudioPlayer::new(audio.crash.clone()),
            PlaybackSettings::DESPAWN.with_volume(Volume::Linear(0.9)),
            AudioBaseGain(0.9),
        ));
        *reason = crate::game::resources::GameOverReason::Wrecked;
        next.set(GameState::GameOver);
        // No point flashing once the GameOver overlay covers the screen.
        return;
    }

    // Penalty thud: `penalty.wav` at a lower volume (0.5) so repeated
    // scrapes aren't louder than the chicken/coin SFX. Carrying the matching
    // `AudioBaseGain` lets the live master bridge in `audio.rs` rescale this
    // DESPAWN one-shot without compounding, just like the other gameplay SFX.
    commands.spawn((
        AudioPlayer::new(audio.penalty.clone()),
        PlaybackSettings::DESPAWN.with_volume(Volume::Linear(0.5)),
        AudioBaseGain(0.5),
    ));
    if should_spawn_damage_flash(settings.reduced_motion) {
        spawn_damage_flash(&mut commands);
    }
}

/// Strongest positive, non-NaN impact in a frame. `total_cmp` gives infinities
/// a defined order while invalid/non-damaging values cannot mask a real hit.
fn strongest_impact(impacts: impl IntoIterator<Item = f32>) -> Option<f32> {
    impacts
        .into_iter()
        .filter(|impact| !impact.is_nan() && *impact > 0.0)
        .max_by(|a, b| a.total_cmp(b))
}

/// Advance the contact-damage gate only while gameplay is active.
fn tick_obstacle_damage_cooldown(time: Res<Time>, mut cooldown: ResMut<ObstacleDamageCooldown>) {
    cooldown.tick(time.delta_secs());
}

/// Reset health and its contact gate on a fresh round. Skipped when resuming
/// from `Paused` (the round is still active), per the fresh-round rule (risk
/// E11), so pausing cannot bypass an active cooldown.
fn reset_health(
    mut health: ResMut<Health>,
    mut cooldown: ResMut<ObstacleDamageCooldown>,
    round_active: Res<RoundActive>,
) {
    if round_active.0 {
        return;
    }
    health.0 = HEALTH_MAX;
    *cooldown = ObstacleDamageCooldown::default();
}

// ---------------------------------------------------------------------------
// Health bar UI
// ---------------------------------------------------------------------------

/// Spawn the health panel in its desktop or touch-aware position. Lives only
/// while `Playing` (despawned by [`despawn_marker::<HealthBarRoot>`] on exit).
fn spawn_health_bar(mut commands: Commands, touch: Res<TouchControlsActive>) {
    let track_width = health_track_width(touch.0);
    commands
        .spawn((health_root_node(touch.0), HealthBarRoot))
        .with_children(|col| {
            // Inner panel so the bar + label sit in a tidy boxed cluster
            // rather than spanning the whole screen width.
            col.spawn((
                health_panel_node(touch.0),
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.35)),
                HealthPanel,
            ))
            .with_children(|panel| {
                // "HEALTH" label
                panel.spawn((
                    Text::new("HEALTH"),
                    TextFont {
                        font_size: FontSize::Px(13.0),
                        ..default()
                    },
                    TextColor(Color::srgba(0.75, 0.75, 0.8, 1.0).into()),
                    Node {
                        margin: UiRect::bottom(px(3.0)),
                        ..default()
                    },
                ));
                // Bar track (dark background) with the colored fill child.
                panel
                    .spawn((
                        Node {
                            width: px(track_width),
                            height: px(BAR_H),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
                        HealthBarTrack,
                    ))
                    .with_child((
                        Node {
                            width: px(track_width),
                            height: Val::Percent(100.0),
                            ..default()
                        },
                        BackgroundColor(health_color(1.0)),
                        HealthBarFill,
                    ));
                // Live numeric health value (accent).
                panel
                    .spawn((
                        Text::new(""),
                        TextFont {
                            font_size: FontSize::Px(18.0),
                            ..default()
                        },
                        TextColor(crate::palette::HUD_ACCENT.into()),
                        Node {
                            margin: UiRect::top(px(3.0)),
                            display: if touch.0 {
                                Display::None
                            } else {
                                Display::Flex
                            },
                            ..default()
                        },
                        HealthNumeric,
                    ))
                    .with_child((
                        TextSpan::default(),
                        TextFont {
                            font_size: FontSize::Px(18.0),
                            ..default()
                        },
                        TextColor(crate::palette::HUD_ACCENT.into()),
                        HealthText,
                    ));
            });
        });
}

/// Desktop keeps the original centered strip. Once touch controls become
/// active, move health to the upper-left of the driving band so neither the
/// panel nor its root intercepts lower-center HANDBRAKE/BRAKE touches.
fn health_root_node(touch_active: bool) -> Node {
    if touch_active {
        Node {
            position_type: PositionType::Absolute,
            top: px(TOUCH_HEALTH_TOP),
            left: px(TOUCH_HEALTH_LEFT),
            width: px(TOUCH_HEALTH_WIDTH),
            height: px(TOUCH_HEALTH_HEIGHT),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            flex_direction: FlexDirection::Column,
            ..default()
        }
    } else {
        Node {
            position_type: PositionType::Absolute,
            bottom: px(HEALTH_DESKTOP_BOTTOM),
            left: px(0.0),
            width: Val::Percent(100.0),
            // Preserve the original desktop layout exactly.
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            flex_direction: FlexDirection::Column,
            ..default()
        }
    }
}

fn health_panel_node(touch_active: bool) -> Node {
    Node {
        flex_direction: FlexDirection::Column,
        padding: UiRect::all(px(if touch_active { 4.0 } else { 8.0 })),
        ..default()
    }
}

const fn health_track_width(touch_active: bool) -> f32 {
    if touch_active {
        TOUCH_BAR_TRACK_W
    } else {
        BAR_TRACK_W
    }
}

fn update_health_layout(
    touch: Res<TouchControlsActive>,
    health: Res<Health>,
    mut root: Query<
        &mut Node,
        (
            With<HealthBarRoot>,
            Without<HealthPanel>,
            Without<HealthBarTrack>,
            Without<HealthBarFill>,
            Without<HealthNumeric>,
        ),
    >,
    mut panels: Query<
        &mut Node,
        (
            With<HealthPanel>,
            Without<HealthBarRoot>,
            Without<HealthBarTrack>,
            Without<HealthBarFill>,
            Without<HealthNumeric>,
        ),
    >,
    mut tracks: Query<
        &mut Node,
        (
            With<HealthBarTrack>,
            Without<HealthBarRoot>,
            Without<HealthPanel>,
            Without<HealthBarFill>,
            Without<HealthNumeric>,
        ),
    >,
    mut fills: Query<
        &mut Node,
        (
            With<HealthBarFill>,
            Without<HealthBarRoot>,
            Without<HealthPanel>,
            Without<HealthBarTrack>,
            Without<HealthNumeric>,
        ),
    >,
    mut numeric: Query<
        &mut Node,
        (
            With<HealthNumeric>,
            Without<HealthBarRoot>,
            Without<HealthPanel>,
            Without<HealthBarTrack>,
            Without<HealthBarFill>,
        ),
    >,
) {
    if !touch.is_changed() {
        return;
    }
    for mut node in &mut root {
        *node = health_root_node(touch.0);
    }
    for mut node in &mut panels {
        *node = health_panel_node(touch.0);
    }
    for mut node in &mut tracks {
        node.width = px(health_track_width(touch.0));
    }
    let fraction = (health.0 / HEALTH_MAX).clamp(0.0, 1.0);
    for mut node in &mut fills {
        node.width = px(health_track_width(touch.0) * fraction);
    }
    for mut node in &mut numeric {
        node.display = if touch.0 {
            Display::None
        } else {
            Display::Flex
        };
    }
}

#[cfg(test)]
fn health_panel_bounds(viewport: Vec2, touch_active: bool) -> ScreenBounds {
    if touch_active {
        ScreenBounds {
            left: TOUCH_HEALTH_LEFT,
            top: TOUCH_HEALTH_TOP,
            right: TOUCH_HEALTH_LEFT + TOUCH_HEALTH_WIDTH,
            bottom: TOUCH_HEALTH_TOP + TOUCH_HEALTH_HEIGHT,
        }
    } else {
        let left = (viewport.x - HEALTH_PANEL_SIZE.x) * 0.5;
        ScreenBounds {
            left,
            top: viewport.y - HEALTH_DESKTOP_BOTTOM - HEALTH_PANEL_SIZE.y,
            right: left + HEALTH_PANEL_SIZE.x,
            bottom: viewport.y - HEALTH_DESKTOP_BOTTOM,
        }
    }
}

/// Refresh the fill width and the numeric span each frame. (The fill's
/// `BackgroundColor` is updated by [`update_health_bar_color`].)
fn update_health_bar(
    health: Res<Health>,
    touch: Res<TouchControlsActive>,
    mut fill: Query<&mut Node, With<HealthBarFill>>,
    mut text: Query<&mut TextSpan, With<HealthText>>,
) {
    let frac = (health.0 / HEALTH_MAX).clamp(0.0, 1.0);
    for mut node in &mut fill {
        node.width = px(health_track_width(touch.0) * frac);
    }
    for mut span in &mut text {
        **span = format!("{:.0}", health.0.ceil());
    }
}

/// Recolor the fill green -> yellow -> red by health threshold. Runs every
/// frame in every state (the query is empty/trivial outside `Playing`).
fn update_health_bar_color(
    health: Res<Health>,
    mut fill: Query<&mut BackgroundColor, With<HealthBarFill>>,
) {
    let frac = (health.0 / HEALTH_MAX).clamp(0.0, 1.0);
    let color = health_color(frac);
    for mut bg in &mut fill {
        bg.0 = color;
    }
}

/// Green (>50%) -> yellow (25-50%) -> red (<25%) by threshold.
fn health_color(frac: f32) -> Color {
    if frac > 0.5 {
        // Green, drifting slightly toward lime as it drops.
        Color::srgb(0.20, 0.75, 0.25)
    } else if frac > 0.25 {
        // Yellow/amber band.
        Color::srgb(0.95, 0.80, 0.15)
    } else {
        // Red, brightening as it gets critical for urgency.
        Color::srgb(0.90, 0.18, 0.15)
    }
}

// ---------------------------------------------------------------------------
// Damage flash vignette
// ---------------------------------------------------------------------------

/// Whether new damage should produce a full-screen flash.
fn should_spawn_damage_flash(reduced_motion: bool) -> bool {
    !reduced_motion
}

/// Spawn a full-screen red overlay that fades over [`FLASH_DURATION`].
fn spawn_damage_flash(commands: &mut Commands) {
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            top: px(0.0),
            left: px(0.0),
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        BackgroundColor(Color::srgba(1.0, 0.0, 0.0, FLASH_PEAK_ALPHA)),
        DamageFlash { t: FLASH_DURATION },
    ));
}

/// Fade the flash alpha toward 0 and despawn once expired.
fn update_damage_flash(
    mut commands: Commands,
    time: Res<Time>,
    settings: Res<Settings>,
    mut q: Query<(Entity, &mut DamageFlash, &mut BackgroundColor)>,
) {
    for (e, mut flash, mut bg) in &mut q {
        flash.t -= time.delta_secs();
        if should_retire_damage_flash(settings.reduced_motion, flash.t) {
            commands.entity(e).despawn();
            continue;
        }
        let frac = (flash.t / FLASH_DURATION).clamp(0.0, 1.0);
        bg.0 = Color::srgba(1.0, 0.0, 0.0, FLASH_PEAK_ALPHA * frac);
    }
}

/// Accessibility changes take effect on an already-visible flash immediately.
fn should_retire_damage_flash(reduced_motion: bool, remaining: f32) -> bool {
    reduced_motion || remaining <= 0.0
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Despawn every entity tagged with marker `M` (mirrors `ui.rs`).
fn despawn_marker<M: Component>(mut commands: Commands, q: Query<Entity, With<M>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BAR_TRACK_W, DAMAGE_K, ObstacleDamageCooldown, TOUCH_BAR_TRACK_W, health_panel_bounds,
        health_track_width, obstacle_damage, should_retire_damage_flash, should_spawn_damage_flash,
        strongest_impact,
    };
    use crate::modifiers::{ActiveModifier, ModifierKind};
    use crate::touch::touch_driving_band_bounds;
    use bevy::prelude::Vec2;

    #[test]
    fn touch_health_panel_clears_driving_band_at_target_viewports() {
        for viewport in [Vec2::new(844.0, 390.0), Vec2::new(1440.0, 900.0)] {
            let health = health_panel_bounds(viewport, true);
            let driving = touch_driving_band_bounds(viewport);
            assert!(!health.overlaps(driving), "{health:?} overlaps {driving:?}");
            assert!(health.left >= 0.0 && health.top >= 0.0);
            assert!(health.right <= viewport.x && health.bottom <= viewport.y);
        }
    }

    #[test]
    fn desktop_health_panel_keeps_bottom_center_placement() {
        for viewport in [Vec2::new(844.0, 390.0), Vec2::new(1440.0, 900.0)] {
            let health = health_panel_bounds(viewport, false);
            assert_eq!((health.left + health.right) * 0.5, viewport.x * 0.5);
            assert_eq!(viewport.y - health.bottom, super::HEALTH_DESKTOP_BOTTOM);
        }
    }

    #[test]
    fn health_track_width_is_compact_only_for_touch() {
        assert_eq!(health_track_width(true), TOUCH_BAR_TRACK_W);
        assert_eq!(health_track_width(true), 178.0);
        assert_eq!(health_track_width(false), BAR_TRACK_W);
        assert_eq!(health_track_width(false), 240.0);
    }

    #[test]
    fn standard_and_glass_cannon_damage_are_exact() {
        let standard = ActiveModifier(ModifierKind::Standard);
        let glass_cannon = ActiveModifier(ModifierKind::GlassCannon);

        assert_eq!(obstacle_damage(12.0, &standard), 12.0 * DAMAGE_K);
        assert_eq!(obstacle_damage(12.0, &glass_cannon), 12.0 * DAMAGE_K * 2.0);
    }

    #[test]
    fn obstacle_damage_cooldown_blocks_then_reopens() {
        let mut cooldown = ObstacleDamageCooldown::default();

        assert!(cooldown.try_start());
        assert!(!cooldown.try_start());
        cooldown.tick(0.25);
        assert!(!cooldown.try_start());
        cooldown.tick(0.25);
        assert!(cooldown.try_start());
    }

    #[test]
    fn damage_flash_respects_reduced_motion() {
        assert!(should_spawn_damage_flash(false));
        assert!(!should_spawn_damage_flash(true));
    }

    #[test]
    fn existing_flash_retires_immediately_when_reduced_motion_toggles() {
        assert!(should_retire_damage_flash(true, 0.2));
        assert!(!should_retire_damage_flash(false, 0.2));
        assert!(should_retire_damage_flash(false, 0.0));
    }

    #[test]
    fn cooldown_candidate_is_strongest_regardless_of_event_order() {
        assert_eq!(strongest_impact([7.0, 12.0, 9.0]), Some(12.0));
        assert_eq!(strongest_impact([9.0, 7.0, 12.0]), Some(12.0));
        assert_eq!(strongest_impact([f32::NAN, -1.0, 0.0]), None);
    }

    #[test]
    fn damage_preserves_non_damaging_impacts_and_clamps_extremes() {
        let glass_cannon = ActiveModifier(ModifierKind::GlassCannon);

        assert_eq!(obstacle_damage(0.0, &glass_cannon), 0.0);
        assert_eq!(obstacle_damage(-1.0, &glass_cannon), 0.0);
        assert_eq!(obstacle_damage(f32::NAN, &glass_cannon), 0.0);
        assert_eq!(obstacle_damage(f32::INFINITY, &glass_cannon), f32::MAX);
        assert_eq!(obstacle_damage(f32::MAX, &glass_cannon), f32::MAX);
    }
}
