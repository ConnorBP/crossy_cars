mod audio;
mod camera;
mod car;
mod game;
mod palette;
mod shaders;
mod textures;
mod ui;
mod world;

use bevy::prelude::*;

use audio::AudioPlugin;
use camera::CameraPlugin;
use car::CarPlugin;
use game::GamePlugin;
use shaders::ShaderPlugin;
use textures::TexturesPlugin;
use ui::UiPlugin;
use world::WorldPlugin;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
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
        ))
        .run();
}
