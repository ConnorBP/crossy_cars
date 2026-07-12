mod audio;
mod camera;
mod car;
mod chickens;
mod combos;
mod countdown;
mod critters;
mod difficulty;
mod effects;
mod game;
mod health;
mod minimap;
mod modifiers;
mod palette;
mod persist;
mod pickups;
mod run_events;
mod shaders;
mod textures;
mod transparency;
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
use critters::CrittersPlugin;
use difficulty::DifficultyPlugin;
use effects::EffectsPlugin;
use game::GamePlugin;
use health::HealthPlugin;
use minimap::MinimapPlugin;
use modifiers::ModifiersPlugin;
use persist::PersistPlugin;
use pickups::PickupsPlugin;
use run_events::RunEventsPlugin;
use shaders::ShaderPlugin;
use textures::TexturesPlugin;
use transparency::TransparencyPlugin;
use ui::UiPlugin;
use world::WorldPlugin;

fn main() {
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(AssetPlugin {
                    // On web, trunk's dev server returns index.html for the sidecar
                    // `<asset>.meta` 404s, which Bevy then fails to parse as RON.
                    // `Never` skips the .meta lookup and uses default meta, so the
                    // wav/wgsl assets load cleanly on both native and web.
                    meta_check: AssetMetaCheck::Never,
                    ..default()
                })
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Roady Car".into(),
                        ..default()
                    }),
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
        // Registered separately so neither plugin tuple approaches Bevy's
        // tuple implementation limit as features are added.
        .add_plugins(TransparencyPlugin)
        // Modifier selection must be registered independently of the feature
        // tuple so adding it cannot approach Bevy's plugin tuple limit.
        .add_plugins(ModifiersPlugin)
        // Mid-run event scheduling is independent of the feature tuple for
        // the same tuple-limit reason as modifiers.
        .add_plugins(RunEventsPlugin)
        .add_plugins((
            ChickensPlugin,
            HealthPlugin,
            MinimapPlugin,
            CountdownPlugin,
            EffectsPlugin,
            CombosPlugin,
            PickupsPlugin,
            CrittersPlugin,
            DifficultyPlugin,
        ))
        .run();
}
