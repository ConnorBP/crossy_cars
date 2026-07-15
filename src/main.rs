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
mod leaderboard;
mod minimap;
mod modifiers;
mod objectives;
mod palette;
mod persist;
mod pickups;
mod round_intro;
mod run_events;
mod settings;
mod shaders;
mod textures;
mod touch;
mod toy_shading;
mod transparency;
mod ui;
mod world;

use bevy::asset::{AssetMetaCheck, AssetPlugin};
use bevy::prelude::*;

use audio::AudioPlugin;
use camera::{CameraPlugin, CarReviewCameraPlugin, WorldReviewCameraPlugin};
use car::{CarPlugin, CarReviewPlugin};
use chickens::ChickensPlugin;
use combos::CombosPlugin;
use countdown::CountdownPlugin;
use critters::CrittersPlugin;
use difficulty::DifficultyPlugin;
use effects::EffectsPlugin;
use game::GamePlugin;
use health::HealthPlugin;
use leaderboard::LeaderboardPlugin;
use minimap::MinimapPlugin;
use modifiers::ModifiersPlugin;
use objectives::ObjectivesPlugin;
use persist::PersistPlugin;
use pickups::PickupsPlugin;
use round_intro::RoundIntroPlugin;
use run_events::RunEventsPlugin;
use settings::SettingsPlugin;
use shaders::{ShaderPlugin, WaterMaterialPlugin};
use textures::TexturesPlugin;
use touch::TouchPlugin;
use toy_shading::ToyShadingPlugin;
use transparency::TransparencyPlugin;
use ui::UiPlugin;
use world::{WorldPlugin, WorldReviewPlugin};

/// Shared exact query/native-env flag parser. Review modes remain explicit;
/// normal production startup is unchanged.
fn query_flag_requested(query_name: &str, native_env: &str) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = native_env;
        return web_sys::window()
            .and_then(|window| js_sys::Reflect::get(window.as_ref(), &"location".into()).ok())
            .and_then(|location| js_sys::Reflect::get(&location, &"search".into()).ok())
            .and_then(|search| search.as_string())
            .is_some_and(|query| {
                query.trim_start_matches('?').split('&').any(|part| {
                    part.split_once('=').is_some_and(|(name, value)| {
                        name == query_name && matches!(value, "1" | "true")
                    })
                })
            });
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = query_name;
        std::env::var(native_env).is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE"))
    }
}

/// URL `?world_review=1` on WASM, or native `ROADY_WORLD_REVIEW=1`.
fn world_review_requested() -> bool {
    query_flag_requested("world_review", "ROADY_WORLD_REVIEW")
}

/// Explicit opt-in only: URL `?car_review=1&car_view=front_left` on WASM,
/// or native `ROADY_CAR_REVIEW=1` plus `ROADY_CAR_REVIEW_VIEW=front_left`.
fn car_review_requested() -> bool {
    query_flag_requested("car_review", "ROADY_CAR_REVIEW")
}

fn main() {
    let world_review = world_review_requested();
    let car_review = car_review_requested();
    let mut app = App::new();
    let defaults = DefaultPlugins
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
                // On Wasm, keep Bevy's logical window and touch
                // coordinates matched to the responsive CSS canvas.
                fit_canvas_to_parent: true,
                ..default()
            }),
            ..default()
        });
    app.add_plugins(defaults)
        .insert_resource(ClearColor(palette::SKY))
        .insert_resource(GlobalAmbientLight {
            color: Color::WHITE,
            // Tuned down for HDR + TonyMcMapface tonemapping (T9). 150.0 was
            // pre-HDR and washed the scene out once bloom/tonemapping landed;
            // the directional sun + IBL now carry the lighting.
            brightness: 40.0,
            ..default()
        });

    if car_review {
        // Controlled visual-review harness: production car rendering only,
        // isolated from the production world, UI, audio and gameplay systems.
        app.insert_resource(ClearColor(Color::srgb(0.12, 0.125, 0.13)))
            .insert_resource(GlobalAmbientLight {
                color: Color::WHITE,
                brightness: 260.0,
                ..default()
            })
            .add_plugins((
                TexturesPlugin,
                ToyShadingPlugin,
                CarReviewPlugin,
                CarReviewCameraPlugin,
            ));
        app.run();
        return;
    }

    if world_review {
        // Smallest robust harness: production world/textures/rendering only.
        // No game state, car, HUD, audio, movement, timers, or recycling.
        app.add_plugins((WaterMaterialPlugin, ShaderPlugin))
            .add_plugins((
                TexturesPlugin,
                ToyShadingPlugin,
                WorldReviewCameraPlugin,
                WorldReviewPlugin,
            ));
        app.run();
        return;
    }

    app
        // Settings is registered separately so its shared resource exists
        // before audio installs the live mixer bridge.
        .add_plugins(SettingsPlugin)
        .add_plugins((
            GamePlugin,
            CameraPlugin,
            CarPlugin,
            WaterMaterialPlugin,
            WorldPlugin,
            UiPlugin,
            AudioPlugin,
            ShaderPlugin,
            TexturesPlugin,
            ToyShadingPlugin,
            PersistPlugin,
        ))
        // Registered separately so neither plugin tuple approaches Bevy's
        // tuple implementation limit as features are added.
        .add_plugins(TransparencyPlugin)
        // Touch controls share the ordered car input sets but remain an
        // independently registered input/UI feature.
        .add_plugins(TouchPlugin)
        // Modifier selection must be registered independently of the feature
        // tuple so adding it cannot approach Bevy's plugin tuple limit.
        .add_plugins(ModifiersPlugin)
        // Bonus objectives own their deterministic round state and HUD, and
        // remain independent of persistence and the main feature tuple.
        .add_plugins(ObjectivesPlugin)
        // Fresh-round mission announcement is presentation-only and ordered
        // after objective selection.
        .add_plugins(RoundIntroPlugin)
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
        // Cloudflare leaderboard web client; degrades to read-only/unavailable
        // on native or when LEADERBOARD_API_URL is not set at build time.
        .add_plugins(LeaderboardPlugin)
        .run();
}
