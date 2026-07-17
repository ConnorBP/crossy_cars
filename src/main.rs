mod audio;
mod camera;
mod car;
mod chickens;
mod combos;
mod competitive_v3;
mod countdown;
mod critters;
mod difficulty;
mod drowned;
mod effects;
mod game;
mod game_modes;
mod health;
mod leaderboard;
mod ledger;
mod menu;
mod microtexture_review;
mod minimap;
mod modifiers;
mod objectives;
mod palette;
mod pbr_detail_constants;
mod persist;
mod pickups;
mod readiness;
mod right_of_way;
mod rotation;
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
use camera::{
    CameraPlugin, CarReviewCameraPlugin, PondReviewCameraPlugin, WorldReviewCameraPlugin,
};
use car::{CarPlugin, CarReviewPlugin, PondReviewCarPlugin};
use chickens::ChickensPlugin;
use combos::CombosPlugin;
use competitive_v3::CompetitiveV3Plugin;
use countdown::CountdownPlugin;
use critters::CrittersPlugin;
use difficulty::DifficultyPlugin;
use drowned::DrownedPlugin;
use effects::EffectsPlugin;
use game::GamePlugin;
use game_modes::GameModesPlugin;
use health::HealthPlugin;
use leaderboard::LeaderboardPlugin;
use ledger::LedgerPlugin;
use menu::MenuPlugin;
use microtexture_review::MicrotextureReviewPlugin;
use minimap::MinimapPlugin;
use modifiers::ModifiersPlugin;
use objectives::ObjectivesPlugin;
use persist::PersistPlugin;
use pickups::PickupsPlugin;
use readiness::ReadinessPlugin;
use right_of_way::RightOfWayPlugin;
use rotation::RotationPlugin;
use round_intro::RoundIntroPlugin;
use run_events::RunEventsPlugin;
use settings::SettingsPlugin;
use shaders::{ShaderPlugin, WaterMaterialPlugin, WaterReviewMotion};
use textures::TexturesPlugin;
use touch::TouchPlugin;
use toy_shading::ToyShadingPlugin;
use transparency::TransparencyPlugin;
use ui::UiPlugin;
use world::{PondReviewPlugin, WorldPlugin, WorldReviewPlugin};

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

/// Explicit opt-in only: URL `?pond_review=1` on WASM, or native
/// `ROADY_POND_REVIEW=1`. Pond review takes precedence over broader reviews.
fn pond_review_requested() -> bool {
    query_flag_requested("pond_review", "ROADY_POND_REVIEW")
}

/// Exact opt-in only. A normal URL contains no microtexture review marker.
fn microtexture_review_requested() -> bool {
    query_flag_requested("microtexture_review", "ROADY_MICROTEXTURE_REVIEW")
}

fn pond_review_reduced_motion() -> bool {
    !query_flag_requested("pond_motion", "ROADY_POND_REVIEW_MOTION")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StartupMode {
    Production,
    MicrotextureReview,
    CarReview,
    WorldReview,
    PondReview,
}

pub(crate) fn startup_mode(microtexture: bool, pond: bool, world: bool, car: bool) -> StartupMode {
    if microtexture {
        StartupMode::MicrotextureReview
    } else if pond {
        StartupMode::PondReview
    } else if world {
        StartupMode::WorldReview
    } else if car {
        StartupMode::CarReview
    } else {
        StartupMode::Production
    }
}

fn main() {
    let mode = startup_mode(
        microtexture_review_requested(),
        pond_review_requested(),
        world_review_requested(),
        car_review_requested(),
    );
    let microtexture_review = mode == StartupMode::MicrotextureReview;
    let pond_review = mode == StartupMode::PondReview;
    let world_review = mode == StartupMode::WorldReview;
    let car_review = mode == StartupMode::CarReview;
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

    if microtexture_review {
        // One deterministic A/B tableau, with no game state, gameplay, UI,
        // audio, persistence, leaderboard, or network plugins installed.
        app.insert_resource(ClearColor(Color::srgb(0.095, 0.10, 0.11)))
            .insert_resource(GlobalAmbientLight {
                color: Color::WHITE,
                brightness: 90.0,
                ..default()
            })
            .add_plugins((TexturesPlugin, ToyShadingPlugin, MicrotextureReviewPlugin));
        app.run();
        return;
    }

    if pond_review {
        // Static review-only tableau. Production gameplay, game state, score,
        // persistence, UI, audio and network plugins are intentionally absent.
        app.insert_resource(ClearColor(Color::srgb(0.36, 0.55, 0.68)))
            // Default freezes water for repeatable captures; `pond_motion=1`
            // (or ROADY_POND_REVIEW_MOTION=1) enables normal shader motion.
            .insert_resource(WaterReviewMotion {
                reduced: pond_review_reduced_motion(),
            })
            .add_plugins((WaterMaterialPlugin, ShaderPlugin))
            .add_plugins((
                TexturesPlugin,
                ToyShadingPlugin,
                PondReviewCarPlugin,
                PondReviewCameraPlugin,
                PondReviewPlugin,
            ));
        app.run();
        return;
    }

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
            GameModesPlugin,
            RotationPlugin,
            RightOfWayPlugin,
            LedgerPlugin,
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
        // Pond drowning is local gameplay layered over immutable score rules.
        .add_plugins(DrownedPlugin)
        // Registered separately so neither plugin tuple approaches Bevy's
        // tuple implementation limit as features are added.
        .add_plugins(TransparencyPlugin)
        // Touch controls share the ordered car input sets but remain an
        // independently registered input/UI feature.
        .add_plugins(TouchPlugin)
        // Modifier selection must be registered independently of the feature
        // tuple so adding it cannot approach Bevy's plugin tuple limit.
        .add_plugins(ModifiersPlugin)
        // Responsive presentation consumes only authoritative game resources.
        .add_plugins(MenuPlugin)
        // Publish a truthful DOM-ready signal only after the initial menu,
        // world, camera, and imported player car are actually usable.
        .add_plugins(ReadinessPlugin)
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
        // Frozen legacy v1 reads/submission remain isolated in this plugin.
        .add_plugins(LeaderboardPlugin)
        // Additive exact-tuple v3 Ranked browser client. It consumes the
        // game-owned receipt/ledger interfaces and never routes Casual runs.
        .add_plugins(CompetitiveV3Plugin)
        .run();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_flag_precedence_is_microtexture_then_pond_world_car() {
        assert_eq!(
            startup_mode(true, true, true, true),
            StartupMode::MicrotextureReview
        );
        assert_eq!(
            startup_mode(false, true, true, true),
            StartupMode::PondReview
        );
        assert_eq!(
            startup_mode(false, false, true, true),
            StartupMode::WorldReview
        );
        assert_eq!(
            startup_mode(false, false, false, true),
            StartupMode::CarReview
        );
        assert_eq!(
            startup_mode(false, false, false, false),
            StartupMode::Production
        );
    }
}
