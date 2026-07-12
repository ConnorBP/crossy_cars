//! "3-2-1-GO" countdown intro shown at the start of each fresh round.
//!
//! While the countdown is active the car is frozen (`InputFrozen`) and the
//! 60s round timer doesn't burn — both `move_car` (in `car.rs`) and
//! `tick_timeleft` (in `game/mod.rs`) early-return while `InputFrozen.0` is
//! true. The countdown only fires on a FRESH round (coming from Menu or
//! GameOver, where `end_round` reset `RoundActive` to false); resuming from
//! `Paused` skips it because `RoundActive` is still true there.

use bevy::{
    audio::{AudioPlayer, AudioSource, PlaybackSettings, Volume},
    prelude::*,
    text::FontSize,
};

use crate::car::InputFrozen;
use crate::game::SpawnSet;
use crate::game::resources::RoundActive;
use crate::game::state::GameState;
use crate::palette;

const PUNCH_DURATION: f32 = 0.2;
const BASE_FONT_SIZE: f32 = 96.0;
const PUNCH_FONT_SIZE: f32 = 128.0;
const BEEP_VOLUME: f32 = 0.45;

/// A cue is recorded in the resource rather than in system-local state so a
/// fresh round and every cleanup can reset transition tracking explicitly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CountdownCue {
    Three,
    Two,
    One,
    Go,
}

impl CountdownCue {
    fn index(self) -> u8 {
        match self {
            Self::Three => 0,
            Self::Two => 1,
            Self::One => 2,
            Self::Go => 3,
        }
    }

    fn from_index(index: u8) -> Option<Self> {
        match index {
            0 => Some(Self::Three),
            1 => Some(Self::Two),
            2 => Some(Self::One),
            3 => Some(Self::Go),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Three => "3",
            Self::Two => "2",
            Self::One => "1",
            Self::Go => "GO!",
        }
    }

    fn speed(self) -> f32 {
        match self {
            Self::Three => 0.8,
            Self::Two => 1.0,
            Self::One => 1.2,
            Self::Go => 1.6,
        }
    }
}

/// Remaining seconds in the countdown (3.0 → 0.0), together with explicit
/// per-round visual/audio transition state. The overlay remains for the short
/// GO punch after `t` reaches zero, while input is released immediately.
#[derive(Resource)]
pub struct Countdown {
    pub t: f32,
    active: bool,
    last_cue: Option<CountdownCue>,
    punch_remaining: f32,
}

impl Default for Countdown {
    fn default() -> Self {
        Self {
            t: 0.0,
            active: false,
            last_cue: None,
            punch_remaining: 0.0,
        }
    }
}

/// Countdown owns its click handle so it does not depend on the resources in
/// `audio.rs`. Bevy's audio mixer still applies `GlobalVolume` automatically.
#[derive(Resource)]
struct CountdownAudio {
    click: Handle<AudioSource>,
}

impl FromWorld for CountdownAudio {
    fn from_world(world: &mut World) -> Self {
        let asset_server = world.resource::<AssetServer>();
        Self {
            click: asset_server.load("audio/click.wav"),
        }
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
            .init_resource::<CountdownAudio>()
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
            .add_systems(OnExit(GameState::Playing), cleanup_countdown)
            // Tick the countdown down each frame while Playing.
            .add_systems(Update, tick_countdown.run_if(in_state(GameState::Playing)));
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
    // Fresh round: arm and explicitly reset every piece of transition state,
    // freeze input, then spawn the overlay.
    countdown.t = 3.0;
    countdown.active = true;
    countdown.last_cue = None;
    countdown.punch_remaining = 0.0;
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
                    font_size: FontSize::Px(BASE_FONT_SIZE),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
            ))
            .with_child((
                TextSpan::new("3"),
                TextFont {
                    font_size: FontSize::Px(BASE_FONT_SIZE),
                    ..default()
                },
                TextColor(palette::HUD_ACCENT.into()),
                CountdownText,
            ));
        });
}

/// Return the cue represented by a remaining-time value. Boundaries belong
/// to the newly-entered cue, so exactly 2.0 means "2" and 0.0 means "GO".
fn cue_for_remaining(t: f32) -> CountdownCue {
    if t > 2.0 {
        CountdownCue::Three
    } else if t > 1.0 {
        CountdownCue::Two
    } else if t > 0.0 {
        CountdownCue::One
    } else {
        CountdownCue::Go
    }
}

