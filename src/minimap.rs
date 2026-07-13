//! Minimap (T4): a small top-right radar panel that plots coins, chickens and
//! obstacles as high-contrast markers around a car-centered, heading-rotated
//! view.
//!
//! Design:
//! - A fixed pool of dot children is spawned up front (hidden by default) and
//!   **repurposed** each frame — no per-frame spawns/despawns (web-friendly).
//! - Each frame the car's XZ offset to every nearby coin / chicken / obstacle
//!   is projected onto the car's right/forward axes, scaled into the panel,
//!   and assigned to the next pool dot (north = car forward). Unused dots are
//!   hidden. The first dot is always the car (tall, red, and centered).
//! - The panel is spawned on `OnEnter(Playing)` and despawned on
//!   `OnExit(Playing)`, mirroring `health.rs`'s HUD lifecycle. It is inset
//!   from the right to clear the separately owned difficulty-level label.
//!   Owns its UI; does not touch `ui.rs`.

use bevy::prelude::*;

use crate::car::Car;
use crate::chickens::Chicken;
use crate::game::state::GameState;
use crate::world::{Coin, Collider};

// ---------------------------------------------------------------------------
// Tuning constants
// ---------------------------------------------------------------------------

/// Minimap plotting surface size in UI pixels (square).
const MAP_SIZE: f32 = 132.0;
/// World-space radius (units) represented by the inner frame.
const RANGE: f32 = 60.0;
/// Width of the high-contrast outer panel border.
const PANEL_BORDER: f32 = 2.0;
/// Inset and width of the frame that marks the plotted range.
const INNER_FRAME_INSET: f32 = 6.0;
const INNER_FRAME_WIDTH: f32 = 1.0;
/// Dots are kept wholly inside the inner edge of the range frame.
const PLOT_MIN: f32 = INNER_FRAME_INSET + INNER_FRAME_WIDTH;
const PLOT_MAX: f32 = MAP_SIZE - PLOT_MIN;
const PLOT_SIZE: f32 = PLOT_MAX - PLOT_MIN;
const MAP_CENTER: f32 = MAP_SIZE / 2.0;
/// Fixed pool size — bounds the entity count plotted per frame (web-friendly:
/// no per-frame spawns). One slot is reserved for the car dot.
const POOL_SIZE: usize = 64;
/// Panel top offset: clears the timer's 24px text plus its new vertical panel
/// padding (`top: 12`), leaving a visible gap below the contrast panel.
const PANEL_TOP: f32 = 62.0;
/// The separate difficulty label is fixed at `right: 16, top: 182` in its
/// owning module. The actual minimap is 132px plus a 2px border on each side,
/// so its bottom is y=198 rather than the old 174px assumption. Shift the map
/// left to clear that label horizontally without moving either HUD into the
/// timer/objective strips.
const PANEL_RIGHT: f32 = 72.0;
const PANEL_OUTER_SIZE: f32 = MAP_SIZE + PANEL_BORDER * 2.0;
#[cfg(test)]
const DIFFICULTY_RIGHT: f32 = 16.0;
#[cfg(test)]
const DIFFICULTY_TOP: f32 = 182.0;
#[cfg(test)]
const DIFFICULTY_WIDTH: f32 = 48.0;
#[cfg(test)]
const DIFFICULTY_HEIGHT: f32 = 26.0;

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct UiBounds {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

#[cfg(test)]
fn bounds_overlap(a: UiBounds, b: UiBounds) -> bool {
    a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top
}

/// Pure HUD bounds used to keep this module's map clear of the externally
/// owned level label while accounting for all 132 plotting pixels and border.
#[cfg(test)]
fn minimap_and_level_bounds(viewport_width: f32) -> (UiBounds, UiBounds) {
    let map_right = viewport_width - PANEL_RIGHT;
    let map = UiBounds {
        left: map_right - PANEL_OUTER_SIZE,
        top: PANEL_TOP,
        right: map_right,
        bottom: PANEL_TOP + PANEL_OUTER_SIZE,
    };
    let level_right = viewport_width - DIFFICULTY_RIGHT;
    let level = UiBounds {
        left: level_right - DIFFICULTY_WIDTH,
        top: DIFFICULTY_TOP,
        right: level_right,
        bottom: DIFFICULTY_TOP + DIFFICULTY_HEIGHT,
    };
    (map, level)
}

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

/// Root node of the minimap panel. Despawned on exit from `Playing`
/// (recursively nukes the pooled dot children — safe in 0.19).
#[derive(Component)]
struct MinimapRoot;

/// A pooled dot child. `kind` is refreshed each frame to match whatever entity
/// the dot is currently plotting (it drives all visual properties via
/// [`dot_style`]).
#[derive(Component)]
struct MapDot {
    kind: DotKind,
}

/// What a dot represents. Color and silhouette both vary by kind so the map
/// remains readable without relying on color perception alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DotKind {
    Coin,
    Chicken,
    Car,
    Obstacle,
}

