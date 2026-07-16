use bevy::audio::{
    AudioPlayer, AudioSink, AudioSinkPlayback, AudioSource, GlobalVolume, PlaybackSettings,
    SpatialAudioSink, Volume,
};
use bevy::prelude::*;

use crate::car::{Car, DriftLatch, DrivingSet, PlayerInput};
use crate::critters::CritterHit;
use crate::game::events::{ChickenHit, CoinCollected};
use crate::game::resources::GameConfig;
use crate::game::state::GameState;
use crate::objectives::ObjectiveCompleted;
use crate::settings::Settings;

/// Handles for every sound effect plus the looping engine + ambient sources.
/// Loaded once at startup so gameplay systems can fire them without
/// blocking on the asset server.
#[derive(Resource)]
struct AudioHandles {
    hit: Handle<AudioSource>,
    coin: Handle<AudioSource>,
    click: Handle<AudioSource>,
    engine: Handle<AudioSource>,
    ambient: Handle<AudioSource>,
    positive: Handle<AudioSource>,
    penalty: Handle<AudioSource>,
    tire_scrub: Handle<AudioSource>,
}

/// Marker + smoothed state for the single looping engine audio entity, so we
/// can find its `AudioSink` each frame and retune pitch/volume to the car's
/// speed. The `smooth_*` fields lag the speed-driven targets via an
/// exponential lerp so the pitch/volume never jump abruptly.
#[derive(Component)]
struct EngineSound {
    /// Smoothed playback rate (lags the speed-driven target).
    smooth_rate: f32,
    /// Smoothed linear volume (lags the speed-driven target).
    smooth_vol: f32,
}

/// Optional ambient bed policy. The shipped noise-heavy loop is retained for
/// future redesign/debugging but disabled by default because it reads as
/// persistent static during play.
const ENABLE_AMBIENT_BED: bool = false;

/// Marker for an opt-in ambient loop, cleaned up on exit from Playing.
#[derive(Component)]
struct AmbientSound;

#[derive(Component)]
struct TireScrubSound;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum TireScrubKind {
    #[default]
    None,
    HardTurn,
    Drift,
}

#[derive(Resource, Default, Debug, PartialEq)]
struct TireScrubState {
    active: TireScrubKind,
    cooldown: f32,
}

/// Unscaled local gain retained so live master changes do not compound.
/// Constructible crate-wide so any audio-emitting module can attach it to a
/// spawned one-shot and have the live master bridge scale it without
/// compounding.
#[derive(Component, Clone, Copy)]
pub(crate) struct AudioBaseGain(pub(crate) f32);

pub struct AudioPlugin;

impl Plugin for AudioPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AudioHandles>()
            .init_resource::<TireScrubState>()
            .add_systems(
                Update,
                (
                    play_hit,
                    play_coin,
                    play_positive,
                    play_penalty,
                    // Settings is the single source of truth. M writes it and
                    // this bridge applies it before dynamic engine updates.
                    (toggle_mute, apply_audio_settings, sync_new_audio_sinks)
                        .chain()
                        .before(update_engine),
                    update_engine.run_if(in_state(GameState::Playing)),
                    update_tire_scrub
                        .after(DrivingSet)
                        .run_if(in_state(GameState::Playing)),
                ),
            )
            .add_systems(
                OnEnter(GameState::Playing),
                (spawn_continuous_audio, play_click),
            )
            .add_systems(
                OnExit(GameState::Playing),
                (cleanup_engine, cleanup_ambient, cleanup_tire_scrub),
            )
            .add_systems(OnEnter(GameState::Menu), play_click);
    }
}

/// Preserve the existing global M shortcut, but write the shared Settings
/// resource. SettingsPlugin observes the change and persists the full schema.
fn toggle_mute(keys: Res<ButtonInput<KeyCode>>, mut settings: ResMut<Settings>) {
    if keys.just_pressed(KeyCode::KeyM) {
        settings.muted = !settings.muted;
    }
}

