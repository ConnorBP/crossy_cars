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
use crate::modifiers::{ActiveModifier, ModifierKind};
use crate::palette;
use crate::persist::{ConditionBestsAtRoundStart, Medal, medal_for};
use crate::settings::Settings;

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
    active_modifier: Res<ActiveModifier>,
    condition_bests: Res<ConditionBestsAtRoundStart>,
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
    spawn_countdown_overlay(&mut commands, &active_modifier, &condition_bests);
}

/// Format the active condition's pre-round record and its earned medal.
fn format_best_medal(kind: ModifierKind, best: u32) -> String {
    let medal = match medal_for(kind, best) {
        Medal::None => "NO MEDAL",
        Medal::Bronze => "BRONZE",
        Medal::Silver => "SILVER",
        Medal::Gold => "GOLD",
    };
    format!("BEST {best} · {medal}")
}

/// Spawn the full-screen centered overlay with a big number/word that
/// `tick_countdown` updates each frame. The initial span is "3" so there's
/// no one-frame flash before the first tick fires.
fn spawn_countdown_overlay(
    commands: &mut Commands,
    active_modifier: &ActiveModifier,
    condition_bests: &ConditionBestsAtRoundStart,
) {
    let best = condition_bests.by_kind[active_modifier.0.index()];
    let best_medal = format_best_medal(active_modifier.0, best);
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
            // Light dim so the big number pops without hiding the road.
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.25)),
            CountdownRoot,
        ))
        .with_children(|p| {
            // Announce the freshly selected condition independently of the
            // animated cue so 3-2-1/GO retains its own text and punch.
            p.spawn((
                Text::new("ROAD CONDITION"),
                TextFont {
                    font_size: FontSize::Px(16.0),
                    ..default()
                },
                TextColor(Color::srgba(0.85, 0.85, 0.9, 1.0).into()),
                Node {
                    margin: UiRect::bottom(px(3.0)),
                    ..default()
                },
            ));
            p.spawn((
                Text::new(active_modifier.display_name()),
                TextFont {
                    font_size: FontSize::Px(32.0),
                    ..default()
                },
                TextColor(active_modifier.color()),
                Node {
                    margin: UiRect::bottom(px(3.0)),
                    ..default()
                },
            ));
            p.spawn((
                Text::new(best_medal),
                TextFont {
                    font_size: FontSize::Px(16.0),
                    ..default()
                },
                TextColor(Color::srgba(0.85, 0.85, 0.9, 1.0).into()),
                Node {
                    margin: UiRect::bottom(px(18.0)),
                    ..default()
                },
            ));
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

/// Presentation policy derived from the live accessibility preference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CountdownMotionFlags {
    size_punch: bool,
    color_punch: bool,
}

fn countdown_motion_flags(reduced_motion: bool) -> CountdownMotionFlags {
    CountdownMotionFlags {
        size_punch: !reduced_motion,
        color_punch: !reduced_motion,
    }
}

/// Pure final visual values for the countdown cue. Reduced motion keeps the
/// cue at its readable base size and accent color while cue text and audio
/// continue to transition normally.
#[derive(Clone, Copy, Debug, PartialEq)]
struct CountdownVisualValues {
    font_size: f32,
    red: f32,
    green: f32,
    blue: f32,
}

fn countdown_visual_values(
    flags: CountdownMotionFlags,
    punch_remaining: f32,
) -> CountdownVisualValues {
    let punch = punch_strength(punch_remaining);
    let size_punch = if flags.size_punch { punch } else { 0.0 };
    let color_punch = if flags.color_punch { punch } else { 0.0 };
    CountdownVisualValues {
        font_size: BASE_FONT_SIZE + (PUNCH_FONT_SIZE - BASE_FONT_SIZE) * size_punch,
        red: 1.0,
        green: 0.8 + 0.2 * color_punch,
        blue: 0.35 * color_punch,
    }
}

/// Advance 3 → 2 → 1 → GO, emitting one pitched click and restarting the
/// short text punch on each transition. GO releases gameplay immediately;
/// its overlay remains only long enough to finish the punch.
fn tick_countdown(
    mut commands: Commands,
    time: Res<Time>,
    audio: Res<CountdownAudio>,
    settings: Res<Settings>,
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
    let visual = countdown_visual_values(
        countdown_motion_flags(settings.reduced_motion),
        countdown.punch_remaining,
    );
    for (_, mut font, mut color) in &mut text {
        font.font_size = FontSize::Px(visual.font_size);
        // Normal mode fades from a bright creamy flash to the gold accent;
        // reduced motion holds the accent steady from the next frame onward.
        color.0 = Color::srgb(visual.red, visual.green, visual.blue);
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

    #[test]
    fn reduced_motion_countdown_values_are_static() {
        let flags = countdown_motion_flags(true);
        assert!(!flags.size_punch);
        assert!(!flags.color_punch);

        let transition = countdown_visual_values(flags, PUNCH_DURATION);
        let settled = countdown_visual_values(flags, 0.0);
        assert_eq!(transition, settled);
        assert_eq!(transition.font_size, BASE_FONT_SIZE);
        assert_eq!(
            (transition.red, transition.green, transition.blue),
            (1.0, 0.8, 0.0)
        );
    }

    #[test]
    fn normal_countdown_values_preserve_existing_punch() {
        let flags = countdown_motion_flags(false);
        assert!(flags.size_punch);
        assert!(flags.color_punch);

        let transition = countdown_visual_values(flags, PUNCH_DURATION);
        assert_eq!(transition.font_size, PUNCH_FONT_SIZE);
        assert_eq!(
            (transition.red, transition.green, transition.blue),
            (1.0, 1.0, 0.35)
        );

        let settled = countdown_visual_values(flags, 0.0);
        assert_eq!(settled.font_size, BASE_FONT_SIZE);
        assert_eq!((settled.red, settled.green, settled.blue), (1.0, 0.8, 0.0));
    }

    #[test]
    fn best_medal_format_covers_no_medal_and_all_medals() {
        assert_eq!(
            format_best_medal(ModifierKind::Standard, 19),
            "BEST 19 · NO MEDAL"
        );
        assert_eq!(
            format_best_medal(ModifierKind::Standard, 20),
            "BEST 20 · BRONZE"
        );
        assert_eq!(
            format_best_medal(ModifierKind::Standard, 40),
            "BEST 40 · SILVER"
        );
        assert_eq!(
            format_best_medal(ModifierKind::Standard, 70),
            "BEST 70 · GOLD"
        );
    }

    #[test]
    fn best_medal_format_uses_the_active_conditions_thresholds() {
        assert_eq!(
            format_best_medal(ModifierKind::RushHour, 15),
            "BEST 15 · BRONZE"
        );
        assert_eq!(
            format_best_medal(ModifierKind::ChickenFrenzy, 20),
            "BEST 20 · NO MEDAL"
        );
    }
}
