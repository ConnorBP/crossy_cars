mod camera;
mod car;
mod game;
mod palette;
mod ui;
mod world;

use bevy::prelude::*;

use camera::CameraPlugin;
use car::CarPlugin;
use game::GamePlugin;
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
        .add_plugins((GamePlugin, CameraPlugin, CarPlugin, WorldPlugin, UiPlugin))
        .run();
}
