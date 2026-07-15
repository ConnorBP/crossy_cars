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
use bevy::window::PrimaryWindow;

use crate::car::Car;
use crate::chickens::Chicken;
use crate::game::TouchStateSet;
use crate::game::state::GameState;
use crate::touch::{
    TOUCH_MINIMAP_OUTER_SIZE, TOUCH_MINIMAP_RIGHT, TOUCH_MINIMAP_TOP, TouchControlsActive,
    is_touch_portrait, touch_minimap_bounds,
};
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
#[cfg(test)]
const PLOT_MIN: f32 = INNER_FRAME_INSET + INNER_FRAME_WIDTH;
#[cfg(test)]
const PLOT_MAX: f32 = MAP_SIZE - PLOT_MIN;
#[cfg(test)]
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
#[cfg(test)]
const PANEL_OUTER_SIZE: f32 = MAP_SIZE + PANEL_BORDER * 2.0;
const TOUCH_MAP_SIZE: f32 = TOUCH_MINIMAP_OUTER_SIZE - PANEL_BORDER * 2.0;
const TOUCH_INNER_FRAME_INSET: f32 = 5.0;

#[derive(Clone, Copy)]
struct MapLayout {
    map_size: f32,
    inner_frame_inset: f32,
    top: f32,
    right: f32,
    compact: bool,
}

impl MapLayout {
    fn for_viewport(compact: bool, viewport: Option<Vec2>) -> Self {
        if compact {
            if let Some(viewport) = viewport.filter(|viewport| is_touch_portrait(*viewport)) {
                let bounds = touch_minimap_bounds(viewport);
                Self {
                    map_size: bounds.width() - PANEL_BORDER * 2.0,
                    inner_frame_inset: TOUCH_INNER_FRAME_INSET,
                    top: bounds.top,
                    right: viewport.x - bounds.right,
                    compact: true,
                }
            } else {
                Self {
                    map_size: TOUCH_MAP_SIZE,
                    inner_frame_inset: TOUCH_INNER_FRAME_INSET,
                    top: TOUCH_MINIMAP_TOP,
                    right: TOUCH_MINIMAP_RIGHT,
                    compact: true,
                }
            }
        } else {
            Self {
                map_size: MAP_SIZE,
                inner_frame_inset: INNER_FRAME_INSET,
                top: PANEL_TOP,
                right: PANEL_RIGHT,
                compact: false,
            }
        }
    }

    #[cfg(test)]
    fn for_touch(compact: bool) -> Self {
        Self::for_viewport(compact, None)
    }

    fn outer_size(self) -> f32 {
        self.map_size + PANEL_BORDER * 2.0
    }

    fn plot_min(self) -> f32 {
        self.inner_frame_inset + INNER_FRAME_WIDTH
    }

    fn plot_max(self) -> f32 {
        self.map_size - self.plot_min()
    }

    fn plot_size(self) -> f32 {
        self.plot_max() - self.plot_min()
    }

    fn center(self) -> f32 {
        self.map_size * 0.5
    }
}
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

/// Pure desktop HUD bounds used to keep this module's map clear of the
/// externally owned level label while accounting for all plotting pixels and
/// border. Compact bounds are audited from the shared touch constants.
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
#[derive(Component)]
struct MinimapSurface;
#[derive(Component)]
struct MinimapInnerFrame;
#[derive(Component)]
struct MinimapVerticalCrosshair;
#[derive(Component)]
struct MinimapHorizontalCrosshair;
#[derive(Component)]
struct MinimapNorthTick;

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
            .add_systems(
                Update,
                (
                    update_minimap_layout.after(TouchStateSet),
                    update_minimap.after(TouchStateSet),
                )
                    .run_if(in_state(GameState::Playing)),
            );
    }
}

// ---------------------------------------------------------------------------
// Spawn / despawn
// ---------------------------------------------------------------------------

