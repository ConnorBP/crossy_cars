//! Minimap (T4): a small top-right radar panel that plots coins, chickens and
//! obstacles as colored dots around a car-centered, heading-rotated view.
//!
//! Design:
//! - A fixed pool of dot children is spawned up front (hidden by default) and
//!   **repurposed** each frame — no per-frame spawns/despawns (web-friendly).
//! - Each frame the car's XZ offset to every nearby coin / chicken / obstacle
//!   is projected onto the car's right/forward axes, scaled into the panel,
//!   and assigned to the next pool dot (north = car forward). Unused dots are
//!   hidden. The first dot is always the car (red, centered).
//! - The panel is spawned on `OnEnter(Playing)` and despawned on
//!   `OnExit(Playing)`, mirroring `health.rs`'s HUD lifecycle. Owns its UI;
//!   does not touch `ui.rs`.

use bevy::prelude::*;

use crate::car::Car;
use crate::chickens::Chicken;
use crate::game::state::GameState;
use crate::world::{Coin, Collider};

// ---------------------------------------------------------------------------
// Tuning constants
// ---------------------------------------------------------------------------

/// Minimap panel size in UI pixels (square).
const MAP_SIZE: f32 = 120.0;
/// World-space radius (units) mapped to the panel's half-size. Entities
/// farther than this are not plotted. 60u => 1.0 px per unit.
const RANGE: f32 = 60.0;
/// Size of each dot (square) in UI pixels.
const DOT_SIZE: f32 = 4.0;
/// Fixed pool size — bounds the entity count plotted per frame (web-friendly:
/// no per-frame spawns). One slot is reserved for the car dot.
const POOL_SIZE: usize = 64;
/// Panel top offset: sits below the top-right timer (`top: 12`, ~24px text).
const PANEL_TOP: f32 = 54.0;
/// Panel right offset (matches the timer's `right: 16`).
const PANEL_RIGHT: f32 = 16.0;

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

/// Root node of the minimap panel. Despawned on exit from `Playing`
/// (recursively nukes the pooled dot children — safe in 0.19).
#[derive(Component)]
struct MinimapRoot;

/// A pooled dot child. `kind` is refreshed each frame to match whatever entity
/// the dot is currently plotting (it drives the color via `dot_color`).
#[derive(Component)]
struct MapDot {
    kind: DotKind,
}

/// What a dot represents. Determines its color.
#[derive(Clone, Copy)]
enum DotKind {
    Coin,
    Chicken,
    Car,
    Obstacle,
}

/// A world entity projected into minimap-local coordinates, ready to assign to
/// a pool dot. Held in a `Local<Vec<…>>` that is cleared and refilled each
/// frame (no per-frame allocation after warmup).
struct Plot {
    /// Minimap-local X (px) — dot center.
    x: f32,
    /// Minimap-local Y (px) — dot center.
    y: f32,
    kind: DotKind,
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct MinimapPlugin;

impl Plugin for MinimapPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(GameState::Playing), spawn_minimap)
            .add_systems(
                OnExit(GameState::Playing),
                despawn_marker::<MinimapRoot>,
            )
            .add_systems(
                Update,
                update_minimap.run_if(in_state(GameState::Playing)),
            );
    }
}

// ---------------------------------------------------------------------------
// Spawn / despawn
// ---------------------------------------------------------------------------

/// Spawn the minimap panel + its fixed pool of dot children (hidden). Lives
/// only while `Playing`; the root is despawned on exit (recursively removing
/// the dot children).
fn spawn_minimap(mut commands: Commands) {
    let center = MAP_SIZE / 2.0;
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: px(PANEL_TOP),
                right: px(PANEL_RIGHT),
                width: px(MAP_SIZE),
                height: px(MAP_SIZE),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.5)),
            MinimapRoot,
        ))
        .with_children(|p| {
            for _ in 0..POOL_SIZE {
                p.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(center),
                        top: px(center),
                        width: px(DOT_SIZE),
                        height: px(DOT_SIZE),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.0)),
                    Visibility::Hidden,
                    MapDot { kind: DotKind::Car },
                ));
            }
        });
}

/// Despawn every entity tagged with marker `M` (mirrors `ui.rs` / `health.rs`).
fn despawn_marker<M: Component>(mut commands: Commands, q: Query<Entity, With<M>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}

// ---------------------------------------------------------------------------
// Per-frame dot update
// ---------------------------------------------------------------------------