/// Apply master/mute changes live. GlobalVolume covers future playback, while
/// sinks already created by Bevy are updated explicitly.
fn apply_audio_settings(
    settings: Res<Settings>,
    mut global_volume: ResMut<GlobalVolume>,
    mut sinks: Query<(&mut AudioSink, Option<&AudioBaseGain>)>,
    mut spatial_sinks: Query<&mut SpatialAudioSink>,
) {
    if !settings.is_changed() {
        return;
    }
    apply_global_volume(&settings, &mut global_volume);
    for (mut sink, base) in &mut sinks {
        // Sources owned here retain their authored gain. Unknown sources still
        // receive the master setting rather than being left at stale volume.
        let base_gain = base.map_or(1.0, |base| base.0);
        sink.set_volume(Volume::Linear(base_gain * settings.master_gain()));
        apply_sink_mute(&mut *sink, settings.muted);
    }
    for mut sink in &mut spatial_sinks {
        sink.set_volume(Volume::Linear(settings.master_gain()));
        apply_sink_mute(&mut *sink, settings.muted);
    }
}

/// Sinks arrive asynchronously after AudioPlayer entities are spawned, so a
/// newly inserted sink receives current settings even on an unchanged frame.
fn sync_new_audio_sinks(
    settings: Res<Settings>,
    mut sinks: Query<(&mut AudioSink, Option<&AudioBaseGain>), Added<AudioSink>>,
    mut spatial_sinks: Query<&mut SpatialAudioSink, Added<SpatialAudioSink>>,
) {
    for (mut sink, base) in &mut sinks {
        let base_gain = base.map_or(1.0, |base| base.0);
        sink.set_volume(Volume::Linear(base_gain * settings.master_gain()));
        apply_sink_mute(&mut *sink, settings.muted);
    }
    for mut sink in &mut spatial_sinks {
        sink.set_volume(Volume::Linear(settings.master_gain()));
        apply_sink_mute(&mut *sink, settings.muted);
    }
}

fn apply_global_volume(settings: &Settings, global_volume: &mut GlobalVolume) {
    global_volume.volume = if settings.muted {
        Volume::SILENT
    } else {
        Volume::Linear(settings.master_gain())
    };
}

fn apply_sink_mute(sink: &mut impl AudioSinkPlayback, muted: bool) {
    if muted {
        sink.mute();
    } else {
        sink.unmute();
    }
}

/// Preload all wav handles. Built eagerly via `FromWorld` (at app build time)
/// so the handles exist before any `Update` system (e.g. `play_hit`) runs —
/// avoiding a "Resource does not exist" panic from a deferred `insert_resource`.
impl FromWorld for AudioHandles {
    fn from_world(world: &mut World) -> Self {
        let asset_server = world.resource::<AssetServer>();
        AudioHandles {
            hit: asset_server.load("audio/hit.wav"),
            coin: asset_server.load("audio/coin.wav"),
            click: asset_server.load("audio/click.wav"),
            engine: asset_server.load("audio/engine.wav"),
            ambient: asset_server.load("audio/ambient.wav"),
            positive: asset_server.load("audio/positive.wav"),
            penalty: asset_server.load("audio/penalty.wav"),
            tire_scrub: asset_server.load("audio/tire_scrub.wav"),
        }
    }
}

// --- One-shot cue gain + pitch constants --------------------------------
//
// Authored linear gains for the gameplay SFX one-shots. Each is routed
// through `bounded_cue_gain` so a mistuned constant can never blow past unity
// or emit non-finite volume, and each spawned entity carries the same value
// as its `AudioBaseGain` so the live master bridge scales it without
// compounding. The chicken strike is pitched slightly upward (`with_speed`
// > 1) to read as a positive, rewarding hit — the same pitch API the engine
// loop uses via `AudioSink::set_speed`.
const CHICKEN_HIT_VOLUME: f32 = 0.6;
const CHICKEN_HIT_PITCH: f32 = 1.1;
const POSITIVE_CUE_VOLUME: f32 = 0.55;
const PENALTY_CUE_VOLUME: f32 = 0.5;
const HARD_TURN_VOLUME: f32 = 0.24;
const HARD_TURN_PITCH: f32 = 1.10;
const DRIFT_VOLUME: f32 = 0.32;
const DRIFT_PITCH: f32 = 0.92;
const TIRE_SCRUB_COOLDOWN: f32 = 0.55;

