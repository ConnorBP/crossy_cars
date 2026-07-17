//! Isolated deterministic microtexture A/B visual-review harness.
//!
//! Each query-selected focus places matched OFF and ON production assets next
//! to one another under one camera and one low-angle key/fill rig. The control
//! roots opt out of the production binders; the ON roots run those binders
//! unchanged.

use bevy::asset::{LoadState, RecursiveDependencyLoadState, UntypedAssetId};
use bevy::camera::ScalingMode;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::gltf::GltfMaterialName;
use bevy::prelude::*;
use bevy::text::FontSize;
use serde::Serialize;

use crate::difficulty::{
    ImportedTrafficPaintMaterial, ImportedTrafficPaintOwner, ImportedTrafficReady,
    ImportedTrafficVisual, ImportedTrafficWheelAnimation, bind_imported_traffic_paint,
    bind_imported_traffic_wheels, update_imported_traffic_ready,
};
use crate::textures::{PbrDetailAssets, TextureAssets};
use crate::toy_shading::{
    ImportedMicrotextureCache, ImportedMicrotextureSet, ImportedWorldVisual,
    MicrotextureDetailEnabled, MicrotexturedImportedPrimitive,
};

pub struct MicrotextureReviewPlugin;

const APARTMENT_PATH: &str = "models/world/isometric/apartment_modern_balconies.glb#Scene0";
const COTTAGE_PATH: &str = "models/world/isometric/house_cottage_gabled.glb#Scene0";
const TREE_PATH: &str = "models/world/isometric/tree_urban_blocky.glb#Scene0";
const TRAFFIC_ASSETS: [(&str, &str); 5] = [
    (
        "models/traffic/toy/npc_toy_sedan.glb#Scene0",
        "npc_toy_sedan",
    ),
    (
        "models/traffic/toy/npc_toy_city_van.glb#Scene0",
        "npc_toy_city_van",
    ),
    (
        "models/traffic/toy/npc_toy_hatchback.glb#Scene0",
        "npc_toy_hatchback",
    ),
    (
        "models/traffic/toy/npc_toy_pickup.glb#Scene0",
        "npc_toy_pickup",
    ),
    ("models/traffic/toy/npc_toy_suv.glb#Scene0", "npc_toy_suv"),
];
const DETAIL_PATHS: [&str; 13] = [
    "textures/pbr_detail/concrete_albedo.png",
    "textures/pbr_detail/foliage_albedo.png",
    "textures/pbr_detail/traffic_paint_albedo.png",
    "textures/pbr_detail/traffic_paint_orm.png",
    "textures/pbr_detail/plastic_normal.png",
    "textures/pbr_detail/plastic_orm.png",
    "textures/pbr_detail/concrete_normal.png",
    "textures/pbr_detail/concrete_orm.png",
    "textures/pbr_detail/wood_normal.png",
    "textures/pbr_detail/wood_orm.png",
    "textures/pbr_detail/grass_normal.png",
    "textures/pbr_detail/grass_orm.png",
    "textures/pbr_detail/soil_orm.png",
];
const STAGES: [&str; 5] = [
    "assets-loaded",
    "scenes-instantiated",
    "detail-maps-loaded",
    "runtime-bindings-complete",
    "matched-frame-ready",
];
const SIDES: [(bool, f32); 2] = [(false, -5.4), (true, 5.4)];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ReviewFocus {
    #[default]
    Apartment,
    Materials,
    Traffic,
}

impl ReviewFocus {
    const fn name(self) -> &'static str {
        match self {
            Self::Apartment => "apartment",
            Self::Materials => "materials",
            Self::Traffic => "traffic",
        }
    }

    const fn expected_primitives(self) -> usize {
        match self {
            Self::Apartment => 10,
            Self::Materials => 14,
            Self::Traffic => 71,
        }
    }

    const fn expected_roots(self) -> usize {
        match self {
            Self::Apartment => 1,
            Self::Materials => 2,
            Self::Traffic => 5,
        }
    }
}

fn parse_focus(value: &str) -> ReviewFocus {
    match value {
        "materials" => ReviewFocus::Materials,
        "traffic" => ReviewFocus::Traffic,
        _ => ReviewFocus::Apartment,
    }
}

