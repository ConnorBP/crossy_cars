#![allow(dead_code)] // Integration API is intentionally unused until object spawns land.

//! Shared, inexpensive ground shading for the toy-scale scene.
//!
//! The two shadow cards in this module are deliberately ordinary unlit
//! [`StandardMaterial`]s.  They need no custom shader, render target, or shadow
//! map, and therefore follow the same path on native and WebGL2.  All images,
//! materials, and meshes are constructed once by [`ToyShadingPlugin`]; callers
//! must clone these cached handles instead of adding assets while spawning an
//! object.

use std::collections::HashSet;

use bevy::asset::{AssetId, RenderAssetUsages};
use bevy::gltf::GltfMaterialName;
use bevy::image::{Image, ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::textures::PbrDetailAssets;

/// Canonical PBR finishes for procedural miniature-town materials.
///
/// The palette deliberately owns only the five finish controls below. Applying
/// a family never changes an authored color, texture, alpha mode, emissive cue,
/// or any other `StandardMaterial` field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToyMaterialFamily {
    CoatedPlastic,
    Ceramic,
    PaintedWood,
    RawWood,
    Clay,
    Rubber,
    PaintedMetal,
    BareMetal,
    Foliage,
    Concrete,
    Asphalt,
    SoilHay,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ToyMaterialFinish {
    pub(crate) metallic: f32,
    pub(crate) roughness: f32,
    pub(crate) reflectance: f32,
    pub(crate) clearcoat: f32,
    pub(crate) clearcoat_roughness: f32,
}

impl ToyMaterialFamily {
    /// One exact shared table keeps every procedural cache visually coherent.
    pub(crate) const fn finish(self) -> ToyMaterialFinish {
        match self {
            Self::CoatedPlastic => ToyMaterialFinish {
                metallic: 0.0,
                roughness: 0.30,
                reflectance: 0.50,
                clearcoat: 0.85,
                clearcoat_roughness: 0.20,
            },
            Self::Ceramic => ToyMaterialFinish {
                metallic: 0.0,
                roughness: 0.24,
                reflectance: 0.50,
                clearcoat: 0.70,
                clearcoat_roughness: 0.16,
            },
            Self::PaintedWood => ToyMaterialFinish {
                metallic: 0.0,
                roughness: 0.48,
                reflectance: 0.50,
                clearcoat: 0.45,
                clearcoat_roughness: 0.28,
            },
            Self::RawWood => ToyMaterialFinish {
                metallic: 0.0,
                roughness: 0.78,
                reflectance: 0.45,
                clearcoat: 0.0,
                clearcoat_roughness: 0.50,
            },
            Self::Clay => ToyMaterialFinish {
                metallic: 0.0,
                roughness: 0.72,
                reflectance: 0.45,
                clearcoat: 0.0,
                clearcoat_roughness: 0.50,
            },
            Self::Rubber => ToyMaterialFinish {
                metallic: 0.0,
                roughness: 0.88,
                reflectance: 0.35,
                clearcoat: 0.0,
                clearcoat_roughness: 0.50,
            },
            Self::PaintedMetal => ToyMaterialFinish {
                metallic: 0.15,
                roughness: 0.30,
                reflectance: 0.50,
                clearcoat: 0.65,
                clearcoat_roughness: 0.20,
            },
            Self::BareMetal => ToyMaterialFinish {
                metallic: 0.92,
                roughness: 0.24,
                reflectance: 0.50,
                clearcoat: 0.0,
                clearcoat_roughness: 0.50,
            },
            Self::Foliage => ToyMaterialFinish {
                metallic: 0.0,
                roughness: 0.82,
                reflectance: 0.40,
                clearcoat: 0.0,
                clearcoat_roughness: 0.50,
            },
            Self::Concrete => ToyMaterialFinish {
                metallic: 0.0,
                roughness: 0.90,
                reflectance: 0.45,
                clearcoat: 0.0,
                clearcoat_roughness: 0.50,
            },
            Self::Asphalt => ToyMaterialFinish {
                metallic: 0.0,
                roughness: 0.96,
                reflectance: 0.35,
                clearcoat: 0.0,
                clearcoat_roughness: 0.50,
            },
            Self::SoilHay => ToyMaterialFinish {
                metallic: 0.0,
                roughness: 0.95,
                reflectance: 0.35,
                clearcoat: 0.0,
                clearcoat_roughness: 0.50,
            },
        }
    }
}

/// Apply only the canonical PBR finish, preserving all semantic material data.
pub(crate) fn apply_toy_material(material: &mut StandardMaterial, family: ToyMaterialFamily) {
    let finish = family.finish();
    material.metallic = finish.metallic;
    material.perceptual_roughness = finish.roughness;
    material.reflectance = finish.reflectance;
    material.clearcoat = finish.clearcoat;
    material.clearcoat_perceptual_roughness = finish.clearcoat_roughness;
}

/// Builder form used by one-time material caches.
pub(crate) fn toy_material(
    family: ToyMaterialFamily,
    mut material: StandardMaterial,
) -> StandardMaterial {
    apply_toy_material(&mut material, family);
    material
}

/// Resolution of both procedural shadow cards.
pub(crate) const TOY_SHADOW_TEXTURE_SIZE: u32 = 64;

/// World-space location of the fixed production key light.
///
/// Projected cards use this instead of looking up a light entity, keeping their
/// direction deterministic in gameplay and in the review harnesses.
pub(crate) const TOY_SUN_SOURCE: Vec3 = Vec3::new(30.0, 25.0, 15.0);

/// Small separation from ground surfaces to avoid z fighting.
pub(crate) const TOY_CONTACT_SHADOW_HEIGHT: f32 = 0.021;
pub(crate) const TOY_PROJECTED_SHADOW_HEIGHT: f32 = 0.022;

const MAX_SAFE_DIMENSION: f32 = 10_000.0;

/// Marker for a soft card immediately beneath an object.
#[derive(Component, Clone, Copy, Debug, Default)]
#[require(bevy::light::NotShadowCaster)]
pub(crate) struct ToyContactShadow;

/// Marker for a card cast away from [`TOY_SUN_SOURCE`].
#[derive(Component, Clone, Copy, Debug, Default)]
#[require(bevy::light::NotShadowCaster)]
pub(crate) struct ToyCastShadow;

/// The complete shared cache used by toy-shaded objects.
///
/// There are exactly two images, two materials, and two unit XZ planes.  The
/// meshes are kept separately even though both are unit planes so each part of
/// the cache has a stable semantic handle and can evolve without changing
/// object-spawn code.
#[derive(Resource)]
pub(crate) struct ToyShadingAssets {
    pub(crate) contact_image: Handle<Image>,
    pub(crate) cast_image: Handle<Image>,
    pub(crate) contact_material: Handle<StandardMaterial>,
    pub(crate) cast_material: Handle<StandardMaterial>,
    pub(crate) contact_plane: Handle<Mesh>,
    pub(crate) cast_plane: Handle<Mesh>,
}

/// Marks the `WorldAssetRoot` wrapper of one of the nine imported world-kit
/// scenes. Imported scene descendants inherit no marker, so material tuning
/// explicitly walks their [`ChildOf`] chain back to this boundary.
///
/// The imported player car deliberately does not carry this marker.
#[derive(Component, Clone, Copy, Debug, Default)]
pub(crate) struct ImportedWorldVisual;

/// Imported glTF material handles that have already received semantic tuning.
///
/// World scenes are instanced many times and share their material assets. The
/// set prevents repeated mutation both between primitives and across streamed
/// scene instances.
#[derive(Resource, Default)]
struct TunedWorldMaterials(HashSet<AssetId<StandardMaterial>>);

/// Coarse physical surface represented by an authored glTF material name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorldMaterialSemantic {
    PaintedWood,
    RawWood,
    Concrete,
    Glass,
    Metal,
    Clay,
    Coated,
    Foliage,
}