/// Spawn the minimap panel + its fixed pool of dot children (hidden). Lives
/// only while `Playing`; the root is despawned on exit (recursively removing
/// the dot children).
fn spawn_minimap(
    mut commands: Commands,
    touch: Res<TouchControlsActive>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let viewport = windows
        .single()
        .ok()
        .map(|window| Vec2::new(window.width(), window.height()));
    let layout = MapLayout::for_viewport(touch.0, viewport);
    let initial_style = dot_style(DotKind::Car, layout.compact);
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: px(layout.top),
                right: px(layout.right),
                width: px(layout.outer_size()),
                height: px(layout.outer_size()),
                border: UiRect::all(px(PANEL_BORDER)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.015, 0.02, 0.035, 0.88)),
            BorderColor::all(Color::srgb(0.82, 0.90, 1.0)),
            MinimapRoot,
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    width: px(layout.map_size),
                    height: px(layout.map_size),
                    ..default()
                },
                MinimapSurface,
            ))
            .with_children(|map| {
                // A second frame clearly defines the maximum represented
                // range. It and the heading cues are static pooled-UI siblings,
                // never entities spawned by the update system.
                map.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(layout.inner_frame_inset),
                        top: px(layout.inner_frame_inset),
                        width: px(layout.map_size - layout.inner_frame_inset * 2.0),
                        height: px(layout.map_size - layout.inner_frame_inset * 2.0),
                        border: UiRect::all(px(INNER_FRAME_WIDTH)),
                        ..default()
                    },
                    BorderColor::all(Color::srgba(0.58, 0.70, 0.84, 0.75)),
                    MinimapInnerFrame,
                ));

                // Fixed range crosshair: the full line spans center-to-edge in
                // each direction, making distance and orientation easy to read.
                map.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(layout.center() - 0.5),
                        top: px(layout.plot_min()),
                        width: px(1.0),
                        height: px(layout.plot_size()),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.55, 0.68, 0.82, 0.24)),
                    MinimapVerticalCrosshair,
                ));
                map.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(layout.plot_min()),
                        top: px(layout.center() - 0.5),
                        width: px(layout.plot_size()),
                        height: px(1.0),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.55, 0.68, 0.82, 0.24)),
                    MinimapHorizontalCrosshair,
                ));

                // Bright, fixed north tick: north on this map is always the
                // car's forward direction.
                map.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(layout.center() - 2.0),
                        top: px(layout.plot_min() + 2.0),
                        width: px(4.0),
                        height: px(if layout.compact { 9.0 } else { 12.0 }),
                        border: UiRect::all(px(1.0)),
                        ..default()
                    },
                    BackgroundColor(Color::srgb(0.30, 0.95, 1.0)),
                    BorderColor::all(Color::srgb(0.95, 1.0, 1.0)),
                    MinimapNorthTick,
                ));

                // The only dynamic minimap UI: a fixed 64-node pool.
                for _ in 0..POOL_SIZE {
                    map.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(layout.center()),
                            top: px(layout.center()),
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

fn update_minimap_layout(
    touch: Res<TouchControlsActive>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut nodes: Query<
        (
            &mut Node,
            Option<&MinimapRoot>,
            Option<&MinimapSurface>,
            Option<&MinimapInnerFrame>,
            Option<&MinimapVerticalCrosshair>,
            Option<&MinimapHorizontalCrosshair>,
            Option<&MinimapNorthTick>,
        ),
        Or<(
            With<MinimapRoot>,
            With<MinimapSurface>,
            With<MinimapInnerFrame>,
            With<MinimapVerticalCrosshair>,
            With<MinimapHorizontalCrosshair>,
            With<MinimapNorthTick>,
        )>,
    >,
) {
    if !touch.0 {
        return;
    }
    let viewport = windows
        .single()
        .ok()
        .map(|window| Vec2::new(window.width(), window.height()));
    let layout = MapLayout::for_viewport(true, viewport);
    for (mut node, root, surface, frame, vertical, horizontal, north) in &mut nodes {
        if root.is_some() {
            node.top = px(layout.top);
            node.right = px(layout.right);
            node.width = px(layout.outer_size());
            node.height = px(layout.outer_size());
        } else if surface.is_some() {
            node.width = px(layout.map_size);
            node.height = px(layout.map_size);
        } else if frame.is_some() {
            node.left = px(layout.inner_frame_inset);
            node.top = px(layout.inner_frame_inset);
            node.width = px(layout.map_size - layout.inner_frame_inset * 2.0);
            node.height = px(layout.map_size - layout.inner_frame_inset * 2.0);
        } else if vertical.is_some() {
            node.left = px(layout.center() - 0.5);
            node.top = px(layout.plot_min());
            node.height = px(layout.plot_size());
        } else if horizontal.is_some() {
            node.left = px(layout.plot_min());
            node.top = px(layout.center() - 0.5);
            node.width = px(layout.plot_size());
        } else if north.is_some() {
            node.left = px(layout.center() - 2.0);
            node.top = px(layout.plot_min() + 2.0);
            node.height = px(9.0);
        }
    }
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
    touch: Res<TouchControlsActive>,
    windows: Query<&Window, With<PrimaryWindow>>,
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
    let viewport = windows
        .single()
        .ok()
        .map(|window| Vec2::new(window.width(), window.height()));
    let layout = MapLayout::for_viewport(touch.0, viewport);
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
        layout,
    );
    collect_plots(
        chickens.iter(),
        car_pos,
        sin_h,
        cos_h,
        DotKind::Chicken,
        &mut plots,
        max_plots,
        layout,
    );
    collect_plots(
        obstacles.iter(),
        car_pos,
        sin_h,
        cos_h,
        DotKind::Obstacle,
        &mut plots,
        max_plots,
        layout,
    );

    // Assign pool dots: first = car (centered), rest = plots in order. Each
    // pooled node is fully restyled, so its silhouette follows its new kind.
    let mut iter = dots.iter_mut();
    if let Some((mut node, mut bg, mut border, mut vis, mut dot)) = iter.next() {
        let style = dot_style(DotKind::Car, layout.compact);
        apply_dot_style(&mut node, &mut bg, &mut border, style);
        let position = clamped_dot_position(Vec2::splat(layout.center()), style, layout);
        node.left = px(position.x);
        node.top = px(position.y);
        *vis = Visibility::Visible;
        dot.kind = DotKind::Car;
    }
    for (i, (mut node, mut bg, mut border, mut vis, mut dot)) in iter.enumerate() {
        if let Some(plot) = plots.get(i) {
            let style = dot_style(plot.kind, layout.compact);
            apply_dot_style(&mut node, &mut bg, &mut border, style);
            let position = clamped_dot_position(Vec2::new(plot.x, plot.y), style, layout);
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
    layout: MapLayout,
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
        if let Some(point) = project_offset_for_layout(dx, dz, sin_h, cos_h, layout) {
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
#[cfg(test)]
fn project_offset(dx: f32, dz: f32, sin_h: f32, cos_h: f32) -> Option<Vec2> {
    project_offset_for_layout(dx, dz, sin_h, cos_h, MapLayout::for_touch(false))
}

fn project_offset_for_layout(
    dx: f32,
    dz: f32,
    sin_h: f32,
    cos_h: f32,
    layout: MapLayout,
) -> Option<Vec2> {
    if dx * dx + dz * dz > RANGE * RANGE {
        return None;
    }
    let right_comp = dx * cos_h - dz * sin_h;
    let fwd_comp = -dx * sin_h - dz * cos_h;
    let scale = layout.plot_size() / (2.0 * RANGE);
    Some(Vec2::new(
        layout.center() + right_comp * scale,
        layout.center() - fwd_comp * scale,
    ))
}

/// Convert a desired dot center into a clamped top-left coordinate. The clamp
/// includes each style's full dimensions, keeping every pooled node wholly
/// within the inner edge of the range frame.
fn clamped_dot_position(center: Vec2, style: DotStyle, layout: MapLayout) -> Vec2 {
    Vec2::new(
        (center.x - style.width / 2.0).clamp(layout.plot_min(), layout.plot_max() - style.width),
        (center.y - style.height / 2.0).clamp(layout.plot_min(), layout.plot_max() - style.height),
    )
}

/// Visual encoding by kind. Every kind has a distinct size/aspect/border
/// signature as well as a high-contrast color, avoiding color-only meaning.
fn dot_style(kind: DotKind, compact: bool) -> DotStyle {
    let scale = if compact { 0.78 } else { 1.0 };
    let scaled =
        |width: f32, height: f32, border_width: f32, fill: Color, border: Color| DotStyle {
            width: width * scale,
            height: height * scale,
            border_width,
            fill,
            border,
        };
    match kind {
        // Compact square with a dark keyline.
        DotKind::Coin => scaled(
            8.0,
            8.0,
            1.0,
            Color::srgb(1.0, 0.82, 0.05),
            Color::srgb(0.20, 0.13, 0.0),
        ),
        // Wide dash, visually distinct from every square marker.
        DotKind::Chicken => scaled(
            11.0,
            6.0,
            1.0,
            Color::srgb(1.0, 1.0, 1.0),
            Color::srgb(0.10, 0.12, 0.16),
        ),
        // Tall marker reinforces that the car points toward map north.
        DotKind::Car => scaled(
            8.0,
            12.0,
            1.0,
            Color::srgb(1.0, 0.12, 0.18),
            Color::srgb(1.0, 0.92, 0.92),
        ),
        // Largest square and uniquely heavy border.
        DotKind::Obstacle => scaled(
            11.0,
            11.0,
            if compact { 1.0 } else { 2.0 },
            Color::srgb(0.38, 0.62, 0.82),
            Color::srgb(0.04, 0.08, 0.13),
        ),
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
    fn desktop_132px_map_panel_clears_level_label() {
        assert_eq!(PANEL_OUTER_SIZE, 136.0);
        for viewport_width in [1440.0] {
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
            let style = dot_style(kind, false);
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
            let layout = MapLayout::for_touch(false);
            let style = dot_style(kind, false);
            for center in [Vec2::splat(-1000.0), Vec2::splat(1000.0)] {
                let position = clamped_dot_position(center, style, layout);
                assert!(position.x >= PLOT_MIN && position.y >= PLOT_MIN);
                assert!(position.x + style.width <= PLOT_MAX);
                assert!(position.y + style.height <= PLOT_MAX);
            }
        }
    }

    #[test]
    fn compact_projection_uses_the_actual_resized_plot_surface() {
        let layout = MapLayout::for_touch(true);
        assert_eq!(layout.outer_size(), 108.0);
        let forward = project_offset_for_layout(0.0, -RANGE, 0.0, 1.0, layout).unwrap();
        let right = project_offset_for_layout(RANGE, 0.0, 0.0, 1.0, layout).unwrap();
        assert!((forward.x - layout.center()).abs() < 0.001);
        assert!((forward.y - layout.plot_min()).abs() < 0.001);
        assert!((right.x - layout.plot_max()).abs() < 0.001);
        assert!((right.y - layout.center()).abs() < 0.001);
    }

    #[test]
    fn portrait_touch_map_uses_responsive_96_and_88_pixel_panels() {
        let wide = MapLayout::for_viewport(true, Some(Vec2::new(390.0, 844.0)));
        let narrow = MapLayout::for_viewport(true, Some(Vec2::new(320.0, 700.0)));
        assert_eq!(wide.outer_size(), 96.0);
        assert_eq!(narrow.outer_size(), 88.0);
        assert_eq!(wide.right, 8.0);
        assert_eq!(narrow.right, 8.0);

        // Landscape touch and non-touch remain pixel-identical.
        assert_eq!(
            MapLayout::for_viewport(true, Some(Vec2::new(844.0, 390.0))).outer_size(),
            108.0
        );
        assert_eq!(
            MapLayout::for_viewport(false, Some(Vec2::new(390.0, 844.0))).outer_size(),
            136.0
        );
    }
}
