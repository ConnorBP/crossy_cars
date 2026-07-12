use bevy::audio::{AudioSink, AudioPlayer, AudioSource, PlaybackSettings, Volume};
use bevy::prelude::*;

use crate::car::Car;
use crate::game::events::{ChickenHit, CoinCollected};
use crate::game::state::GameState;

/// Handles for every sound effect plus the looping engine source.
/// Loaded once at startup so gameplay systems can fire them without
/// blocking on the asset server.
#[derive(Resource)]
struct AudioHandles {
    hit: Handle<AudioSource>,
    coin: Handle<AudioSource>,
    click: Handle<AudioSource>,
    engine: Handle<AudioSource>,
}

/// Marker for the single looping engine audio entity so we can find its
/// `AudioSink` each frame and retune pitch/volume to the car's speed.
#[derive(Component)]
struct EngineSound;

pub struct AudioPlugin;

impl Plugin for AudioPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AudioHandles>()
            .add_systems(
                Update,
                (play_hit, play_coin, update_engine.run_if(in_state(GameState::Playing))),
            )
            .add_systems(OnEnter(GameState::Playing), (spawn_engine, play_click))
            .add_systems(OnExit(GameState::Playing), cleanup_engine)
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
        }
    }
}

/// One-shot hit SFX for every chicken strike. `PlaybackSettings::DESPAWN`
/// reclaims the spawned audio entity automatically once the clip finishes.
fn play_hit(
    mut events: MessageReader<ChickenHit>,
    mut commands: Commands,
    handles: Res<AudioHandles>,
) {
    for _ in events.read() {
        commands.spawn((
            AudioPlayer::new(handles.hit.clone()),
            PlaybackSettings::DESPAWN,
        ));
    }
}

/// One-shot coin pickup SFX for every `CoinCollected` event.
fn play_coin(
    mut events: MessageReader<CoinCollected>,
    mut commands: Commands,
    handles: Res<AudioHandles>,
) {
    for _ in events.read() {
        commands.spawn((
            AudioPlayer::new(handles.coin.clone()),
            PlaybackSettings::DESPAWN,
        ));
    }
}

/// Short UI click on state transitions into Playing (and Menu). On web this
/// also fires right after the user's Enter/Space at the menu, unlocking the
/// suspended `AudioContext` so subsequent sounds can play.
fn play_click(mut commands: Commands, handles: Res<AudioHandles>) {
    commands.spawn((
        AudioPlayer::new(handles.click.clone()),
        // UI/menu click — kept quiet (it was playing at default max volume,
        // which was jarring on the startup menu).
        PlaybackSettings::DESPAWN.with_volume(Volume::Linear(0.25)),
    ));
}

/// Spawn the looping engine source when entering Playing. Pitch/volume are
/// driven each frame by `update_engine`; `cleanup_engine` stops it on exit.
fn spawn_engine(mut commands: Commands, handles: Res<AudioHandles>) {
    commands.spawn((
        AudioPlayer::new(handles.engine.clone()),
        PlaybackSettings::LOOP,
        EngineSound,
    ));
}

/// Stop the engine whenever we leave Playing (-> Paused / GameOver / Menu).
/// It respawns fresh on the next `OnEnter(Playing)` via `spawn_engine`.
fn cleanup_engine(mut commands: Commands, engine: Query<Entity, With<EngineSound>>) {
    for entity in &engine {
        commands.entity(entity).despawn();
    }
}

/// Drive the engine pitch and volume from the car's speed. Faster => higher
/// pitch (playback rate) and louder. `&Car` and `&mut AudioSink` touch
/// disjoint components, so the two queries don't conflict (B0001-safe).
fn update_engine(
    car: Query<&Car>,
    mut engine: Query<&mut AudioSink, With<EngineSound>>,
) {
    let Ok(car) = car.single() else {
        return;
    };
    // Normalize speed against the gameplay max (12.0) into a 0..1 ratio.
    let ratio = (car.speed.abs() / 12.0).clamp(0.0, 1.0);

    for mut sink in &mut engine {
        // Playback rate: 1.0 (idle) up to 2.5 (full speed).
        sink.set_speed(1.0 + ratio * 1.5);
        // Volume: gentle idle bed + speed swell.
        sink.set_volume(Volume::Linear(0.25 + ratio * 0.4));
    }
}