/// Find the next not-yet-emitted cue up to `target`. Advancing one cue at a
/// time makes transition handling robust to a long frame crossing multiple
/// boundaries: each of 3, 2, 1, and GO is still emitted at most once.
fn next_cue(last: Option<CountdownCue>, target: CountdownCue) -> Option<CountdownCue> {
    let next_index = last.map_or(0, |cue| cue.index() + 1);
    (next_index <= target.index())
        .then(|| CountdownCue::from_index(next_index))
        .flatten()
}

fn punch_strength(remaining: f32) -> f32 {
    (remaining / PUNCH_DURATION).clamp(0.0, 1.0)
}

/// Advance 3 → 2 → 1 → GO, emitting one pitched click and restarting the
/// short text punch on each transition. GO releases gameplay immediately;
/// its overlay remains only long enough to finish the punch.
fn tick_countdown(
    mut commands: Commands,
    time: Res<Time>,
    audio: Res<CountdownAudio>,
    mut countdown: ResMut<Countdown>,
    mut input_frozen: ResMut<InputFrozen>,
    overlay: Query<Entity, With<CountdownRoot>>,
    mut text: Query<(&mut TextSpan, &mut TextFont, &mut TextColor), With<CountdownText>>,
) {
    // No active countdown — nothing to tick (normal gameplay or resume).
    if !countdown.active {
        return;
    }

    let dt = time.delta_secs();
    countdown.t = (countdown.t - dt).max(0.0);
    let target = cue_for_remaining(countdown.t);
    let mut transitioned = false;

    // Usually this loop runs zero or one time. It can run several times after
    // a long frame, preserving exactly one beep for every crossed boundary.
    while let Some(cue) = next_cue(countdown.last_cue, target) {
        commands.spawn((
            AudioPlayer::new(audio.click.clone()),
            PlaybackSettings::DESPAWN
                .with_speed(cue.speed())
                .with_volume(Volume::Linear(BEEP_VOLUME)),
        ));

        countdown.last_cue = Some(cue);
        countdown.punch_remaining = PUNCH_DURATION;
        transitioned = true;
        for (mut span, _, _) in &mut text {
            **span = cue.label().to_owned();
        }
    }

    // Give a transition a complete punch starting this frame. On subsequent
    // frames it decays to the normal size/accent color over about 0.2s.
    if !transitioned {
        countdown.punch_remaining = (countdown.punch_remaining - dt).max(0.0);
    }
    let punch = punch_strength(countdown.punch_remaining);
    for (_, mut font, mut color) in &mut text {
        font.font_size = FontSize::Px(BASE_FONT_SIZE + (PUNCH_FONT_SIZE - BASE_FONT_SIZE) * punch);
        // Fade from a bright creamy flash back to the normal gold accent.
        color.0 = Color::srgb(1.0, 0.8 + 0.2 * punch, 0.35 * punch);
    }

    if countdown.last_cue == Some(CountdownCue::Go) {
        input_frozen.0 = false;
        if countdown.punch_remaining <= 0.0 {
            countdown.active = false;
            for e in &overlay {
                commands.entity(e).despawn();
            }
        }
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
    countdown.active = false;
    countdown.last_cue = None;
    countdown.punch_remaining = 0.0;
    input_frozen.0 = false;
    for e in &overlay {
        commands.entity(e).despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaining_time_maps_to_all_four_cues() {
        assert_eq!(cue_for_remaining(3.0), CountdownCue::Three);
        assert_eq!(cue_for_remaining(2.0), CountdownCue::Two);
        assert_eq!(cue_for_remaining(1.0), CountdownCue::One);
        assert_eq!(cue_for_remaining(0.0), CountdownCue::Go);
    }

    #[test]
    fn skipped_time_still_walks_each_transition_once() {
        let target = CountdownCue::Go;
        let mut last = None;
        let mut cues = Vec::new();
        while let Some(cue) = next_cue(last, target) {
            cues.push(cue);
            last = Some(cue);
        }
        assert_eq!(
            cues,
            vec![
                CountdownCue::Three,
                CountdownCue::Two,
                CountdownCue::One,
                CountdownCue::Go,
            ]
        );
        assert_eq!(next_cue(last, target), None);
    }

    #[test]
    fn punch_strength_clamps_and_decays() {
        assert_eq!(punch_strength(PUNCH_DURATION), 1.0);
        assert!((punch_strength(PUNCH_DURATION / 2.0) - 0.5).abs() < f32::EPSILON);
        assert_eq!(punch_strength(0.0), 0.0);
        assert_eq!(punch_strength(-1.0), 0.0);
    }
}
