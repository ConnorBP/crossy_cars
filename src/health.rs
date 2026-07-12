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

use crate::car::Car;
use crate::game::SpawnSet;
use crate::game::events::ObstacleHit;
use crate::game::resources::RoundActive;
use crate::game::state::GameState;
use crate::modifiers::ActiveModifier;
use crate::settings::Settings;

/// Damage multiplier: `impact_speed * DAMAGE_K` health lost per hit.
/// A full-speed (~12 u/s) hit => ~48 dmg, so 2-3 hard hits wreck the car.
const DAMAGE_K: f32 = 4.0;

/// Maximum / full health value.
const HEALTH_MAX: f32 = 100.0;

/// Damage-flash vignette lifetime in seconds.
const FLASH_DURATION: f32 = 0.25;
/// Damage-flash peak alpha (fades to 0 over [`FLASH_DURATION`]).
const FLASH_PEAK_ALPHA: f32 = 0.45;

/// Width of the health bar track in UI pixels.
const BAR_TRACK_W: f32 = 240.0;
/// Height of the health bar fill in UI pixels.
const BAR_H: f32 = 14.0;

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

/// Preloaded audio handles for damage/destroy SFX. Built eagerly via
/// [`FromWorld`] (mirroring `audio.rs::AudioHandles`) so the handles exist
/// before any `Update` system fires them.
#[derive(Resource)]
struct DamageAudioHandles {
    /// Reused `hit.wav` played at reduced volume on every damaging hit.
    thud: Handle<AudioSource>,
    /// Distinct `crash.wav` played once when the car is wrecked.
    crash: Handle<AudioSource>,
}

impl FromWorld for DamageAudioHandles {
    fn from_world(world: &mut World) -> Self {
        let asset_server = world.resource::<AssetServer>();
        DamageAudioHandles {
            thud: asset_server.load("audio/hit.wav"),
            crash: asset_server.load("audio/crash.wav"),
        }
    }
}

// ---------------------------------------------------------------------------
// UI markers
// ---------------------------------------------------------------------------

/// Root node of the health bar (bottom-center, absolute). Despawned on exit
/// from `Playing` and respawned on (re)enter, mirroring `ui.rs::HudRoot`.
#[derive(Component)]
struct HealthBarRoot;

/// The colored fill inside the bar track; its width + color are refreshed
/// each frame by [`update_health_bar`].
#[derive(Component)]
struct HealthBarFill;

/// Dynamic number span showing the live health value.
#[derive(Component)]
struct HealthText;

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
            .init_resource::<DamageAudioHandles>()
            // Fresh-round reset (skipped on resume from Paused). MUST run
            // before `reset_run` flips `RoundActive` on, so it's placed in
            // `SpawnSet` (which `reset_run` follows via `.after(SpawnSet)`).
            .add_systems(OnEnter(GameState::Playing), reset_health.in_set(SpawnSet))
            // UI lifecycle tied to the Playing state (despawned on exit so a
            // pause/resume cycle respawns it cleanly, like the HUD).
            .add_systems(OnEnter(GameState::Playing), spawn_health_bar)
            .add_systems(OnExit(GameState::Playing), despawn_marker::<HealthBarRoot>)
            .add_systems(
                Update,
                (apply_damage, update_health_bar, update_damage_flash)
                    .run_if(in_state(GameState::Playing)),
            )
            // Color refresh runs in every state so the bar recolors even while
            // paused (health can't change then, but the query is trivial).
            .add_systems(Update, update_health_bar_color);
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