const MAX_WORLD_VISUAL_ANCESTRY: usize = 32;

pub(crate) struct ToyShadingPlugin;

impl Plugin for ToyShadingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ToyShadingAssets>()
            .init_resource::<TunedWorldMaterials>()
            // glTF scene primitives are added asynchronously. Restricting the
            // query to Added avoids a permanent whole-world material scan.
            .add_systems(Update, tune_imported_world_materials);
    }
}

fn material_tokens(name: &str) -> impl Iterator<Item = &str> {
    name.split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
}

fn has_token(name: &str, expected: &str) -> bool {
    material_tokens(name).any(|token| token.eq_ignore_ascii_case(expected))
}

fn classify_world_material(name: &str) -> Option<WorldMaterialSemantic> {
    use WorldMaterialSemantic::*;

    // Preserve authored window/glow transmission and emission exactly.
    if has_token(name, "glass") || has_token(name, "window") || has_token(name, "glow") {
        return Some(Glass);
    }
    // Explicit metal wins over painted trim and masonry descriptions. The
    // semantic maps to BareMetal below; merely being trim must never imply it.
    if has_token(name, "metal") {
        return Some(Metal);
    }
    if has_token(name, "terracotta")
        || has_token(name, "brick")
        || has_token(name, "masonry")
        || (has_token(name, "roof") && has_token(name, "red"))
    {
        return Some(Clay);
    }
    if has_token(name, "concrete") || has_token(name, "stone") {
        return Some(Concrete);
    }
    // The imported building kits use ivory painted wood for fascia, frames,
    // and other trim. Treat it as sealed paint unless the name said Metal.
    if has_token(name, "trim") {
        return Some(PaintedWood);
    }
    if has_token(name, "leaf") || has_token(name, "green") || has_token(name, "planter") {
        return Some(Foliage);
    }
    if has_token(name, "wood") || has_token(name, "slats") || has_token(name, "cedar") {
        // Building wood/trim is sealed paint; prop wood (tree trunks,
        // benches, mailbox posts) retains the visibly raw authored finish.
        let painted = has_token(name, "painted") || has_token(name, "building");
        return Some(if painted { PaintedWood } else { RawWood });
    }
    // Named colors and authored wall/shell finishes are coated toy surfaces.
    if [
        "blue", "cream", "stucco", "red", "ivory", "charcoal", "roof", "shell", "paint",
    ]
    .into_iter()
    .any(|token| has_token(name, token))
    {
        return Some(Coated);
    }
    None
}

