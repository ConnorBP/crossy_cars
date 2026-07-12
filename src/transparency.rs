//! Foreground-building occlusion fading.
//!
//! Buildings are collision roots, while their rendered body and roof are
//! descendants.  This plugin tests the world-space camera-to-car segment
//! against each building footprint and swaps only the elevated building
//! meshes to one shared translucent material.  The original material handle
//! is retained on each affected mesh so restoration does not clone materials
//! or create assets per frame.

use bevy::prelude::*;
use bevy::transform::TransformSystems;

use crate::car::Car;
use crate::game::state::GameState;
use crate::world::{Building, Collider};

/// A small footprint expansion accounts for the width of the car and for a
/// roof overhang, rather than requiring the camera-to-car line to hit the
/// mathematical center of the car exactly.
const FOOTPRINT_PADDING: f32 = 0.65;

/// Building shadows are also mesh descendants, but sit almost on the ground.
/// Body and roof mesh centers are well above this threshold (the shortest
/// generated building body is centered at y=2).  Keeping this classification
/// local to the building root avoids fading its ground shadow or unrelated
/// props in the containing city block.
const MIN_VISUAL_CENTER_HEIGHT: f32 = 0.5;

/// The roof child transform is at its center.  Generated roofs are 0.4 units
/// tall; the slight extra allowance avoids a precision gap at the roof plane.
const TOP_HALF_HEIGHT_ALLOWANCE: f32 = 0.25;

/// Ignore intersections that exist only at a segment endpoint.  Occluders
/// must occupy some interval strictly between the camera and car.
const SEGMENT_ENDPOINT_EPSILON: f32 = 1.0e-4;

/// The single material shared by every currently faded building mesh.
#[derive(Resource)]
struct BuildingGhostMaterial(Handle<StandardMaterial>);

impl FromWorld for BuildingGhostMaterial {
    fn from_world(world: &mut World) -> Self {
        let handle = world
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                // A slightly opaque, neutral-cool tint preserves enough
                // diffuse shading to read the building's shape while still
                // revealing the car through it.
                base_color: Color::srgba(0.68, 0.76, 0.82, 0.3),
                // Ordinary alpha blending works on both WebGL2 and the
                // camera's multisampled render target.  In particular, this
                // deliberately does not rely on alpha-to-coverage.
                alpha_mode: AlphaMode::Blend,
                // Keep the shared ghost material lit and moderately rough so
                // faces and roof planes retain their PBR lighting cues.
                perceptual_roughness: 0.72,
                ..default()
            });
        Self(handle)
    }
}

/// Original handle for a body/roof mesh that has been ghosted at least once.
/// This remains on the child after restoration, making later swaps immediate
/// and allocation-free.
#[derive(Component)]
struct OriginalBuildingMaterial(Handle<StandardMaterial>);

pub struct TransparencyPlugin;

impl Plugin for TransparencyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BuildingGhostMaterial>()
            // GlobalTransforms for recycled block/building descendants are
            // current by this point in the frame.
            .add_systems(
                PostUpdate,
                fade_occluding_buildings
                    .after(TransformSystems::Propagate)
                    .run_if(in_state(GameState::Playing)),
            )
            // Pausing and every other departure from play must immediately
            // put opaque materials back; no faded menu/game-over backdrop.
            .add_systems(OnExit(GameState::Playing), restore_all_buildings);
    }
}

/// Swap elevated descendant meshes of buildings intersecting the camera-car
/// segment.  Queries that can overlap only read shared components; the sole
/// mutable query is restricted to non-Building mesh children, avoiding Bevy's
/// B0001 runtime query conflict.
fn fade_occluding_buildings(
    mut commands: Commands,
    ghost: Res<BuildingGhostMaterial>,
    camera: Query<&GlobalTransform, (With<Camera3d>, Without<Car>, Without<Building>)>,
    car: Query<&GlobalTransform, (With<Car>, Without<Camera3d>, Without<Building>)>,
    buildings: Query<
        (&GlobalTransform, &Collider, &Children),
        (With<Building>, Without<Car>, Without<Camera3d>),
    >,
    hierarchy: Query<&Children, Without<Building>>,
    mut visuals: Query<
        (
            &GlobalTransform,
            &mut MeshMaterial3d<StandardMaterial>,
            Option<&OriginalBuildingMaterial>,
        ),
        (With<Mesh3d>, Without<Building>),
    >,
    mut descendants: Local<Vec<Entity>>,
) {
    let (Ok(camera_transform), Ok(car_transform)) = (camera.single(), car.single()) else {
        // A transient missing/duplicate camera or car must not leave stale
        // ghosts behind.
        for (_, mut material, original) in &mut visuals {
            if let Some(original) = original {
                material.0 = original.0.clone();
            }
        }
        return;
    };
    let camera_position = camera_transform.translation();
    let car_position = car_transform.translation();

    for (building_transform, collider, children) in &buildings {
        descendants.clear();
        collect_descendants(children, &hierarchy, &mut descendants);

        let base = building_transform.translation();
        let mut highest_center = f32::NEG_INFINITY;

        // Find the actual rendered height from elevated mesh descendants.
        // This keeps the vertical occlusion test accurate for the generated
        // 4..9 unit building range instead of assuming one global height.
        for &entity in descendants.iter() {
            if let Ok((transform, _, _)) = visuals.get_mut(entity) {
                let center_y = transform.translation().y;
                if center_y > base.y + MIN_VISUAL_CENTER_HEIGHT {
                    highest_center = highest_center.max(center_y);
                }
            }
        }

        let occluded = if highest_center.is_finite() {
            let min = Vec3::new(
                base.x - collider.half_x - FOOTPRINT_PADDING,
                base.y,
                base.z - collider.half_z - FOOTPRINT_PADDING,
            );
            let max = Vec3::new(
                base.x + collider.half_x + FOOTPRINT_PADDING,
                highest_center + TOP_HALF_HEIGHT_ALLOWANCE,
                base.z + collider.half_z + FOOTPRINT_PADDING,
            );
            segment_intersects_box(camera_position, car_position, min, max)
        } else {
            false
        };

        for &entity in descendants.iter() {
            let Ok((transform, mut material, original)) = visuals.get_mut(entity) else {
                continue;
            };
            let is_body_or_roof = transform.translation().y > base.y + MIN_VISUAL_CENTER_HEIGHT;

            if occluded && is_body_or_roof {
                if original.is_none() {
                    // Capture before replacing. Commands are deferred, but the
                    // handle itself is changed now, so the fade is visible in
                    // this frame without allocating an asset.
                    commands
                        .entity(entity)
                        .insert(OriginalBuildingMaterial(material.0.clone()));
                }
                material.0 = ghost.0.clone();
            } else if let Some(original) = original {
                material.0 = original.0.clone();
            }
        }
    }
}