/// Convert [`ObstacleHit`] impacts into health loss. On any damage, play a
/// low-volume thud and flash the vignette. When health hits zero, stop the
/// car, play the crash SFX, and transition to [`GameState::GameOver`].
///
/// `ObstacleHit` is already registered via `game/mod.rs` — not re-registered.
fn apply_damage(
    mut hits: MessageReader<ObstacleHit>,
    mut health: ResMut<Health>,
    mut car: Query<&mut Car>,
    mut next: ResMut<NextState<GameState>>,
    mut reason: ResMut<crate::game::resources::GameOverReason>,
    mut commands: Commands,
    audio: Res<DamageAudioHandles>,
    modifier: Res<ActiveModifier>,
    settings: Res<Settings>,
) {
    let mut damaged_this_frame = false;
    for hit in hits.read() {
        let dmg = obstacle_damage(hit.impact_speed, &modifier);
        if dmg <= 0.0 {
            continue;
        }
        health.0 = (health.0 - dmg).max(0.0);
        damaged_this_frame = true;

        if health.0 <= 0.0 {
            // Wrecked: stop the car, play the crash, end the round.
            if let Ok(mut c) = car.single_mut() {
                c.speed = 0.0;
            }
            commands.spawn((
                AudioPlayer::new(audio.crash.clone()),
                PlaybackSettings::DESPAWN.with_volume(Volume::Linear(0.9)),
            ));
            *reason = crate::game::resources::GameOverReason::Wrecked;
            next.set(GameState::GameOver);
            // No point flashing once the GameOver overlay covers the screen.
            return;
        }
    }

    if damaged_this_frame {
        // Thud: reuse hit.wav at a lower volume so repeated scrapes aren't
        // louder than the chicken/coin SFX.
        commands.spawn((
            AudioPlayer::new(audio.thud.clone()),
            PlaybackSettings::DESPAWN.with_volume(Volume::Linear(0.5)),
        ));
        if should_spawn_damage_flash(settings.reduced_motion) {
            spawn_damage_flash(&mut commands);
        }
    }
}

/// Reset health to full on a fresh round. Skipped when resuming from `Paused`
/// (the round is still active), per the fresh-round rule (risk E11).
fn reset_health(mut health: ResMut<Health>, round_active: Res<RoundActive>) {
    if round_active.0 {
        return;
    }
    health.0 = HEALTH_MAX;
}

// ---------------------------------------------------------------------------
// Health bar UI
// ---------------------------------------------------------------------------

/// Spawn the bottom-center health bar panel. Lives only while `Playing`
/// (despawned by [`despawn_marker::<HealthBarRoot>`] on exit).
fn spawn_health_bar(mut commands: Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                bottom: px(12.0),
                left: px(0.0),
                width: Val::Percent(100.0),
                // Center the bar horizontally within the full-width strip.
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                flex_direction: FlexDirection::Column,
                ..default()
            },
            HealthBarRoot,
        ))
        .with_children(|col| {
            // Inner panel so the bar + label sit in a tidy boxed cluster
            // rather than spanning the whole screen width.
            col.spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::all(px(8.0)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.35)),
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
                            width: px(BAR_TRACK_W),
                            height: px(BAR_H),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
                    ))
                    .with_child((
                        Node {
                            width: px(BAR_TRACK_W),
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
                            ..default()
                        },
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

/// Refresh the fill width and the numeric span each frame. (The fill's
/// `BackgroundColor` is updated by [`update_health_bar_color`].)
fn update_health_bar(
    health: Res<Health>,
    mut fill: Query<&mut Node, With<HealthBarFill>>,
    mut text: Query<&mut TextSpan, With<HealthText>>,
) {
    let frac = (health.0 / HEALTH_MAX).clamp(0.0, 1.0);
    for mut node in &mut fill {
        node.width = px(BAR_TRACK_W * frac);
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
    mut q: Query<(Entity, &mut DamageFlash, &mut BackgroundColor)>,
) {
    for (e, mut flash, mut bg) in &mut q {
        flash.t -= time.delta_secs();
        if flash.t <= 0.0 {
            commands.entity(e).despawn();
            continue;
        }
        let frac = (flash.t / FLASH_DURATION).clamp(0.0, 1.0);
        bg.0 = Color::srgba(1.0, 0.0, 0.0, FLASH_PEAK_ALPHA * frac);
    }
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
    use super::{DAMAGE_K, obstacle_damage, should_spawn_damage_flash};
    use crate::modifiers::{ActiveModifier, ModifierKind};

    #[test]
    fn standard_and_glass_cannon_damage_are_exact() {
        let standard = ActiveModifier(ModifierKind::Standard);
        let glass_cannon = ActiveModifier(ModifierKind::GlassCannon);

        assert_eq!(obstacle_damage(12.0, &standard), 12.0 * DAMAGE_K);
        assert_eq!(obstacle_damage(12.0, &glass_cannon), 12.0 * DAMAGE_K * 2.0);
    }

    #[test]
    fn damage_flash_respects_reduced_motion() {
        assert!(should_spawn_damage_flash(false));
        assert!(!should_spawn_damage_flash(true));
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