fn apply_world_semantic(
    material: &mut StandardMaterial,
    semantic: WorldMaterialSemantic,
    details: Option<&PbrDetailAssets>,
) {
    use WorldMaterialSemantic::*;
    // Imported GLBs have UV0 but no authored tangents. Apply only safe ORM
    // modulation; normal detail remains on tangent-bearing procedural meshes.
    let (family, detail) = match semantic {
        Glass => return,
        PaintedWood => (ToyMaterialFamily::PaintedWood, None),
        RawWood => (
            ToyMaterialFamily::RawWood,
            details.map(|assets| &assets.wood_orm),
        ),
        Concrete => (
            ToyMaterialFamily::Concrete,
            details.map(|assets| &assets.concrete_orm),
        ),
        Metal => (ToyMaterialFamily::BareMetal, None),
        Clay => (ToyMaterialFamily::Clay, None),
        Coated => (
            ToyMaterialFamily::CoatedPlastic,
            details.map(|assets| &assets.plastic_orm),
        ),
        Foliage => (ToyMaterialFamily::Foliage, None),
    };
    apply_toy_material(material, family);
    if let Some(orm) = detail {
        if material.metallic_roughness_texture.is_none() {
            material.metallic_roughness_texture = Some(orm.clone());
        }
        if material.occlusion_texture.is_none() {
            material.occlusion_texture = Some(orm.clone());
        }
    }
}

fn has_imported_world_ancestor(
    mut entity: Entity,
    parents: &Query<&ChildOf>,
    world_roots: &Query<(), With<ImportedWorldVisual>>,
) -> bool {
    for _ in 0..MAX_WORLD_VISUAL_ANCESTRY {
        let Ok(parent) = parents.get(entity) else {
            return false;
        };
        entity = parent.parent();
        if world_roots.contains(entity) {
            return true;
        }
    }
    false
}

fn tune_imported_world_materials(
    primitives: Query<
        (Entity, &GltfMaterialName, &MeshMaterial3d<StandardMaterial>),
        Added<GltfMaterialName>,
    >,
    parents: Query<&ChildOf>,
    world_roots: Query<(), With<ImportedWorldVisual>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    details: Option<Res<PbrDetailAssets>>,
    mut tuned: ResMut<TunedWorldMaterials>,
) {
    for (entity, name, material_handle) in &primitives {
        if !has_imported_world_ancestor(entity, &parents, &world_roots) {
            continue;
        }
        let id = material_handle.id();
        let Some(semantic) = classify_world_material(name) else {
            continue;
        };
        // Added glTF primitives appear only after their material dependency is
        // loaded. Record the shared ID immediately before its sole mutation.
        if tuned.0.contains(&id) {
            continue;
        }
        if let Some(mut material) = materials.get_mut(id) {
            tuned.0.insert(id);
            apply_world_semantic(&mut material, semantic, details.as_deref());
        }
    }
}