/// Refresh every pool dot from the current car / coin / chicken / obstacle
/// transforms. The first dot is the car (red, centered); the remaining dots
/// are filled with nearby entities in priority order (coins => chickens =>
/// obstacles); unused dots are hidden. Plotted count is capped at the pool
/// size.
fn update_minimap(
    car: Query<(&Car, &Transform)>,
    coins: Query<&Transform, With<Coin>>,
    chickens: Query<&Transform, With<Chicken>>,
    obstacles: Query<&Transform, With<Collider>>,
    mut dots: Query<(&mut Node, &mut BackgroundColor, &mut Visibility, &mut MapDot)>,
    mut plots: Local<Vec<Plot>>,
) {
    let Ok((car, car_t)) = car.single() else {
        return;
    };
    let car_pos = car_t.translation;
    let heading = car.heading;

    // Car axes (heading 0 => forward = -Z, right = +X):
    //   forward = (-sin h, 0, -cos h)
    //   right   = ( cos h, 0, -sin h)
    let (sin_h, cos_h) = heading.sin_cos();

    let center = MAP_SIZE / 2.0;
    let scale = MAP_SIZE / (2.0 * RANGE);
    let range2 = RANGE * RANGE;
    let max_plots = POOL_SIZE - 1; // one slot reserved for the car dot

    // Reuse the Local allocation; clear and refill each frame. Coins and
    // chickens (gameplay-relevant) are plotted before obstacles so they win
    // any tie for the remaining pool slots.
    plots.clear();
    collect_plots(
        coins.iter(),
        car_pos,
        sin_h,
        cos_h,
        center,
        scale,
        range2,
        DotKind::Coin,
        &mut plots,
        max_plots,
    );
    collect_plots(
        chickens.iter(),
        car_pos,
        sin_h,
        cos_h,
        center,
        scale,
        range2,
        DotKind::Chicken,
        &mut plots,
        max_plots,
    );
    collect_plots(
        obstacles.iter(),
        car_pos,
        sin_h,
        cos_h,
        center,
        scale,
        range2,
        DotKind::Obstacle,
        &mut plots,
        max_plots,
    );

    // Assign pool dots: first = car (centered, red), rest = plots in order.
    let mut iter = dots.iter_mut();
    // --- Car dot (always visible, centered, red) ---
    if let Some((mut node, mut bg, mut vis, mut dot)) = iter.next() {
        node.left = px(center - DOT_SIZE / 2.0);
        node.top = px(center - DOT_SIZE / 2.0);
        bg.0 = dot_color(DotKind::Car);
        *vis = Visibility::Visible;
        dot.kind = DotKind::Car;
    }
    // --- Entity dots (repurposed from the pool; extras hidden) ---
    for (i, (mut node, mut bg, mut vis, mut dot)) in iter.enumerate() {
        if let Some(plot) = plots.get(i) {
            let left = plot.x - DOT_SIZE / 2.0;
            let top = plot.y - DOT_SIZE / 2.0;
            node.left = px(left.clamp(0.0, MAP_SIZE - DOT_SIZE));
            node.top = px(top.clamp(0.0, MAP_SIZE - DOT_SIZE));
            bg.0 = dot_color(plot.kind);
            *vis = Visibility::Visible;
            dot.kind = plot.kind;
        } else {
            *vis = Visibility::Hidden;
        }
    }
}

/// Push minimap-local projections of every in-range entity from `iter` into
/// `plots`, stopping once `plots` reaches `max_plots`. Entities are projected
/// onto the car's right/forward axes so north = car forward.
fn collect_plots<'a>(
    iter: impl Iterator<Item = &'a Transform>,
    car_pos: Vec3,
    sin_h: f32,
    cos_h: f32,
    center: f32,
    scale: f32,
    range2: f32,
    kind: DotKind,
    plots: &mut Vec<Plot>,
    max_plots: usize,
) {
    for tf in iter {
        if plots.len() >= max_plots {
            break;
        }
        let dx = tf.translation.x - car_pos.x;
        let dz = tf.translation.z - car_pos.z;
        if dx * dx + dz * dz > range2 {
            continue;
        }
        // Project onto car right / forward axes.
        let right_comp = dx * cos_h - dz * sin_h;
        let fwd_comp = -dx * sin_h - dz * cos_h;
        // Map to minimap-local px (forward = up = decreasing top).
        plots.push(Plot {
            x: center + right_comp * scale,
            y: center - fwd_comp * scale,
            kind,
        });
    }
}

/// Dot color by kind: gold = coin, white = chicken, red = car, grey = obstacle.
fn dot_color(kind: DotKind) -> Color {
    match kind {
        DotKind::Coin => Color::srgb(1.0, 0.85, 0.10),
        DotKind::Chicken => Color::srgb(0.95, 0.95, 0.95),
        DotKind::Car => Color::srgb(1.0, 0.20, 0.20),
        DotKind::Obstacle => Color::srgb(0.55, 0.55, 0.60),
    }
}