/// Clamp an authored cue gain to the safe linear range [0, 1] and replace
/// non-finite values with silence. Bounds every one-shot cue so the master
/// bus is protected even if a constant is mistuned.
fn bounded_cue_gain(raw: f32) -> f32 {
    if raw.is_finite() {
        raw.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// One-shot hit SFX for every chicken strike. `PlaybackSettings::DESPAWN`
/// reclaims the spawned audio entity automatically once the clip finishes.
/// Kept below unity so repeated strikes aren't jarring next to the coin/click
/// SFX and the thud (health.rs, 0.5). Pitched slightly upward (`with_speed`
/// > 1) so the strike reads as a positive, rewarding hit — the same pitch API
/// the engine loop uses via `AudioSink::set_speed`.
fn play_hit(
    mut events: MessageReader<ChickenHit>,
    mut commands: Commands,
    handles: Res<AudioHandles>,
) {
    for _ in events.read() {
        let gain = bounded_cue_gain(CHICKEN_HIT_VOLUME);
        commands.spawn((
            AudioPlayer::new(handles.hit.clone()),
            PlaybackSettings::DESPAWN
                .with_volume(Volume::Linear(gain))
                .with_speed(CHICKEN_HIT_PITCH),
            AudioBaseGain(gain),
        ));
    }
}

/// One-shot coin pickup SFX for every `CoinCollected` event. Kept pleasant —
/// present but not piercing, below the hit so pickups feel rewarding rather
/// than startling.
fn play_coin(
    mut events: MessageReader<CoinCollected>,
    mut commands: Commands,
    handles: Res<AudioHandles>,
) {
    for _ in events.read() {
        commands.spawn((
            AudioPlayer::new(handles.coin.clone()),
            PlaybackSettings::DESPAWN.with_volume(Volume::Linear(0.5)),
            AudioBaseGain(0.5),
        ));
    }
}

/// One-shot positive cue for every `ObjectiveCompleted` event — a brief
/// rewarding sting when the round's bonus objective is fulfilled. Like the
/// other gameplay SFX this is a bounded `DESPAWN` one-shot carrying its
/// authored `AudioBaseGain` so the live master bridge scales it without
/// compounding. Sits just above the coin pickup so completion feels earned.
/// This `MessageReader` is independent; consuming the message here does not
/// affect the objectives system's own completion reader.
fn play_positive(
    mut events: MessageReader<ObjectiveCompleted>,
    mut commands: Commands,
    handles: Res<AudioHandles>,
) {
    for _ in events.read() {
        let gain = bounded_cue_gain(POSITIVE_CUE_VOLUME);
        commands.spawn((
            AudioPlayer::new(handles.positive.clone()),
            PlaybackSettings::DESPAWN.with_volume(Volume::Linear(gain)),
            AudioBaseGain(gain),
        ));
    }
}

/// One-shot penalty cue for every `CritterHit` event — a negative sting
/// layered over the physical impact thud (critters.rs). Bounded `DESPAWN`
/// one-shot with `AudioBaseGain` so master volume changes apply cleanly.
/// This `MessageReader` is independent; consuming the message here does not
/// affect any other `CritterHit` reader.
fn play_penalty(
    mut events: MessageReader<CritterHit>,
    mut commands: Commands,
    handles: Res<AudioHandles>,
) {
    for _ in events.read() {
        let gain = bounded_cue_gain(PENALTY_CUE_VOLUME);
        commands.spawn((
            AudioPlayer::new(handles.penalty.clone()),
            PlaybackSettings::DESPAWN.with_volume(Volume::Linear(gain)),
            AudioBaseGain(gain),
        ));
    }
}

/// Short UI click on state transitions into Playing (and Menu). On web this
/// also fires right after the user's Enter/Space at the menu, unlocking the
/// suspended `AudioContext` so subsequent sounds can play.
fn play_click(mut commands: Commands, handles: Res<AudioHandles>) {
    commands.spawn((
        AudioPlayer::new(handles.click.clone()),
        // UI/menu click — kept quiet (soft short sample + low volume).
        PlaybackSettings::DESPAWN.with_volume(Volume::Linear(0.15)),
        AudioBaseGain(0.15),
    ));
}

/// Spawn the looping engine source when entering Playing. Pitch/volume are
/// driven each frame by `update_engine`; `cleanup_engine` stops it on exit.
/// The `EngineSound` starts at idle (speed 0) values so there's no initial
/// pop — `update_engine` lerps from there as the car speeds up.
fn spawn_continuous_audio(
    mut commands: Commands,
    handles: Res<AudioHandles>,
    engine: Query<(), With<EngineSound>>,
    ambient: Query<(), With<AmbientSound>>,
) {
    // State transitions normally clean these up, but idempotence prevents a
    // duplicate loop if Playing is re-entered through an unusual lifecycle.
    if engine.is_empty() {
        commands.spawn((
            AudioPlayer::new(handles.engine.clone()),
            PlaybackSettings::LOOP,
            EngineSound {
                smooth_rate: ENGINE_IDLE_RATE,
                smooth_vol: ENGINE_IDLE_VOL,
            },
            AudioBaseGain(ENGINE_IDLE_VOL),
        ));
    }
    if ENABLE_AMBIENT_BED && ambient.is_empty() {
        commands.spawn((
            AudioPlayer::new(handles.ambient.clone()),
            PlaybackSettings::LOOP.with_volume(Volume::Linear(AMBIENT_VOLUME)),
            AmbientSound,
            AudioBaseGain(AMBIENT_VOLUME),
        ));
    }
}

/// Stop the engine whenever we leave Playing (-> Paused / GameOver / Menu).
/// It respawns fresh on the next `OnEnter(Playing)` via `spawn_engine`.
fn cleanup_engine(mut commands: Commands, engine: Query<Entity, With<EngineSound>>) {
    for entity in &engine {
        commands.entity(entity).despawn();
    }
}

/// Stop the ambient bed whenever we leave Playing.
fn cleanup_ambient(mut commands: Commands, ambient: Query<Entity, With<AmbientSound>>) {
    for entity in &ambient {
        commands.entity(entity).despawn();
    }
}

fn cleanup_tire_scrub(
    mut commands: Commands,
    cues: Query<Entity, With<TireScrubSound>>,
    mut state: ResMut<TireScrubState>,
) {
    for entity in &cues {
        commands.entity(entity).despawn();
    }
    *state = TireScrubState::default();
}

fn tire_scrub_kind(
    previous: TireScrubKind,
    speed: f32,
    steer: f32,
    slip: f32,
    drift_latched: bool,
    max_speed: f32,
    turn_rate: f32,
) -> TireScrubKind {
    if ![speed, steer, slip, max_speed, turn_rate]
        .into_iter()
        .all(f32::is_finite)
        || max_speed <= 0.0
        || turn_rate < 0.0
    {
        return TireScrubKind::None;
    }
    let speed_abs = speed.abs();
    let yaw_rate = steer.abs().clamp(0.0, 1.0) * turn_rate * speed_abs / max_speed;
    let drift = match previous {
        TireScrubKind::Drift => drift_latched && speed > 1.0 && slip.abs() > 0.025,
        _ => drift_latched && speed > 1.25 && slip.abs() >= 0.06,
    };
    if drift {
        return TireScrubKind::Drift;
    }
    let hard_turn = match previous {
        TireScrubKind::HardTurn => speed_abs > 3.5 && yaw_rate > 0.9,
        _ => speed_abs > 4.0 && yaw_rate > 1.2,
    };
    if hard_turn {
        TireScrubKind::HardTurn
    } else {
        TireScrubKind::None
    }
}

fn update_tire_scrub(
    mut commands: Commands,
    handles: Res<AudioHandles>,
    car: Query<(&Car, &DriftLatch)>,
    input: Res<PlayerInput>,
    config: Res<GameConfig>,
    time: Res<Time>,
    mut state: ResMut<TireScrubState>,
) {
    state.cooldown = (state.cooldown - time.delta_secs().min(0.1)).max(0.0);
    let Ok((car, latch)) = car.single() else {
        state.active = TireScrubKind::None;
        return;
    };
    let next = tire_scrub_kind(
        state.active,
        car.speed,
        input.steer,
        car.drift,
        latch.is_latched(),
        config.max_speed,
        config.turn_rate,
    );
    state.active = next;
    if next == TireScrubKind::None || state.cooldown > 0.0 {
        return;
    }
    let (gain, pitch) = match next {
        TireScrubKind::HardTurn => (HARD_TURN_VOLUME, HARD_TURN_PITCH),
        TireScrubKind::Drift => (DRIFT_VOLUME, DRIFT_PITCH),
        TireScrubKind::None => unreachable!(),
    };
    let gain = bounded_cue_gain(gain);
    commands.spawn((
        AudioPlayer::new(handles.tire_scrub.clone()),
        PlaybackSettings::DESPAWN
            .with_volume(Volume::Linear(gain))
            .with_speed(pitch),
        AudioBaseGain(gain),
        TireScrubSound,
    ));
    state.cooldown = TIRE_SCRUB_COOLDOWN;
}

// --- Engine curve constants ----------------------------------------------
//
// A believable engine curve: idle rumble at speed 0, rising pitch + a slight
// volume swell toward full speed. The playback rate maps the speed ratio
// (0..1) to ENGINE_IDLE_RATE..ENGINE_MAX_RATE, and the volume maps it to
// ENGINE_IDLE_VOL..ENGINE_MAX_VOL. Both are smoothed (exponential lerp) so
// there are no sudden jumps even if speed changes abruptly.
//
// Pitch range 0.8..1.8: at idle the loop plays a little slow (low rumble),
// at full speed it plays 1.8x (winding out). Volume stays gentle (0.18..0.42)
// so the engine never drowns the SFX.
const ENGINE_IDLE_RATE: f32 = 0.8;
const ENGINE_MAX_RATE: f32 = 1.8;
const ENGINE_IDLE_VOL: f32 = 0.18;
const ENGINE_MAX_VOL: f32 = 0.42;
const AMBIENT_VOLUME: f32 = 0.12;
/// Exponential-lerp time constant for smoothing (seconds). Larger = sluggish,
/// smaller = snappy. ~0.12s tracks the car's eased speed without lagging or
/// popping.
const ENGINE_SMOOTH_TAU: f32 = 0.12;

/// Drive the engine pitch and volume from the car's speed. Faster => higher
/// pitch (playback rate) and slightly louder. The speed ratio is mapped
/// through a believable engine curve and both pitch + volume are
/// exponentially smoothed so they never jump. `&Car` and `&mut AudioSink`
/// touch disjoint components, so the two queries don't conflict (B0001-safe).
fn update_engine(
    car: Query<&Car>,
    cfg: Res<GameConfig>,
    time: Res<Time>,
    settings: Res<Settings>,
    mut engine: Query<(&mut AudioSink, &mut EngineSound)>,
) {
    let Ok(car) = car.single() else {
        return;
    };
    // Normalize speed against the gameplay max into a 0..1 ratio. Use abs() so
    // reversing winds the engine up too (the pitch tracks magnitude, not sign).
    let ratio = (car.speed.abs() / cfg.max_speed).clamp(0.0, 1.0);

    // Speed-driven targets: idle -> max across the believable engine curve.
    let target_rate = ENGINE_IDLE_RATE + ratio * (ENGINE_MAX_RATE - ENGINE_IDLE_RATE);
    let target_vol = ENGINE_IDLE_VOL + ratio * (ENGINE_MAX_VOL - ENGINE_IDLE_VOL);

    // Exponential-lerp smoothing factor for this frame. Clamp dt so a long
    // frame (e.g. after a pause / debug breakpoint) doesn't snap instantly.
    let dt = time.delta_secs().min(0.05);
    let alpha = 1.0 - (-dt / ENGINE_SMOOTH_TAU).exp();

    for (mut sink, mut eng) in &mut engine {
        // Ease the smoothed values toward this frame's targets.
        eng.smooth_rate += (target_rate - eng.smooth_rate) * alpha;
        eng.smooth_vol += (target_vol - eng.smooth_vol) * alpha;
        // `set_speed` takes `&self`, `set_volume` takes `&mut self` — the
        // `&mut AudioSink` satisfies both.
        sink.set_speed(eng.smooth_rate);
        // GlobalVolume is captured when playback starts, while this local
        // volume is rewritten each frame. Include master here so the dynamic
        // engine loop continues to respect the configured master gain.
        sink.set_volume(Volume::Linear(eng.smooth_vol * settings.master_gain()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_continuous_audio_policy_disables_noise_heavy_ambient_bed() {
        assert!(!ENABLE_AMBIENT_BED);
        assert!((ENGINE_IDLE_VOL..=ENGINE_MAX_VOL).contains(&ENGINE_IDLE_VOL));
        assert!(
            AMBIENT_VOLUME > 0.0,
            "asset remains available for future opt-in"
        );
    }

    #[test]
    fn continuous_audio_spawn_is_idempotent_and_engine_only_by_default() {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        .init_asset::<AudioSource>()
        .init_resource::<AudioHandles>()
        .add_systems(Update, spawn_continuous_audio);

        app.update();
        app.update();
        let world = app.world_mut();
        assert_eq!(world.query::<&EngineSound>().iter(world).count(), 1);
        assert_eq!(world.query::<&AmbientSound>().iter(world).count(), 0);
    }

    #[test]
    fn cleanup_removes_all_continuous_loop_markers() {
        let mut app = App::new();
        app.add_systems(Update, (cleanup_engine, cleanup_ambient));
        app.world_mut().spawn(EngineSound {
            smooth_rate: ENGINE_IDLE_RATE,
            smooth_vol: ENGINE_IDLE_VOL,
        });
        app.world_mut().spawn(AmbientSound);
        app.update();
        let world = app.world_mut();
        assert_eq!(world.query::<&EngineSound>().iter(world).count(), 0);
        assert_eq!(world.query::<&AmbientSound>().iter(world).count(), 0);
    }

    #[test]
    fn tire_scrub_policy_prioritizes_drift_and_uses_hysteresis() {
        assert_eq!(
            tire_scrub_kind(TireScrubKind::None, 8.0, 1.0, 0.0, false, 12.0, 2.5),
            TireScrubKind::HardTurn
        );
        assert_eq!(
            tire_scrub_kind(TireScrubKind::None, 8.0, 1.0, 0.08, true, 12.0, 2.5),
            TireScrubKind::Drift
        );
        assert_eq!(
            tire_scrub_kind(TireScrubKind::Drift, 5.0, 0.0, 0.03, true, 12.0, 2.5),
            TireScrubKind::Drift,
            "latched centered steering should retain an audible slide"
        );
        assert_eq!(
            tire_scrub_kind(TireScrubKind::Drift, 8.0, 1.0, 0.08, false, 12.0, 2.5),
            TireScrubKind::HardTurn
        );
        assert_eq!(
            tire_scrub_kind(TireScrubKind::HardTurn, 3.9, 1.0, 0.0, false, 5.0, 2.5),
            TireScrubKind::HardTurn
        );
    }

    #[test]
    fn tire_scrub_policy_rejects_boundaries_reverse_drift_and_nonfinite() {
        assert_eq!(
            tire_scrub_kind(TireScrubKind::None, 4.0, 1.0, 0.0, false, 6.0, 2.0),
            TireScrubKind::None
        );
        assert_eq!(
            tire_scrub_kind(TireScrubKind::None, -8.0, 1.0, 0.08, true, 12.0, 2.5),
            TireScrubKind::HardTurn,
            "reverse may squeal during a hard turn but cannot drift"
        );
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(
                tire_scrub_kind(TireScrubKind::Drift, value, 1.0, 0.1, true, 12.0, 2.5),
                TireScrubKind::None
            );
        }
    }

    #[test]
    fn tire_scrub_cleanup_resets_state_and_removes_cues() {
        let mut app = App::new();
        app.init_resource::<TireScrubState>()
            .add_systems(Update, cleanup_tire_scrub);
        app.world_mut().resource_mut::<TireScrubState>().active = TireScrubKind::Drift;
        app.world_mut().resource_mut::<TireScrubState>().cooldown = 0.4;
        app.world_mut().spawn(TireScrubSound);
        app.update();
        assert_eq!(
            *app.world().resource::<TireScrubState>(),
            TireScrubState::default()
        );
        let world = app.world_mut();
        assert_eq!(world.query::<&TireScrubSound>().iter(world).count(), 0);
    }

    #[test]
    fn bounded_cue_gain_clamps_to_unit_and_silences_nonfinite() {
        assert_eq!(bounded_cue_gain(0.0), 0.0);
        assert_eq!(bounded_cue_gain(0.55), 0.55);
        assert_eq!(bounded_cue_gain(1.0), 1.0);
        // Out-of-range authored gains are clamped to the safe bus range.
        assert_eq!(bounded_cue_gain(1.5), 1.0);
        assert_eq!(bounded_cue_gain(-0.3), 0.0);
        // Non-finite constants collapse to silence rather than poisoning the
        // master bus.
        assert_eq!(bounded_cue_gain(f32::NAN), 0.0);
        assert_eq!(bounded_cue_gain(f32::INFINITY), 0.0);
        assert_eq!(bounded_cue_gain(f32::NEG_INFINITY), 0.0);
    }
}
