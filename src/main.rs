mod audio;
mod camera;
mod car;
mod chickens;
mod countdown;
mod effects;
mod game;
mod health;
mod palette;
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
use countdown::CountdownPlugin;
use effects::EffectsPlugin;
use game::GamePlugin;
use health::HealthPlugin;
use persist::PersistPlugin;
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
            brightness: 150.0,
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
            ChickensPlugin,
            HealthPlugin,
            CountdownPlugin,
            EffectsPlugin,
        ))
        .run();
}