impl FromWorld for ToyShadingAssets {
    fn from_world(world: &mut World) -> Self {
        let (contact_plane, cast_plane) =
            world.resource_scope(|_, mut meshes: Mut<Assets<Mesh>>| {
                (
                    meshes.add(Plane3d::default().mesh().size(1.0, 1.0)),
                    meshes.add(Plane3d::default().mesh().size(1.0, 1.0)),
                )
            });

        world.resource_scope::<Assets<Image>, _>(|world, mut images| {
            let contact_image = images.add(contact_shadow_image());
            let cast_image = images.add(cast_shadow_image());
            let (contact_material, cast_material) = {
                let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
                (
                    materials.add(shadow_material(contact_image.clone(), 0.30)),
                    materials.add(shadow_material(cast_image.clone(), 0.18)),
                )
            };

            Self {
                contact_image,
                cast_image,
                contact_material,
                cast_material,
                contact_plane,
                cast_plane,
            }
        })
    }
}

/// A platform-safe translucent material.  Alpha comes from both the tint and
/// procedural texture; lighting cannot brighten or recolor the shadow.
fn shadow_material(image: Handle<Image>, opacity: f32) -> StandardMaterial {
    StandardMaterial {
        base_color: Color::srgba(0.025, 0.03, 0.035, opacity),
        base_color_texture: Some(image),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        perceptual_roughness: 1.0,
        metallic: 0.0,
        ..default()
    }
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn shadow_image(fill_alpha: impl Fn(u32, u32) -> u8) -> Image {
    let size = TOY_SHADOW_TEXTURE_SIZE;
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            // White texels preserve the material tint; only alpha describes
            // the card.  This also avoids dark color fringes under filtering.
            data.extend_from_slice(&[255, 255, 255, fill_alpha(x, y)]);
        }
    }

    let mut image = Image::new(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        address_mode_w: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

/// Radially symmetric soft contact mask.  The clear one-pixel border prevents
/// filtering from producing a visible rectangular rim.
fn contact_shadow_image() -> Image {
    let center = (TOY_SHADOW_TEXTURE_SIZE - 1) as f32 * 0.5;
    shadow_image(|x, y| {
        let dx = (x as f32 - center) / center;
        let dy = (y as f32 - center) / center;
        let radius = dx.hypot(dy);
        let alpha = 1.0 - smoothstep(0.28, 0.98, radius);
        (alpha * 255.0).round() as u8
    })
}

/// Directional soft mask.  Texture V=0 is the caster end and V=1 is the tail;
/// it narrows and fades continuously away from the object.
fn cast_shadow_image() -> Image {
    let denominator = (TOY_SHADOW_TEXTURE_SIZE - 1) as f32;
    shadow_image(|x, y| {
        let across = ((x as f32 / denominator) * 2.0 - 1.0).abs();
        let along = y as f32 / denominator;
        let half_width = 0.92 - 0.28 * smoothstep(0.0, 1.0, along);
        let side_falloff = 1.0 - smoothstep(half_width * 0.62, half_width, across);
        let tail_falloff = 1.0 - smoothstep(0.36, 0.98, along);
        (side_falloff * tail_falloff * 255.0).round() as u8
    })
}

fn safe_dimension(value: f32) -> f32 {
    if value.is_finite() {
        value.abs().clamp(0.0, MAX_SAFE_DIMENSION)
    } else {
        0.0
    }
}

fn safe_ground_height(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(-MAX_SAFE_DIMENSION, MAX_SAFE_DIMENSION)
    } else {
        0.0
    }
}

/// Transform a cached unit plane into a soft contact card.
///
/// `footprint` is the object's full X/Z size and `ground_height` is expressed
/// in the same (usually parent-local) space as the returned transform.
pub(crate) fn contact_shadow_transform(footprint: Vec2, ground_height: f32) -> Transform {
    let footprint = Vec2::new(
        safe_dimension(footprint.x).max(0.001),
        safe_dimension(footprint.y).max(0.001),
    );
    Transform::from_xyz(
        0.0,
        safe_ground_height(ground_height) + TOY_CONTACT_SHADOW_HEIGHT,
        0.0,
    )
    .with_scale(Vec3::new(footprint.x, 1.0, footprint.y))
}

/// Transform a cached unit plane into a classical shadow projected from the
/// fixed production sun.
///
/// `footprint` is the caster's full X/Z extent. `caster_height` controls the
/// physically projected distance; invalid inputs are sanitized so malformed
/// authored data can never introduce NaNs into transform propagation.  Local
/// +Z points away from the sun after applying the returned yaw.
pub(crate) fn projected_shadow_transform(
    footprint: Vec2,
    caster_height: f32,
    ground_height: f32,
) -> Transform {
    projected_shadow_transform_from_sun(footprint, caster_height, ground_height, TOY_SUN_SOURCE)
}

