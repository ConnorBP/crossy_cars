//! Foreground-building occlusion fading.
//!
//! Buildings are collision roots, while their rendered body and roof are
//! descendants.  This plugin tests the world-space camera-to-car segment
//! against each building footprint and swaps only the elevated building
//! meshes to one shared translucent material.  The original material handle
//! is retained on each affected mesh so restoration does not clone materials
//! or create assets per frame.

use bevy::transform::TransformSystems;
use bevy::{camera::primitives::Aabb, prelude::*};

use crate::car::Car;
use crate::game::state::GameState;
use crate::world::{Building, BuildingGroundShadow, BuildingVisualProfile, Collider};

/// A small footprint expansion accounts for the width of the car and for a
/// roof overhang, rather than requiring the camera-to-car line to hit the
/// mathematical center of the car exactly.
const FOOTPRINT_PADDING: f32 = 0.65;

/// Building shadows are also mesh descendants, but remain within this thin
/// near-ground band. Classification uses each mesh's transformed upper bound,
/// so a wall whose center is low still fades when its geometry rises above the
/// band, while the procedural ground-shadow child remains untouched.
const MIN_VISUAL_HEIGHT: f32 = 0.5;

/// The audited profile is already the full visual height. Only a tiny numeric
/// allowance is needed to avoid a precision gap at its top plane.
const TOP_ALLOWANCE: f32 = 0.05;

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
                // Imported buildings contain several overlapping primitives;
                // keep each layer faint so their blended sum remains clear.
                base_color: Color::srgba(0.34, 0.42, 0.48, 0.13),
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
        (
            &GlobalTransform,
            &Collider,
            &BuildingVisualProfile,
            &Children,
        ),
        (With<Building>, Without<Car>, Without<Camera3d>),
    >,
    hierarchy: Query<&Children, Without<Building>>,
    mut visuals: Query<
        (
            &GlobalTransform,
            Option<&Aabb>,
            &mut MeshMaterial3d<StandardMaterial>,
            Option<&OriginalBuildingMaterial>,
            Option<&BuildingGroundShadow>,
        ),
        (With<Mesh3d>, Without<Building>),
    >,
    mut descendants: Local<Vec<Entity>>,
) {
    let (Ok(camera_transform), Ok(car_transform)) = (camera.single(), car.single()) else {
        // A transient missing/duplicate camera or car must not leave stale
        // ghosts behind.
        for (_, _, mut material, original, _) in &mut visuals {
            if let Some(original) = original {
                material.0 = original.0.clone();
            }
        }
        return;
    };
    let camera_position = camera_transform.translation();
    let car_position = car_transform.translation();

    for (building_transform, collider, profile, children) in &buildings {
        descendants.clear();
        collect_descendants(children, &hierarchy, &mut descendants);

        let base = building_transform.translation();
        let min = Vec3::new(
            base.x - collider.half_x - FOOTPRINT_PADDING,
            base.y,
            base.z - collider.half_z - FOOTPRINT_PADDING,
        );
        let max = Vec3::new(
            base.x + collider.half_x + FOOTPRINT_PADDING,
            base.y + profile.height + TOP_ALLOWANCE,
            base.z + collider.half_z + FOOTPRINT_PADDING,
        );
        let occluded = segment_intersects_box(camera_position, car_position, min, max);

        for &entity in descendants.iter() {
            let Ok((transform, aabb, mut material, original, ground_shadow)) =
                visuals.get_mut(entity)
            else {
                continue;
            };
            let is_building_visual = if ground_shadow.is_some() {
                false
            } else if let Some(aabb) = aabb {
                let (_, visual_max_y) = transformed_vertical_bounds(transform, Some(aabb));
                visual_max_y > base.y + MIN_VISUAL_HEIGHT
            } else {
                // Imported scene primitives can be visible before Bevy has
                // populated their AABB. Treat them conservatively as elevated
                // so a foreground building never flashes opaque during load.
                true
            };

            if occluded && is_building_visual {
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

/// Return a mesh's world-space vertical bounds. Bevy's AABB is mesh-local, so
/// the projected half extent must account for arbitrary rotation and scale.
/// Scene instantiation can briefly expose a mesh before bounds calculation;
/// during that window, conservatively classify from its transformed center.
fn transformed_vertical_bounds(transform: &GlobalTransform, aabb: Option<&Aabb>) -> (f32, f32) {
    let center_fallback = transform.translation().y;
    let Some(aabb) = aabb else {
        return (center_fallback, center_fallback);
    };
    let affine = transform.affine();
    let center = affine.transform_point3a(aabb.center);
    let world_half_extents = affine.matrix3.abs() * aabb.half_extents.abs();
    (
        center.y - world_half_extents.y,
        center.y + world_half_extents.y,
    )
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
    use crate::world::BuildingAssetKind;

    #[test]
    fn transformed_aabb_bounds_include_rotation_scale_and_local_center() {
        let transform = GlobalTransform::from(
            Transform::from_xyz(2.0, 5.0, -3.0)
                .with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2))
                .with_scale(Vec3::new(2.0, 3.0, 4.0)),
        );
        let aabb = Aabb {
            center: Vec3A::new(1.0, 0.5, 0.0),
            half_extents: Vec3A::new(0.25, 1.0, 0.5),
        };
        let (min, max) = transformed_vertical_bounds(&transform, Some(&aabb));
        // After the quarter-turn, local X (including its scale) projects onto
        // world Y: center 7, half extent 0.5.
        assert!((min - 6.5).abs() < 1e-5);
        assert!((max - 7.5).abs() < 1e-5);
    }

    #[test]
    fn transformed_aabb_falls_back_to_world_center_while_bounds_are_missing() {
        let transform =
            GlobalTransform::from(Transform::from_xyz(1.0, 2.75, 3.0).with_scale(Vec3::splat(8.0)));
        assert_eq!(transformed_vertical_bounds(&transform, None), (2.75, 2.75));
    }

    #[test]
    fn profile_height_catches_ray_that_passes_over_old_center_estimate() {
        let camera = Vec3::new(10.0, 10.0, 10.0);
        let car = Vec3::ZERO;
        let min = Vec3::new(5.0, 0.0, 5.0);
        assert!(!segment_intersects_box(
            camera,
            car,
            min,
            Vec3::new(6.0, 4.25, 6.0),
        ));
        assert!(segment_intersects_box(
            camera,
            car,
            min,
            Vec3::new(6.0, 8.55 + TOP_ALLOWANCE, 6.0),
        ));
    }

    #[test]
    fn hierarchy_fades_only_elevated_meshes_and_restores_exact_handles() {
        let mut app = App::new();
        app.init_resource::<Assets<StandardMaterial>>()
            .init_resource::<BuildingGhostMaterial>()
            .add_systems(Update, fade_occluding_buildings);

        let (opaque, glass, shadow) = {
            let mut materials = app.world_mut().resource_mut::<Assets<StandardMaterial>>();
            (
                materials.add(StandardMaterial::default()),
                materials.add(StandardMaterial {
                    alpha_mode: AlphaMode::Blend,
                    ..default()
                }),
                materials.add(StandardMaterial::default()),
            )
        };
        let ghost = app.world().resource::<BuildingGhostMaterial>().0.clone();

        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                GlobalTransform::from_xyz(10.0, 10.0, 10.0),
            ))
            .id();
        app.world_mut().spawn((
            Car {
                speed: 0.0,
                heading: 0.0,
                drift: 0.0,
            },
            GlobalTransform::IDENTITY,
        ));
        let building = app
            .world_mut()
            .spawn((
                Building,
                Collider {
                    half_x: 1.0,
                    half_z: 1.0,
                },
                BuildingVisualProfile {
                    kind: BuildingAssetKind::Apartment,
                    height: 8.55,
                },
                GlobalTransform::from_xyz(5.0, 0.0, 5.0),
            ))
            .id();
        let intermediate = app.world_mut().spawn(GlobalTransform::IDENTITY).id();
        let ground_shadow = app
            .world_mut()
            .spawn((
                Mesh3d::default(),
                MeshMaterial3d(shadow.clone()),
                Aabb::from_min_max(Vec3::new(-2.0, -0.025, -2.0), Vec3::new(2.0, 0.025, 2.0)),
                GlobalTransform::from_xyz(5.0, 0.025, 5.0),
            ))
            .id();
        // The wall's center is below the old threshold, but its upper AABB
        // extent rises above it and must therefore be classified as visual.
        let lower_wall = app
            .world_mut()
            .spawn((
                Mesh3d::default(),
                MeshMaterial3d(opaque.clone()),
                Aabb::from_min_max(Vec3::new(-1.0, -0.2, -1.0), Vec3::new(1.0, 1.0, 1.0)),
                GlobalTransform::from_xyz(5.0, 0.2, 5.0),
            ))
            .id();
        let window = app
            .world_mut()
            .spawn((
                Mesh3d::default(),
                MeshMaterial3d(glass.clone()),
                Aabb::from_min_max(Vec3::splat(-0.5), Vec3::splat(0.5)),
                GlobalTransform::from_xyz(5.0, 4.0, 5.0),
            ))
            .id();
        app.world_mut()
            .entity_mut(intermediate)
            .add_children(&[lower_wall, window]);
        app.world_mut()
            .entity_mut(building)
            .add_children(&[intermediate, ground_shadow]);

        app.update();
        assert_eq!(
            app.world()
                .get::<MeshMaterial3d<StandardMaterial>>(lower_wall)
                .unwrap()
                .0,
            ghost
        );
        assert_eq!(
            app.world()
                .get::<MeshMaterial3d<StandardMaterial>>(window)
                .unwrap()
                .0,
            ghost
        );
        assert_eq!(
            app.world()
                .get::<MeshMaterial3d<StandardMaterial>>(ground_shadow)
                .unwrap()
                .0,
            shadow
        );

        // Missing camera cardinality takes the immediate restoration path.
        app.world_mut().despawn(camera);
        app.update();
        assert_eq!(
            app.world()
                .get::<MeshMaterial3d<StandardMaterial>>(lower_wall)
                .unwrap()
                .0,
            opaque
        );
        assert_eq!(
            app.world()
                .get::<MeshMaterial3d<StandardMaterial>>(window)
                .unwrap()
                .0,
            glass
        );
        assert_eq!(
            app.world()
                .get::<MeshMaterial3d<StandardMaterial>>(ground_shadow)
                .unwrap()
                .0,
            shadow
        );
    }

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