/// Complete, reusable visual style for a pooled dot.
#[derive(Clone, Copy)]
struct DotStyle {
    width: f32,
    height: f32,
    border_width: f32,
    fill: Color,
    border: Color,
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
            .add_systems(OnExit(GameState::Playing), despawn_marker::<MinimapRoot>)
            .add_systems(Update, update_minimap.run_if(in_state(GameState::Playing)));
    }
}

// ---------------------------------------------------------------------------
// Spawn / despawn
// ---------------------------------------------------------------------------

/// Spawn the minimap panel + its fixed pool of dot children (hidden). Lives
/// only while `Playing`; the root is despawned on exit (recursively removing
/// the dot children).
fn spawn_minimap(mut commands: Commands) {
    let initial_style = dot_style(DotKind::Car);
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: px(PANEL_TOP),
                right: px(PANEL_RIGHT),
                // The border-box is MAP_SIZE plus both 2px borders; plotting
                // coordinates remain based on the 132px child surface.
                width: px(PANEL_OUTER_SIZE),
                height: px(PANEL_OUTER_SIZE),
                border: UiRect::all(px(PANEL_BORDER)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.015, 0.02, 0.035, 0.88)),
            BorderColor::all(Color::srgb(0.82, 0.90, 1.0)),
            MinimapRoot,
        ))
        .with_children(|root| {
            root.spawn(Node {
                width: px(MAP_SIZE),
                height: px(MAP_SIZE),
                ..default()
            })
            .with_children(|map| {
                // A second frame clearly defines the maximum represented
                // range. It and the heading cues are static pooled-UI siblings,
                // never entities spawned by the update system.
                map.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(INNER_FRAME_INSET),
                        top: px(INNER_FRAME_INSET),
                        width: px(MAP_SIZE - INNER_FRAME_INSET * 2.0),
                        height: px(MAP_SIZE - INNER_FRAME_INSET * 2.0),
                        border: UiRect::all(px(INNER_FRAME_WIDTH)),
                        ..default()
                    },
                    BorderColor::all(Color::srgba(0.58, 0.70, 0.84, 0.75)),
                ));

                // Fixed range crosshair: the full line spans center-to-edge in
                // each direction, making distance and orientation easy to read.
                map.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(MAP_CENTER - 0.5),
                        top: px(PLOT_MIN),
                        width: px(1.0),
                        height: px(PLOT_SIZE),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.55, 0.68, 0.82, 0.24)),
                ));
                map.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(PLOT_MIN),
                        top: px(MAP_CENTER - 0.5),
                        width: px(PLOT_SIZE),
                        height: px(1.0),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.55, 0.68, 0.82, 0.24)),
                ));

                // Bright, fixed north tick: north on this map is always the
                // car's forward direction.
                map.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(MAP_CENTER - 2.0),
                        top: px(PLOT_MIN + 2.0),
                        width: px(4.0),
                        height: px(12.0),
                        border: UiRect::all(px(1.0)),
                        ..default()
                    },
                    BackgroundColor(Color::srgb(0.30, 0.95, 1.0)),
                    BorderColor::all(Color::srgb(0.95, 1.0, 1.0)),
                ));

                // The only dynamic minimap UI: a fixed 64-node pool.
                for _ in 0..POOL_SIZE {
                    map.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(MAP_CENTER),
                            top: px(MAP_CENTER),
                            width: px(initial_style.width),
                            height: px(initial_style.height),
                            border: UiRect::all(px(initial_style.border_width)),
                            ..default()
                        },
                        BackgroundColor(initial_style.fill),
                        BorderColor::all(initial_style.border),
                        Visibility::Hidden,
                        MapDot { kind: DotKind::Car },
                    ));
                }
            });
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
/// transforms. The first dot is the car (tall, red, and centered); the
/// remaining dots are filled with nearby entities in priority order (coins =>
/// chickens => obstacles); unused dots are hidden. Plotted count is capped at
/// the pool size.
fn update_minimap(
    car: Query<(&Car, &Transform)>,
    coins: Query<&GlobalTransform, With<Coin>>,
    chickens: Query<&GlobalTransform, With<Chicken>>,
    obstacles: Query<&GlobalTransform, With<Collider>>,
    mut dots: Query<(
        &mut Node,
        &mut BackgroundColor,
        &mut BorderColor,
        &mut Visibility,
        &mut MapDot,
    )>,
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
        DotKind::Coin,
        &mut plots,
        max_plots,
    );
    collect_plots(
        chickens.iter(),
        car_pos,
        sin_h,
        cos_h,
        DotKind::Chicken,
        &mut plots,
        max_plots,
    );
    collect_plots(
        obstacles.iter(),
        car_pos,
        sin_h,
        cos_h,
        DotKind::Obstacle,
        &mut plots,
        max_plots,
    );

    // Assign pool dots: first = car (centered), rest = plots in order. Each
    // pooled node is fully restyled, so its silhouette follows its new kind.
    let mut iter = dots.iter_mut();
    if let Some((mut node, mut bg, mut border, mut vis, mut dot)) = iter.next() {
        let style = dot_style(DotKind::Car);
        apply_dot_style(&mut node, &mut bg, &mut border, style);
        let position = clamped_dot_position(Vec2::splat(MAP_CENTER), style);
        node.left = px(position.x);
        node.top = px(position.y);
        *vis = Visibility::Visible;
        dot.kind = DotKind::Car;
    }
    for (i, (mut node, mut bg, mut border, mut vis, mut dot)) in iter.enumerate() {
        if let Some(plot) = plots.get(i) {
            let style = dot_style(plot.kind);
            apply_dot_style(&mut node, &mut bg, &mut border, style);
            let position = clamped_dot_position(Vec2::new(plot.x, plot.y), style);
            node.left = px(position.x);
            node.top = px(position.y);
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
    iter: impl Iterator<Item = &'a GlobalTransform>,
    car_pos: Vec3,
    sin_h: f32,
    cos_h: f32,
    kind: DotKind,
    plots: &mut Vec<Plot>,
    max_plots: usize,
) {
    for tf in iter {
        if plots.len() >= max_plots {
            break;
        }
        // Coins/obstacles are chunk-root children -> their `Transform` is
        // local; `GlobalTransform` gives the world position so minimap dots
        // line up with the actual entities.
        let pos = tf.translation();
        let dx = pos.x - car_pos.x;
        let dz = pos.z - car_pos.z;
        if let Some(point) = project_offset(dx, dz, sin_h, cos_h) {
            plots.push(Plot {
                x: point.x,
                y: point.y,
                kind,
            });
        }
    }
}

/// Project an XZ world offset through the car's heading into minimap-local
/// coordinates. Returning `None` keeps the radar's range circular. Forward is
/// always decreasing UI Y; this is deliberately independent of ECS state so
/// projection semantics can be tested directly.
fn project_offset(dx: f32, dz: f32, sin_h: f32, cos_h: f32) -> Option<Vec2> {
    if dx * dx + dz * dz > RANGE * RANGE {
        return None;
    }
    let right_comp = dx * cos_h - dz * sin_h;
    let fwd_comp = -dx * sin_h - dz * cos_h;
    let scale = PLOT_SIZE / (2.0 * RANGE);
    Some(Vec2::new(
        MAP_CENTER + right_comp * scale,
        MAP_CENTER - fwd_comp * scale,
    ))
}

/// Convert a desired dot center into a clamped top-left coordinate. The clamp
/// includes each style's full dimensions, keeping every pooled node wholly
/// within the inner edge of the range frame.
fn clamped_dot_position(center: Vec2, style: DotStyle) -> Vec2 {
    Vec2::new(
        (center.x - style.width / 2.0).clamp(PLOT_MIN, PLOT_MAX - style.width),
        (center.y - style.height / 2.0).clamp(PLOT_MIN, PLOT_MAX - style.height),
    )
}

/// Visual encoding by kind. Every kind has a distinct size/aspect/border
/// signature as well as a high-contrast color, avoiding color-only meaning.
fn dot_style(kind: DotKind) -> DotStyle {
    match kind {
        // Compact square with a dark keyline.
        DotKind::Coin => DotStyle {
            width: 8.0,
            height: 8.0,
            border_width: 1.0,
            fill: Color::srgb(1.0, 0.82, 0.05),
            border: Color::srgb(0.20, 0.13, 0.0),
        },
        // Wide dash, visually distinct from every square marker.
        DotKind::Chicken => DotStyle {
            width: 11.0,
            height: 6.0,
            border_width: 1.0,
            fill: Color::srgb(1.0, 1.0, 1.0),
            border: Color::srgb(0.10, 0.12, 0.16),
        },
        // Tall marker reinforces that the car points toward map north.
        DotKind::Car => DotStyle {
            width: 8.0,
            height: 12.0,
            border_width: 1.0,
            fill: Color::srgb(1.0, 0.12, 0.18),
            border: Color::srgb(1.0, 0.92, 0.92),
        },
        // Largest square and uniquely heavy border.
        DotKind::Obstacle => DotStyle {
            width: 11.0,
            height: 11.0,
            border_width: 2.0,
            fill: Color::srgb(0.38, 0.62, 0.82),
            border: Color::srgb(0.04, 0.08, 0.13),
        },
    }
}

fn apply_dot_style(
    node: &mut Node,
    background: &mut BackgroundColor,
    border: &mut BorderColor,
    style: DotStyle,
) {
    node.width = px(style.width);
    node.height = px(style.height);
    node.border = UiRect::all(px(style.border_width));
    background.0 = style.fill;
    *border = BorderColor::all(style.border);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actual_132px_map_panel_clears_level_label_on_mobile_and_desktop() {
        assert_eq!(PANEL_OUTER_SIZE, 136.0);
        for viewport_width in [844.0, 1440.0] {
            let (map, level) = minimap_and_level_bounds(viewport_width);
            // Their vertical ranges overlap (the original bug), so clearance
            // must be genuine horizontal separation rather than bad map math.
            assert!(map.bottom > level.top);
            assert!(!bounds_overlap(map, level));
            assert!(map.right <= level.left - 8.0);
        }
    }

    #[test]
    fn dot_styles_have_distinct_non_color_signatures() {
        let kinds = [
            DotKind::Coin,
            DotKind::Chicken,
            DotKind::Car,
            DotKind::Obstacle,
        ];
        let signatures = kinds.map(|kind| {
            let style = dot_style(kind);
            assert!(style.width > 4.0 && style.height > 4.0);
            (style.width, style.height, style.border_width)
        });

        for i in 0..signatures.len() {
            for j in (i + 1)..signatures.len() {
                assert_ne!(signatures[i], signatures[j]);
            }
        }
    }

    #[test]
    fn projection_preserves_forward_and_range_semantics() {
        let forward = project_offset(0.0, -RANGE, 0.0, 1.0).unwrap();
        assert!((forward.x - MAP_CENTER).abs() < 0.001);
        assert!((forward.y - PLOT_MIN).abs() < 0.001);
        let right = project_offset(RANGE, 0.0, 0.0, 1.0).unwrap();
        assert!((right.x - PLOT_MAX).abs() < 0.001);
        assert!((right.y - MAP_CENTER).abs() < 0.001);
        assert!(project_offset(RANGE + 0.01, 0.0, 0.0, 1.0).is_none());
    }

    #[test]
    fn every_style_is_clamped_wholly_inside_inner_frame() {
        for kind in [
            DotKind::Coin,
            DotKind::Chicken,
            DotKind::Car,
            DotKind::Obstacle,
        ] {
            let style = dot_style(kind);
            for center in [Vec2::splat(-1000.0), Vec2::splat(1000.0)] {
                let position = clamped_dot_position(center, style);
                assert!(position.x >= PLOT_MIN && position.y >= PLOT_MIN);
                assert!(position.x + style.width <= PLOT_MAX);
                assert!(position.y + style.height <= PLOT_MAX);
            }
        }
    }
}
