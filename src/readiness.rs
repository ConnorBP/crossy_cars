//! Truthful production readiness signal for the HTML loading shell.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::car::ImportedCarReady;
use crate::game::state::GameState;
use crate::menu::ResponsiveMenuRoot;
use crate::world::{Block, GridConfig};

#[derive(Resource, Default)]
struct PlayableReadiness {
    complete_updates: u8,
    published: bool,
}

pub struct ReadinessPlugin;

impl Plugin for ReadinessPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PlayableReadiness>()
            .add_systems(Update, publish_playable_readiness);
    }
}

fn readiness_complete(
    state: GameState,
    expected_blocks: i32,
    blocks: usize,
    windows: usize,
    cameras: usize,
    menus: usize,
    imported_car_ready: usize,
) -> bool {
    let expected_blocks = expected_blocks.max(0) as usize;
    state == GameState::Menu
        && expected_blocks > 0
        && blocks == expected_blocks * expected_blocks
        && windows == 1
        && cameras >= 1
        && menus >= 2 // vignette plus interactive menu composition
        && imported_car_ready == 1
}

#[allow(clippy::too_many_arguments)]
fn publish_playable_readiness(
    state: Res<State<GameState>>,
    config: Res<GridConfig>,
    blocks: Query<(), With<Block>>,
    windows: Query<(), With<PrimaryWindow>>,
    cameras: Query<(), With<Camera3d>>,
    menus: Query<(), With<ResponsiveMenuRoot>>,
    imported_car_ready: Query<(), With<ImportedCarReady>>,
    mut readiness: ResMut<PlayableReadiness>,
) {
    if readiness.published {
        return;
    }
    let complete = readiness_complete(
        *state.get(),
        config.count,
        blocks.iter().count(),
        windows.iter().count(),
        cameras.iter().count(),
        menus.iter().count(),
        imported_car_ready.iter().count(),
    );
    readiness.complete_updates = if complete {
        readiness.complete_updates.saturating_add(1)
    } else {
        0
    };
    if readiness.complete_updates < 2 {
        return;
    }
    publish_dom_ready();
    readiness.published = true;
}

#[cfg(target_arch = "wasm32")]
fn publish_dom_ready() {
    if let Some(root) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.document_element())
    {
        let _ = root.set_attribute("data-roady-ready", "playable");
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn publish_dom_ready() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playable_requires_complete_semantic_scene() {
        assert!(readiness_complete(GameState::Menu, 5, 25, 1, 1, 2, 1));
        assert!(!readiness_complete(GameState::Playing, 5, 25, 1, 1, 2, 1));
        assert!(!readiness_complete(GameState::Menu, 5, 24, 1, 1, 2, 1));
        assert!(!readiness_complete(GameState::Menu, 5, 25, 1, 1, 1, 1));
        assert!(!readiness_complete(GameState::Menu, 5, 25, 1, 1, 2, 0));
    }

    #[test]
    fn readiness_requires_two_consecutive_complete_updates() {
        let mut state = PlayableReadiness::default();
        for complete in [true, false, true, true] {
            state.complete_updates = if complete {
                state.complete_updates.saturating_add(1)
            } else {
                0
            };
        }
        assert_eq!(state.complete_updates, 2);
    }
}
