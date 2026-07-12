//! "3-2-1-GO" countdown intro shown at the start of each fresh round.
//!
//! While the countdown is active the car is frozen (`InputFrozen`) and the
//! 60s round timer doesn't burn — both `move_car` (in `car.rs`) and
//! `tick_timeleft` (in `game/mod.rs`) early-return while `InputFrozen.0` is
//! true. The countdown only fires on a FRESH round (coming from Menu or
//! GameOver, where `end_round` reset `RoundActive` to false); resuming from
//! `Paused` skips it because `RoundActive` is still true there.

use bevy::{prelude::*, text::FontSize};

use crate::car::InputFrozen;
use crate::game::resources::RoundActive;
use crate::game::state::GameState;
use crate::game::SpawnSet;
use crate::palette;

/// Remaining seconds in the countdown (3.0 → 0.0). While `t > 0` the car is
/// frozen and the overlay is visible; when `t` hits 0 the car is released.
#[derive(Resource)]
pub struct Countdown {
    pub t: f32,
}

impl Default for Countdown {
    fn default() -> Self {
        Self { t: 3.0 }
    }
}

/// Marker for the full-screen countdown overlay root node.
#[derive(Component)]
struct CountdownRoot;

/// Marker for the dynamic countdown number/word text span.
#[derive(Component)]
struct CountdownText;

pub struct CountdownPlugin;

impl Plugin for CountdownPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Countdown>()
            // Start (or skip) the countdown on entering Playing. Runs inside
            // SpawnSet so it executes BEFORE reset_run flips RoundActive on
            // (risk E11). The RoundActive.0 check skips resume-from-Paused.
            .add_systems(
                OnEnter(GameState::Playing),
                start_countdown.in_set(SpawnSet),
            )
            // Tear down the overlay + unfreeze input whenever we LEAVE
            // Playing (pause, game-over, back-to-menu). This does NOT fire
            // when resuming from Paused (OnExit(Paused) fires instead), so a
            // stale overlay can't linger and the car can't get stuck frozen
            // if a transition happens mid-countdown.
            .add_systems(
                OnExit(GameState::Playing),
                cleanup_countdown,
            )
            // Tick the countdown down each frame while Playing.
            .add_systems(
                Update,
                tick_countdown.run_if(in_state(GameState::Playing)),
            );
    }
}

/// Begin a fresh countdown on entering Playing — but only on a FRESH round
/// (`RoundActive` is false). On resume from Paused, `RoundActive` is already
/// true, so we bail out (no countdown when unpausing). Runs inside
/// `SpawnSet` so it executes before `reset_run` flips `RoundActive` on.
fn start_countdown(
    mut commands: Commands,
    round_active: Res<RoundActive>,
    mut countdown: ResMut<Countdown>,
    mut input_frozen: ResMut<InputFrozen>,
) {
    // Resume from Paused: round already active -> no countdown.
    if round_active.0 {
        return;
    }
    // Fresh round: arm the countdown, freeze input, spawn the overlay.
    countdown.t = 3.0;
    input_frozen.0 = true;
    spawn_countdown_overlay(&mut commands);
}

/// Spawn the full-screen centered overlay with a big number/word that
/// `tick_countdown` updates each frame. The initial span is "3" so there's
/// no one-frame flash before the first tick fires.
fn spawn_countdown_overlay(commands: &mut Commands) {
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
            // Light dim so the big number pops without hiding the road.
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.25)),
            CountdownRoot,
        ))
        .with_children(|p| {
            p.spawn((
                Text::new(""),
                TextFont {
                    font_size: FontSize::Px(96.0),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
            ))
            .with_child((
                TextSpan::new("3"),
                TextFont {
                    font_size: FontSize::Px(96.0),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
                CountdownText,
            ));
        });
}

/// Decrement the countdown, update the overlay text to "3" / "2" / "GO!",
/// and when it reaches zero release the car (`InputFrozen = false`) and
/// despawn the overlay. Early-returns when no countdown is active so this
/// is a no-op during normal gameplay (after the countdown finishes) and on
/// resume-from-Paused (where `start_countdown` skipped).
///
/// Text mapping (t starts at 3.0 and decrements):
/// - `t` in `(2, 3]` → "3"   (first second)
/// - `t` in `(1, 2]` → "2"   (second second)
/// - `t` in `(0, 1]` → "GO!" (final second, per the task spec)
fn tick_countdown(
    mut commands: Commands,
    time: Res<Time>,
    mut countdown: ResMut<Countdown>,
    mut input_frozen: ResMut<InputFrozen>,
    overlay: Query<Entity, With<CountdownRoot>>,
    mut text: Query<&mut TextSpan, With<CountdownText>>,
) {
    // No active countdown — nothing to tick (normal gameplay or resume).
    if countdown.t <= 0.0 {
        return;
    }
    countdown.t -= time.delta_secs();

    if countdown.t <= 0.0 {
        // Release the car + round timer; remove the overlay.
        countdown.t = 0.0;
        input_frozen.0 = false;
        for e in &overlay {
            commands.entity(e).despawn();
        }
        return;
    }

    // Update the overlay text. `ceil` gives the number of whole seconds
    // remaining: 3 for (2,3], 2 for (1,2]. The final second (0,1] shows
    // "GO!" instead of "1" per the task spec.
    let label = if countdown.t > 1.0 {
        format!("{}", countdown.t.ceil() as i32)
    } else {
        "GO!".to_string()
    };
    for mut span in &mut text {
        **span = label.clone();
    }
}

/// Despawn the overlay and release input when leaving Playing. Ensures no
/// stale overlay lingers and the car is never stuck frozen across a state
/// transition (e.g. pausing mid-countdown cancels it; on resume the player
/// drives immediately — a fresh countdown only starts on a new round).
fn cleanup_countdown(
    mut commands: Commands,
    mut countdown: ResMut<Countdown>,
    mut input_frozen: ResMut<InputFrozen>,
    overlay: Query<Entity, With<CountdownRoot>>,
) {
    countdown.t = 0.0;
    input_frozen.0 = false;
    for e in &overlay {
        commands.entity(e).despawn();
    }
}
