mod audio;
mod camera;
mod car;
mod chickens;
mod combos;
mod countdown;
mod effects;
mod game;
mod health;
mod minimap;
mod palette;
mod pickups;
mod persist;
mod shaders;
mod textures;
mod ui;
mod world;

use bevy::asset::{AssetMetaCheck, AssetPlugin};
use bevy::prelude::*;

use audio::AudioPlugin;
use camera::CameraPlugin;
use car::CarPlugin;
use chickens::ChickensPlugin;
use combos::CombosPlugin;
use countdown::CountdownPlugin;
use effects::EffectsPlugin;
use game::GamePlugin;
use health::HealthPlugin;
use minimap::MinimapPlugin;
use persist::PersistPlugin;
use pickups::PickupsPlugin;
use shaders::ShaderPlugin;
use textures::TexturesPlugin;
use ui::UiPlugin;
use world::WorldPlugin;

fn main() {
    App::new()
        .add_plugins(
            DefaultPlugins.set(AssetPlugin {
                // On web, trunk's dev server returns index.html for the sidecar
                // `<asset>.meta` 404s, which Bevy then fails to parse as RON.
                // `Never` skips the .meta lookup and uses default meta, so the
                // wav/wgsl assets load cleanly on both native and web.
                meta_check: AssetMetaCheck::Never,
                ..default()
            }),
        )
        .insert_resource(ClearColor(palette::SKY))
        .insert_resource(GlobalAmbientLight {
            color: Color::WHITE,
            // Tuned down for HDR + TonyMcMapface tonemapping (T9). 150.0 was
            // pre-HDR and washed the scene out once bloom/tonemapping landed;
            // the directional sun + IBL now carry the lighting.
            brightness: 40.0,
            ..default()
        })
        .add_plugins((
            GamePlugin,
            CameraPlugin,
            CarPlugin,
            WorldPlugin,
            UiPlugin,
            AudioPlugin,
            ShaderPlugin,
            TexturesPlugin,
            PersistPlugin,
        ))
        .add_plugins((
            ChickensPlugin,
            HealthPlugin,
            MinimapPlugin,
            CountdownPlugin,
            EffectsPlugin,
            CombosPlugin,
            PickupsPlugin,
        ))
        .run();
}