fn requested_focus() -> ReviewFocus {
    #[cfg(target_arch = "wasm32")]
    {
        return web_sys::window()
            .and_then(|window| js_sys::Reflect::get(window.as_ref(), &"location".into()).ok())
            .and_then(|location| js_sys::Reflect::get(&location, &"search".into()).ok())
            .and_then(|search| search.as_string())
            .and_then(|query| {
                query
                    .trim_start_matches('?')
                    .split('&')
                    .find_map(|part| part.strip_prefix("microtexture_focus="))
                    .map(parse_focus)
            })
            .unwrap_or_default();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var("ROADY_MICROTEXTURE_FOCUS")
            .map(|value| parse_focus(&value))
            .unwrap_or_default()
    }
}

#[derive(Resource)]
struct ReviewAssets {
    focus: ReviewFocus,
    scenes: Vec<Handle<WorldAsset>>,
    paths: Vec<&'static str>,
}

impl FromWorld for ReviewAssets {
    fn from_world(world: &mut World) -> Self {
        let focus = requested_focus();
        let paths = match focus {
            ReviewFocus::Apartment => vec![APARTMENT_PATH],
            ReviewFocus::Materials => vec![COTTAGE_PATH, TREE_PATH],
            ReviewFocus::Traffic => TRAFFIC_ASSETS.iter().map(|(path, _)| *path).collect(),
        };
        let server = world.resource::<AssetServer>();
        let scenes = paths.iter().map(|path| server.load(*path)).collect();
        Self {
            focus,
            scenes,
            paths,
        }
    }
}

#[derive(Component)]
struct ReviewSceneRoot;

#[derive(Component)]
struct ReviewTrafficRoot;

#[derive(Component, Clone, Copy)]
struct ReviewSide {
    detail: bool,
}

#[derive(Resource, Default)]
struct ReadyDelay {
    stable_updates: u8,
    published: bool,
    plateau_counts: Option<(usize, usize, usize)>,
}

#[derive(Serialize)]
struct PrimitiveCounts {
    expected_per_side: usize,
    off_processed: usize,
    on_processed: usize,
    pending: usize,
}

#[derive(Serialize)]
struct OnMapCounts {
    albedo: usize,
    normal: usize,
    orm: usize,
}

#[derive(Serialize)]
struct CacheCounts {
    meshes: usize,
    materials: usize,
    failed_meshes: usize,
    stable_updates: u8,
}