/// Recursively gather all descendants because a building visual may gain an
/// intermediate transform node without changing the occlusion implementation.
fn collect_descendants(
    children: &Children,
    hierarchy: &Query<&Children, Without<Building>>,
    output: &mut Vec<Entity>,
) {
    for child in children.iter() {
        output.push(child);
        if let Ok(grandchildren) = hierarchy.get(child) {
            collect_descendants(grandchildren, hierarchy, output);
        }
    }
}

fn restore_all_buildings(
    mut visuals: Query<(
        &mut MeshMaterial3d<StandardMaterial>,
        &OriginalBuildingMaterial,
    )>,
) {
    for (mut material, original) in &mut visuals {
        material.0 = original.0.clone();
    }
}

/// Pure segment-vs-axis-aligned-box slab test.  The box is the building's
/// world-space footprint extruded from its root/base to its rendered roof.
/// Restricting the accepted interval to the open camera/car segment prevents
/// buildings behind either endpoint from fading.
fn segment_intersects_box(start: Vec3, end: Vec3, min: Vec3, max: Vec3) -> bool {
    if min.x > max.x || min.y > max.y || min.z > max.z {
        return false;
    }

    let delta = end - start;
    let mut enter = 0.0_f32;
    let mut exit = 1.0_f32;

    for (origin, direction, slab_min, slab_max) in [
        (start.x, delta.x, min.x, max.x),
        (start.y, delta.y, min.y, max.y),
        (start.z, delta.z, min.z, max.z),
    ] {
        if direction.abs() <= f32::EPSILON {
            if origin < slab_min || origin > slab_max {
                return false;
            }
            continue;
        }

        let mut near = (slab_min - origin) / direction;
        let mut far = (slab_max - origin) / direction;
        if near > far {
            std::mem::swap(&mut near, &mut far);
        }
        enter = enter.max(near);
        exit = exit.min(far);
        if enter > exit {
            return false;
        }
    }

    enter.max(SEGMENT_ENDPOINT_EPSILON) <= exit.min(1.0 - SEGMENT_ENDPOINT_EPSILON)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tall_box_between_camera_and_car_occludes() {
        let camera = Vec3::new(10.0, 10.0, 10.0);
        let car = Vec3::ZERO;
        assert!(segment_intersects_box(
            camera,
            car,
            Vec3::new(4.0, 0.0, 4.0),
            Vec3::new(6.0, 6.0, 6.0),
        ));
    }

    #[test]
    fn camera_ray_passes_over_short_building() {
        let camera = Vec3::new(10.0, 10.0, 10.0);
        let car = Vec3::ZERO;
        assert!(!segment_intersects_box(
            camera,
            car,
            Vec3::new(4.0, 0.0, 4.0),
            Vec3::new(6.0, 3.0, 6.0),
        ));
    }

    #[test]
    fn footprint_off_the_camera_car_line_does_not_occlude() {
        let camera = Vec3::new(10.0, 10.0, 10.0);
        let car = Vec3::ZERO;
        assert!(!segment_intersects_box(
            camera,
            car,
            Vec3::new(7.0, 0.0, 4.0),
            Vec3::new(9.0, 10.0, 6.0),
        ));
    }

    #[test]
    fn building_beyond_car_is_not_between_endpoints() {
        let camera = Vec3::new(10.0, 10.0, 10.0);
        let car = Vec3::ZERO;
        assert!(!segment_intersects_box(
            camera,
            car,
            Vec3::new(-3.0, 0.0, -3.0),
            Vec3::new(-1.0, 5.0, -1.0),
        ));
    }

    #[test]
    fn parallel_segment_outside_slab_is_rejected() {
        assert!(!segment_intersects_box(
            Vec3::new(0.0, 2.0, 10.0),
            Vec3::new(0.0, 2.0, 0.0),
            Vec3::new(1.0, 0.0, 4.0),
            Vec3::new(3.0, 4.0, 6.0),
        ));
    }
}