fn projected_shadow_transform_from_sun(
    footprint: Vec2,
    caster_height: f32,
    ground_height: f32,
    sun_source: Vec3,
) -> Transform {
    let footprint = Vec2::new(safe_dimension(footprint.x), safe_dimension(footprint.y));
    let caster_height = safe_dimension(caster_height);

    let horizontal = Vec2::new(sun_source.x, sun_source.z);
    let horizontal_length = if horizontal.is_finite() {
        horizontal.length()
    } else {
        0.0
    };
    let away_from_sun = if horizontal_length > f32::EPSILON {
        -horizontal / horizontal_length
    } else {
        Vec2::NEG_X
    };
    let sun_height = if sun_source.y.is_finite() {
        sun_source.y.abs()
    } else {
        0.0
    };
    let projected_length = if sun_height > f32::EPSILON {
        caster_height * horizontal_length / sun_height
    } else {
        MAX_SAFE_DIMENSION
    }
    .clamp(0.0, MAX_SAFE_DIMENSION);

    // Project the axis-aligned footprint onto the card's transverse axis.  It
    // keeps wide goals (and rotated-looking rectangular props) fully covered.
    let transverse = Vec2::new(-away_from_sun.y, away_from_sun.x);
    let card_width =
        (footprint.x * transverse.x.abs() + footprint.y * transverse.y.abs()).max(0.001);
    let card_length = projected_length.max(0.001);
    let center = away_from_sun * (projected_length * 0.5);
    let yaw = away_from_sun.x.atan2(away_from_sun.y);

    Transform::from_xyz(
        center.x,
        safe_ground_height(ground_height) + TOY_PROJECTED_SHADOW_HEIGHT,
        center.y,
    )
    .with_rotation(Quat::from_rotation_y(yaw))
    .with_scale(Vec3::new(card_width, 1.0, card_length))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn material_test_app() -> App {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .add_plugins(ToyShadingPlugin);
        app
    }

    fn spawn_named_primitive(
        app: &mut App,
        parent: Entity,
        name: &str,
        material: Handle<StandardMaterial>,
    ) -> Entity {
        let primitive = app
            .world_mut()
            .spawn((GltfMaterialName(name.to_owned()), MeshMaterial3d(material)))
            .id();
        app.world_mut().entity_mut(parent).add_child(primitive);
        primitive
    }

    #[test]
    fn classifier_covers_world_kit_surface_semantics() {
        use WorldMaterialSemantic::*;
        let cases = [
            ("Painted_Wood_Blue", Some(PaintedWood)),
            ("Prop Warm Wood", Some(RawWood)),
            ("Building_Cottage_Gabled_Wood", Some(PaintedWood)),
            ("Iso Concrete", Some(Concrete)),
            ("Iso Warm Window", Some(Glass)),
            ("Prop Dark Metal", Some(Metal)),
            ("Iso Ivory Trim", Some(PaintedWood)),
            ("Iso Brick Red", Some(Clay)),
            ("Iso Terracotta Roof", Some(Clay)),
            ("Wall_Dusty_Blue", Some(Coated)),
            ("Prop Leaf Green", Some(Foliage)),
            ("unknown", None),
        ];

        for (name, expected) in cases {
            assert_eq!(classify_world_material(name), expected, "{name}");
        }
    }

    #[test]
    fn classifier_priority_preserves_explicit_material_semantics() {
        use WorldMaterialSemantic::*;
        let cases = [
            // Trim is authored painted wood unless Metal is an explicit token.
            ("Building_Trim", Some(PaintedWood)),
            ("Iso Metal Trim", Some(Metal)),
            ("Iso Trim Metal", Some(Metal)),
            ("Building_Metal_Trim_Red", Some(Metal)),
            // Brick/masonry and red roof surfaces are clay, not generic paint.
            ("Iso Brick Red", Some(Clay)),
            ("Iso Red Masonry", Some(Clay)),
            ("Roof_Red", Some(Clay)),
            // Transmission/emission cues take priority and remain untouched.
            ("Iso Warm Window", Some(Glass)),
            ("Metal Window Glass", Some(Glass)),
        ];

        for (name, expected) in cases {
            assert_eq!(classify_world_material(name), expected, "{name}");
        }
    }

    #[test]
    fn tuning_requires_world_wrapper_ancestry_and_excludes_player_car() {
        let mut app = material_test_app();
        let world_material = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let player_material = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());

        let world_wrapper = app.world_mut().spawn(ImportedWorldVisual).id();
        let scene_node = app.world_mut().spawn_empty().id();
        app.world_mut()
            .entity_mut(world_wrapper)
            .add_child(scene_node);
        spawn_named_primitive(&mut app, scene_node, "Iso Concrete", world_material.clone());

        let player = app
            .world_mut()
            .spawn(crate::car::Car {
                speed: 0.0,
                heading: 0.0,
                drift: 0.0,
            })
            .id();
        let imported_car_wrapper = app.world_mut().spawn_empty().id();
        app.world_mut()
            .entity_mut(player)
            .add_child(imported_car_wrapper);
        spawn_named_primitive(
            &mut app,
            imported_car_wrapper,
            "Car Painted Metal",
            player_material.clone(),
        );

        app.update();
        let materials = app.world().resource::<Assets<StandardMaterial>>();
        assert_eq!(
            materials.get(&world_material).unwrap().perceptual_roughness,
            0.90
        );
        assert_eq!(
            materials
                .get(&player_material)
                .unwrap()
                .perceptual_roughness,
            0.5
        );
        assert_eq!(materials.get(&player_material).unwrap().metallic, 0.0);
    }

    #[test]
    fn shared_material_is_tuned_once_and_authored_appearance_is_preserved() {
        let mut app = material_test_app();
        let (base_texture, emissive_texture, metallic_texture) = {
            let mut images = app.world_mut().resource_mut::<Assets<Image>>();
            (
                images.add(Image::default()),
                images.add(Image::default()),
                images.add(Image::default()),
            )
        };
        let original_color = Color::srgba(0.21, 0.32, 0.43, 0.54);
        let original_emissive = LinearRgba::new(2.0, 1.0, 0.5, 1.0);
        let material = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                base_color: original_color,
                base_color_texture: Some(base_texture.clone()),
                emissive: original_emissive,
                emissive_texture: Some(emissive_texture.clone()),
                metallic_roughness_texture: Some(metallic_texture.clone()),
                alpha_mode: AlphaMode::Blend,
                ..default()
            });
        let wrapper = app.world_mut().spawn(ImportedWorldVisual).id();
        spawn_named_primitive(&mut app, wrapper, "Prop Dark Metal", material.clone());
        app.update();

        {
            let tuned = app
                .world()
                .resource::<Assets<StandardMaterial>>()
                .get(&material)
                .unwrap();
            assert_eq!(tuned.metallic, 0.92);
            assert_eq!(tuned.perceptual_roughness, 0.24);
            assert_eq!(tuned.base_color, original_color);
            assert_eq!(tuned.base_color_texture.as_ref(), Some(&base_texture));
            assert_eq!(tuned.emissive, original_emissive);
            assert_eq!(tuned.emissive_texture.as_ref(), Some(&emissive_texture));
            assert_eq!(
                tuned.metallic_roughness_texture.as_ref(),
                Some(&metallic_texture)
            );
            assert_eq!(tuned.alpha_mode, AlphaMode::Blend);
        }

        // A later streamed instance reuses this handle. Changing the value
        // here makes a second semantic application observable.
        app.world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .get_mut(&material)
            .unwrap()
            .perceptual_roughness = 0.123;
        spawn_named_primitive(&mut app, wrapper, "Prop Dark Metal", material.clone());
        app.update();
        assert_eq!(
            app.world()
                .resource::<Assets<StandardMaterial>>()
                .get(&material)
                .unwrap()
                .perceptual_roughness,
            0.123
        );
        assert_eq!(
            app.world().resource::<TunedWorldMaterials>().0,
            HashSet::from([material.id()])
        );
    }

    #[test]
    fn glass_is_classified_but_preserved_exactly() {
        let mut material = StandardMaterial {
            metallic: 0.37,
            perceptual_roughness: 0.14,
            reflectance: 0.81,
            alpha_mode: AlphaMode::Blend,
            emissive: LinearRgba::new(3.0, 2.0, 1.0, 1.0),
            ..default()
        };
        let before = material.clone();
        apply_world_semantic(&mut material, WorldMaterialSemantic::Glass, None);
        assert_eq!(material.metallic, before.metallic);
        assert_eq!(material.perceptual_roughness, before.perceptual_roughness);
        assert_eq!(material.reflectance, before.reflectance);
        assert_eq!(material.alpha_mode, before.alpha_mode);
        assert_eq!(material.emissive, before.emissive);
    }

    fn pixels(image: &Image) -> &[u8] {
        image.data.as_deref().expect("procedural image has data")
    }

    fn alpha(image: &Image, x: u32, y: u32) -> u8 {
        pixels(image)[((y * TOY_SHADOW_TEXTURE_SIZE + x) * 4 + 3) as usize]
    }

    fn assert_webgl_card(image: &Image) {
        assert_eq!(image.texture_descriptor.size.width, 64);
        assert_eq!(image.texture_descriptor.size.height, 64);
        assert_eq!(image.texture_descriptor.size.depth_or_array_layers, 1);
        assert_eq!(image.texture_descriptor.dimension, TextureDimension::D2);
        assert_eq!(
            image.texture_descriptor.format,
            TextureFormat::Rgba8UnormSrgb
        );
        assert_eq!(pixels(image).len(), 64 * 64 * 4);
        assert!(pixels(image).chunks_exact(4).all(|p| p[..3] == [255; 3]));
        let ImageSampler::Descriptor(sampler) = &image.sampler else {
            panic!("shadow cards require an explicit sampler")
        };
        assert_eq!(sampler.address_mode_u, ImageAddressMode::ClampToEdge);
        assert_eq!(sampler.address_mode_v, ImageAddressMode::ClampToEdge);
        assert_eq!(sampler.mag_filter, ImageFilterMode::Linear);
        assert_eq!(sampler.min_filter, ImageFilterMode::Linear);
    }

    #[test]
    fn material_family_table_is_exact() {
        let expected = [
            (
                ToyMaterialFamily::CoatedPlastic,
                [0.0, 0.30, 0.50, 0.85, 0.20],
            ),
            (ToyMaterialFamily::Ceramic, [0.0, 0.24, 0.50, 0.70, 0.16]),
            (
                ToyMaterialFamily::PaintedWood,
                [0.0, 0.48, 0.50, 0.45, 0.28],
            ),
            (ToyMaterialFamily::RawWood, [0.0, 0.78, 0.45, 0.0, 0.50]),
            (ToyMaterialFamily::Clay, [0.0, 0.72, 0.45, 0.0, 0.50]),
            (ToyMaterialFamily::Rubber, [0.0, 0.88, 0.35, 0.0, 0.50]),
            (
                ToyMaterialFamily::PaintedMetal,
                [0.15, 0.30, 0.50, 0.65, 0.20],
            ),
            (ToyMaterialFamily::BareMetal, [0.92, 0.24, 0.50, 0.0, 0.50]),
            (ToyMaterialFamily::Foliage, [0.0, 0.82, 0.40, 0.0, 0.50]),
            (ToyMaterialFamily::Concrete, [0.0, 0.90, 0.45, 0.0, 0.50]),
            (ToyMaterialFamily::Asphalt, [0.0, 0.96, 0.35, 0.0, 0.50]),
            (ToyMaterialFamily::SoilHay, [0.0, 0.95, 0.35, 0.0, 0.50]),
        ];
        for (family, values) in expected {
            let finish = family.finish();
            assert_eq!(
                [
                    finish.metallic,
                    finish.roughness,
                    finish.reflectance,
                    finish.clearcoat,
                    finish.clearcoat_roughness
                ],
                values
            );
        }
    }

    #[test]
    fn applying_family_preserves_every_unowned_material_field() {
        let mut material = StandardMaterial {
            base_color: Color::srgba(0.1, 0.2, 0.3, 0.4),
            base_color_texture: Some(Handle::default()),
            normal_map_texture: Some(Handle::default()),
            emissive: LinearRgba::new(2.0, 1.0, 0.5, 0.75),
            emissive_texture: Some(Handle::default()),
            alpha_mode: AlphaMode::Blend,
            double_sided: true,
            unlit: true,
            fog_enabled: false,
            metallic: 0.13,
            perceptual_roughness: 0.17,
            reflectance: 0.19,
            clearcoat: 0.23,
            clearcoat_perceptual_roughness: 0.29,
            ..default()
        };
        let original = material.clone();
        apply_toy_material(&mut material, ToyMaterialFamily::Ceramic);
        assert_eq!(material.metallic, 0.0);
        assert_eq!(material.perceptual_roughness, 0.24);
        assert_eq!(material.reflectance, 0.50);
        assert_eq!(material.clearcoat, 0.70);
        assert_eq!(material.clearcoat_perceptual_roughness, 0.16);

        material.metallic = original.metallic;
        material.perceptual_roughness = original.perceptual_roughness;
        material.reflectance = original.reflectance;
        material.clearcoat = original.clearcoat;
        material.clearcoat_perceptual_roughness = original.clearcoat_perceptual_roughness;
        assert_eq!(format!("{material:?}"), format!("{original:?}"));
    }

    #[test]
    fn images_are_deterministic_webgl_safe_rgba_cards() {
        let contact_a = contact_shadow_image();
        let contact_b = contact_shadow_image();
        let cast_a = cast_shadow_image();
        let cast_b = cast_shadow_image();
        assert_webgl_card(&contact_a);
        assert_webgl_card(&cast_a);
        assert_eq!(pixels(&contact_a), pixels(&contact_b));
        assert_eq!(pixels(&cast_a), pixels(&cast_b));
    }

    #[test]
    fn contact_and_projected_masks_have_soft_monotonic_falloff() {
        let contact = contact_shadow_image();
        assert!(alpha(&contact, 32, 32) > alpha(&contact, 48, 32));
        assert!(alpha(&contact, 48, 32) > alpha(&contact, 63, 32));
        assert_eq!(alpha(&contact, 0, 0), 0);
        assert_eq!(alpha(&contact, 63, 63), 0);

        let cast = cast_shadow_image();
        assert!(alpha(&cast, 32, 4) > alpha(&cast, 32, 36));
        assert!(alpha(&cast, 32, 36) > alpha(&cast, 32, 63));
        assert_eq!(alpha(&cast, 0, 32), 0);
        assert_eq!(alpha(&cast, 63, 32), 0);
    }

    #[test]
    fn shared_cache_adds_only_two_of_each_asset() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>();
        let before = (
            app.world().resource::<Assets<Image>>().len(),
            app.world().resource::<Assets<Mesh>>().len(),
            app.world().resource::<Assets<StandardMaterial>>().len(),
        );
        app.add_plugins(ToyShadingPlugin);
        let after = (
            app.world().resource::<Assets<Image>>().len(),
            app.world().resource::<Assets<Mesh>>().len(),
            app.world().resource::<Assets<StandardMaterial>>().len(),
        );
        assert_eq!(after, (before.0 + 2, before.1 + 2, before.2 + 2));

        let assets = app.world().resource::<ToyShadingAssets>();
        assert_ne!(assets.contact_image, assets.cast_image);
        assert_ne!(assets.contact_plane, assets.cast_plane);
        assert_ne!(assets.contact_material, assets.cast_material);
        let materials = app.world().resource::<Assets<StandardMaterial>>();
        for handle in [&assets.contact_material, &assets.cast_material] {
            let material = materials.get(handle).unwrap();
            assert!(material.unlit);
            assert_eq!(material.alpha_mode, AlphaMode::Blend);
            assert!(material.base_color_texture.is_some());
        }
    }

    #[test]
    fn fixed_sun_projection_points_away_and_is_deterministic() {
        let a = projected_shadow_transform(Vec2::new(4.0, 0.4), 2.5, 0.0);
        let b = projected_shadow_transform(Vec2::new(4.0, 0.4), 2.5, 0.0);
        assert_eq!(a, b);
        let expected = -Vec2::new(TOY_SUN_SOURCE.x, TOY_SUN_SOURCE.z).normalize();
        let actual = Vec2::new(a.translation.x, a.translation.z).normalize();
        assert!(actual.dot(expected) > 0.9999);
        assert!(a.scale.x > 0.0 && a.scale.z > 0.0);
    }

    #[test]
    fn contact_and_projection_transforms_are_finite_for_bad_inputs() {
        let transforms = [
            contact_shadow_transform(Vec2::new(f32::NAN, f32::INFINITY), f32::NEG_INFINITY),
            projected_shadow_transform(
                Vec2::new(f32::NAN, f32::NEG_INFINITY),
                f32::INFINITY,
                f32::NAN,
            ),
            projected_shadow_transform_from_sun(
                Vec2::splat(-3.0),
                -2.0,
                f32::INFINITY,
                Vec3::splat(f32::NAN),
            ),
        ];
        for transform in transforms {
            assert!(transform.translation.is_finite());
            assert!(transform.rotation.is_finite());
            assert!(transform.scale.is_finite());
            assert!(transform.scale.cmpgt(Vec3::ZERO).all());
        }
    }
}
