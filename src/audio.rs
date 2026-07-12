use bevy::audio::{AudioSink, AudioPlayer, AudioSource, PlaybackSettings, Volume};
use bevy::prelude::*;

use crate::car::Car;
use crate::game::events::{ChickenHit, CoinCollected};
use crate::game::resources::GameConfig;
use crate::game::state::GameState;

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

/// Marker for the single looping ambient wind/hum entity, cleaned up on exit
/// from Playing.
#[derive(Component)]
struct AmbientSound;

pub struct AudioPlugin;

impl Plugin for AudioPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AudioHandles>()
            .add_systems(
                Update,
                (play_hit, play_coin, update_engine.run_if(in_state(GameState::Playing))),
            )
            .add_systems(
                OnEnter(GameState::Playing),
                (spawn_engine, spawn_ambient, play_click),
            )
            .add_systems(OnExit(GameState::Playing), (cleanup_engine, cleanup_ambient))
            .add_systems(OnEnter(GameState::Menu), play_click);
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
        }
    }
}

/// One-shot hit SFX for every chicken strike. `PlaybackSettings::DESPAWN`
/// reclaims the spawned audio entity automatically once the clip finishes.
/// Kept below unity so repeated strikes aren't jarring next to the coin/click
/// SFX and the thud (health.rs, 0.5).
fn play_hit(
    mut events: MessageReader<ChickenHit>,
    mut commands: Commands,
    handles: Res<AudioHandles>,
) {
    for _ in events.read() {
        commands.spawn((
            AudioPlayer::new(handles.hit.clone()),
            PlaybackSettings::DESPAWN.with_volume(Volume::Linear(0.6)),
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
    ));
}

/// Spawn the looping engine source when entering Playing. Pitch/volume are
/// driven each frame by `update_engine`; `cleanup_engine` stops it on exit.
/// The `EngineSound` starts at idle (speed 0) values so there's no initial
/// pop — `update_engine` lerps from there as the car speeds up.
fn spawn_engine(mut commands: Commands, handles: Res<AudioHandles>) {
    commands.spawn((
        AudioPlayer::new(handles.engine.clone()),
        PlaybackSettings::LOOP,
        EngineSound {
            smooth_rate: ENGINE_IDLE_RATE,
            smooth_vol: ENGINE_IDLE_VOL,
        },
    ));
}

/// Spawn the looping ambient wind/hum bed when entering Playing. Very low
/// volume so it sits under the engine without competing for attention.
/// `cleanup_ambient` stops it on exit.
fn spawn_ambient(mut commands: Commands, handles: Res<AudioHandles>) {
    commands.spawn((
        AudioPlayer::new(handles.ambient.clone()),
        PlaybackSettings::LOOP.with_volume(Volume::Linear(0.12)),
        AmbientSound,
    ));
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
        sink.set_volume(Volume::Linear(eng.smooth_vol));
    }
}