#[derive(Serialize)]
struct TuningMetadata {
    concrete_albedo_srgb: [u8; 2],
    concrete_repeat: u8,
    concrete_maps: [&'static str; 2],
    concrete_normal: &'static str,
    foliage_albedo_srgb: [u8; 2],
    foliage_repeat: u8,
    foliage_scope: &'static str,
    traffic_albedo_srgb: [u8; 2],
    traffic_orm_ranges: [[u8; 2]; 4],
    traffic_repeat: u8,
    traffic_normal: &'static str,
    traffic_exclusions: [&'static str; 8],
}

#[derive(Serialize)]
struct ReviewMetadata {
    schema: &'static str,
    ready: bool,
    focus: &'static str,
    sides: [&'static str; 2],
    camera: &'static str,
    lighting: &'static str,
    assets: Vec<&'static str>,
    detail_maps: [&'static str; 13],
    tuning: TuningMetadata,
    stages: [&'static str; 5],
    primitives: PrimitiveCounts,
    on_maps: OnMapCounts,
    cache: CacheCounts,
}

impl Plugin for MicrotextureReviewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ReviewAssets>()
            .init_resource::<ReadyDelay>()
            .add_systems(Startup, spawn_review)
            // These are the production traffic systems, merely ungated from
            // gameplay in this isolated review process.
            .add_systems(
                Update,
                (
                    bind_imported_traffic_wheels,
                    bind_imported_traffic_paint.after(ImportedMicrotextureSet),
                    update_imported_traffic_ready,
                    publish_ready,
                )
                    .chain(),
            );
    }
}

fn spawn_review(
    mut commands: Commands,
    assets: Res<ReviewAssets>,
    textures: Res<TextureAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let backing = materials.add(StandardMaterial {
        base_color: Color::srgb(0.12, 0.125, 0.135),
        perceptual_roughness: 0.94,
        ..default()
    });
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(24.0, 17.0))),
        MeshMaterial3d(backing),
        Transform::from_xyz(0.0, -0.04, 0.0),
    ));

    // A strong, shallow warm key makes normal relief visible; the weaker cool
    // reverse fill preserves form without flattening the A/B comparison.
    commands.spawn((
        DirectionalLight {
            color: Color::srgb(1.0, 0.91, 0.77),
            illuminance: 28_000.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(-34.0, 5.8, -24.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight {
            color: Color::srgb(0.56, 0.69, 1.0),
            illuminance: 6_000.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(27.0, 8.0, 18.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    let (viewport_height, target_y, camera) = match assets.focus {
        ReviewFocus::Apartment => (12.4, 4.15, Vec3::new(18.5, 11.0, 21.0)),
        ReviewFocus::Materials => (8.8, 2.15, Vec3::new(18.5, 10.0, 20.0)),
        ReviewFocus::Traffic => (12.0, 0.9, Vec3::new(18.5, 12.0, 23.0)),
    };
    commands.spawn((
        Camera3d::default(),
        Msaa::Sample4,
        Tonemapping::TonyMcMapface,
        Projection::from(OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical { viewport_height },
            ..OrthographicProjection::default_3d()
        }),
        Transform::from_translation(camera).looking_at(Vec3::new(0.0, target_y, 0.0), Vec3::Y),
    ));

    spawn_labels(&mut commands, assets.focus);
    match assets.focus {
        ReviewFocus::Apartment => spawn_apartments(&mut commands, &assets),
        ReviewFocus::Materials => spawn_materials(
            &mut commands,
            &assets,
            &textures,
            &mut meshes,
            &mut materials,
        ),
        ReviewFocus::Traffic => spawn_traffic(&mut commands, &assets),
    }
}

fn spawn_labels(commands: &mut Commands, focus: ReviewFocus) {
    commands.spawn((
        Text::new(format!("MICROTEXTURE / {}", focus.name().to_uppercase())),
        TextFont {
            font_size: FontSize::Px(22.0),
            ..default()
        },
        TextColor(Color::srgb(0.92, 0.92, 0.94)),
        Node {
            position_type: PositionType::Absolute,
            top: px(18.0),
            left: px(0.0),
            width: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            ..default()
        },
    ));
    for (detail, left) in [(false, 23.0), (true, 73.0)] {
        commands.spawn((
            Text::new(if detail { "DETAIL ON" } else { "DETAIL OFF" }),
            TextFont {
                font_size: FontSize::Px(30.0),
                ..default()
            },
            TextColor(if detail {
                Color::srgb(0.55, 1.0, 0.62)
            } else {
                Color::srgb(1.0, 0.72, 0.48)
            }),
            Node {
                position_type: PositionType::Absolute,
                bottom: px(22.0),
                left: Val::Percent(left),
                ..default()
            },
        ));
    }
}

fn spawn_world_root(
    commands: &mut Commands,
    scene: Handle<WorldAsset>,
    detail: bool,
    transform: Transform,
) {
    commands.spawn((
        WorldAssetRoot(scene),
        transform,
        ImportedWorldVisual,
        MicrotextureDetailEnabled(detail),
        ReviewSceneRoot,
        ReviewSide { detail },
    ));
}

fn spawn_apartments(commands: &mut Commands, assets: &ReviewAssets) {
    for (detail, x) in SIDES {
        // Identical transform and orthographic scale on either side. The front
        // corner exposes broad vertical concrete plus the full roof silhouette.
        spawn_world_root(
            commands,
            assets.scenes[0].clone(),
            detail,
            Transform::from_xyz(x, 0.0, 0.5).with_rotation(Quat::from_rotation_y(0.20)),
        );
    }
}

fn off_material(
    materials: &mut Assets<StandardMaterial>,
    source: &Handle<StandardMaterial>,
) -> Handle<StandardMaterial> {
    let mut material = materials
        .get(source.id())
        .cloned()
        .expect("cached production ground material exists");
    material.normal_map_texture = None;
    material.metallic_roughness_texture = None;
    material.occlusion_texture = None;
    materials.add(material)
}

fn spawn_materials(
    commands: &mut Commands,
    assets: &ReviewAssets,
    textures: &TextureAssets,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    let patch = meshes.add(Plane3d::default().mesh().size(3.4, 2.5));
    let off_grass = off_material(materials, &textures.grass);
    let soil = textures.orchard_ground[0].clone();
    let off_soil = off_material(materials, &soil);
    for (detail, x) in SIDES {
        spawn_world_root(
            commands,
            assets.scenes[0].clone(),
            detail,
            Transform::from_xyz(x - 1.4, 0.0, 0.7).with_scale(Vec3::splat(1.15)),
        );
        spawn_world_root(
            commands,
            assets.scenes[1].clone(),
            detail,
            Transform::from_xyz(x + 1.8, 0.0, 0.5).with_scale(Vec3::splat(2.0)),
        );
        for (z, on, off) in [
            (-3.1, textures.grass.clone(), off_grass.clone()),
            (0.1, soil.clone(), off_soil.clone()),
        ] {
            commands.spawn((
                Mesh3d(patch.clone()),
                MeshMaterial3d(if detail { on } else { off }),
                Transform::from_xyz(x + 2.0, 0.015, z),
            ));
        }
    }
}

fn spawn_traffic(commands: &mut Commands, assets: &ReviewAssets) {
    let positions = [
        Vec3::new(-3.2, 0.0, -2.4),
        Vec3::new(0.0, 0.0, -2.4),
        Vec3::new(3.2, 0.0, -2.4),
        Vec3::new(-1.7, 0.0, 2.0),
        Vec3::new(1.7, 0.0, 2.0),
    ];
    for (detail, x) in SIDES {
        for (index, ((_, prefix), position)) in TRAFFIC_ASSETS.iter().zip(positions).enumerate() {
            commands
                .spawn((
                    Transform::from_translation(position + Vec3::X * x)
                        .with_rotation(Quat::from_rotation_y(-0.18))
                        .with_scale(Vec3::splat(2.0)),
                    Visibility::default(),
                ))
                .with_child((
                    WorldAssetRoot(assets.scenes[index].clone()),
                    Transform::IDENTITY,
                    ImportedTrafficVisual {
                        asset_prefix: prefix,
                        paint_index: index,
                    },
                    ImportedTrafficPaintMaterial::default(),
                    ImportedTrafficWheelAnimation::default(),
                    MicrotextureDetailEnabled(detail),
                    ReviewTrafficRoot,
                    ReviewSide { detail },
                ));
        }
    }
}

fn all_assets_loaded(
    server: &AssetServer,
    mut handles: impl Iterator<Item = UntypedAssetId>,
) -> bool {
    handles.all(|id| {
        matches!(server.get_load_state(id), Some(LoadState::Loaded))
            && matches!(
                server.get_recursive_dependency_load_state(id),
                Some(RecursiveDependencyLoadState::Loaded)
            )
    })
}

fn root_descendants<'a>(
    children: &'a Children,
    hierarchy: &'a Query<&Children>,
) -> impl Iterator<Item = Entity> + 'a {
    children
        .iter()
        .flat_map(|child| std::iter::once(child).chain(hierarchy.iter_descendants(child)))
}

fn publish_ready(
    server: Res<AssetServer>,
    assets: Res<ReviewAssets>,
    details: Res<PbrDetailAssets>,
    cache: Res<ImportedMicrotextureCache>,
    scene_roots: Query<(&ReviewSide, &Children), With<ReviewSceneRoot>>,
    traffic_roots: Query<
        (&ReviewSide, &Children, Option<&ImportedTrafficReady>),
        With<ReviewTrafficRoot>,
    >,
    hierarchy: Query<&Children>,
    descendants: Query<(
        Option<&GltfMaterialName>,
        Option<&MicrotexturedImportedPrimitive>,
        Option<&ImportedTrafficPaintOwner>,
        Option<&MeshMaterial3d<StandardMaterial>>,
    )>,
    materials: Res<Assets<StandardMaterial>>,
    mut delay: ResMut<ReadyDelay>,
) {
    if delay.published {
        return;
    }
    let scene_assets_ready = all_assets_loaded(
        &server,
        assets.scenes.iter().map(|handle| handle.id().untyped()),
    );
    let detail_handles = [
        details.concrete_albedo.id(),
        details.foliage_albedo.id(),
        details.traffic_paint_albedo.id(),
        details.traffic_paint_orm.id(),
        details.plastic_normal.id(),
        details.plastic_orm.id(),
        details.concrete_normal.id(),
        details.concrete_orm.id(),
        details.wood_normal.id(),
        details.wood_orm.id(),
        details.grass_normal.id(),
        details.grass_orm.id(),
        details.soil_orm.id(),
    ];
    let maps_ready = all_assets_loaded(&server, detail_handles.into_iter().map(AssetId::untyped));

    let root_count = |detail: bool| {
        scene_roots
            .iter()
            .filter(|(side, _)| side.detail == detail)
            .count()
            + traffic_roots
                .iter()
                .filter(|(side, _, _)| side.detail == detail)
                .count()
    };
    let rows_present = root_count(false) == assets.focus.expected_roots()
        && root_count(true) == assets.focus.expected_roots();

    let mut off_processed = 0;
    let mut on_processed = 0;
    let mut pending = 0;
    let mut on_albedo = 0;
    let mut on_normal = 0;
    let mut on_orm = 0;
    for (side, children) in &scene_roots {
        for entity in root_descendants(children, &hierarchy) {
            if let Ok((name, marker, _, material)) = descendants.get(entity)
                && name.is_some()
            {
                let complete = marker.is_some();
                if side.detail {
                    on_processed += usize::from(complete);
                    if let Some(material) = material.and_then(|handle| materials.get(handle.id())) {
                        on_albedo +=
                            usize::from(material.base_color_texture.as_ref().is_some_and(|map| {
                                map == &details.concrete_albedo || map == &details.foliage_albedo
                            }));
                        on_normal += usize::from(material.normal_map_texture.is_some());
                        on_orm += usize::from(
                            material.metallic_roughness_texture.is_some()
                                && material.occlusion_texture.is_some(),
                        );
                    }
                } else {
                    off_processed += usize::from(complete);
                }
                pending += usize::from(!complete);
            }
        }
    }
    for (side, children, ready) in &traffic_roots {
        if ready.is_none() {
            pending += 1;
        }
        for entity in root_descendants(children, &hierarchy) {
            if let Ok((name, _, paint, material)) = descendants.get(entity)
                && name.is_some()
            {
                // Every traffic primitive is instantiated/processed; the
                // production path deliberately mutates only Toy_Paint.
                if side.detail {
                    on_processed += 1;
                    if paint.is_some()
                        && let Some(material) =
                            material.and_then(|handle| materials.get(handle.id()))
                    {
                        on_albedo += usize::from(
                            material.base_color_texture.as_ref()
                                == Some(&details.traffic_paint_albedo),
                        );
                        on_normal += usize::from(material.normal_map_texture.is_some());
                        on_orm += usize::from(
                            material.metallic_roughness_texture.as_ref()
                                == Some(&details.traffic_paint_orm)
                                && material.occlusion_texture.as_ref()
                                    == Some(&details.traffic_paint_orm),
                        );
                    }
                } else {
                    off_processed += 1;
                }
            }
        }
    }

    // Runtime ground uses production ON material handles and explicit clones
    // with generated slots removed for OFF. Include both patches in materials.
    if assets.focus == ReviewFocus::Materials {
        off_processed += 2;
        on_processed += 2;
        on_normal += 2;
        on_orm += 2;
    }
    let expected = assets.focus.expected_primitives();
    let complete = scene_assets_ready
        && maps_ready
        && rows_present
        && pending == 0
        && off_processed == expected
        && on_processed == expected
        && on_albedo > 0
        && on_orm > 0;
    if !complete {
        delay.stable_updates = 0;
        delay.plateau_counts = None;
        return;
    }

    let legacy_cache_counts = cache.counts();
    let cache_counts = legacy_cache_counts;
    if delay.plateau_counts == Some(cache_counts) {
        delay.stable_updates = delay.stable_updates.saturating_add(1);
    } else {
        delay.plateau_counts = Some(cache_counts);
        delay.stable_updates = 1;
    }
    if delay.stable_updates < 2 {
        return;
    }

    let metadata = ReviewMetadata {
        schema: "roady-microtexture-review-v4",
        ready: true,
        focus: assets.focus.name(),
        sides: ["detail-off", "detail-on"],
        camera: "matched-orthographic-grazing-isometric",
        lighting: "shared-low-angle-key-fill",
        assets: assets
            .paths
            .iter()
            .copied()
            .chain(
                (assets.focus == ReviewFocus::Materials)
                    .then_some(["runtime:ground-grass", "runtime:ground-soil"])
                    .into_iter()
                    .flatten(),
            )
            .collect(),
        detail_maps: DETAIL_PATHS,
        tuning: TuningMetadata {
            concrete_albedo_srgb: [228, 255],
            concrete_repeat: 2,
            concrete_maps: ["albedo", "orm"],
            concrete_normal: "none (authored facade geometry only)",
            foliage_albedo_srgb: [236, 255],
            foliage_repeat: 2,
            foliage_scope: "closed Leaf only; Planter Green excluded",
            traffic_albedo_srgb: [232, 255],
            traffic_orm_ranges: [[250, 255], [220, 255], [255, 255], [255, 255]],
            traffic_repeat: 2,
            traffic_normal: "plastic_normal strength 0.035",
            traffic_exclusions: [
                "buildings",
                "player",
                "accent",
                "glass",
                "lights",
                "trim",
                "tires",
                "authored-texture-slots",
            ],
        },
        stages: STAGES,
        primitives: PrimitiveCounts {
            expected_per_side: expected,
            off_processed,
            on_processed,
            pending,
        },
        on_maps: OnMapCounts {
            albedo: on_albedo,
            normal: on_normal,
            orm: on_orm,
        },
        cache: CacheCounts {
            meshes: cache_counts.0,
            materials: cache_counts.1,
            failed_meshes: cache_counts.2,
            stable_updates: delay.stable_updates,
        },
    };
    let json = serde_json::to_string(&metadata).expect("review metadata serializes");
    #[cfg(target_arch = "wasm32")]
    if let Some(window) = web_sys::window() {
        let _ = js_sys::Reflect::set(
            window.as_ref(),
            &"__ROADY_MICROTEXTURE_REVIEW__".into(),
            &json.clone().into(),
        );
        if let Some(root) = window
            .document()
            .and_then(|document| document.document_element())
        {
            let _ = root.set_attribute("data-roady-microtexture-review-ready", "true");
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        println!("ROADY_MICROTEXTURE_REVIEW_JSON={json}");
        println!("ROADY_MICROTEXTURE_REVIEW_READY=1");
    }
    delay.published = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::AssetPlugin;

    use crate::difficulty::Difficulty;
    use crate::game::state::GameState;
    use crate::{StartupMode, startup_mode};

    #[test]
    fn focus_parser_defaults_and_accepts_only_contract_values() {
        assert_eq!(parse_focus("apartment"), ReviewFocus::Apartment);
        assert_eq!(parse_focus("materials"), ReviewFocus::Materials);
        assert_eq!(parse_focus("traffic"), ReviewFocus::Traffic);
        assert_eq!(parse_focus("invalid"), ReviewFocus::Apartment);
        assert_eq!(ReviewFocus::default(), ReviewFocus::Apartment);
    }

    #[test]
    fn focus_inventory_is_matched_and_complete() {
        assert_eq!(SIDES, [(false, -5.4), (true, 5.4)]);
        assert_eq!(ReviewFocus::Apartment.expected_primitives(), 10);
        assert_eq!(ReviewFocus::Materials.expected_primitives(), 14);
        assert_eq!(ReviewFocus::Traffic.expected_primitives(), 71);
        assert_eq!(TRAFFIC_ASSETS.len(), 5);
        assert_eq!(DETAIL_PATHS.len(), 13);
        assert!(
            DETAIL_PATHS
                .iter()
                .filter(|path| path.ends_with("_albedo.png"))
                .count()
                == 3
        );
    }

    #[test]
    fn microtexture_review_has_highest_isolated_startup_precedence() {
        assert_eq!(
            startup_mode(true, true, true, true),
            StartupMode::MicrotextureReview
        );
    }

    #[test]
    fn plugin_isolation_does_not_install_gameplay_state_or_traffic_manager() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<WorldAsset>()
            .add_plugins(MicrotextureReviewPlugin);
        assert!(!app.world().contains_resource::<State<GameState>>());
        assert!(!app.world().contains_resource::<Difficulty>());
    }

    #[test]
    fn normal_url_cannot_select_microtexture_review() {
        assert_eq!(
            startup_mode(false, false, false, false),
            StartupMode::Production
        );
    }
}
